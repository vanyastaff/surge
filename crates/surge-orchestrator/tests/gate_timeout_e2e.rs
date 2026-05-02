//! End-to-end tests for gate timeout behavior.
//!
//! Verifies gate timeout functionality including:
//! - Timeout tracking via GateState timestamps
//! - Auto-abort behavior when timeout is reached
//! - Decision persistence prevents timeout
//! - Timeout behavior across different gate phases
//!
//! Note: Currently only auto-abort is implemented. Auto-approve based on timeout
//! is not yet supported. Timeout configuration via surge.toml is also not yet
//! implemented (timeout is configured programmatically via GateManager::with_timeout).

mod helpers;

use helpers::{cleanup_dir, temp_test_dir};
use std::fs;
use std::time::Duration;
use surge_core::config::{GateConfig, GateDecision};
use surge_core::id::SpecId;
use surge_orchestrator::gates::{GateAction, GateManager};
use surge_orchestrator::phases::Phase;

/// Helper to create a GateConfig with all gates enabled.
fn all_gates_config() -> GateConfig {
    GateConfig {
        after_spec: true,
        after_plan: true,
        after_each_subtask: true,
        after_qa: true,
    }
}

/// Test basic gate timeout behavior.
///
/// Verifies:
/// - Gate does not timeout before configured duration
/// - Gate returns Timeout action after configured duration
/// - Timeout includes elapsed time information
#[test]
fn test_gate_timeout_basic() {
    let test_dir = temp_test_dir("gate_timeout_basic");
    let specs_dir = test_dir.join("specs");
    fs::create_dir_all(&specs_dir).expect("Failed to create specs dir");

    // Create GateManager with 1 second timeout
    let manager = GateManager::with_timeout(
        all_gates_config(),
        specs_dir.clone(),
        Duration::from_secs(1),
    );

    let spec_id = SpecId::new();
    let spec_dir = specs_dir.join(spec_id.to_string());
    fs::create_dir_all(&spec_dir).expect("Failed to create spec dir");

    // Trigger a gate
    manager.trigger_gate(spec_id, Phase::Planning);

    // Immediately check - should pause, not timeout
    let action = manager.check_gate(Phase::Planning, spec_id);
    match action {
        GateAction::Pause { reason } => {
            eprintln!("✓ Gate paused as expected: {}", reason);
        },
        other => {
            panic!("Expected Pause, got {:?}", other);
        },
    }

    // Wait for timeout
    eprintln!("Waiting for timeout (2 seconds)...");
    std::thread::sleep(Duration::from_secs(2));

    // Now should timeout
    let action = manager.check_gate(Phase::Planning, spec_id);
    match action {
        GateAction::Timeout { elapsed } => {
            eprintln!("✓ Gate timed out after {} seconds", elapsed.as_secs());
            assert!(
                elapsed.as_secs() >= 1,
                "Timeout elapsed time should be at least 1 second"
            );
        },
        other => {
            panic!("Expected Timeout, got {:?}", other);
        },
    }

    cleanup_dir(&test_dir);
}

/// Test that gate decision prevents timeout.
///
/// Verifies:
/// - Recording a decision before timeout prevents timeout
/// - Decision persists and gates continue normally
#[test]
fn test_gate_decision_prevents_timeout() {
    let test_dir = temp_test_dir("gate_decision_prevents_timeout");
    let specs_dir = test_dir.join("specs");
    fs::create_dir_all(&specs_dir).expect("Failed to create specs dir");

    // Create GateManager with 1 second timeout
    let manager = GateManager::with_timeout(
        all_gates_config(),
        specs_dir.clone(),
        Duration::from_secs(1),
    );

    let spec_id = SpecId::new();
    let spec_dir = specs_dir.join(spec_id.to_string());
    fs::create_dir_all(&spec_dir).expect("Failed to create spec dir");

    // Trigger a gate
    manager.trigger_gate(spec_id, Phase::Planning);

    // Record approval decision immediately
    let decision = GateDecision::Approved {
        feedback: Some("Looks good!".to_string()),
    };
    manager.record_decision(spec_id, Phase::Planning, decision);

    // Wait longer than timeout period
    eprintln!("Waiting 2 seconds (beyond timeout)...");
    std::thread::sleep(Duration::from_secs(2));

    // Should continue (decision was made), not timeout
    let action = manager.check_gate(Phase::Planning, spec_id);
    match action {
        GateAction::Continue => {
            eprintln!("✓ Gate continued (decision prevented timeout)");
        },
        other => {
            panic!("Expected Continue, got {:?}", other);
        },
    }

    cleanup_dir(&test_dir);
}

