//! Gate management — controls pipeline pausing and human input.

use std::fs;
use std::path::PathBuf;

use surge_core::config::GateConfig;
use surge_core::id::SpecId;
use tracing::{debug, info};

use crate::phases::Phase;
use crate::qa::QaVerdict;

/// Context data for gate reviews — provides rich summaries per phase.
#[derive(Debug, Clone)]
pub struct GateContext {
    /// The pipeline phase this gate is for.
    pub phase: Phase,
    /// Plan diff summary (for Planning phase).
    pub plan_diff: Option<String>,
    /// Code changes summary (for Executing phase).
    pub code_changes: Option<String>,
    /// QA review results (for QaReview phase).
    pub qa_results: Option<QaVerdict>,
}

impl GateContext {
    /// Create a new gate context for a specific phase.
    #[must_use]
    pub fn new(phase: Phase) -> Self {
        Self {
            phase,
            plan_diff: None,
            code_changes: None,
            qa_results: None,
        }
    }

    /// Set the plan diff summary for the Planning phase.
    #[must_use]
    pub fn with_plan_diff(mut self, diff: String) -> Self {
        self.plan_diff = Some(diff);
        self
    }

    /// Set the code changes summary for the Executing phase.
    #[must_use]
    pub fn with_code_changes(mut self, changes: String) -> Self {
        self.code_changes = Some(changes);
        self
    }

    /// Set the QA results for the QaReview phase.
    #[must_use]
    pub fn with_qa_results(mut self, results: QaVerdict) -> Self {
        self.qa_results = Some(results);
        self
    }

    /// Generate a rich summary for human review at this gate.
    ///
    /// Returns a formatted markdown string with phase-specific details:
    /// - Planning: plan diff and key decisions
    /// - Executing: code changes and files modified
    /// - QaReview: QA verdict and any issues found
    #[must_use]
    pub fn generate_summary(&self) -> String {
        let mut summary = format!("# Gate Review: {}\n\n", self.phase);

        match self.phase {
            Phase::Planning => {
                summary.push_str("## Plan Review\n\n");
                if let Some(diff) = &self.plan_diff {
                    summary.push_str("### Plan Changes\n");
                    summary.push_str(diff);
                    summary.push('\n');
                } else {
                    summary.push_str("*No plan diff available*\n");
                }
            }
            Phase::Executing => {
                summary.push_str("## Execution Review\n\n");
                if let Some(changes) = &self.code_changes {
                    summary.push_str("### Code Changes\n");
                    summary.push_str(changes);
                    summary.push('\n');
                } else {
                    summary.push_str("*No code changes available*\n");
                }
            }
            Phase::QaReview => {
                summary.push_str("## QA Review Results\n\n");
                if let Some(qa) = &self.qa_results {
                    match qa {
                        QaVerdict::Approved => {
                            summary.push_str("**Verdict:** ✅ Approved\n\n");
                            summary.push_str("All acceptance criteria have been met.\n");
                        }
                        QaVerdict::Partial { met, unmet } => {
                            summary.push_str("**Verdict:** ⚠️ Partial\n\n");
                            summary.push_str("### Criteria Met\n");
                            for criterion in met {
                                summary.push_str(&format!("- ✅ {criterion}\n"));
                            }
                            summary.push_str("\n### Criteria Not Met\n");
                            for criterion in unmet {
                                summary.push_str(&format!("- ❌ {criterion}\n"));
                            }
                        }
                        QaVerdict::NeedsFix { issues } => {
                            summary.push_str("**Verdict:** ❌ Needs Fix\n\n");
                            summary.push_str("### Issues Found\n");
                            summary.push_str(issues);
                            summary.push('\n');
                        }
                    }
                } else {
                    summary.push_str("*No QA results available*\n");
                }
            }
            Phase::SpecCreation | Phase::QaFix | Phase::HumanReview | Phase::Merging => {
                summary.push_str("*No specific context available for this phase*\n");
            }
        }

        summary
    }
}

