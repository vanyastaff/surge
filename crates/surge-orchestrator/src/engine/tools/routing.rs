//! `RoutingToolDispatcher` — fans out [`ToolDispatcher::dispatch`]
//! between the engine's built-in tools (e.g.,
//! [`crate::engine::tools::worktree::WorktreeToolDispatcher`]) and an
//! [`McpRegistry`]. Routing decisions are precomputed at
//! construction time from the merged tool catalog.

use crate::engine::tools::{
    DeclaredTool, McpEscalation, ToolCall, ToolDispatchContext, ToolDispatcher, ToolResultPayload,
};
use async_trait::async_trait;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use surge_mcp::{McpContent, McpError, McpRegistry, McpToolEntry};

/// Surge-injected tool names that are added separately by the ACP
/// bridge (not via any `ToolDispatcher::declared_tools`) and dispatched
/// upstream of this router. They are reserved: an MCP server must never
/// shadow them in the agent's tool catalog or routing table. This is
/// the **single canonical arbitration site** for injected-tool name
/// precedence — see ADR-0006 (uniform injected-tool surface) and
/// ADR-0014. The regression test asserts no second list exists.
pub(crate) const RESERVED_INJECTED_TOOLS: [&str; 2] =
    ["report_stage_outcome", "request_human_input"];

/// One row in the routing table — where a given tool name lives.
#[derive(Clone, Debug)]
enum ToolOrigin {
    /// Engine-built-in tool — delegate to `engine_dispatcher`.
    Engine,
    /// MCP server tool — call via `mcp_registry`.
    Mcp { server: String, timeout: Duration },
}

/// Restart-exhaustion escalation accumulator. `pending` is drained by
/// the agent stage into `EscalationRequested` events; `seen` records
/// every server already escalated so a single permanent outage emits
/// exactly one card instead of one per subsequent tool call — the
/// give-up fact is stable (see `engine::stage::agent`).
#[derive(Default)]
struct EscalationState {
    pending: Vec<McpEscalation>,
    seen: HashSet<String>,
}

/// `ToolDispatcher` impl that routes between engine + MCP. Constructed
/// once per engine session (per agent stage), with the routing table
/// precomputed from the merged catalog.
pub struct RoutingToolDispatcher {
    engine_dispatcher: Arc<dyn ToolDispatcher>,
    mcp_registry: Arc<McpRegistry>,
    routing_table: HashMap<String, ToolOrigin>,
    declared: Vec<DeclaredTool>,
    /// MCP restart-exhaustion escalations observed during dispatch,
    /// drained by the agent stage into `EscalationRequested` events.
    /// De-duplicated per server so one outage cannot spam the
    /// operator surface.
    escalations: Mutex<EscalationState>,
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

        // Sort MCP entries by (server, tool) for deterministic
        // first-wins collision resolution across MCP servers.
        let mut sorted_mcp: Vec<&McpToolEntry> = mcp_tools.iter().collect();
        sorted_mcp.sort_by(|a, b| a.server.cmp(&b.server).then_with(|| a.tool.cmp(&b.tool)));

        for entry in sorted_mcp {
            if table.contains_key(&entry.tool) {
                // Collision: another (sorted-earlier) MCP server already
                // claimed this tool name. Drop this entry and warn so
                // operators can rename or namespace if needed.
                tracing::warn!(
                    server = %entry.server,
                    tool = %entry.tool,
                    "MCP tool name collision; first-wins (by sorted server name) — this entry skipped"
                );
                continue;
            }
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

        // Reserved-name guard (single canonical arbitration site):
        // surge-injected tools are dispatched by the ACP bridge before
        // this router ever sees them, so an MCP tool of the same name
        // would shadow the catalog while being unreachable. Drop any
        // such MCP entry from the routing table and the declared
        // catalog, warning once per collision (ADR-0006 / ADR-0014).
        for reserved in RESERVED_INJECTED_TOOLS {
            if let Some(ToolOrigin::Mcp { server, .. }) = table.get(reserved) {
                tracing::warn!(
                    target: "mcp::supervisor",
                    tool = reserved,
                    server = %server,
                    "MCP server advertises a surge-injected tool name; \
                     dropping it — injected tools win (ADR-0006)"
                );
                table.remove(reserved);
            }
            declared.retain(|d| d.name != reserved);
        }

        Self {
            engine_dispatcher,
            mcp_registry,
            routing_table: table,
            declared,
            escalations: Mutex::new(EscalationState::default()),
        }
    }

