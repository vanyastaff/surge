//! Gate management — controls pipeline pausing and human input.

use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};
use surge_core::config::{GateConfig, GateDecision};
use surge_core::id::SpecId;
use tracing::{debug, info, warn};

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
    /// Gate timed out waiting for decision.
    Timeout { elapsed: Duration },
}

/// Persisted gate state for tracking decisions and timeouts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateState {
    /// The phase this gate is for.
    pub phase: Phase,
    /// When the gate was triggered (Unix timestamp in seconds).
    pub triggered_at: u64,
    /// The decision made at this gate, if any.
    pub decision: Option<GateDecision>,
    /// When the decision was made (Unix timestamp in seconds).
    pub decided_at: Option<u64>,
}

/// Manages gate checks that control pipeline flow.
pub struct GateManager {
    config: GateConfig,
    specs_dir: PathBuf,
    /// Optional timeout in seconds for gate decisions. None means no timeout.
    timeout_secs: Option<u64>,
}

impl GateManager {
    /// Create a new gate manager.
    #[must_use]
    pub fn new(config: GateConfig, specs_dir: PathBuf) -> Self {
        Self {
            config,
            specs_dir,
            timeout_secs: None,
        }
    }

    /// Create a new gate manager with a timeout.
    ///
    /// # Arguments
    /// * `config` - Gate configuration
    /// * `specs_dir` - Directory containing spec subdirectories
    /// * `timeout` - Duration after which gates auto-timeout if no decision is made
    #[must_use]
    pub fn with_timeout(config: GateConfig, specs_dir: PathBuf, timeout: Duration) -> Self {
        Self {
            config,
            specs_dir,
            timeout_secs: Some(timeout.as_secs()),
        }
    }

    /// Get the path to the gate state file for a spec.
    fn gate_state_path(&self, spec_id: SpecId) -> PathBuf {
        self.specs_dir
            .join(spec_id.to_string())
            .join("GATE_STATE.json")
    }

    /// Get the path to the gate decision file for a spec.
    fn gate_decision_path(&self, spec_id: SpecId) -> PathBuf {
        self.specs_dir
            .join(spec_id.to_string())
            .join("DECISION.json")
    }

    /// Load the current gate state from disk.
    fn load_gate_state(&self, spec_id: SpecId) -> Option<GateState> {
        let path = self.gate_state_path(spec_id);
        if !path.exists() {
            return None;
        }

        match fs::read_to_string(&path) {
            Ok(content) => match serde_json::from_str(&content) {
                Ok(state) => Some(state),
                Err(e) => {
                    debug!(error = %e, "failed to parse gate state");
                    None
                }
            },
            Err(e) => {
                debug!(error = %e, "failed to read gate state");
                None
            }
        }
    }

    /// Save gate state to disk.
    fn save_gate_state(&self, spec_id: SpecId, state: &GateState) {
        let path = self.gate_state_path(spec_id);

        // Ensure spec directory exists
        if let Some(parent) = path.parent() {
            if let Err(e) = fs::create_dir_all(parent) {
                warn!(error = %e, "failed to create spec directory");
                return;
            }
        }

        match serde_json::to_string_pretty(state) {
            Ok(json) => {
                if let Err(e) = fs::write(&path, json) {
                    warn!(error = %e, "failed to write gate state");
                } else {
                    debug!(spec_id = %spec_id, "saved gate state");
                }
            }
            Err(e) => {
                warn!(error = %e, "failed to serialize gate state");
            }
        }
    }

