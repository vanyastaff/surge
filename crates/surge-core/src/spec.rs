//! Spec types for Surge task definitions.

use crate::id::{SpecId, SubtaskId};
use serde::{Deserialize, Serialize};

/// Execution state of a single subtask.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SubtaskState {
    #[default]
    Pending,
    Running,
    Completed,
    Failed,
    Skipped,
}

impl SubtaskState {
    /// Returns `true` if no further execution will happen for this subtask.
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Skipped)
    }

    /// Returns `true` if the subtask is currently being executed by an agent.
    #[must_use]
    pub fn is_active(self) -> bool {
        matches!(self, Self::Running)
    }
}

/// Execution metadata for a single subtask, persisted to spec.toml.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SubtaskExecution {
    /// Current execution state.
    #[serde(default)]
    pub state: SubtaskState,
    /// When execution started (Unix timestamp ms). None if not yet started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<u64>,
    /// When execution finished (Unix timestamp ms). None if not yet finished.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<u64>,
    /// Error message if state is Failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Complexity level for a task or subtask.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Complexity {
    Simple,
    Standard,
    Complex,
}

/// Acceptance criteria for a subtask.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcceptanceCriteria {
    /// Human-readable description of what must be true.
    pub description: String,
    /// Whether this criterion is currently met.
    #[serde(default)]
    pub met: bool,
}

/// A subtask within a spec.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Subtask {
    /// Unique identifier.
    pub id: SubtaskId,
    /// Short title.
    pub title: String,
    /// Detailed description of work to do.
    pub description: String,
    /// Estimated complexity.
    pub complexity: Complexity,
    /// Files this subtask will touch.
    #[serde(default)]
    pub files: Vec<String>,
    /// Acceptance criteria that must pass.
    #[serde(default)]
    pub acceptance_criteria: Vec<AcceptanceCriteria>,
    /// Dependencies on other subtask IDs (must complete first).
    #[serde(default)]
    pub depends_on: Vec<SubtaskId>,
    /// Relative path to the story file (e.g. `stories/story-001.md`).
    /// When set, the agent receives the story file content instead of field-assembled prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub story_file: Option<String>,
    /// Which agent to use for this subtask (overrides routing config).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// Execution state and timing. Persisted to spec.toml.
    #[serde(default)]
    pub execution: SubtaskExecution,
}

