//! Task lifecycle state machine.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskState {
    Draft,
    Planning,
    Planned {
        subtask_count: usize,
    },
    Executing {
        completed: usize,
        total: usize,
    },
    QaReview {
        verdict: Option<String>,
        reasoning: Option<String>,
    },
    QaFix {
        iteration: u32,
        verdict: Option<String>,
        reasoning: Option<String>,
    },
    HumanReview,
    Merging,
    Completed,
    Failed {
        reason: String,
    },
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
                | Self::QaReview { .. }
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

    /// Returns `true` when transitioning to this state requires cleanup of
    /// task resources (worktrees, processes, temp files).
    ///
    /// Cleanup is required for all terminal states to enforce the
    /// zero-garbage guarantee.
    #[must_use]
    pub fn requires_cleanup(&self) -> bool {
        self.is_terminal()
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
        assert!(
            TaskState::QaReview {
                verdict: None,
                reasoning: None
            }
            .is_active()
        );
        assert!(
            TaskState::QaFix {
                iteration: 2,
                verdict: None,
                reasoning: None
            }
            .is_active()
        );
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
            TaskState::QaReview {
                verdict: None,
                reasoning: None,
            },
            TaskState::QaFix {
                iteration: 1,
                verdict: None,
                reasoning: None,
            },
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

    #[test]
    fn test_cleanup_required_on_terminal_states() {
        // All terminal states require cleanup (zero-garbage guarantee)
        assert!(TaskState::Completed.requires_cleanup());
        assert!(
            TaskState::Failed {
                reason: "test".into()
            }
            .requires_cleanup()
        );
        assert!(TaskState::Cancelled.requires_cleanup());
    }

    #[test]
    fn test_cleanup_not_required_on_non_terminal_states() {
        // Non-terminal states should not trigger cleanup
        assert!(!TaskState::Draft.requires_cleanup());
        assert!(!TaskState::Planning.requires_cleanup());
        assert!(!TaskState::Planned { subtask_count: 1 }.requires_cleanup());
        assert!(
            !TaskState::Executing {
                completed: 0,
                total: 1
            }
            .requires_cleanup()
        );
        assert!(
            !TaskState::QaReview {
                verdict: None,
                reasoning: None
            }
            .requires_cleanup()
        );
        assert!(
            !TaskState::QaFix {
                iteration: 1,
                verdict: None,
                reasoning: None
            }
            .requires_cleanup()
        );
        assert!(!TaskState::HumanReview.requires_cleanup());
        assert!(!TaskState::Merging.requires_cleanup());
    }

    #[test]
    fn test_cleanup_hooks_on_state_transitions() {
        // Simulate state transitions and verify cleanup is triggered
        // at the right time (only on transitions to terminal states)

        // Test successful completion flow
        let states = vec![
            TaskState::Draft,
            TaskState::Planning,
            TaskState::Planned { subtask_count: 3 },
            TaskState::Executing {
                completed: 0,
                total: 3,
            },
            TaskState::Executing {
                completed: 1,
                total: 3,
            },
            TaskState::Executing {
                completed: 2,
                total: 3,
            },
            TaskState::Executing {
                completed: 3,
                total: 3,
            },
            TaskState::QaReview {
                verdict: Some("APPROVED".into()),
                reasoning: None,
            },
            TaskState::HumanReview,
            TaskState::Merging,
            TaskState::Completed,
        ];

        let mut cleanup_triggered = false;
        for (i, state) in states.iter().enumerate() {
            if state.requires_cleanup() {
                cleanup_triggered = true;
                // Cleanup should only trigger on the last state (Completed)
                assert_eq!(
                    i,
                    states.len() - 1,
                    "Cleanup triggered too early at {state}"
                );
            }
        }
        assert!(
            cleanup_triggered,
            "Cleanup never triggered for completion flow"
        );

        // Test failure flow
        let failure_states = vec![
            TaskState::Draft,
            TaskState::Planning,
            TaskState::Failed {
                reason: "Agent crashed".into(),
            },
        ];

        cleanup_triggered = false;
        for (i, state) in failure_states.iter().enumerate() {
            if state.requires_cleanup() {
                cleanup_triggered = true;
                assert_eq!(i, failure_states.len() - 1, "Cleanup triggered too early");
            }
        }
        assert!(
            cleanup_triggered,
            "Cleanup never triggered for failure flow"
        );

        // Test cancellation flow
        let cancel_states = vec![
            TaskState::Executing {
                completed: 1,
                total: 5,
            },
            TaskState::Cancelled,
        ];

        cleanup_triggered = false;
        for (i, state) in cancel_states.iter().enumerate() {
            if state.requires_cleanup() {
                cleanup_triggered = true;
                assert_eq!(i, cancel_states.len() - 1, "Cleanup triggered too early");
            }
        }
        assert!(
            cleanup_triggered,
            "Cleanup never triggered for cancellation flow"
        );
    }

    #[test]
    fn test_cleanup_invariant() {
        // Invariant: requires_cleanup() == is_terminal()
        // This ensures cleanup is always triggered for terminal states
        // and never for non-terminal states
        for state in [
            TaskState::Draft,
            TaskState::Planning,
            TaskState::Planned { subtask_count: 1 },
            TaskState::Executing {
                completed: 0,
                total: 1,
            },
            TaskState::QaReview {
                verdict: None,
                reasoning: None,
            },
            TaskState::QaFix {
                iteration: 1,
                verdict: None,
                reasoning: None,
            },
            TaskState::HumanReview,
            TaskState::Merging,
            TaskState::Completed,
            TaskState::Failed { reason: "x".into() },
            TaskState::Cancelled,
        ] {
            assert_eq!(
                state.requires_cleanup(),
                state.is_terminal(),
                "Cleanup invariant violated for {state}"
            );
        }
    }

    #[test]
    fn test_qa_review_with_metadata() {
        let state = TaskState::QaReview {
            verdict: Some("NEEDS_FIX".into()),
            reasoning: Some("Missing error handling".into()),
        };
        assert!(state.is_active());
        assert!(!state.is_terminal());
        assert!(!state.is_waiting());

        let display = format!("{state}");
        assert!(display.contains("QA Review"));
        assert!(display.contains("NEEDS_FIX"));
        assert!(display.contains("Missing error handling"));
    }

    #[test]
    fn test_qa_fix_with_metadata() {
        let state = TaskState::QaFix {
            iteration: 2,
            verdict: Some("PARTIAL".into()),
            reasoning: Some("2 of 5 tests passing".into()),
        };
        assert!(state.is_active());
        assert!(!state.is_terminal());
        assert!(!state.is_waiting());

        let display = format!("{state}");
        assert!(display.contains("QA Fix"));
        assert!(display.contains("iteration 2"));
        assert!(display.contains("PARTIAL"));
        assert!(display.contains("2 of 5 tests passing"));
    }

    #[test]
    fn test_qa_states_without_metadata() {
        let review = TaskState::QaReview {
            verdict: None,
            reasoning: None,
        };
        assert_eq!(format!("{review}"), "QA Review");

        let fix = TaskState::QaFix {
            iteration: 1,
            verdict: None,
            reasoning: None,
        };
        assert_eq!(format!("{fix}"), "QA Fix (iteration 1)");
    }

    #[test]
    fn test_task_state_display() {
        // Test all TaskState variants have proper Display implementation
        assert_eq!(format!("{}", TaskState::Draft), "Draft");
        assert_eq!(format!("{}", TaskState::Planning), "Planning");
        assert_eq!(
            format!("{}", TaskState::Planned { subtask_count: 5 }),
            "Planned (5 subtasks)"
        );
        assert_eq!(
            format!(
                "{}",
                TaskState::Executing {
                    completed: 3,
                    total: 5
                }
            ),
            "Executing (3/5)"
        );
        assert_eq!(format!("{}", TaskState::HumanReview), "Human Review");
        assert_eq!(format!("{}", TaskState::Merging), "Merging");
        assert_eq!(format!("{}", TaskState::Completed), "Completed");
        assert_eq!(
            format!(
                "{}",
                TaskState::Failed {
                    reason: "test error".into()
                }
            ),
            "Failed: test error"
        );
        assert_eq!(format!("{}", TaskState::Cancelled), "Cancelled");

        // QA Review without metadata
        let qa_review_basic = TaskState::QaReview {
            verdict: None,
            reasoning: None,
        };
        let display = format!("{qa_review_basic}");
        assert_eq!(display, "QA Review");

        // QA Review with verdict only
        let qa_review_verdict = TaskState::QaReview {
            verdict: Some("APPROVED".into()),
            reasoning: None,
        };
        let display = format!("{qa_review_verdict}");
        assert_eq!(display, "QA Review - APPROVED");

        // QA Review with verdict and reasoning
        let qa_review_full = TaskState::QaReview {
            verdict: Some("NEEDS_FIX".into()),
            reasoning: Some("Missing error handling".into()),
        };
        let display = format!("{qa_review_full}");
        assert!(display.contains("QA Review"));
        assert!(display.contains("NEEDS_FIX"));
        assert!(display.contains("Missing error handling"));
        assert_eq!(display, "QA Review - NEEDS_FIX: Missing error handling");

        // QA Fix without metadata
        let qa_fix_basic = TaskState::QaFix {
            iteration: 1,
            verdict: None,
            reasoning: None,
        };
        let display = format!("{qa_fix_basic}");
        assert_eq!(display, "QA Fix (iteration 1)");

        // QA Fix with verdict and reasoning
        let qa_fix_full = TaskState::QaFix {
            iteration: 2,
            verdict: Some("PARTIAL".into()),
            reasoning: Some("2 of 5 tests passing".into()),
        };
        let display = format!("{qa_fix_full}");
        assert!(display.contains("QA Fix"));
        assert!(display.contains("iteration 2"));
        assert!(display.contains("PARTIAL"));
        assert!(display.contains("2 of 5 tests passing"));
        assert_eq!(
            display,
            "QA Fix (iteration 2) - PARTIAL: 2 of 5 tests passing"
        );
    }
}

impl std::fmt::Display for TaskState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Draft => write!(f, "Draft"),
            Self::Planning => write!(f, "Planning"),
            Self::Planned { subtask_count } => write!(f, "Planned ({subtask_count} subtasks)"),
            Self::Executing { completed, total } => write!(f, "Executing ({completed}/{total})"),
            Self::QaReview { verdict, reasoning } => {
                write!(f, "QA Review")?;
                if let Some(v) = verdict {
                    write!(f, " - {v}")?;
                }
                if let Some(r) = reasoning {
                    write!(f, ": {r}")?;
                }
                Ok(())
            },
            Self::QaFix {
                iteration,
                verdict,
                reasoning,
            } => {
                write!(f, "QA Fix (iteration {iteration})")?;
                if let Some(v) = verdict {
                    write!(f, " - {v}")?;
                }
                if let Some(r) = reasoning {
                    write!(f, ": {r}")?;
                }
                Ok(())
            },
            Self::HumanReview => write!(f, "Human Review"),
            Self::Merging => write!(f, "Merging"),
            Self::Completed => write!(f, "Completed"),
            Self::Failed { reason } => write!(f, "Failed: {reason}"),
            Self::Cancelled => write!(f, "Cancelled"),
        }
    }
}
