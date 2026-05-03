//! Engine error taxonomy.

use std::path::PathBuf;
use surge_core::id::RunId;
use surge_core::node::NodeKind;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum EngineError {
    #[error("run is already active in this process: {0}")]
    RunAlreadyActive(RunId),

    #[error("graph validation failed: {0}")]
    GraphInvalid(String),

    #[error("graph contains M6+ feature ({kind:?}); not supported in M5")]
    UnsupportedNodeKind { kind: NodeKind },

    #[error("worktree path does not exist: {0}")]
    WorktreeMissing(PathBuf),

    #[error("storage error: {0}")]
    Storage(String),

    #[error("bridge error: {0}")]
    Bridge(String),

    #[error("run not found: {0}")]
    RunNotFound(RunId),

    #[error("internal engine error: {0}")]
    Internal(String),
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
            EngineError::UnsupportedNodeKind { kind: NodeKind::Loop }
                .to_string()
                .contains("Loop")
        );
    }
}
