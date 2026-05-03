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
    pub call_id: String,
    pub tool: String,
    pub arguments: serde_json::Value,
}

/// Result payload returned to the bridge in reply to a tool call.
///
/// Mirror of the ACP `tools::ToolResultPayload` shape — duplicated here so
/// engine code doesn't have to depend on the ACP crate's tool types directly.
/// Engine wraps/unwraps at the boundary in `stage::agent`.
#[derive(Debug, Clone, PartialEq)]
pub enum ToolResultPayload {
    Ok { content: serde_json::Value },
    Error { message: String },
    Unsupported { message: String },
    Cancelled,
}

/// Per-call context handed to the dispatcher.
pub struct ToolDispatchContext<'a> {
    pub run_id: RunId,
    pub session_id: SessionId,
    pub worktree_root: &'a Path,
    pub run_memory: &'a RunMemory,
}

/// Routes non-special ACP tool calls to implementations. Engine calls
/// `dispatch` for every ToolCall whose name is not `report_stage_outcome`
/// or `request_human_input` (those are engine-handled).
#[async_trait]
pub trait ToolDispatcher: Send + Sync {
    async fn dispatch(
        &self,
        ctx: &ToolDispatchContext<'_>,
        call: &ToolCall,
    ) -> ToolResultPayload;
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
}
