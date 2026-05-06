//! Triage Author dispatcher: assembles inputs, parses LLM output.
//!
//! This module owns the input/output schema for the bootstrap-stage 0
//! Triage Author. The actual agent invocation (LLM call via ACP) will
//! be wired in a follow-up polish task; for now this module provides
//! the typed interface.

use serde::{Deserialize, Serialize};
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