    /// Record a restart-exhaustion escalation, de-duplicated per
    /// server: the first exhaustion of `server` is queued for the
    /// stage to escalate; later ones (every subsequent tool call hits
    /// the already-`Exhausted` connection) are dropped so one outage
    /// cannot spam the AFK/operator surface. Lock poisoning is
    /// swallowed — an escalation is best-effort telemetry, not
    /// load-bearing control flow.
    fn record_escalation(&self, server: &str, attempts: u32) {
        if let Ok(mut st) = self.escalations.lock()
            && st.seen.insert(server.to_owned())
        {
            st.pending.push(McpEscalation {
                server: server.to_owned(),
                attempts,
            });
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
                    Err(e) => {
                        // Type-safe escalation capture (no string
                        // sniffing): a restart-exhausted server is a
                        // permanent failure the AFK operator must see.
                        // De-duplicated per server inside
                        // `record_escalation`.
                        if let McpError::RestartExhausted { server, attempts } = &e {
                            self.record_escalation(server, *attempts);
                        }
                        ToolResultPayload::Error {
                            message: format!("MCP error: {e}"),
                        }
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

    fn drain_mcp_escalations(&self) -> Vec<McpEscalation> {
        self.escalations
            .lock()
            .map(|mut st| std::mem::take(&mut st.pending))
            .unwrap_or_default()
    }

    fn resolved_origin(&self, tool: &str) -> Option<String> {
        match self.routing_table.get(tool) {
            Some(ToolOrigin::Mcp { server, .. }) => Some(server.clone()),
            // Engine-built-in or unknown → no MCP attribution.
            _ => None,
        }
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

    /// Single-arbitration-site guard: the reserved injected-tool list
    /// is defined exactly once, here. If this breaks, a second list was
    /// introduced or a name changed without updating the contract —
    /// re-derive against the ACP bridge's injected tools (ADR-0006).
    #[test]
    fn reserved_injected_tools_is_the_canonical_pair() {
        assert_eq!(
            RESERVED_INJECTED_TOOLS,
            ["report_stage_outcome", "request_human_input"],
            "reserved injected-tool contract changed; this const is the \
             single source of truth — verify against the ACP bridge"
        );
    }

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
        let mcp = Arc::new(McpRegistry::from_config(&[], None));
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
    async fn mcp_mcp_collision_first_wins_by_server() {
        let mcp = Arc::new(McpRegistry::from_config(&[], None));
        // Two servers that both expose "shared". "a_server" sorts before
        // "z_server", so "a_server"'s entry should win.
        let mcp_tools = vec![
            McpToolEntry::new(
                "z_server".into(),
                "shared".into(),
                Some("from z".into()),
                serde_json::json!({}),
            ),
            McpToolEntry::new(
                "a_server".into(),
                "shared".into(),
                Some("from a".into()),
                serde_json::json!({}),
            ),
        ];
        let r = RoutingToolDispatcher::new(Arc::new(EngineStub), mcp, &mcp_tools, &HashMap::new());
        let declared = r.declared_tools();
        // Exactly one entry for "shared" — first by sorted server name (a_server).
        assert_eq!(declared.iter().filter(|t| t.name == "shared").count(), 1);
        assert_eq!(
            declared
                .iter()
                .find(|t| t.name == "shared")
                .unwrap()
                .description
                .as_deref(),
            Some("from a")
        );
    }

    #[test]
    fn escalations_dedupe_per_server() {
        let mcp = Arc::new(McpRegistry::from_config(&[], None));
        let r = RoutingToolDispatcher::new(Arc::new(EngineStub), mcp, &[], &HashMap::new());
        // A permanently-exhausted server is hit by every later tool
        // call — only the first must escalate.
        r.record_escalation("flaky", 5);
        r.record_escalation("flaky", 5);
        r.record_escalation("other", 5);
        let drained = r.drain_mcp_escalations();
        assert_eq!(drained.len(), 2, "one escalation per server, got {drained:?}");
        // A post-drain repeat of an already-escalated server stays
        // suppressed: the give-up fact is recorded once per run.
        r.record_escalation("flaky", 5);
        assert!(
            r.drain_mcp_escalations().is_empty(),
            "re-escalation after drain must stay suppressed"
        );
    }

    #[tokio::test]
    async fn unknown_tool_is_unsupported() {
        let mcp = Arc::new(McpRegistry::from_config(&[], None));
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
