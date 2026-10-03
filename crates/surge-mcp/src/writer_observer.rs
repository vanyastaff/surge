//! Host-supplied writer ownership; MCP transport never invents cleanup proof.

use surge_core::id::ExecutionWriterId;

/// A durable ownership observation failed before a child side effect.
#[derive(Debug, thiserror::Error)]
#[error("MCP writer ownership observation failed: {0}")]
pub struct WriterObservationError(pub String);

/// Narrow host boundary injected by the engine without importing storage or ACP.
#[async_trait::async_trait]
pub trait HostWriterObserver: Send + Sync {
    /// Commit a distinct ownership intent before each child launch, including respawns.
    async fn before_child(&self, server: &str)
    -> Result<ExecutionWriterId, WriterObservationError>;
    /// Commit the actual spawned identity before MCP initialization may write.
    async fn child_started(
        &self,
        writer: ExecutionWriterId,
        pid: Option<u32>,
    ) -> Result<(), WriterObservationError>;
}
