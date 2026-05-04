//! Engine error taxonomy.

use std::path::PathBuf;
use surge_core::id::RunId;
use surge_core::node::NodeKind;
use thiserror::Error;

/// Errors that can be returned by `Engine` methods.
#[derive(Debug, Error)]
pub enum EngineError {
    /// A run with this ID is already executing in this process.
    #[error("run is already active in this process: {0}")]
    RunAlreadyActive(RunId),

    /// The submitted graph failed structural validation.
    #[error("graph validation failed: {0}")]
    GraphInvalid(String),

    /// The graph contains a node kind not supported until M6+.
    #[error("graph contains M6+ feature ({kind:?}); not supported in M5")]
    UnsupportedNodeKind {
        /// The unsupported `NodeKind` variant.
        kind: NodeKind,
    },

    /// The provided worktree path does not exist on disk.
    #[error("worktree path does not exist: {0}")]
    WorktreeMissing(PathBuf),

    /// A persistence-layer operation failed.
    #[error("storage error: {0}")]
    Storage(String),

    /// An ACP bridge operation failed.
    #[error("bridge error: {0}")]
    Bridge(String),

    /// No active run found for the given `RunId`.
    #[error("run not found: {0}")]
    RunNotFound(RunId),

    /// An unexpected internal condition occurred.
    #[error("internal engine error: {0}")]
    Internal(String),

    /// A Loop node references a body subgraph that is not in `graph.subgraphs`.
    #[error("loop body reference {0} not found in graph.subgraphs")]
    LoopBodyMissing(surge_core::keys::SubgraphKey),

    /// A Subgraph node references an inner subgraph that is not in `graph.subgraphs`.
    #[error("subgraph reference {0} not found in graph.subgraphs")]
    SubgraphMissing(surge_core::keys::SubgraphKey),
}

#[cfg(test)]
mod tests {
    use super::*;
    use surge_core::node::NodeKind;

    #[test]
    fn display_messages() {
        assert_eq!(
            EngineError::WorktreeMissing(PathBuf::from("/missing")).to_string(),
            "worktree path does not exist: /missing"
        );
        assert!(
            EngineError::UnsupportedNodeKind {
                kind: NodeKind::Loop
            }
            .to_string()
            .contains("Loop")
        );
    }
}
