//! Triage Author dispatcher: assembles inputs, parses LLM output.
//!
//! This module owns the input/output schema for the bootstrap-stage 0
//! Triage Author. The actual agent invocation (LLM call via ACP) will
//! be wired in a follow-up polish task; for now this module provides
//! the typed interface.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;
use surge_intake::types::{Priority, TaskDetails, TaskId, TaskSummary, TriageDecision};

/// Full input bundle handed to Triage Author at the start of its session.
///
/// Serialised as the top-level JSON the agent receives. The agent reads
/// `task` (the new ticket), `candidates` (similar open tickets +
/// recent specs), and `active_runs` (current Surge runs).
#[derive(Debug, Clone, Serialize)]
pub struct TriageInput {
    pub task: TaskDetails,
    pub candidates: Vec<TaskSummary>,
    pub active_runs: Vec<ActiveRunSummary>,
}

/// Snapshot of an active Surge run for triage dedup context.
#[derive(Debug, Clone, Serialize)]
pub struct ActiveRunSummary {
    pub run_id: String,
    pub task_id: Option<String>,
    pub status: String,
    pub started_at: String,
}

/// Raw JSON shape Triage Author writes to `triage_decision.json`.
///
/// The agent fills as many fields as apply for its chosen decision; this
/// type is intentionally permissive (all variant-specific fields are
/// `Option`/`#[serde(default)]`) so we can normalise into the strict
/// [`TriageDecision`] enum via [`Self::into_decision`].
#[derive(Debug, Clone, Deserialize)]
pub struct TriageJson {
    pub decision: String,
    #[serde(default)]
    pub duplicate_of: Option<String>,
    #[serde(default)]
    pub priority: Option<String>,
    #[serde(default)]
    pub priority_reasoning: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub question: Option<String>,
}

impl TriageJson {
    /// Convert the loose JSON into a strict [`TriageDecision`].
    ///
    /// Returns `Err(message)` if required fields for the chosen decision
    /// are missing (e.g. `duplicate` without `duplicate_of`) or the
    /// decision/priority strings are unrecognised.
    pub fn into_decision(self) -> Result<TriageDecision, String> {
        let prio_str = self.priority.as_deref().unwrap_or("medium");
        let priority = match prio_str {
            "urgent" => Priority::Urgent,
            "high" => Priority::High,
            "medium" => Priority::Medium,
            "low" => Priority::Low,
            other => return Err(format!("unknown priority: {other}")),
        };
        match self.decision.as_str() {
            "enqueued" => Ok(TriageDecision::Enqueued {
                priority,
                reasoning: self.priority_reasoning.unwrap_or_default(),
                summary: self.summary.unwrap_or_default(),
            }),
            "duplicate" => {
                let dup = self
                    .duplicate_of
                    .ok_or_else(|| "duplicate decision requires duplicate_of".to_string())?;
                let id = TaskId::try_new(dup).map_err(|e| format!("invalid duplicate_of: {e}"))?;
                Ok(TriageDecision::Duplicate {
                    of: id,
                    reasoning: self.priority_reasoning.unwrap_or_default(),
                })
            },
            "out_of_scope" => Ok(TriageDecision::OutOfScope {
                reasoning: self.priority_reasoning.unwrap_or_default(),
            }),
            "unclear" => Ok(TriageDecision::Unclear {
                question: self
                    .question
                    .unwrap_or_else(|| "no question provided".into()),
            }),
            other => Err(format!("unknown decision: {other}")),
        }
    }
}

/// Tunable parameters for [`dispatch_triage`].
///
/// Construct via [`Self::with_scratch_root`] for the typical case of
/// passing only the per-task scratch root and Claude binary path.
#[derive(Debug, Clone)]
pub struct TriageOptions {
    /// Resolved Claude binary path. If `None`, the dispatcher
    /// returns `Ok(TriageDecision::Unclear)` immediately on the
    /// first attempt with a configuration-hint message.
    pub claude_binary: Option<PathBuf>,
    /// Per-attempt timeout. Default: 5 min (matches RFC-0010 §"Bootstrap stage failures").
    pub attempt_timeout: Duration,
    /// Maximum attempts before falling back to `Unclear`. Default: 3.
    pub max_attempts: u32,
    /// Root directory for per-call scratch dirs.
    pub scratch_root: PathBuf,
    /// Whether to keep scratch on Unclear / TriageError for post-mortem.
    pub keep_scratch_on_failure: bool,
}

