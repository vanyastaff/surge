//! Per-session authenticated stage-tool transport. Engine semantics stay in the orchestrator.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use surge_core::stage_tool::StageToolContext;
use tokio::sync::oneshot;

/// Catalog entry exposed by the session's stdio MCP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageToolDefinition {
    /// Exact tool name.
    pub name: String,
    /// User-facing description.
    pub description: String,
    /// JSON Schema after sandbox visibility filtering.
    pub input_schema: Value,
}

/// A reliable, authenticated call delivered to the owning stage.
pub struct StageToolCall {
    /// Server-pinned identity; not accepted from model arguments.
    pub context: StageToolContext,
    /// Catalog tool name.
    pub tool: String,
    /// Arguments including the required durable `call_id`.
    pub arguments: Value,
    /// Reply only after durable acceptance. Receiver drop cancels the call.
    pub reply: oneshot::Sender<StageToolReply>,
}

/// MCP result, distinct from successful stage completion.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageToolReply {
    /// Whether the tool rejected the call.
    pub is_error: bool,
    /// JSON result returned to the provider.
    pub value: Value,
}

impl StageToolReply {
    /// Return an explicit tool error without fabricating a result.
    #[must_use]
    pub fn rejected(reason: impl Into<String>) -> Self {
        Self {
            is_error: true,
            value: serde_json::json!({"error": reason.into()}),
        }
    }
}

/// Endpoint or helper failure. Authentication values are never included.
#[derive(Debug, thiserror::Error)]
pub enum StageTransportError {
    /// Local I/O failure.
    #[error("stage transport I/O: {0}")]
    Io(#[from] std::io::Error),
    /// Invalid local framing or arguments.
    #[error("invalid stage transport message: {0}")]
    Json(#[from] serde_json::Error),
    /// Session ended, authentication failed, or protocol shape was rejected.
    #[error("stage transport rejected: {0}")]
    Rejected(String),
    /// Cleanup was not confirmed.
    #[error("stage transport cleanup could not be confirmed")]
    CleanupUnconfirmed,
}

mod bounded_stdio;
mod helper;
mod local;
mod transport;
pub use helper::serve_stdio;
pub use transport::{AUTH_ENV, ENDPOINT_ENV, StageEndpoint};