/// Action determined by a gate check.
#[derive(Debug, Clone)]
pub enum GateAction {
    /// Pipeline should continue.
    Continue,
    /// Pipeline should pause and wait for a signal.
    Pause { reason: String },
    /// Human input was provided via file.
    HumanInput { content: String },
}

/// Manages gate checks that control pipeline flow.
pub struct GateManager {
    config: GateConfig,
    specs_dir: PathBuf,
}

impl GateManager {
    /// Create a new gate manager.
    #[must_use]
    pub fn new(config: GateConfig, specs_dir: PathBuf) -> Self {
        Self { config, specs_dir }
    }

    /// Check the gate for a given phase and spec.
    ///
    /// Checks in order:
    /// 1. PAUSE file in the spec directory -> Pause
    /// 2. HUMAN_INPUT.md in the spec directory -> HumanInput (consumed)
    /// 3. Configured gates for the phase -> Pause or Continue
    pub fn check_gate(&self, phase: Phase, spec_id: SpecId) -> GateAction {
        let spec_dir = self.specs_dir.join(spec_id.to_string());

        // Check for PAUSE file
        let pause_file = spec_dir.join("PAUSE");
        if pause_file.exists() {
            info!(spec_id = %spec_id, "PAUSE file detected");
            return GateAction::Pause {
                reason: format!("PAUSE file found for spec {spec_id}"),
            };
        }

        // Check for HUMAN_INPUT.md file (consume it)
        let input_file = spec_dir.join("HUMAN_INPUT.md");
        if input_file.exists() {
            match fs::read_to_string(&input_file) {
                Ok(content) => {
                    // Consume the file
                    if let Err(e) = fs::remove_file(&input_file) {
                        debug!(error = %e, "failed to remove HUMAN_INPUT.md");
                    }
                    info!(spec_id = %spec_id, "consumed HUMAN_INPUT.md");
                    return GateAction::HumanInput { content };
                }
                Err(e) => {
                    debug!(error = %e, "failed to read HUMAN_INPUT.md");
                }
            }
        }

        // Check configured gates for this phase
        let gate_enabled = match phase {
            Phase::SpecCreation => self.config.after_spec,
            Phase::Planning => self.config.after_plan,
            Phase::Executing => self.config.after_each_subtask,
            Phase::QaReview => self.config.after_qa,
            Phase::HumanReview | Phase::QaFix | Phase::Merging => false,
        };

        if gate_enabled {
            GateAction::Pause {
                reason: format!("gate configured for phase {phase}"),
            }
        } else {
            GateAction::Continue
        }
    }

