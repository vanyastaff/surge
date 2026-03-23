//! Error types for Surge.

#[derive(Debug, thiserror::Error)]
pub enum SurgeError {
    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Agent not found: {0}")]
    AgentNotFound(String),

    #[error("Agent connection failed: {0}")]
    AgentConnection(String),

    #[error("Spec error: {0}")]
    Spec(String),

    #[error("Git error: {0}")]
    Git(String),

    #[error("Task in invalid state: expected {expected}, got {actual}")]
    InvalidState { expected: String, actual: String },

    #[error("ACP protocol error: {0}")]
    Acp(String),

    #[error("Operation timed out: {0}")]
    Timeout(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),
}