impl TriageOptions {
    /// Build options with sensible defaults given a scratch root and
    /// (optional) Claude binary path.
    #[must_use]
    pub fn with_scratch_root(scratch_root: PathBuf, claude_binary: Option<PathBuf>) -> Self {
        Self {
            claude_binary,
            attempt_timeout: Duration::from_secs(300),
            max_attempts: 3,
            scratch_root,
            keep_scratch_on_failure: true,
        }
    }
}

/// Errors returned from [`dispatch_triage`] for invariant violations.
///
/// Note: retry-eligible failures (timeout, agent crash, malformed JSON)
/// are NOT surfaced as `TriageError` — they retry up to
/// `opts.max_attempts` times and on exhaustion become
/// `Ok(TriageDecision::Unclear { question })`. `TriageError` is
/// reserved for failures that prevent any forward progress.
#[derive(Debug, thiserror::Error)]
pub enum TriageError {
    /// Could not create or write to the per-call scratch directory.
    #[error("scratch dir setup failed: {0}")]
    Scratch(#[from] std::io::Error),
    /// Bridge facade itself failed at the JSON-RPC / process level
    /// (open_session / send_message / close_session).
    #[error("acp bridge: {0}")]
    Bridge(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_enqueued() {
        let raw = r#"{"decision":"enqueued","priority":"high","priority_reasoning":"prod crash","summary":"Fix panic"}"#;
        let parsed: TriageJson = serde_json::from_str(raw).unwrap();
        let dec = parsed.into_decision().unwrap();
        match dec {
            TriageDecision::Enqueued { priority, .. } => assert_eq!(priority, Priority::High),
            other => panic!("expected Enqueued, got {other:?}"),
        }
    }

    #[test]
    fn parse_duplicate_requires_duplicate_of() {
        let raw = r#"{"decision":"duplicate"}"#;
        let parsed: TriageJson = serde_json::from_str(raw).unwrap();
        let err = parsed.into_decision().unwrap_err();
        assert!(err.contains("duplicate_of"));
    }

    #[test]
    fn parse_unknown_priority_errors() {
        let raw = r#"{"decision":"enqueued","priority":"super-urgent"}"#;
        let parsed: TriageJson = serde_json::from_str(raw).unwrap();
        let err = parsed.into_decision().unwrap_err();
        assert!(err.contains("unknown priority"));
    }

    #[test]
    fn parse_out_of_scope() {
        let raw = r#"{"decision":"out_of_scope","priority":"low","priority_reasoning":"not a coding task"}"#;
        let parsed: TriageJson = serde_json::from_str(raw).unwrap();
        let dec = parsed.into_decision().unwrap();
        assert!(matches!(dec, TriageDecision::OutOfScope { .. }));
    }

    #[test]
    fn parse_unclear() {
        let raw = r#"{"decision":"unclear","priority":"medium","question":"What does this mean?"}"#;
        let parsed: TriageJson = serde_json::from_str(raw).unwrap();
        let dec = parsed.into_decision().unwrap();
        match dec {
            TriageDecision::Unclear { question } => assert_eq!(question, "What does this mean?"),
            other => panic!("expected Unclear, got {other:?}"),
        }
    }

    #[test]
    fn parse_unknown_decision_errors() {
        let raw = r#"{"decision":"meow","priority":"medium"}"#;
        let parsed: TriageJson = serde_json::from_str(raw).unwrap();
        let err = parsed.into_decision().unwrap_err();
        assert!(err.contains("unknown decision"));
    }
}
