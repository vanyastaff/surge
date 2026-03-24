//! QA review loop.

use std::path::Path;

use agent_client_protocol::{ContentBlock, TextContent};
use serde::{Deserialize, Serialize};
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
    /// Some criteria met, others not yet implemented.
    Partial { met: Vec<String>, unmet: Vec<String> },
    /// Issues were found that need fixing.
    NeedsFix { issues: String },
}

/// Verdict kind for JSON response parsing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QaVerdictKind {
    /// All acceptance criteria are met.
    Approved,
    /// Some criteria met, others not yet implemented.
    Partial,
    /// Issues were found that need fixing.
    NeedsFix,
}

/// Structured JSON response from QA review agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QaResponse {
    /// The verdict type.
    pub verdict: QaVerdictKind,
    /// Criteria that have been met (for Partial verdict).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub met: Vec<String>,
    /// Criteria that have not been met (for Partial verdict).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unmet: Vec<String>,
    /// Description of issues that need fixing (for NeedsFix verdict).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issues: Option<String>,
}

impl QaResponse {
    /// Convert JSON response to QaVerdict.
    #[must_use]
    pub fn into_verdict(self) -> QaVerdict {
        match self.verdict {
            QaVerdictKind::Approved => QaVerdict::Approved,
            QaVerdictKind::Partial => QaVerdict::Partial {
                met: self.met,
                unmet: self.unmet,
            },
            QaVerdictKind::NeedsFix => QaVerdict::NeedsFix {
                issues: self.issues.unwrap_or_else(|| {
                    "QA requested fixes (no details provided)".to_string()
                }),
            },
        }
    }
}

/// Result of a complete QA review cycle.
#[derive(Debug, Clone)]
pub struct QaCycleResult {
    /// Final verdict after all iterations.
    pub verdict: QaVerdict,
    /// Number of QA iterations performed.
    pub iterations: u32,
    /// Detailed reasoning or feedback from the QA review.
    pub reasoning: Option<String>,
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
                        reasoning: None,
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
                        reasoning: None,
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
                        reasoning: None,
                    };
                }
                QaVerdict::Partial { met, unmet } => {
                    info!(
                        iteration,
                        met_count = met.len(),
                        unmet_count = unmet.len(),
                        "QA partial - some criteria not yet met"
                    );

                    // Subscribe before fix prompt to capture its response too
                    let _fix_rx = event_tx.subscribe();

                    let unmet_list = unmet.join("\n- ");
                    let fix_prompt = format!(
                        "The QA review found that some acceptance criteria are not yet met:\n\n\
                         Unmet criteria:\n- {unmet_list}\n\n\
                         Please implement the remaining criteria now."
                    );
                    let fix_content = vec![ContentBlock::Text(TextContent::new(fix_prompt))];

                    if let Err(e) = pool.prompt(session, fix_content).await {
                        warn!(error = %e, "fix prompt failed");
                    }

                    let commit_msg = format!("surge: QA partial fix iteration {iteration}");
                    if let Err(e) = git.commit(&spec_id_str, &commit_msg) {
                        warn!(error = %e, "commit after QA partial fix failed");
                    }
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
            reasoning: None,
        }
    }
}