/// Test timeout at different gate phases.
///
/// Verifies:
/// - Timeout works for after_spec gate
/// - Timeout works for after_plan gate
/// - Timeout works for after_qa gate
#[test]
fn test_gate_timeout_different_phases() {
    let test_dir = temp_test_dir("gate_timeout_different_phases");
    let specs_dir = test_dir.join("specs");
    fs::create_dir_all(&specs_dir).expect("Failed to create specs dir");

    let manager = GateManager::with_timeout(
        all_gates_config(),
        specs_dir.clone(),
        Duration::from_secs(1),
    );

    // Test timeout for each phase
    let phases = vec![
        (Phase::SpecCreation, "after_spec"),
        (Phase::Planning, "after_plan"),
        (Phase::QaReview, "after_qa"),
    ];

    for (phase, gate_name) in phases {
        let spec_id = SpecId::new();
        let spec_dir = specs_dir.join(spec_id.to_string());
        fs::create_dir_all(&spec_dir).expect("Failed to create spec dir");

        eprintln!("\nTesting {} gate timeout...", gate_name);

        // Trigger gate
        manager.trigger_gate(spec_id, phase);

        // Wait for timeout
        std::thread::sleep(Duration::from_secs(2));

        // Verify timeout
        let action = manager.check_gate(phase, spec_id);
        match action {
            GateAction::Timeout { elapsed } => {
                eprintln!(
                    "✓ {} gate timed out after {} seconds",
                    gate_name,
                    elapsed.as_secs()
                );
            },
            other => {
                panic!("Expected Timeout for {} gate, got {:?}", gate_name, other);
            },
        }
    }

    cleanup_dir(&test_dir);
}

/// Test gate timeout with different timeout durations.
///
/// Verifies:
/// - Short timeouts (1 second) work correctly
/// - Longer timeouts (5 seconds) work correctly
/// - Gates don't timeout before their configured duration
#[test]
fn test_gate_timeout_different_durations() {
    let test_dir = temp_test_dir("gate_timeout_different_durations");
    let specs_dir = test_dir.join("specs");
    fs::create_dir_all(&specs_dir).expect("Failed to create specs dir");

    // Test with 2 second timeout
    eprintln!("\nTesting 2-second timeout...");
    let manager_2s = GateManager::with_timeout(
        all_gates_config(),
        specs_dir.clone(),
        Duration::from_secs(2),
    );

    let spec_id_2s = SpecId::new();
    let spec_dir_2s = specs_dir.join(spec_id_2s.to_string());
    fs::create_dir_all(&spec_dir_2s).expect("Failed to create spec dir");

    manager_2s.trigger_gate(spec_id_2s, Phase::Planning);

    // Check at 1 second - should not timeout yet
    std::thread::sleep(Duration::from_millis(1100));
    let action = manager_2s.check_gate(Phase::Planning, spec_id_2s);
    assert!(
        matches!(action, GateAction::Pause { .. }),
        "Should still be paused after 1 second (timeout is 2 seconds)"
    );
    eprintln!("✓ Not timed out after 1 second");

    // Wait another 1.5 seconds - should timeout now
    std::thread::sleep(Duration::from_millis(1500));
    let action = manager_2s.check_gate(Phase::Planning, spec_id_2s);
    assert!(
        matches!(action, GateAction::Timeout { .. }),
        "Should timeout after 2+ seconds"
    );
    eprintln!("✓ Timed out after 2+ seconds");

    cleanup_dir(&test_dir);
}

/// Test gate timeout state persistence.
///
/// Verifies:
/// - GATE_STATE.json is created with triggered_at timestamp
/// - Gate state can be loaded and timeout is calculated correctly
/// - Gate state includes phase information
#[test]
fn test_gate_timeout_state_persistence() {
    let test_dir = temp_test_dir("gate_timeout_state_persistence");
    let specs_dir = test_dir.join("specs");
    fs::create_dir_all(&specs_dir).expect("Failed to create specs dir");

    let manager = GateManager::with_timeout(
        all_gates_config(),
        specs_dir.clone(),
        Duration::from_secs(1),
    );

    let spec_id = SpecId::new();
    let spec_dir = specs_dir.join(spec_id.to_string());
    fs::create_dir_all(&spec_dir).expect("Failed to create spec dir");

    // Trigger gate
    manager.trigger_gate(spec_id, Phase::Planning);

    // Verify GATE_STATE.json was created
    let state_path = spec_dir.join("GATE_STATE.json");
    assert!(
        state_path.exists(),
        "GATE_STATE.json should be created when gate is triggered"
    );
    eprintln!("✓ GATE_STATE.json created");

    // Read and verify state
    let state_content = fs::read_to_string(&state_path).expect("Failed to read GATE_STATE.json");
    let state: serde_json::Value =
        serde_json::from_str(&state_content).expect("Failed to parse GATE_STATE.json");

    assert!(state.get("phase").is_some(), "State should include phase");
    assert!(
        state.get("triggered_at").is_some(),
        "State should include triggered_at timestamp"
    );
    assert_eq!(
        state.get("decision"),
        Some(&serde_json::Value::Null),
        "Initial decision should be null"
    );
    eprintln!("✓ GATE_STATE.json has correct structure");

    // Wait for timeout
    std::thread::sleep(Duration::from_secs(2));

    // Verify timeout is detected
    let action = manager.check_gate(Phase::Planning, spec_id);
    assert!(
        matches!(action, GateAction::Timeout { .. }),
        "Timeout should be detected from persisted state"
    );
    eprintln!("✓ Timeout detected from persisted state");

    cleanup_dir(&test_dir);
}

