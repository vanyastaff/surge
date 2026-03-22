//! Gate management — controls pipeline pausing and human input.

use std::fs;
use std::path::PathBuf;

use surge_core::config::GateConfig;
use surge_core::id::SpecId;
use tracing::{debug, info};

use crate::phases::Phase;

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
            Phase::Planning => self.config.after_spec,
            Phase::Executing => self.config.after_each_subtask,
            Phase::QaReview => self.config.after_qa,
            Phase::HumanReview => self.config.after_plan,
            Phase::QaFix | Phase::Merging => false,
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
}
