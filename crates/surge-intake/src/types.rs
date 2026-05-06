//! Shared types for `surge-intake`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Identifier of an external ticket, formatted as `provider:scope#id`.
///
/// Examples:
/// - `"github_issues:user/repo#1234"`
/// - `"linear:wsp_acme/ABC-42"`
///
/// `TaskId` is opaque; the only operation supported is creation from a string
/// (via `try_new`) and serialization. Provider-specific parsing belongs to
/// the implementation, not to this type.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TaskId(String);

impl TaskId {
    pub fn try_new(s: impl Into<String>) -> Result<Self, String> {
        let s = s.into();
        if s.is_empty() {
            return Err("task id must not be empty".into());
        }
        if !s.contains(':') {
            return Err(format!("task id must contain provider prefix: {s}"));
        }
        Ok(Self(s))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TaskId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty() {
        assert!(TaskId::try_new("").is_err());
    }

    #[test]
    fn rejects_no_provider_prefix() {
        assert!(TaskId::try_new("just-a-string").is_err());
    }

    #[test]
    fn accepts_valid() {
        let id = TaskId::try_new("github_issues:user/repo#1234").unwrap();
        assert_eq!(id.as_str(), "github_issues:user/repo#1234");
    }

    #[test]
    fn round_trip_serde_json() {
        let id = TaskId::try_new("linear:wsp_acme/ABC-42").unwrap();
        let s = serde_json::to_string(&id).unwrap();
        assert_eq!(s, "\"linear:wsp_acme/ABC-42\"");
        let back: TaskId = serde_json::from_str(&s).unwrap();
        assert_eq!(back, id);
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn round_trip(provider in "[a-z_]{3,15}", scope in "[a-zA-Z0-9_/-]{1,40}", num in 0u32..1_000_000) {
            let raw = format!("{provider}:{scope}#{num}");
            let id = TaskId::try_new(&raw).unwrap();
            let s = serde_json::to_string(&id).unwrap();
            let back: TaskId = serde_json::from_str(&s).unwrap();
            prop_assert_eq!(back, id);
        }
    }
}

/// Priority levels assigned by Triage Author from ticket text and labels.
///
/// Ordering reflects scheduling precedence: `Urgent > High > Medium > Low`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Priority {
    Low,
    Medium,
    High,
    Urgent,
}

impl Priority {
    /// Stable string label, used in tracker labels (`surge-priority/<level>`).
    pub fn label(&self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Urgent => "urgent",
        }
    }
}

/// Triage Author's verdict on whether a ticket should enter the bootstrap pipeline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "decision")]
pub enum TriageDecision {
    Enqueued {
        priority: Priority,
        reasoning: String,
        summary: String,
    },
    Duplicate {
        of: TaskId,
        reasoning: String,
    },
    OutOfScope {
        reasoning: String,
    },
    Unclear {
        question: String,
    },
}

/// Output of Tier-1 (computational) dedup pre-filter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tier1Decision {
    /// New ticket: pass to Triage Author.
    Pass,
    /// Already an active run for this exact ticket; skip the LLM stage entirely.
    EarlyDuplicate { run_id: String },
}

#[cfg(test)]
mod priority_tests {
    use super::*;

    #[test]
    fn priority_ordering() {
        assert!(Priority::Urgent > Priority::High);
        assert!(Priority::High > Priority::Medium);
        assert!(Priority::Medium > Priority::Low);
    }

    #[test]
    fn priority_label_is_stable() {
        assert_eq!(Priority::Urgent.label(), "urgent");
        assert_eq!(Priority::Low.label(), "low");
    }

    #[test]
    fn priority_serializes_as_lowercase() {
        let s = serde_json::to_string(&Priority::High).unwrap();
        assert_eq!(s, "\"high\"");
    }
}

#[cfg(test)]
mod triage_decision_tests {
    use super::*;

    #[test]
    fn enqueued_round_trip() {
        let d = TriageDecision::Enqueued {
            priority: Priority::High,
            reasoning: "production crash".into(),
            summary: "Fix panic".into(),
        };
        let s = serde_json::to_string(&d).unwrap();
        let back: TriageDecision = serde_json::from_str(&s).unwrap();
        assert_eq!(back, d);
    }