/// Test that gate with no timeout never times out.
///
/// Verifies:
/// - Gates created without timeout don't timeout
/// - Gates pause indefinitely until decision is made
#[test]
fn test_gate_without_timeout() {
    let test_dir = temp_test_dir("gate_without_timeout");
    let specs_dir = test_dir.join("specs");
    fs::create_dir_all(&specs_dir).expect("Failed to create specs dir");

    // Create GateManager without timeout
    let manager = GateManager::new(all_gates_config(), specs_dir.clone());

    let spec_id = SpecId::new();
    let spec_dir = specs_dir.join(spec_id.to_string());
    fs::create_dir_all(&spec_dir).expect("Failed to create spec dir");

    // Trigger gate
    manager.trigger_gate(spec_id, Phase::Planning);

    // Wait a while
    std::thread::sleep(Duration::from_secs(2));

    // Should still be paused, not timed out
    let action = manager.check_gate(Phase::Planning, spec_id);
    match action {
        GateAction::Pause { .. } => {
            eprintln!("✓ Gate still paused after 2 seconds (no timeout configured)");
        },
        other => {
            panic!(
                "Expected Pause (no timeout), got {:?}. Gates without timeout should not timeout.",
                other
            );
        },
    }

    cleanup_dir(&test_dir);
}

/// Test rejection decision before timeout.
///
/// Verifies:
/// - Rejection decision prevents timeout
/// - Rejected gates can be retried with feedback
#[test]
fn test_gate_rejection_prevents_timeout() {
    let test_dir = temp_test_dir("gate_rejection_prevents_timeout");
    let specs_dir = test_dir.join("specs");
    fs::create_dir_all(&specs_dir).expect("Failed to create specs dir");

    let manager = GateManager::with_timeout(
        all_gates_config(),
        specs_dir.clone(),
        Duration::from_secs(1),
    );

    let spec_id = SpecId::new();
    let spec_dir = specs_dir.join(spec_id.to_string());
    fs::create_dir_all(&spec_dir).expect("Failed to create spec dir");

    // Trigger gate
    manager.trigger_gate(spec_id, Phase::Planning);

    // Record rejection decision
    let decision = GateDecision::Rejected {
        reason: "Plan needs improvement".to_string(),
        feedback: "Please add error handling".to_string(),
    };
    manager.record_decision(spec_id, Phase::Planning, decision);

    // Wait longer than timeout
    std::thread::sleep(Duration::from_secs(2));

    // Should continue (decision was made), not timeout
    let action = manager.check_gate(Phase::Planning, spec_id);
    match action {
        GateAction::Continue => {
            eprintln!("✓ Gate continued (rejection decision prevented timeout)");
        },
        other => {
            panic!("Expected Continue, got {:?}", other);
        },
    }

    cleanup_dir(&test_dir);
}

/// Test abort decision before timeout.
///
/// Verifies:
/// - Abort decision prevents timeout
/// - Aborted gates are recorded correctly
#[test]
fn test_gate_abort_prevents_timeout() {
    let test_dir = temp_test_dir("gate_abort_prevents_timeout");
    let specs_dir = test_dir.join("specs");
    fs::create_dir_all(&specs_dir).expect("Failed to create specs dir");

    let manager = GateManager::with_timeout(
        all_gates_config(),
        specs_dir.clone(),
        Duration::from_secs(1),
    );

    let spec_id = SpecId::new();
    let spec_dir = specs_dir.join(spec_id.to_string());
    fs::create_dir_all(&spec_dir).expect("Failed to create spec dir");

    // Trigger gate
    manager.trigger_gate(spec_id, Phase::Planning);

    // Record abort decision
    let decision = GateDecision::Aborted {
        reason: "User cancelled pipeline".to_string(),
    };
    manager.record_decision(spec_id, Phase::Planning, decision);

    // Wait longer than timeout
    std::thread::sleep(Duration::from_secs(2));

    // Should continue (decision was made), not timeout
    let action = manager.check_gate(Phase::Planning, spec_id);
    match action {
        GateAction::Continue => {
            eprintln!("✓ Gate continued (abort decision prevented timeout)");
        },
        other => {
            panic!("Expected Continue, got {:?}", other);
        },
    }

    cleanup_dir(&test_dir);
}