    /// Clear the PAUSE file for a spec, allowing the pipeline to continue.
    pub fn clear_pause(&self, spec_id: SpecId) {
        let pause_file = self.specs_dir.join(spec_id.to_string()).join("PAUSE");
        if pause_file.exists() {
            if let Err(e) = fs::remove_file(&pause_file) {
                debug!(error = %e, "failed to remove PAUSE file");
            } else {
                info!(spec_id = %spec_id, "cleared PAUSE file");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::qa::QaVerdict;

    fn no_gates_config() -> GateConfig {
        GateConfig {
            after_spec: false,
            after_plan: false,
            after_each_subtask: false,
            after_qa: false,
        }
    }

    fn all_gates_config() -> GateConfig {
        GateConfig {
            after_spec: true,
            after_plan: true,
            after_each_subtask: true,
            after_qa: true,
        }
    }

    #[test]
    fn test_gate_continue() {
        let dir = tempfile::tempdir().unwrap();
        let manager = GateManager::new(no_gates_config(), dir.path().to_path_buf());
        let spec_id = SpecId::new();

        let action = manager.check_gate(Phase::Executing, spec_id);
        assert!(matches!(action, GateAction::Continue));
    }

    #[test]
    fn test_gate_pause() {
        let dir = tempfile::tempdir().unwrap();
        let manager = GateManager::new(all_gates_config(), dir.path().to_path_buf());
        let spec_id = SpecId::new();

        let action = manager.check_gate(Phase::Executing, spec_id);
        assert!(matches!(action, GateAction::Pause { .. }));
    }

    #[test]
    fn test_pause_file_detection() {
        let dir = tempfile::tempdir().unwrap();
        let spec_id = SpecId::new();

        // Create spec directory and PAUSE file
        let spec_dir = dir.path().join(spec_id.to_string());
        fs::create_dir_all(&spec_dir).unwrap();
        fs::write(spec_dir.join("PAUSE"), "").unwrap();

        let manager = GateManager::new(no_gates_config(), dir.path().to_path_buf());
        let action = manager.check_gate(Phase::Executing, spec_id);

        assert!(matches!(action, GateAction::Pause { .. }));
    }

    #[test]
    fn test_human_input_file() {
        let dir = tempfile::tempdir().unwrap();
        let spec_id = SpecId::new();

        // Create spec directory and HUMAN_INPUT.md
        let spec_dir = dir.path().join(spec_id.to_string());
        fs::create_dir_all(&spec_dir).unwrap();
        let input_path = spec_dir.join("HUMAN_INPUT.md");
        fs::write(&input_path, "Please add error handling").unwrap();

        let manager = GateManager::new(no_gates_config(), dir.path().to_path_buf());
        let action = manager.check_gate(Phase::Executing, spec_id);

        match action {
            GateAction::HumanInput { content } => {
                assert_eq!(content, "Please add error handling");
            }
            other => panic!("expected HumanInput, got {other:?}"),
        }

        // File should be consumed
        assert!(!input_path.exists());
    }

    #[test]
    fn test_gate_context_planning() {
        let context = GateContext::new(Phase::Planning)
            .with_plan_diff("Added 3 subtasks for API implementation".to_string());

        let summary = context.generate_summary();
        assert!(summary.contains("Gate Review: Planning"));
        assert!(summary.contains("Plan Changes"));
        assert!(summary.contains("Added 3 subtasks"));
    }

    #[test]
    fn test_gate_context_executing() {
        let context = GateContext::new(Phase::Executing).with_code_changes(
            "Modified: src/api.rs (+45, -12)\nModified: src/types.rs (+8, -3)".to_string(),
        );

        let summary = context.generate_summary();
        assert!(summary.contains("Gate Review: Executing"));
        assert!(summary.contains("Code Changes"));
        assert!(summary.contains("src/api.rs"));
    }

    #[test]
    fn test_gate_context_qa_approved() {
        let context =
            GateContext::new(Phase::QaReview).with_qa_results(QaVerdict::Approved);

        let summary = context.generate_summary();
        assert!(summary.contains("Gate Review: QA Review"));
        assert!(summary.contains("Approved"));
        assert!(summary.contains("acceptance criteria"));
    }

    #[test]
    fn test_gate_context_qa_partial() {
        let context = GateContext::new(Phase::QaReview).with_qa_results(QaVerdict::Partial {
            met: vec!["API endpoint works".to_string()],
            unmet: vec!["Error handling missing".to_string()],
        });

        let summary = context.generate_summary();
        assert!(summary.contains("Partial"));
        assert!(summary.contains("API endpoint works"));
        assert!(summary.contains("Error handling missing"));
    }

    #[test]
    fn test_gate_context_qa_needs_fix() {
        let context = GateContext::new(Phase::QaReview).with_qa_results(QaVerdict::NeedsFix {
            issues: "Test failures in api_test.rs".to_string(),
        });

        let summary = context.generate_summary();
        assert!(summary.contains("Needs Fix"));
        assert!(summary.contains("Test failures in api_test.rs"));
    }

    #[test]
    fn test_gate_context_builder_pattern() {
        let context = GateContext::new(Phase::Planning)
            .with_plan_diff("Plan diff".to_string())
            .with_code_changes("Code changes".to_string())
            .with_qa_results(QaVerdict::Approved);

        assert_eq!(context.phase, Phase::Planning);
        assert!(context.plan_diff.is_some());
        assert!(context.code_changes.is_some());
        assert!(context.qa_results.is_some());
    }
}
