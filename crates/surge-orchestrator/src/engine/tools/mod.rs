//! Tool dispatch for engine-driven agent stages.

pub mod path_guard;
pub mod worktree;

use async_trait::async_trait;
use std::path::Path;
use surge_core::id::{RunId, SessionId};
use surge_core::run_state::RunMemory;

/// One ACP tool call observed via the bridge facade.
#[derive(Debug, Clone)]
pub struct ToolCall {
    /// Opaque identifier from the ACP protocol; used to route the result back.
    pub call_id: String,
    /// Registered tool name (e.g. `"read_file"`, `"shell_exec"`).
    pub tool: String,
    /// Raw JSON arguments supplied by the agent.
    pub arguments: serde_json::Value,
}

/// Result payload returned to the bridge in reply to a tool call.
///
/// Mirror of the ACP `tools::ToolResultPayload` shape — duplicated here so
/// engine code doesn't have to depend on the ACP crate's tool types directly.
/// Engine wraps/unwraps at the boundary in `stage::agent`.
#[derive(Debug, Clone, PartialEq)]
pub enum ToolResultPayload {
    /// The tool completed successfully; `content` is the JSON result.
    Ok {
        /// JSON payload returned to the agent.
        content: serde_json::Value,
    },
    /// The tool failed; `message` describes the error.
    Error {
        /// Human-readable error description returned to the agent.
        message: String,
    },
    /// The tool name is not recognized by this dispatcher.
    Unsupported {
        /// Reason string explaining which tool was not found.
        message: String,
    },
    /// The tool call was cancelled (e.g. the run was aborted mid-call).
    Cancelled,
}

/// Per-call context handed to the dispatcher.
pub struct ToolDispatchContext<'a> {
    /// Identifier of the current run.
    pub run_id: RunId,
    /// Identifier of the ACP session that issued the tool call.
    pub session_id: SessionId,
    /// Absolute path to the isolated git worktree for this run.
    pub worktree_root: &'a Path,
    /// Accumulated run memory (artifacts, outcomes, costs).
    pub run_memory: &'a RunMemory,
}

/// Declaration metadata for a single tool the dispatcher offers to
/// agent stages. Used by `RoutingToolDispatcher` (M7 Phase 8) to
/// assemble the session's tool list at session-open time.
#[non_exhaustive]
#[derive(Clone, Debug)]
pub struct DeclaredTool {
    /// Tool name as the agent will see it.
    pub name: String,
    /// Human-readable description shown to the agent.
    pub description: Option<String>,
    /// JSON Schema for the tool's input arguments.
    pub input_schema: serde_json::Value,
}

/// Routes non-special ACP tool calls to implementations. Engine calls
/// `dispatch` for every `ToolCall` whose name is not `report_stage_outcome`
/// or `request_human_input` (those are engine-handled).
#[async_trait]
pub trait ToolDispatcher: Send + Sync {
    /// Dispatch a single tool call and return the result payload.
    async fn dispatch(&self, ctx: &ToolDispatchContext<'_>, call: &ToolCall) -> ToolResultPayload;

    /// Tools this dispatcher declares to agent stages. Default returns
    /// empty; the production `WorktreeToolDispatcher` overrides to
    /// expose its built-in catalog (`read_file`, `write_file`, `shell_exec`,
    /// etc.). Used by `RoutingToolDispatcher` to assemble the
    /// session-level tool list.
    fn declared_tools(&self) -> Vec<DeclaredTool> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Compile-time check that a no-op dispatcher satisfies the trait.
    struct NoOp;

    #[async_trait]
    impl ToolDispatcher for NoOp {
        async fn dispatch(
            &self,
            _ctx: &ToolDispatchContext<'_>,
            call: &ToolCall,
        ) -> ToolResultPayload {
            ToolResultPayload::Unsupported {
                message: format!("noop: {}", call.tool),
            }
        }
    }

    #[tokio::test]
    async fn noop_dispatcher_returns_unsupported() {
        let d = NoOp;
        let ctx = ToolDispatchContext {
            run_id: surge_core::id::RunId::new(),
            session_id: surge_core::id::SessionId::new(),
            worktree_root: Path::new("/tmp"),
            run_memory: &surge_core::run_state::RunMemory::default(),
        };
        let call = ToolCall {
            call_id: "c1".into(),
            tool: "read_file".into(),
            arguments: serde_json::json!({}),
        };
        let result = d.dispatch(&ctx, &call).await;
        match result {
            ToolResultPayload::Unsupported { message } => assert!(message.contains("read_file")),
            other => panic!("expected Unsupported, got {other:?}"),
        }
    }

    #[test]
    fn default_declared_tools_is_empty() {
        let d = NoOp;
        assert!(d.declared_tools().is_empty());
    }

    #[tokio::test]
    async fn worktree_dispatcher_declares_its_tools() {
        use std::path::PathBuf;
        let d = crate::engine::tools::worktree::WorktreeToolDispatcher::new(PathBuf::from("/tmp"));
        let tools = d.declared_tools();
        let names: std::collections::HashSet<&str> =
            tools.iter().map(|t| t.name.as_str()).collect();
        let expected: std::collections::HashSet<&str> = ["read_file", "write_file", "shell_exec"]
            .into_iter()
            .collect();
        assert_eq!(
            names, expected,
            "declared_tools must match dispatch's match arms"
        );
    }
}
