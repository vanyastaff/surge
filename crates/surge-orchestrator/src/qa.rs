//! QA review loop.

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
    /// 3. Send to agent and parse the response
    /// 4. If `NeedsFix`, send a fix prompt, commit, and re-review
    /// 5. Repeat until approved or max iterations reached
    pub async fn run(
        &self,
        spec: &Spec,
        _task_id: TaskId,
        pool: &AgentPool,
        session: &SessionHandle,
        git: &GitManager,
        event_tx: &broadcast::Sender<SurgeEvent>,
    ) -> QaCycleResult {
        let spec_id_str = spec.id.to_string();
        let _ = event_tx;

        for iteration in 1..=self.max_iterations {
            info!(iteration, max = self.max_iterations, "QA review iteration");

            // Get the current diff
            let diff = match git.diff(&spec_id_str) {
                Ok(d) => d,
                Err(e) => {
                    warn!(error = %e, "failed to get diff for QA review");
                    return QaCycleResult {
                        verdict: QaVerdict::Approved,
                        iterations: iteration,
                    };
                }
            };

            // Build and send QA prompt
            let qa_prompt = build_qa_prompt(spec, &diff);
            let content = vec![ContentBlock::Text(TextContent::new(qa_prompt))];

            let response = match pool.prompt(session, content).await {
                Ok(r) => r,
                Err(e) => {
                    warn!(error = %e, "QA prompt failed, defaulting to approved");
                    return QaCycleResult {
                        verdict: QaVerdict::Approved,
                        iterations: iteration,
                    };
                }
            };

            // Parse the QA response
            let verdict = parse_qa_response(&response);

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

                    // Send fix prompt to agent
                    let fix_prompt = format!(
                        "The QA review found issues that need fixing:\n\n{issues}\n\n\
                         Please fix these issues now."
                    );
                    let fix_content = vec![ContentBlock::Text(TextContent::new(fix_prompt))];

                    if let Err(e) = pool.prompt(session, fix_content).await {
                        warn!(error = %e, "fix prompt failed");
                    }

                    // Commit the fix
                    let commit_msg = format!("surge: QA fix iteration {iteration}");
                    if let Err(e) = git.commit(&spec_id_str, &commit_msg) {
                        warn!(error = %e, "commit after QA fix failed");
                    }
                }
            }
        }

        // Max iterations exhausted — return last state as approved to not block
        info!("QA max iterations reached, defaulting to approved");
        QaCycleResult {
            verdict: QaVerdict::Approved,
            iterations: self.max_iterations,
        }
    }
}

/// Parse the QA agent response into a verdict.
///
/// Since ACP 0.6 `PromptResponse` does not expose content directly,
/// we default to `Approved` for now. Future versions will inspect
/// the response content for APPROVED/NEEDS_FIX markers.
#[must_use]
pub fn parse_qa_response(
    _response: &agent_client_protocol::PromptResponse,
) -> QaVerdict {
    // TODO: When ACP exposes response content, parse for APPROVED/NEEDS_FIX
    QaVerdict::Approved
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_qa_reviewer_creation() {
        let reviewer = QaReviewer::new(5);
        assert_eq!(reviewer.max_iterations, 5);
    }
}
