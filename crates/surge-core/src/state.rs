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
        matches!(self, Self::Completed | Self::Failed { .. } | Self::Cancelled)
    }

    #[must_use]
    pub fn is_active(&self) -> bool {
        matches!(
            self,
            Self::Planning | Self::Executing { .. } | Self::QaReview | Self::QaFix { .. } | Self::Merging
        )
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
