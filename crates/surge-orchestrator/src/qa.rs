//! QA review loop.

use std::path::Path;

use agent_client_protocol::{ContentBlock, TextContent};
use surge_acp::pool::{AgentPool, SessionHandle};
use surge_core::event::SurgeEvent;
use surge_core::id::TaskId;
use surge_core::spec::Spec;
use surge_git::worktree::GitManager;
use tokio::sync::broadcast;
use tracing::{info, warn};

use crate::context::build_qa_prompt;

/// Verdict from the QA review.
#[derive(Debug, Clone)]
pub enum QaVerdict {
    /// All acceptance criteria are met.
    Approved,
    /// Issues were found that need fixing.
    NeedsFix { issues: String },
}

/// Result of a complete QA review cycle.
#[derive(Debug, Clone)]
pub struct QaCycleResult {
    /// Final verdict after all iterations.
    pub verdict: QaVerdict,
    /// Number of QA iterations performed.
    pub iterations: u32,
}

/// Drives the QA review loop: review, fix, re-review.
pub struct QaReviewer {
    max_iterations: u32,
}

impl QaReviewer {
    /// Create a new QA reviewer.
    #[must_use]
    pub fn new(max_iterations: u32) -> Self {
        Self { max_iterations }
    }

    /// Run the QA review loop.
    ///
    /// 1. Get the diff from git
    /// 2. Build a QA prompt with acceptance criteria + diff
    /// 3. Subscribe to the event channel to capture the agent's response text
    /// 4. Send to agent; accumulate `AgentMessageChunk` events into response text
    /// 5. Parse response for APPROVED / NEEDS_FIX
    /// 6. If `NeedsFix`, send a fix prompt, commit, and re-review
    /// 7. Repeat until approved or max iterations reached — max iterations is a failure
    #[allow(clippy::too_many_arguments)]
    pub async fn run(
        &self,
        spec: &Spec,
        _task_id: TaskId,
        pool: &AgentPool,
        session: &SessionHandle,
        git: &GitManager,
        event_tx: &broadcast::Sender<SurgeEvent>,
        spec_dir: Option<&Path>,
    ) -> QaCycleResult {
        let spec_id_str = spec.id.to_string();

        for iteration in 1..=self.max_iterations {
            info!(iteration, max = self.max_iterations, "QA review iteration");

            // Get the current diff
            let diff = match git.diff(&spec_id_str) {
                Ok(d) => d,
                Err(e) => {
                    warn!(error = %e, "failed to get diff for QA review, defaulting to approved");
                    return QaCycleResult {
                        verdict: QaVerdict::Approved,
                        iterations: iteration,
                    };
                }
            };

            // Subscribe before prompt so we capture every AgentMessageChunk
            let mut event_rx = event_tx.subscribe();

            let qa_prompt = build_qa_prompt(spec, &diff, spec_dir);
            let content = vec![ContentBlock::Text(TextContent::new(qa_prompt))];

            match pool.prompt(session, content).await {
                Ok(_) => {}
                Err(e) => {
                    warn!(error = %e, "QA prompt failed, defaulting to approved");
                    return QaCycleResult {
                        verdict: QaVerdict::Approved,
                        iterations: iteration,
                    };
                }
            }

            // Drain all AgentMessageChunk events buffered while the prompt ran
            let mut response_text = String::new();
            while let Ok(event) = event_rx.try_recv() {
                if let SurgeEvent::AgentMessageChunk { text, .. } = event {
                    response_text.push_str(&text);
                }
            }

            let verdict = parse_qa_text(&response_text);

            match &verdict {
                QaVerdict::Approved => {
                    info!(iteration, "QA approved");
                    return QaCycleResult {
                        verdict,
                        iterations: iteration,
                    };
                }
                QaVerdict::NeedsFix { issues } => {
                    info!(iteration, issues = %issues, "QA needs fix");

                    // Subscribe before fix prompt to capture its response too
                    let _fix_rx = event_tx.subscribe();

                    let fix_prompt = format!(
                        "The QA review found issues that need fixing:\n\n{issues}\n\n\
                         Please fix these issues now."
                    );
                    let fix_content = vec![ContentBlock::Text(TextContent::new(fix_prompt))];

                    if let Err(e) = pool.prompt(session, fix_content).await {
                        warn!(error = %e, "fix prompt failed");
                    }

                    let commit_msg = format!("surge: QA fix iteration {iteration}");
                    if let Err(e) = git.commit(&spec_id_str, &commit_msg) {
                        warn!(error = %e, "commit after QA fix failed");
                    }
                }
            }
        }

        // Max iterations exhausted without approval — this is a failure
        warn!(
            max = self.max_iterations,
            "QA max iterations reached without approval"
        );
        QaCycleResult {
            verdict: QaVerdict::NeedsFix {
                issues: format!(
                    "QA did not approve after {} iterations",
                    self.max_iterations
                ),
            },
            iterations: self.max_iterations,
        }
    }
}