    /// Record a gate decision and persist it to disk.
    ///
    /// The decision is saved to DECISION.json in the spec directory and the
    /// gate state is updated to record the decision timestamp.
    pub fn record_decision(&self, spec_id: SpecId, phase: Phase, decision: GateDecision) {
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        // Load or create gate state
        let mut state = self.load_gate_state(spec_id).unwrap_or_else(|| GateState {
            phase,
            triggered_at: now,
            decision: None,
            decided_at: None,
        });

        // Update state with decision
        state.decision = Some(decision.clone());
        state.decided_at = Some(now);

        // Save updated state
        self.save_gate_state(spec_id, &state);

        // Also write decision to DECISION.json for external consumption
        let decision_path = self.gate_decision_path(spec_id);
        if let Ok(json) = serde_json::to_string_pretty(&decision) {
            if let Err(e) = fs::write(&decision_path, json) {
                warn!(error = %e, "failed to write gate decision");
            } else {
                info!(spec_id = %spec_id, phase = %phase, "recorded gate decision");
            }
        }
    }

    /// Load a gate decision from disk if it exists.
    ///
    /// Returns the decision from DECISION.json if present, and consumes the file.
    pub fn load_decision(&self, spec_id: SpecId) -> Option<GateDecision> {
        let decision_path = self.gate_decision_path(spec_id);
        if !decision_path.exists() {
            return None;
        }

        match fs::read_to_string(&decision_path) {
            Ok(content) => match serde_json::from_str(&content) {
                Ok(decision) => {
                    // Consume the decision file
                    if let Err(e) = fs::remove_file(&decision_path) {
                        debug!(error = %e, "failed to remove decision file");
                    }
                    info!(spec_id = %spec_id, "loaded gate decision");
                    Some(decision)
                }
                Err(e) => {
                    debug!(error = %e, "failed to parse gate decision");
                    None
                }
            },
            Err(e) => {
                debug!(error = %e, "failed to read gate decision");
                None
            }
        }
    }

    /// Trigger a gate — creates initial gate state with timestamp.
    ///
    /// This should be called when a gate is first encountered to record when
    /// the gate was triggered for timeout tracking.
    pub fn trigger_gate(&self, spec_id: SpecId, phase: Phase) {
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let state = GateState {
            phase,
            triggered_at: now,
            decision: None,
            decided_at: None,
        };

        self.save_gate_state(spec_id, &state);
        info!(spec_id = %spec_id, phase = %phase, "triggered gate");
    }

    /// Check if a gate has timed out.
    ///
    /// Returns `Some(elapsed)` if the gate has timed out, `None` otherwise.
    fn check_timeout(&self, spec_id: SpecId) -> Option<Duration> {
        let timeout_secs = self.timeout_secs?;
        let state = self.load_gate_state(spec_id)?;

        // If decision already made, no timeout
        if state.decision.is_some() {
            return None;
        }

        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let elapsed = now.saturating_sub(state.triggered_at);

        if elapsed >= timeout_secs {
            Some(Duration::from_secs(elapsed))
        } else {
            None
        }
    }

