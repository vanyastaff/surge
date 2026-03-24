//! Task lifecycle state machine.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskState {
    Draft,
    Planning,
    Planned { subtask_count: usize },
    Executing { completed: usize, total: usize },
    QaReview,
    QaFix { iteration: u32 },
    HumanReview,
    Merging,
    Completed,
    Failed { reason: String },
    Cancelled,
}

impl TaskState {
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed { .. } | Self::Cancelled
        )
    }

    #[must_use]
    pub fn is_active(&self) -> bool {
        matches!(
            self,
            Self::Planning
                | Self::Executing { .. }
                | Self::QaReview
                | Self::QaFix { .. }
                | Self::Merging
        )
    }

    /// Returns `true` for states where the pipeline is paused waiting for
    /// external input (human gate, spec approval, etc.).
    #[must_use]
    pub fn is_waiting(&self) -> bool {
        matches!(self, Self::Draft | Self::Planned { .. } | Self::HumanReview)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_terminal_states() {
        assert!(TaskState::Completed.is_terminal());
        assert!(TaskState::Failed { reason: "x".into() }.is_terminal());
        assert!(TaskState::Cancelled.is_terminal());

        assert!(!TaskState::Draft.is_terminal());
        assert!(
            !TaskState::Executing {
                completed: 0,
                total: 1
            }
            .is_terminal()
        );
    }

    #[test]
    fn test_active_states() {
        assert!(TaskState::Planning.is_active());
        assert!(
            TaskState::Executing {
                completed: 1,
                total: 3
            }
            .is_active()
        );
        assert!(TaskState::QaReview.is_active());
        assert!(TaskState::QaFix { iteration: 2 }.is_active());
        assert!(TaskState::Merging.is_active());

        assert!(!TaskState::Draft.is_active());
        assert!(!TaskState::Planned { subtask_count: 2 }.is_active());
        assert!(!TaskState::HumanReview.is_active());
        assert!(!TaskState::Completed.is_active());
    }

    #[test]
    fn test_waiting_states() {
        assert!(TaskState::Draft.is_waiting());
        assert!(TaskState::Planned { subtask_count: 3 }.is_waiting());
        assert!(TaskState::HumanReview.is_waiting());

        assert!(!TaskState::Planning.is_waiting());
        assert!(
            !TaskState::Executing {
                completed: 0,
                total: 1
            }
            .is_waiting()
        );
        assert!(!TaskState::Completed.is_waiting());
        assert!(!TaskState::Failed { reason: "x".into() }.is_waiting());
    }

    #[test]
    fn test_states_are_mutually_exclusive() {
        // A state can be at most one of: terminal, active, waiting.
        for state in [
            TaskState::Draft,
            TaskState::Planning,
            TaskState::Planned { subtask_count: 1 },
            TaskState::Executing {
                completed: 0,
                total: 1,
            },
            TaskState::QaReview,
            TaskState::QaFix { iteration: 1 },
            TaskState::HumanReview,
            TaskState::Merging,
            TaskState::Completed,
            TaskState::Failed { reason: "x".into() },
            TaskState::Cancelled,
        ] {
            let flags = [state.is_terminal(), state.is_active(), state.is_waiting()];
            let true_count = flags.iter().filter(|&&b| b).count();
            assert!(true_count <= 1, "{state} matched multiple categories");
        }
    }
}

impl std::fmt::Display for TaskState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Draft => write!(f, "Draft"),
            Self::Planning => write!(f, "Planning"),
            Self::Planned { subtask_count } => write!(f, "Planned ({subtask_count} subtasks)"),
            Self::Executing { completed, total } => write!(f, "Executing ({completed}/{total})"),
            Self::QaReview => write!(f, "QA Review"),
            Self::QaFix { iteration } => write!(f, "QA Fix (iteration {iteration})"),
            Self::HumanReview => write!(f, "Human Review"),
            Self::Merging => write!(f, "Merging"),
            Self::Completed => write!(f, "Completed"),
            Self::Failed { reason } => write!(f, "Failed: {reason}"),
            Self::Cancelled => write!(f, "Cancelled"),
        }
    }
}
