//! `RoutingToolDispatcher` — fans out [`ToolDispatcher::dispatch`]
//! between the engine's built-in tools (e.g.,
//! [`crate::engine::tools::worktree::WorktreeToolDispatcher`]) and an
//! [`McpRegistry`]. Routing decisions are precomputed at
//! construction time from the merged tool catalog.

use crate::engine::tools::{
    DeclaredTool, ToolCall, ToolDispatchContext, ToolDispatcher, ToolResultPayload,
};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use surge_mcp::{McpContent, McpRegistry, McpToolEntry};

/// One row in the routing table — where a given tool name lives.
#[derive(Clone, Debug)]
enum ToolOrigin {
    /// Engine-built-in tool — delegate to `engine_dispatcher`.
    Engine,
    /// MCP server tool — call via `mcp_registry`.
    Mcp { server: String, timeout: Duration },
}

/// `ToolDispatcher` impl that routes between engine + MCP. Constructed
/// once per engine session (per agent stage), with the routing table
/// precomputed from the merged catalog.
pub struct RoutingToolDispatcher {
    engine_dispatcher: Arc<dyn ToolDispatcher>,
    mcp_registry: Arc<McpRegistry>,
    routing_table: HashMap<String, ToolOrigin>,
    declared: Vec<DeclaredTool>,
}

impl RoutingToolDispatcher {
    /// Build with engine dispatcher + MCP registry + filtered list of
    /// MCP tools that should be exposed for the current session.
    /// Engine-built-in tools (from
    /// [`ToolDispatcher::declared_tools`]) are inserted with
    /// [`ToolOrigin::Engine`] and override any MCP entries with the
    /// same name (collision resolution: engine wins).
    #[must_use]
    pub fn new(
        engine_dispatcher: Arc<dyn ToolDispatcher>,
        mcp_registry: Arc<McpRegistry>,
        mcp_tools: &[McpToolEntry],
        per_server_timeouts: &HashMap<String, Duration>,
    ) -> Self {
        let mut table: HashMap<String, ToolOrigin> = HashMap::new();
        let mut declared: Vec<DeclaredTool> = Vec::new();

        for entry in mcp_tools {
            let timeout = per_server_timeouts
                .get(&entry.server)
                .copied()
                .unwrap_or(Duration::from_secs(60));
            table.insert(
                entry.tool.clone(),
                ToolOrigin::Mcp {
                    server: entry.server.clone(),
                    timeout,
                },
            );
            declared.push(DeclaredTool {
                name: entry.tool.clone(),
                description: entry.description.clone(),
                input_schema: entry.input_schema.clone(),
            });
        }

        // Engine tools overwrite MCP collisions (engine wins).
        let engine_tools = engine_dispatcher.declared_tools();
        for et in &engine_tools {
            table.insert(et.name.clone(), ToolOrigin::Engine);
        }
        // Replace any duplicate-named entries in `declared` with the
        // engine's version (description / schema take precedence).
        let engine_names: std::collections::HashSet<&str> =
            engine_tools.iter().map(|t| t.name.as_str()).collect();
        declared.retain(|d| !engine_names.contains(d.name.as_str()));
        declared.extend(engine_tools);

        Self {
            engine_dispatcher,
            mcp_registry,
            routing_table: table,
            declared,
        }
    }
}

#[async_trait]
impl ToolDispatcher for RoutingToolDispatcher {
    async fn dispatch(&self, ctx: &ToolDispatchContext<'_>, call: &ToolCall) -> ToolResultPayload {
        match self.routing_table.get(&call.tool) {
            Some(ToolOrigin::Engine) => self.engine_dispatcher.dispatch(ctx, call).await,
            Some(ToolOrigin::Mcp { server, timeout }) => {
                match self
                    .mcp_registry
                    .call_tool(server, &call.tool, call.arguments.clone(), *timeout)
                    .await
                {
                    Ok(r) if !r.is_error => ToolResultPayload::Ok {
                        content: serde_json::Value::Array(
                            r.content.into_iter().map(content_to_json).collect(),
                        ),
                    },
                    Ok(r) => ToolResultPayload::Error {
                        message: r
                            .content
                            .into_iter()
                            .map(content_to_string)
                            .collect::<Vec<_>>()
                            .join("\n"),
                    },
                    Err(e) => ToolResultPayload::Error {
                        message: format!("MCP error: {e}"),
                    },
                }
            },
            None => ToolResultPayload::Unsupported {
                message: format!("unknown tool: {}", call.tool),
            },
        }
    }