    #[test]
    fn duplicate_round_trip() {
        let d = TriageDecision::Duplicate {
            of: TaskId::try_new("github_issues:user/repo#1").unwrap(),
            reasoning: "same parser path".into(),
        };
        let s = serde_json::to_string(&d).unwrap();
        let back: TriageDecision = serde_json::from_str(&s).unwrap();
        assert_eq!(back, d);
    }
}

/// Kind of change observed on a task by a `TaskSource`.
///
/// `NewTask` is the default for first-time observation; `StatusChanged`
/// and `LabelsChanged` carry deltas; `TaskClosed` indicates the task is
/// no longer active in the source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum TaskEventKind {
    NewTask,
    StatusChanged {
        from: String,
        to: String,
    },
    LabelsChanged {
        added: Vec<String>,
        removed: Vec<String>,
    },
    TaskClosed,
}

/// One observation of an external task at a point in time.
///
/// Emitted by a `TaskSource` from its polling or webhook stream and
/// consumed by the `TaskRouter` (Tier-1 dedup → Triage Author).
/// `raw_payload` is the unmodified provider response body, kept so
/// downstream components can read provider-specific fields without
/// inflating the typed schema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskEvent {
    pub source_id: String,
    pub task_id: TaskId,
    pub kind: TaskEventKind,
    pub seen_at: DateTime<Utc>,
    pub raw_payload: serde_json::Value,
}

/// Full snapshot of an external task, fetched on demand.
///
/// Contains everything Triage Author needs to reason about a ticket.
/// Like `TaskEvent`, `raw_payload` retains provider-specific fields
/// outside the typed schema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskDetails {
    pub task_id: TaskId,
    pub source_id: String,
    pub title: String,
    pub description: String,
    pub status: String,
    pub labels: Vec<String>,
    pub url: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub assignee: Option<String>,
    #[serde(default)]
    pub raw_payload: serde_json::Value,
}

/// Compact representation of a task for list views.
///
/// Subset of `TaskDetails`; used for candidate enumeration and the
/// `list_open_tasks` API where the body and labels aren't needed yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskSummary {
    pub task_id: TaskId,
    pub title: String,
    pub status: String,
    pub url: String,
    pub updated_at: DateTime<Utc>,
}

#[cfg(test)]
mod event_tests {
    use super::*;

    fn sample_task_id() -> TaskId {
        TaskId::try_new("github_issues:user/repo#1234").unwrap()
    }

    #[test]
    fn event_round_trip_new_task() {
        let ev = TaskEvent {
            source_id: "github_issues:user/repo".into(),
            task_id: sample_task_id(),
            kind: TaskEventKind::NewTask,
            seen_at: DateTime::parse_from_rfc3339("2026-05-06T10:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
            raw_payload: serde_json::json!({"id": 1234}),
        };
        let s = serde_json::to_string(&ev).unwrap();
        let back: TaskEvent = serde_json::from_str(&s).unwrap();
        assert_eq!(back, ev);
    }

    #[test]
    fn event_round_trip_labels_changed() {
        let ev = TaskEvent {
            source_id: "linear:wsp1".into(),
            task_id: TaskId::try_new("linear:wsp1/ABC-42").unwrap(),
            kind: TaskEventKind::LabelsChanged {
                added: vec!["surge:enabled".into()],
                removed: vec![],
            },
            seen_at: Utc::now(),
            raw_payload: serde_json::json!({}),
        };
        let s = serde_json::to_string(&ev).unwrap();
        let back: TaskEvent = serde_json::from_str(&s).unwrap();
        assert_eq!(back, ev);
    }

    #[test]
    fn details_round_trip() {
        let d = TaskDetails {
            task_id: sample_task_id(),
            source_id: "github_issues:user/repo".into(),
            title: "Fix parser panic".into(),
            description: "Stack overflow on deep nesting".into(),
            status: "open".into(),
            labels: vec!["surge:enabled".into(), "priority/high".into()],
            url: "https://github.com/user/repo/issues/1234".into(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            assignee: None,
            raw_payload: serde_json::json!({}),
        };
        let s = serde_json::to_string(&d).unwrap();
        let back: TaskDetails = serde_json::from_str(&s).unwrap();
        assert_eq!(back, d);
    }
}
