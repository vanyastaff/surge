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

    #[error("Git error: {message}")]
    Git {
        message: String,
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },

    #[error("Task in invalid state: expected {expected}, got {actual}")]
    InvalidState { expected: String, actual: String },

    #[error("ACP protocol error: {0}")]
    Acp(String),

    #[error("Operation timed out: {0}")]
    Timeout(String),

    /// Agent returned HTTP 429 — rate limit hit.
    #[error(
        "Rate limit exceeded for agent '{agent}': retry after {retry_after_secs}s (attempt {attempt_count})"
    )]
    RateLimit {
        agent: String,
        retry_after_secs: u64,
        attempt_count: u32,
        next_retry_time: Option<std::time::SystemTime>,
    },

    /// Authentication failed with remediation guidance.
    #[error("Authentication failed for agent '{agent}': {remediation}")]
    AuthFailure { agent: String, remediation: String },

    /// Operation was cancelled by user or pipeline gate.
    #[error("Cancelled: {0}")]
    Cancelled(String),

    /// Resource not found (spec file, worktree, etc.).
    #[error("Not found: {0}")]
    NotFound(String),

    /// Graph validation produced one or more errors.
    #[error("Graph validation failed with {count} errors", count = .0.len())]
    GraphValidation(Vec<crate::validation::ValidationError>),

    /// Folding events into RunState failed.
    #[error("Event fold failed: {0}")]
    EventFold(#[from] crate::run_state::FoldError),

    /// Profile TOML could not be parsed.
    #[error("Profile parse error: {0}")]
    ProfileParse(String),

    /// Stored content hash didn't match recomputed hash.
    #[error("Content hash mismatch: expected {expected}, got {actual}")]
    ContentHashMismatch {
        expected: crate::content_hash::ContentHash,
        actual: crate::content_hash::ContentHash,
    },

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl SurgeError {
    /// Construct a `Git` error with a message and no source.
    pub fn git(message: impl Into<String>) -> Self {
        Self::Git {
            message: message.into(),
            source: None,
        }
    }

    /// Construct a `Git` error with a message and a source error.
    pub fn git_source(
        message: impl Into<String>,
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> Self {
        Self::Git {
            message: message.into(),
            source: Some(Box::new(source)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rate_limit_display() {
        let err = SurgeError::RateLimit {
            agent: "claude".to_string(),
            retry_after_secs: 30,
            attempt_count: 2,
            next_retry_time: None,
        };
        let msg = err.to_string();
        assert!(msg.contains("claude"));
        assert!(msg.contains("30"));
        assert!(msg.contains("attempt 2"));
    }

    #[test]
    fn test_auth_failure_display() {
        let err = SurgeError::AuthFailure {
            agent: "claude".to_string(),
            remediation: "Check API key in config".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("claude"));
        assert!(msg.contains("Check API key in config"));
    }

    #[test]
    fn test_git_error_display() {
        let err = SurgeError::git("branch not found");
        assert_eq!(err.to_string(), "Git error: branch not found");
    }

    #[test]
    fn test_git_error_source_chain() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file missing");
        let err = SurgeError::git_source("failed to open repo", io_err);
        assert!(err.to_string().contains("failed to open repo"));
        use std::error::Error;
        assert!(err.source().is_some());
    }

    #[test]
    fn graph_validation_error_displays_count() {
        let err = SurgeError::GraphValidation(vec![]);
        assert!(err.to_string().contains("0 errors"));
    }

    #[test]
    fn content_hash_mismatch_shows_both_hashes() {
        let a = crate::content_hash::ContentHash::compute(b"a");
        let b = crate::content_hash::ContentHash::compute(b"b");
        let err = SurgeError::ContentHashMismatch { expected: a, actual: b };
        let msg = err.to_string();
        assert!(msg.contains("sha256:"));
    }
}