    fn declared_tools(&self) -> Vec<DeclaredTool> {
        self.declared.clone()
    }
}

fn content_to_json(c: McpContent) -> serde_json::Value {
    match c {
        McpContent::Text(s) => serde_json::json!({ "type": "text", "text": s }),
        McpContent::Other { kind, summary } => serde_json::json!({
            "type": kind,
            "summary": summary,
        }),
        // `McpContent` is `#[non_exhaustive]`; catch any future variants with
        // a debug representation so callers always get a valid JSON value.
        _ => serde_json::json!({ "type": "unknown" }),
    }
}

fn content_to_string(c: McpContent) -> String {
    match c {
        McpContent::Text(s) => s,
        McpContent::Other { kind, summary } => format!("[{kind}] {summary}"),
        // `McpContent` is `#[non_exhaustive]`; forward-compatible fallback.
        _ => String::from("[unknown content]"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::tools::{ToolCall, ToolDispatcher, ToolResultPayload};

    /// Stub engine dispatcher that returns a marker payload echoing the
    /// requested tool name and declares one tool (`"shell_exec"`) so we
    /// can verify collision resolution.
    struct EngineStub;

    #[async_trait]
    impl ToolDispatcher for EngineStub {
        async fn dispatch(
            &self,
            _ctx: &ToolDispatchContext<'_>,
            call: &ToolCall,
        ) -> ToolResultPayload {
            ToolResultPayload::Ok {
                content: serde_json::json!({"engine_handled": call.tool}),
            }
        }
        fn declared_tools(&self) -> Vec<DeclaredTool> {
            vec![DeclaredTool {
                name: "shell_exec".into(),
                description: Some("engine version".into()),
                input_schema: serde_json::json!({}),
            }]
        }
    }

    #[tokio::test]
    async fn engine_tool_wins_collision() {
        let mcp = Arc::new(McpRegistry::from_config(&[]));
        let mcp_tools = vec![McpToolEntry::new(
            "fake".into(),
            "shell_exec".into(), // colliding name
            Some("from MCP".into()),
            serde_json::json!({}),
        )];
        let r = RoutingToolDispatcher::new(Arc::new(EngineStub), mcp, &mcp_tools, &HashMap::new());
        let ctx = ToolDispatchContext {
            run_id: surge_core::id::RunId::new(),
            session_id: surge_core::id::SessionId::new(),
            worktree_root: std::path::Path::new("/tmp"),
            run_memory: &surge_core::run_state::RunMemory::default(),
        };
        let call = ToolCall {
            call_id: "c1".into(),
            tool: "shell_exec".into(),
            arguments: serde_json::json!({}),
        };
        let result = r.dispatch(&ctx, &call).await;
        match result {
            ToolResultPayload::Ok { content } => {
                assert_eq!(content["engine_handled"], "shell_exec");
            },
            other => panic!("expected engine route, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn unknown_tool_is_unsupported() {
        let mcp = Arc::new(McpRegistry::from_config(&[]));
        let r = RoutingToolDispatcher::new(Arc::new(EngineStub), mcp, &[], &HashMap::new());
        let ctx = ToolDispatchContext {
            run_id: surge_core::id::RunId::new(),
            session_id: surge_core::id::SessionId::new(),
            worktree_root: std::path::Path::new("/tmp"),
            run_memory: &surge_core::run_state::RunMemory::default(),
        };
        let call = ToolCall {
            call_id: "c2".into(),
            tool: "whatever".into(),
            arguments: serde_json::json!({}),
        };
        match r.dispatch(&ctx, &call).await {
            ToolResultPayload::Unsupported { .. } => {},
            other => panic!("expected Unsupported, got {other:?}"),
        }
    }
}