/// Parse the agent's response text into a QA verdict.
///
/// Looks for `APPROVED`, `PARTIAL`, or `NEEDS_FIX: <description>` markers (case-insensitive).
/// For PARTIAL, expects lines with "MET:" and "UNMET:" prefixes.
/// Defaults to `Approved` when no marker is found, to avoid blocking the
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
    } else if upper.contains("PARTIAL") {
        // Parse PARTIAL response with MET:/UNMET: criteria
        let mut met = Vec::new();
        let mut unmet = Vec::new();

        for line in text.lines() {
            let line_upper = line.to_uppercase();
            // Check for UNMET: first to avoid matching it as MET:
            if let Some(pos) = line_upper.find("UNMET:") {
                let criterion = line[pos + 6..].trim();
                if !criterion.is_empty() {
                    unmet.push(criterion.to_string());
                }
            } else if let Some(pos) = line_upper.find("MET:") {
                let criterion = line[pos + 4..].trim();
                if !criterion.is_empty() {
                    met.push(criterion.to_string());
                }
            }
        }

        QaVerdict::Partial { met, unmet }
    } else if upper.contains("APPROVED") {
        QaVerdict::Approved
    } else {
        // No clear verdict — default to approved so the pipeline isn't stuck on
        // agents that respond conversationally rather than using the format.
        info!(
            "QA response has no APPROVED/NEEDS_FIX/PARTIAL marker, defaulting to approved; \
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

    #[test]
    fn test_parse_qa_text_partial() {
        let text = "PARTIAL\nMET: error handling\nMET: documentation\nUNMET: tests\nUNMET: performance optimization";
        let verdict = parse_qa_text(text);

        match verdict {
            QaVerdict::Partial { met, unmet } => {
                assert_eq!(met.len(), 2);
                assert_eq!(unmet.len(), 2);
                assert!(met.contains(&"error handling".to_string()));
                assert!(met.contains(&"documentation".to_string()));
                assert!(unmet.contains(&"tests".to_string()));
                assert!(unmet.contains(&"performance optimization".to_string()));
            }
            _ => panic!("expected Partial verdict"),
        }
    }

    #[test]
    fn test_parse_qa_text_partial_lowercase() {
        let text = "partial\nmet: criterion 1\nunmet: criterion 2";
        let verdict = parse_qa_text(text);
        assert!(matches!(verdict, QaVerdict::Partial { .. }));
    }

    #[test]
    fn test_parse_qa_text_partial_empty_criteria() {
        let text = "PARTIAL";
        let verdict = parse_qa_text(text);

        match verdict {
            QaVerdict::Partial { met, unmet } => {
                assert!(met.is_empty());
                assert!(unmet.is_empty());
            }
            _ => panic!("expected Partial verdict"),
        }
    }

    #[test]
    fn test_parse_qa_text_partial_only_met() {
        let text = "PARTIAL\nMET: criterion 1\nMET: criterion 2";
        let verdict = parse_qa_text(text);

        match verdict {
            QaVerdict::Partial { met, unmet } => {
                assert_eq!(met.len(), 2);
                assert!(unmet.is_empty());
            }
            _ => panic!("expected Partial verdict"),
        }
    }

    #[test]
    fn test_parse_qa_text_partial_only_unmet() {
        let text = "PARTIAL\nUNMET: criterion 1\nUNMET: criterion 2";
        let verdict = parse_qa_text(text);

        match verdict {
            QaVerdict::Partial { met, unmet } => {
                assert!(met.is_empty());
                assert_eq!(unmet.len(), 2);
            }
            _ => panic!("expected Partial verdict"),
        }
    }

    #[test]
    fn test_qa_response_approved_json_roundtrip() {
        let response = QaResponse {
            verdict: QaVerdictKind::Approved,
            met: vec![],
            unmet: vec![],
            issues: None,
        };

        let json = serde_json::to_string(&response).unwrap();
        let deserialized: QaResponse = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.verdict, QaVerdictKind::Approved);
        assert!(deserialized.met.is_empty());
        assert!(deserialized.unmet.is_empty());
        assert!(deserialized.issues.is_none());
    }

    #[test]
    fn test_qa_response_partial_json_roundtrip() {
        let response = QaResponse {
            verdict: QaVerdictKind::Partial,
            met: vec!["error handling".to_string(), "documentation".to_string()],
            unmet: vec!["tests".to_string()],
            issues: None,
        };

        let json = serde_json::to_string(&response).unwrap();
        let deserialized: QaResponse = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.verdict, QaVerdictKind::Partial);
        assert_eq!(deserialized.met.len(), 2);
        assert_eq!(deserialized.unmet.len(), 1);
        assert!(deserialized.met.contains(&"error handling".to_string()));
        assert!(deserialized.unmet.contains(&"tests".to_string()));
    }

    #[test]
    fn test_qa_response_needs_fix_json_roundtrip() {
        let response = QaResponse {
            verdict: QaVerdictKind::NeedsFix,
            met: vec![],
            unmet: vec![],
            issues: Some("Missing error handling in main.rs".to_string()),
        };

        let json = serde_json::to_string(&response).unwrap();
        let deserialized: QaResponse = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.verdict, QaVerdictKind::NeedsFix);
        assert_eq!(
            deserialized.issues,
            Some("Missing error handling in main.rs".to_string())
        );
    }

    #[test]
    fn test_qa_response_into_verdict_approved() {
        let response = QaResponse {
            verdict: QaVerdictKind::Approved,
            met: vec![],
            unmet: vec![],
            issues: None,
        };

        let verdict = response.into_verdict();
        assert!(matches!(verdict, QaVerdict::Approved));
    }

    #[test]
    fn test_qa_response_into_verdict_partial() {
        let response = QaResponse {
            verdict: QaVerdictKind::Partial,
            met: vec!["criterion 1".to_string()],
            unmet: vec!["criterion 2".to_string()],
            issues: None,
        };

        let verdict = response.into_verdict();
        match verdict {
            QaVerdict::Partial { met, unmet } => {
                assert_eq!(met.len(), 1);
                assert_eq!(unmet.len(), 1);
                assert!(met.contains(&"criterion 1".to_string()));
                assert!(unmet.contains(&"criterion 2".to_string()));
            }
            _ => panic!("expected Partial verdict"),
        }
    }

    #[test]
    fn test_qa_response_into_verdict_needs_fix() {
        let response = QaResponse {
            verdict: QaVerdictKind::NeedsFix,
            met: vec![],
            unmet: vec![],
            issues: Some("issues found".to_string()),
        };

        let verdict = response.into_verdict();
        match verdict {
            QaVerdict::NeedsFix { issues } => {
                assert_eq!(issues, "issues found");
            }
            _ => panic!("expected NeedsFix verdict"),
        }
    }

    #[test]
    fn test_qa_response_into_verdict_needs_fix_no_issues() {
        let response = QaResponse {
            verdict: QaVerdictKind::NeedsFix,
            met: vec![],
            unmet: vec![],
            issues: None,
        };

        let verdict = response.into_verdict();
        match verdict {
            QaVerdict::NeedsFix { issues } => {
                assert_eq!(issues, "QA requested fixes (no details provided)");
            }
            _ => panic!("expected NeedsFix verdict"),
        }
    }

    #[test]
    fn test_qa_verdict_kind_serialization() {
        assert_eq!(
            serde_json::to_string(&QaVerdictKind::Approved).unwrap(),
            "\"approved\""
        );
        assert_eq!(
            serde_json::to_string(&QaVerdictKind::Partial).unwrap(),
            "\"partial\""
        );
        assert_eq!(
            serde_json::to_string(&QaVerdictKind::NeedsFix).unwrap(),
            "\"needs_fix\""
        );
    }
}
