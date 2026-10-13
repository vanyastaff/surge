//! The Triage Author's decision artifact, `triage_decision.json` (#119).
//!
//! One typed shape with one validation rule, used both as the artifact
//! contract an agent invocation checks in-session and by the daemon's intake
//! when it turns an accepted decision into an inbox action.

use serde::{Deserialize, Serialize};

/// Canonical file name of the decision artifact.
pub const TRIAGE_DECISION_PATH: &str = "triage_decision.json";

/// What the Triage Author decided about a ticket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriageVerdict {
    /// Work on it, with a priority and a summary.
    Enqueued,
    /// It duplicates an existing task (`duplicate_of`).
    Duplicate,
    /// It does not belong to this project.
    OutOfScope,
    /// It needs a clarifying `question` first.
    Unclear,
}

/// Priority of an enqueued ticket.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriagePriority {
    /// Drop everything.
    Urgent,
    /// Next up.
    High,
    /// The default when none is given.
    #[default]
    Medium,
    /// When there is time.
    Low,
}

/// `triage_decision.json` as the agent writes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriageDecisionArtifact {
    /// The decision.
    pub decision: TriageVerdict,
    /// Task id this duplicates; required for [`TriageVerdict::Duplicate`].
    #[serde(default)]
    pub duplicate_of: Option<String>,
    /// Priority; [`TriagePriority::Medium`] when absent.
    #[serde(default)]
    pub priority: Option<TriagePriority>,
    /// One sentence on the priority (or the duplicate / scope call).
    #[serde(default)]
    pub priority_reasoning: Option<String>,
    /// One-sentence summary.
    #[serde(default)]
    pub summary: Option<String>,
    /// The clarifying question; required for [`TriageVerdict::Unclear`].
    #[serde(default)]
    pub question: Option<String>,
}

/// Why a parsed decision is not acceptable.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TriageDecisionIssue {
    /// A duplicate without the task it duplicates.
    #[error("decision \"duplicate\" requires a non-empty duplicate_of")]
    DuplicateWithoutTarget,
    /// An unclear decision without its question.
    #[error("decision \"unclear\" requires a non-empty question")]
    UnclearWithoutQuestion,
}

impl TriageDecisionArtifact {
    /// Parse `triage_decision.json`.
    ///
    /// # Errors
    /// The JSON error, naming an unknown `decision` or `priority` value.
    pub fn parse(content: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(content)
    }

    /// Fields the chosen decision requires but the artifact lacks.
    #[must_use]
    pub fn validate(&self) -> Vec<TriageDecisionIssue> {
        let present =
            |value: &Option<String>| value.as_deref().is_some_and(|v| !v.trim().is_empty());
        let mut issues = Vec::new();
        match self.decision {
            TriageVerdict::Duplicate if !present(&self.duplicate_of) => {
                issues.push(TriageDecisionIssue::DuplicateWithoutTarget);
            },
            TriageVerdict::Unclear if !present(&self.question) => {
                issues.push(TriageDecisionIssue::UnclearWithoutQuestion);
            },
            _ => {},
        }
        issues
    }

    /// The priority, defaulting to [`TriagePriority::Medium`].
    #[must_use]
    pub fn priority(&self) -> TriagePriority {
        self.priority.unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decisions_parse_and_validate_per_kind() {
        let enqueued = TriageDecisionArtifact::parse(
            r#"{"decision":"enqueued","priority":"high","summary":"s"}"#,
        )
        .unwrap();
        assert_eq!(enqueued.validate(), []);
        assert_eq!(enqueued.priority(), TriagePriority::High);

        let duplicate =
            TriageDecisionArtifact::parse(r#"{"decision":"duplicate","duplicate_of":" "}"#)
                .unwrap();
        assert_eq!(
            duplicate.validate(),
            [TriageDecisionIssue::DuplicateWithoutTarget]
        );

        let unclear = TriageDecisionArtifact::parse(r#"{"decision":"unclear"}"#).unwrap();
        assert_eq!(
            unclear.validate(),
            [TriageDecisionIssue::UnclearWithoutQuestion]
        );
        assert_eq!(unclear.priority(), TriagePriority::Medium);
    }

    #[test]
    fn unknown_values_are_parse_errors() {
        assert!(TriageDecisionArtifact::parse(r#"{"decision":"maybe"}"#).is_err());
        assert!(
            TriageDecisionArtifact::parse(r#"{"decision":"enqueued","priority":"asap"}"#).is_err()
        );
    }
}