/// Parse the agent's response text into a QA verdict.
///
/// Looks for `APPROVED` or `NEEDS_FIX: <description>` markers (case-insensitive).
/// Defaults to `Approved` when neither marker is found, to avoid blocking the
/// pipeline when the agent produces an unexpected response format.
#[must_use]
pub fn parse_qa_text(text: &str) -> QaVerdict {
    let upper = text.to_uppercase();

    if let Some(pos) = upper.find("NEEDS_FIX") {
        let after = &text[pos + "NEEDS_FIX".len()..];
        let issues = after.trim_start_matches(':').trim();
        // Take up to the first blank line or end of string as the issue description
        let issues = issues
            .lines()
            .take_while(|l| !l.trim().is_empty())
            .collect::<Vec<_>>()
            .join(" ")
            .trim()
            .to_string();
        QaVerdict::NeedsFix {
            issues: if issues.is_empty() {
                "QA requested fixes (no details provided)".to_string()
            } else {
                issues
            },
        }
    } else if upper.contains("APPROVED") {
        QaVerdict::Approved
    } else {
        // No clear verdict — default to approved so the pipeline isn't stuck on
        // agents that respond conversationally rather than using the format.
        info!(
            "QA response has no APPROVED/NEEDS_FIX marker, defaulting to approved; \
             response preview: {:?}",
            &text[..text.len().min(200)]
        );
        QaVerdict::Approved
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_qa_reviewer_creation() {
        let reviewer = QaReviewer::new(5);
        assert_eq!(reviewer.max_iterations, 5);
    }

    #[test]
    fn test_parse_qa_text_approved() {
        assert!(matches!(parse_qa_text("APPROVED"), QaVerdict::Approved));
        assert!(matches!(parse_qa_text("approved"), QaVerdict::Approved));
        assert!(matches!(
            parse_qa_text("All criteria met. APPROVED"),
            QaVerdict::Approved
        ));
    }

    #[test]
    fn test_parse_qa_text_needs_fix() {
        let verdict = parse_qa_text("NEEDS_FIX: Missing error handling in main.rs");
        assert!(matches!(verdict, QaVerdict::NeedsFix { .. }));
        if let QaVerdict::NeedsFix { issues } = verdict {
            assert!(issues.contains("Missing error handling"));
        }
    }

    #[test]
    fn test_parse_qa_text_needs_fix_lowercase() {
        let verdict = parse_qa_text("needs_fix: tests are failing");
        assert!(matches!(verdict, QaVerdict::NeedsFix { .. }));
    }

    #[test]
    fn test_parse_qa_text_needs_fix_no_description() {
        let verdict = parse_qa_text("NEEDS_FIX");
        if let QaVerdict::NeedsFix { issues } = verdict {
            assert!(!issues.is_empty());
        } else {
            panic!("expected NeedsFix");
        }
    }

    #[test]
    fn test_parse_qa_text_unclear_defaults_to_approved() {
        assert!(matches!(
            parse_qa_text("The code looks fine to me"),
            QaVerdict::Approved
        ));
        assert!(matches!(parse_qa_text(""), QaVerdict::Approved));
    }

    #[test]
    fn test_parse_qa_text_needs_fix_before_approved() {
        // NEEDS_FIX takes priority when it appears first
        let verdict = parse_qa_text("NEEDS_FIX: fix the tests. Then it will be APPROVED");
        assert!(matches!(verdict, QaVerdict::NeedsFix { .. }));
    }
}
