//! Pipeline phase definitions.

use serde::{Deserialize, Serialize};

/// Pipeline phases for task execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Phase {
    Planning,
    Executing,
    QaReview,
    QaFix,
    HumanReview,
    Merging,
}

impl std::fmt::Display for Phase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Planning => write!(f, "Planning"),
            Self::Executing => write!(f, "Executing"),
            Self::QaReview => write!(f, "QA Review"),
            Self::QaFix => write!(f, "QA Fix"),
            Self::HumanReview => write!(f, "Human Review"),
            Self::Merging => write!(f, "Merging"),
        }
    }
}
