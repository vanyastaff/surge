//! Error types for Surge.

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
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

    /// Persisted event payload uses a schema version older than the supported minimum.
    #[error("event payload schema {found} is older than supported minimum {min}")]
    SchemaTooOld { found: u32, min: u32 },

    /// Persisted event payload uses a schema version newer than this build can read.
    #[error("event payload schema {found} is newer than supported maximum {max}")]
    SchemaTooNew { found: u32, max: u32 },

    #[error(transparent)]
    Io(#[from] std::io::Error),

    // ── Profile registry errors (Profile registry & bundled roles milestone) ──
    /// A profile reference resolved to no on-disk file and no bundled fallback.
    #[error("profile not found: {0}")]
    ProfileNotFound(String),

    /// A profile reference asked for a specific version that does not match the
    /// version recorded in any candidate file's `[role] version = "..."`.
    #[error("profile version mismatch for {name}: requested {requested}, available {available:?}")]
    ProfileVersionMismatch {
        name: String,
        requested: String,
        available: Vec<String>,
    },

    /// `extends` resolution detected a cycle in the parent chain.
    #[error("profile extends cycle detected: {chain:?}")]
    ProfileExtendsCycle { chain: Vec<String> },

    /// `extends` resolution exceeded `MAX_EXTENDS_DEPTH`.
    #[error("profile extends chain exceeded max depth {max}: {chain:?}")]
    ProfileExtendsTooDeep { max: usize, chain: Vec<String> },

    /// A merge_chain step encountered conflicting fields it could not safely
    /// reconcile (e.g. divergent enum-tagged unions).
    #[error("profile field conflict in {field}: {detail}")]
    ProfileFieldConflict { field: String, detail: String },

    /// A profile key reference (`name@version`) failed to parse.
    #[error("invalid profile key reference: {0}")]
    InvalidProfileKey(String),

    // ── Bootstrap errors (Bootstrap & adaptive flow generation milestone) ──
    /// A bootstrap stage transition referenced a stage that has not been started.
    #[error("bootstrap stage missing: {stage}")]
    BootstrapStageMissing { stage: String },

    /// The bootstrap edit-loop cap was exceeded for a given stage.
    #[error("bootstrap edit loop cap exceeded for stage {stage}: cap={cap}")]
    EditLoopCapExceeded { stage: String, cap: u32 },

    /// Flow Generator output failed validation more times than the configured retry budget.
    #[error("bootstrap validation retry exhausted at stage {stage}")]
    ValidationRetryExhausted { stage: String },

    /// The materialized pipeline graph emitted by Flow Generator is invalid.
    #[error("materialized pipeline graph invalid: {reason}")]
    MaterializedGraphInvalid { reason: String },

    /// Flow Generator output omitted the required `[metadata.archetype]` block.
    #[error("bootstrap archetype block missing in materialized graph")]
    BootstrapArchetypeMissing,

    /// The declared archetype does not match the detected graph topology.
    #[error("bootstrap archetype mismatch: declared {declared}, detected {detected}")]
    BootstrapArchetypeMismatch { declared: String, detected: String },
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
        let err = SurgeError::ContentHashMismatch {
            expected: a,
            actual: b,
        };
        let msg = err.to_string();
        assert!(msg.contains("sha256:"));
    }

    #[test]
    fn schema_too_old_displays_found_and_min() {
        let err = SurgeError::SchemaTooOld { found: 0, min: 1 };
        let msg = err.to_string();
        assert!(msg.contains("0"));
        assert!(msg.contains("1"));
        assert!(msg.contains("older"));
    }

    #[test]
    fn schema_too_new_displays_found_and_max() {
        let err = SurgeError::SchemaTooNew { found: 999, max: 1 };
        let msg = err.to_string();
        assert!(msg.contains("999"));
        assert!(msg.contains("newer"));
    }

    #[test]
    fn edit_loop_cap_exceeded_includes_stage_and_cap() {
        let err = SurgeError::EditLoopCapExceeded {
            stage: "flow".into(),
            cap: 3,
        };
        let msg = err.to_string();
        assert!(msg.contains("flow"));
        assert!(msg.contains("cap=3"));
    }

    #[test]
    fn bootstrap_archetype_mismatch_shows_both_sides() {
        let err = SurgeError::BootstrapArchetypeMismatch {
            declared: "multi-milestone".into(),
            detected: "linear-3".into(),
        };
        let msg = err.to_string();
        assert!(msg.contains("multi-milestone"));
        assert!(msg.contains("linear-3"));
    }

    #[test]
    fn materialized_graph_invalid_carries_reason() {
        let err = SurgeError::MaterializedGraphInvalid {
            reason: "missing start node".into(),
        };
        assert!(err.to_string().contains("missing start node"));
    }

    #[test]
    fn bootstrap_archetype_missing_renders() {
        let err = SurgeError::BootstrapArchetypeMissing;
        assert!(err.to_string().contains("archetype"));
    }
}