    /// Check the gate for a given phase and spec.
    ///
    /// Checks in order:
    /// 1. Existing gate decision (DECISION.json) -> Continue with decision
    /// 2. Gate timeout -> Timeout
    /// 3. PAUSE file in the spec directory -> Pause
    /// 4. HUMAN_INPUT.md in the spec directory -> HumanInput (consumed)
    /// 5. Configured gates for the phase -> Pause or Continue
    pub fn check_gate(&self, phase: Phase, spec_id: SpecId) -> GateAction {
        let spec_dir = self.specs_dir.join(spec_id.to_string());

        // Check for existing decision
        if let Some(decision) = self.load_decision(spec_id) {
            info!(spec_id = %spec_id, decision = ?decision, "gate decision loaded");
            return GateAction::Continue;
        }

        // Check for timeout
        if let Some(elapsed) = self.check_timeout(spec_id) {
            warn!(
                spec_id = %spec_id,
                elapsed_secs = elapsed.as_secs(),
                "gate timed out"
            );
            return GateAction::Timeout { elapsed };
        }

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

    #[test]
    fn test_gate_decision_persistence() {
        use surge_core::config::GateDecision;

        let dir = tempfile::tempdir().unwrap();
        let manager = GateManager::new(no_gates_config(), dir.path().to_path_buf());
        let spec_id = SpecId::new();

        // Create spec directory
        let spec_dir = dir.path().join(spec_id.to_string());
        fs::create_dir_all(&spec_dir).unwrap();

        // Record a decision
        let decision = GateDecision::Approved {
            feedback: Some("Looks good!".to_string()),
        };
        manager.record_decision(spec_id, Phase::Planning, decision.clone());

        // Verify DECISION.json was created
        let decision_path = spec_dir.join("DECISION.json");
        assert!(decision_path.exists());

        // Load the decision
        let loaded = manager.load_decision(spec_id);
        assert!(loaded.is_some());
        assert_eq!(loaded.unwrap(), decision);

        // Decision file should be consumed
        assert!(!decision_path.exists());
    }

    #[test]
    fn test_gate_state_persistence() {
        let dir = tempfile::tempdir().unwrap();
        let manager = GateManager::new(no_gates_config(), dir.path().to_path_buf());
        let spec_id = SpecId::new();

        // Create spec directory
        let spec_dir = dir.path().join(spec_id.to_string());
        fs::create_dir_all(&spec_dir).unwrap();

        // Trigger a gate
        manager.trigger_gate(spec_id, Phase::Planning);

        // Verify GATE_STATE.json was created
        let state_path = spec_dir.join("GATE_STATE.json");
        assert!(state_path.exists());

        // Load the state
        let state = manager.load_gate_state(spec_id);
        assert!(state.is_some());
        let state = state.unwrap();
        assert_eq!(state.phase, Phase::Planning);
        assert!(state.decision.is_none());
        assert!(state.decided_at.is_none());
    }

    #[test]
    fn test_gate_timeout() {
        use std::thread;

        let dir = tempfile::tempdir().unwrap();
        // Create manager with 1 second timeout
        let manager = GateManager::with_timeout(
            all_gates_config(),
            dir.path().to_path_buf(),
            Duration::from_secs(1),
        );
        let spec_id = SpecId::new();

        // Create spec directory
        let spec_dir = dir.path().join(spec_id.to_string());
        fs::create_dir_all(&spec_dir).unwrap();

        // Trigger a gate
        manager.trigger_gate(spec_id, Phase::Planning);

        // Initially, no timeout
        let action = manager.check_gate(Phase::Planning, spec_id);
        assert!(matches!(action, GateAction::Pause { .. }));

        // Wait for timeout
        thread::sleep(Duration::from_secs(2));

        // Now should timeout
        let action = manager.check_gate(Phase::Planning, spec_id);
        assert!(matches!(action, GateAction::Timeout { .. }));
    }

    #[test]
    fn test_gate_decision_prevents_timeout() {
        use surge_core::config::GateDecision;

        let dir = tempfile::tempdir().unwrap();
        let manager = GateManager::with_timeout(
            all_gates_config(),
            dir.path().to_path_buf(),
            Duration::from_secs(1),
        );
        let spec_id = SpecId::new();

        // Create spec directory
        let spec_dir = dir.path().join(spec_id.to_string());
        fs::create_dir_all(&spec_dir).unwrap();

        // Trigger a gate
        manager.trigger_gate(spec_id, Phase::Planning);

        // Record a decision
        let decision = GateDecision::Approved { feedback: None };
        manager.record_decision(spec_id, Phase::Planning, decision);

        // Should not timeout even after waiting
        std::thread::sleep(Duration::from_secs(2));

        // Should continue (decision was made)
        let action = manager.check_gate(Phase::Planning, spec_id);
        assert!(matches!(action, GateAction::Continue));
    }

    #[test]
    fn test_gate_with_timeout_constructor() {
        let dir = tempfile::tempdir().unwrap();
        let manager = GateManager::with_timeout(
            no_gates_config(),
            dir.path().to_path_buf(),
            Duration::from_secs(3600),
        );

        assert_eq!(manager.timeout_secs, Some(3600));
    }

    #[test]
    fn test_gate_without_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let manager = GateManager::new(no_gates_config(), dir.path().to_path_buf());

        assert_eq!(manager.timeout_secs, None);
    }
}