/// A complete spec describing a unit of work.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Spec {
    /// Unique identifier.
    pub id: SpecId,
    /// Short title.
    pub title: String,
    /// Detailed description.
    pub description: String,
    /// Overall complexity.
    pub complexity: Complexity,
    /// Ordered list of subtasks.
    #[serde(default)]
    pub subtasks: Vec<Subtask>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_subtask() -> Subtask {
        Subtask {
            id: SubtaskId::new(),
            title: "t".to_string(),
            description: "d".to_string(),
            complexity: Complexity::Simple,
            files: vec![],
            acceptance_criteria: vec![],
            depends_on: vec![],
            story_file: None,
            agent: None,
            execution: SubtaskExecution::default(),
        }
    }

    #[test]
    fn test_complexity_roundtrip() {
        #[derive(Debug, Serialize, Deserialize, PartialEq)]
        struct Wrapper {
            complexity: Complexity,
        }

        for variant in [
            Complexity::Simple,
            Complexity::Standard,
            Complexity::Complex,
        ] {
            let wrapper = Wrapper {
                complexity: variant,
            };
            let serialized = toml::to_string(&wrapper).unwrap();
            let deserialized: Wrapper = toml::from_str(&serialized).unwrap();
            assert_eq!(deserialized.complexity, variant);
        }
    }

    #[test]
    fn test_spec_toml_roundtrip() {
        let spec = Spec {
            id: SpecId::new(),
            title: "Test spec".to_string(),
            description: "A test specification".to_string(),
            complexity: Complexity::Standard,
            subtasks: vec![Subtask {
                files: vec!["src/main.rs".to_string()],
                acceptance_criteria: vec![AcceptanceCriteria {
                    description: "It compiles".to_string(),
                    met: false,
                }],
                ..make_subtask()
            }],
        };

        let toml_str = toml::to_string(&spec).unwrap();
        let deserialized: Spec = toml::from_str(&toml_str).unwrap();

        assert_eq!(deserialized.id, spec.id);
        assert_eq!(deserialized.title, spec.title);
        assert_eq!(deserialized.description, spec.description);
        assert_eq!(deserialized.complexity, spec.complexity);
        assert_eq!(deserialized.subtasks.len(), 1);
        assert_eq!(deserialized.subtasks[0].title, "t");
        assert_eq!(deserialized.subtasks[0].files, vec!["src/main.rs"]);
        assert_eq!(deserialized.subtasks[0].acceptance_criteria.len(), 1);
        assert_eq!(
            deserialized.subtasks[0].acceptance_criteria[0].description,
            "It compiles"
        );
    }

    #[test]
    fn test_subtask_state_is_terminal() {
        assert!(SubtaskState::Completed.is_terminal());
        assert!(SubtaskState::Failed.is_terminal());
        assert!(SubtaskState::Skipped.is_terminal());
        assert!(!SubtaskState::Pending.is_terminal());
        assert!(!SubtaskState::Running.is_terminal());
    }

    #[test]
    fn test_subtask_state_is_active() {
        assert!(SubtaskState::Running.is_active());
        assert!(!SubtaskState::Pending.is_active());
        assert!(!SubtaskState::Completed.is_active());
        assert!(!SubtaskState::Failed.is_active());
        assert!(!SubtaskState::Skipped.is_active());
    }

    #[test]
    fn test_subtask_state_roundtrip() {
        #[derive(Debug, Serialize, Deserialize, PartialEq)]
        struct Wrapper {
            state: SubtaskState,
        }

        for variant in [
            SubtaskState::Pending,
            SubtaskState::Running,
            SubtaskState::Completed,
            SubtaskState::Failed,
            SubtaskState::Skipped,
        ] {
            let w = Wrapper { state: variant };
            let s = toml::to_string(&w).unwrap();
            let d: Wrapper = toml::from_str(&s).unwrap();
            assert_eq!(d.state, variant);
        }
    }

    #[test]
    fn test_subtask_state_default_is_pending() {
        assert_eq!(SubtaskState::default(), SubtaskState::Pending);
    }

    #[test]
    fn test_subtask_execution_default() {
        let exec = SubtaskExecution::default();
        assert_eq!(exec.state, SubtaskState::Pending);
        assert!(exec.started_at.is_none());
        assert!(exec.finished_at.is_none());
        assert!(exec.error.is_none());
    }

    #[test]
    fn test_subtask_execution_roundtrip() {
        let subtask = Subtask {
            execution: SubtaskExecution {
                state: SubtaskState::Failed,
                started_at: Some(1_700_000_000_000),
                finished_at: Some(1_700_000_005_000),
                error: Some("agent timed out".to_string()),
            },
            ..make_subtask()
        };
        let s = toml::to_string(&subtask).unwrap();
        let d: Subtask = toml::from_str(&s).unwrap();
        assert_eq!(d.execution.state, SubtaskState::Failed);
        assert_eq!(d.execution.started_at, Some(1_700_000_000_000));
        assert_eq!(d.execution.finished_at, Some(1_700_000_005_000));
        assert_eq!(d.execution.error.as_deref(), Some("agent timed out"));
    }

    #[test]
    fn test_subtask_execution_timestamps_omitted_when_none() {
        let subtask = make_subtask();
        let s = toml::to_string(&subtask).unwrap();
        assert!(!s.contains("started_at"));
        assert!(!s.contains("finished_at"));
        assert!(!s.contains("error"));
    }

    #[test]
    fn test_subtask_agent_skipped_when_none() {
        let subtask = make_subtask();
        let s = toml::to_string(&subtask).unwrap();
        assert!(
            !s.contains("agent"),
            "agent field should be omitted when None"
        );
    }

    #[test]
    fn test_subtask_agent_serialized_when_some() {
        let subtask = Subtask {
            agent: Some("claude".to_string()),
            execution: SubtaskExecution {
                state: SubtaskState::Completed,
                ..Default::default()
            },
            ..make_subtask()
        };
        let s = toml::to_string(&subtask).unwrap();
        assert!(s.contains("agent = \"claude\""));
        let d: Subtask = toml::from_str(&s).unwrap();
        assert_eq!(d.agent.as_deref(), Some("claude"));
        assert_eq!(d.execution.state, SubtaskState::Completed);
    }

    #[test]
    fn test_acceptance_criteria_default_met() {
        let toml_str = r#"description = "Tests pass""#;
        let criteria: AcceptanceCriteria = toml::from_str(toml_str).unwrap();
        assert_eq!(criteria.description, "Tests pass");
        assert!(!criteria.met);
    }
}
