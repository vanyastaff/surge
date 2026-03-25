//! End-to-end tests for gate state persistence and resume after restart.
//!
//! Verifies that gate state persists across pipeline restarts:
//! - GATE_STATE.json is created when a gate is triggered
//! - Gate state is restored after simulated restart (new GateManager instance)
//! - Pipeline correctly resumes waiting for approval after restart
//! - Decisions made after restart are properly processed
//! - Timeout tracking persists across restarts

mod helpers;

use helpers::{cleanup_dir, temp_test_dir};
use std::fs;
use std::time::Duration;
use surge_core::config::{GateConfig, GateDecision};
use surge_core::id::SpecId;
use surge_orchestrator::gates::{GateAction, GateManager, GateState};
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

/// Helper to load and parse GATE_STATE.json for verification.
fn load_gate_state_file(specs_dir: &std::path::Path, spec_id: SpecId) -> Option<GateState> {
    let path = specs_dir
        .join(spec_id.to_string())
        .join("GATE_STATE.json");

    if !path.exists() {
        return None;
    }

    let content = fs::read_to_string(&path).ok()?;
    serde_json::from_str(&content).ok()
}

/// Test basic gate state persistence across restart.
///
/// Verifies:
/// - GATE_STATE.json is created when gate is triggered
/// - Gate state persists on disk
/// - New GateManager instance can read persisted state
/// - Gate remains in Pause state after restart
#[test]
fn test_gate_state_persists_across_restart() {
    let test_dir = temp_test_dir("gate_state_persists");
    let specs_dir = test_dir.join("specs");
    fs::create_dir_all(&specs_dir).expect("Failed to create specs dir");

    let spec_id = SpecId::new();
    let spec_dir = specs_dir.join(spec_id.to_string());
    fs::create_dir_all(&spec_dir).expect("Failed to create spec dir");

    // === Phase 1: Initial pipeline run ===
    eprintln!("=== Phase 1: Initial pipeline run ===");

    let manager1 = GateManager::new(all_gates_config(), specs_dir.clone());

    // Trigger gate
    manager1.trigger_gate(spec_id, Phase::Planning);
    eprintln!("✓ Gate triggered at Planning phase");

    // Verify GATE_STATE.json was created
    let state_file = load_gate_state_file(&specs_dir, spec_id);
    assert!(state_file.is_some(), "GATE_STATE.json should exist");

    let state = state_file.unwrap();
    assert_eq!(state.phase, Phase::Planning, "State should record Planning phase");
    assert!(state.triggered_at > 0, "State should have triggered timestamp");
    assert!(state.decision.is_none(), "State should have no decision yet");
    assert!(state.decided_at.is_none(), "State should have no decision timestamp");
    eprintln!("✓ GATE_STATE.json created with correct initial state");

    // Check gate - should pause
    let action1 = manager1.check_gate(Phase::Planning, spec_id);
    match action1 {
        GateAction::Pause { .. } => {
            eprintln!("✓ Gate paused as expected (before restart)");
        }
        other => {
            panic!("Expected Pause before restart, got {:?}", other);
        }
    }

    // Simulate process crash - drop manager1 without making decision
    drop(manager1);
    eprintln!("\n=== Simulating pipeline process crash/restart ===");

    // === Phase 2: After restart ===
    eprintln!("\n=== Phase 2: After pipeline restart ===");

    // Create new GateManager (simulates restart)
    let manager2 = GateManager::new(all_gates_config(), specs_dir.clone());
    eprintln!("✓ New GateManager instance created (restart simulated)");

    // Verify GATE_STATE.json still exists
    let state_after_restart = load_gate_state_file(&specs_dir, spec_id);
    assert!(state_after_restart.is_some(), "GATE_STATE.json should persist after restart");

    let state2 = state_after_restart.unwrap();
    assert_eq!(state2.phase, Phase::Planning, "Phase should be preserved");
    assert_eq!(state2.triggered_at, state.triggered_at, "Timestamp should be preserved");
    assert!(state2.decision.is_none(), "Decision should still be None");
    eprintln!("✓ GATE_STATE.json persisted with same state");

    // Check gate - should still pause (waiting for approval)
    let action2 = manager2.check_gate(Phase::Planning, spec_id);
    match action2 {
        GateAction::Pause { reason } => {
            eprintln!("✓ Gate still paused after restart: {}", reason);
        }
        other => {
            panic!("Expected Pause after restart, got {:?}", other);
        }
    }

    eprintln!("\n✓ Test passed: Gate state persists across restart");
    cleanup_dir(&test_dir);
}

/// Test approval decision after restart.
///
/// Verifies:
/// - Gate state persists across restart
/// - Approval decision after restart updates state correctly
/// - Pipeline continues after approval post-restart
#[test]
fn test_approval_after_restart() {
    let test_dir = temp_test_dir("approval_after_restart");
    let specs_dir = test_dir.join("specs");
    fs::create_dir_all(&specs_dir).expect("Failed to create specs dir");

    let spec_id = SpecId::new();
    let spec_dir = specs_dir.join(spec_id.to_string());
    fs::create_dir_all(&spec_dir).expect("Failed to create spec dir");

    // === Phase 1: Trigger gate ===
    let manager1 = GateManager::new(all_gates_config(), specs_dir.clone());
    manager1.trigger_gate(spec_id, Phase::Planning);

    let original_triggered_at = load_gate_state_file(&specs_dir, spec_id)
        .expect("State file should exist")
        .triggered_at;

    eprintln!("✓ Gate triggered, original timestamp: {}", original_triggered_at);

    // Simulate restart
    drop(manager1);
    eprintln!("=== Pipeline restarted ===");

    // === Phase 2: After restart, make decision ===
    let manager2 = GateManager::new(all_gates_config(), specs_dir.clone());

    // Record approval
    let decision = GateDecision::Approved {
        feedback: Some("Approved after restart".to_string()),
    };
    manager2.record_decision(spec_id, Phase::Planning, decision);
    eprintln!("✓ Approval decision recorded after restart");

    // Verify state was updated
    let state_after_approval = load_gate_state_file(&specs_dir, spec_id)
        .expect("State file should still exist");

    assert_eq!(state_after_approval.triggered_at, original_triggered_at,
        "Original trigger timestamp should be preserved");
    assert!(state_after_approval.decision.is_some(), "Decision should be recorded");
    assert!(state_after_approval.decided_at.is_some(), "Decision timestamp should be recorded");

    if let Some(GateDecision::Approved { feedback }) = &state_after_approval.decision {
        assert_eq!(feedback.as_deref(), Some("Approved after restart"));
        eprintln!("✓ Decision correctly persisted in GATE_STATE.json");
    } else {
        panic!("Expected Approved decision in state");
    }

    // Check gate - should continue (decision was made)
    let action = manager2.check_gate(Phase::Planning, spec_id);
    match action {
        GateAction::Continue => {
            eprintln!("✓ Gate continues after approval (post-restart)");
        }
        other => {
            panic!("Expected Continue after approval, got {:?}", other);
        }
    }

    eprintln!("\n✓ Test passed: Approval after restart works correctly");
    cleanup_dir(&test_dir);
}

/// Test rejection decision after restart.
///
/// Verifies:
/// - Gate state persists across restart
/// - Rejection decision after restart is properly recorded
/// - Rejection feedback is persisted correctly
#[test]
fn test_rejection_after_restart() {
    let test_dir = temp_test_dir("rejection_after_restart");
    let specs_dir = test_dir.join("specs");
    fs::create_dir_all(&specs_dir).expect("Failed to create specs dir");

    let spec_id = SpecId::new();
    let spec_dir = specs_dir.join(spec_id.to_string());
    fs::create_dir_all(&spec_dir).expect("Failed to create spec dir");

    // Trigger gate
    let manager1 = GateManager::new(all_gates_config(), specs_dir.clone());
    manager1.trigger_gate(spec_id, Phase::Executing);
    eprintln!("✓ Gate triggered at Executing phase");

    // Simulate restart
    drop(manager1);
    eprintln!("=== Pipeline restarted ===");

    // After restart, record rejection
    let manager2 = GateManager::new(all_gates_config(), specs_dir.clone());

    let decision = GateDecision::Rejected {
        reason: "Quality issues".to_string(),
        feedback: "Code quality issues detected after reviewing post-restart".to_string(),
    };
    manager2.record_decision(spec_id, Phase::Executing, decision);
    eprintln!("✓ Rejection decision recorded after restart");

    // Verify state
    let state = load_gate_state_file(&specs_dir, spec_id)
        .expect("State file should exist");

    assert!(state.decision.is_some(), "Decision should be recorded");

    if let Some(GateDecision::Rejected { reason, feedback }) = &state.decision {
        assert_eq!(reason, "Quality issues");
        assert_eq!(feedback, "Code quality issues detected after reviewing post-restart");
        eprintln!("✓ Rejection feedback correctly persisted");
    } else {
        panic!("Expected Rejected decision in state");
    }

    // Check gate - should continue (rejection is a decision)
    let action = manager2.check_gate(Phase::Executing, spec_id);
    match action {
        GateAction::Continue => {
            eprintln!("✓ Gate continues after rejection (pipeline will re-run phase)");
        }
        other => {
            panic!("Expected Continue after rejection, got {:?}", other);
        }
    }

    eprintln!("\n✓ Test passed: Rejection after restart works correctly");
    cleanup_dir(&test_dir);
}

/// Test timeout tracking persists across restart.
///
/// Verifies:
/// - Timeout countdown continues across restart
/// - Original trigger timestamp is preserved
/// - Timeout fires correctly after restart if time has elapsed
#[test]
fn test_timeout_persists_across_restart() {
    let test_dir = temp_test_dir("timeout_persists_restart");
    let specs_dir = test_dir.join("specs");
    fs::create_dir_all(&specs_dir).expect("Failed to create specs dir");

    let spec_id = SpecId::new();
    let spec_dir = specs_dir.join(spec_id.to_string());
    fs::create_dir_all(&spec_dir).expect("Failed to create spec dir");

    // === Phase 1: Trigger gate with timeout ===
    let manager1 = GateManager::with_timeout(
        all_gates_config(),
        specs_dir.clone(),
        Duration::from_secs(2),
    );

    manager1.trigger_gate(spec_id, Phase::Planning);
    eprintln!("✓ Gate triggered with 2-second timeout");

    let original_triggered_at = load_gate_state_file(&specs_dir, spec_id)
        .expect("State file should exist")
        .triggered_at;

    // Check immediately - should pause
    let action1 = manager1.check_gate(Phase::Planning, spec_id);
    match action1 {
        GateAction::Pause { .. } => {
            eprintln!("✓ Gate paused (timeout not reached yet)");
        }
        other => {
            panic!("Expected Pause, got {:?}", other);
        }
    }

    // Wait 1 second, then simulate restart
    std::thread::sleep(Duration::from_secs(1));
    drop(manager1);
    eprintln!("=== Pipeline restarted after 1 second ===");

    // === Phase 2: After restart, wait remaining time ===
    let manager2 = GateManager::with_timeout(
        all_gates_config(),
        specs_dir.clone(),
        Duration::from_secs(2),
    );

    // Verify timestamp preserved
    let preserved_triggered_at = load_gate_state_file(&specs_dir, spec_id)
        .expect("State file should exist")
        .triggered_at;

    assert_eq!(preserved_triggered_at, original_triggered_at,
        "Trigger timestamp should be preserved across restart");
    eprintln!("✓ Original trigger timestamp preserved: {}", preserved_triggered_at);

    // Wait additional 1.5 seconds (total 2.5 seconds > 2 second timeout)
    std::thread::sleep(Duration::from_millis(1500));
    eprintln!("Total elapsed time: ~2.5 seconds (beyond 2 second timeout)");

    // Now should timeout
    let action2 = manager2.check_gate(Phase::Planning, spec_id);
    match action2 {
        GateAction::Timeout { elapsed } => {
            eprintln!("✓ Gate timed out after restart: {} seconds elapsed", elapsed.as_secs());
            assert!(elapsed.as_secs() >= 2,
                "Timeout should be at least 2 seconds (was {})", elapsed.as_secs());
        }
        other => {
            panic!("Expected Timeout after restart, got {:?}", other);
        }
    }

    eprintln!("\n✓ Test passed: Timeout tracking persists across restart");
    cleanup_dir(&test_dir);
}

/// Test multiple restarts before decision.
///
/// Verifies:
/// - Gate state persists across multiple restart cycles
/// - State remains consistent after multiple GateManager instances
/// - Decision eventually works after multiple restarts
#[test]
fn test_multiple_restarts_before_decision() {
    let test_dir = temp_test_dir("multiple_restarts");
    let specs_dir = test_dir.join("specs");
    fs::create_dir_all(&specs_dir).expect("Failed to create specs dir");

    let spec_id = SpecId::new();
    let spec_dir = specs_dir.join(spec_id.to_string());
    fs::create_dir_all(&spec_dir).expect("Failed to create spec dir");

    // Trigger gate
    let manager1 = GateManager::new(all_gates_config(), specs_dir.clone());
    manager1.trigger_gate(spec_id, Phase::QaReview);

    let original_triggered_at = load_gate_state_file(&specs_dir, spec_id)
        .expect("State file should exist")
        .triggered_at;

    eprintln!("✓ Gate triggered at QaReview phase");

    // Restart 1
    drop(manager1);
    eprintln!("=== Restart 1 ===");
    let manager2 = GateManager::new(all_gates_config(), specs_dir.clone());

    let action = manager2.check_gate(Phase::QaReview, spec_id);
    assert!(matches!(action, GateAction::Pause { .. }), "Should still pause after restart 1");
    eprintln!("✓ Still paused after restart 1");

    // Restart 2
    drop(manager2);
    eprintln!("=== Restart 2 ===");
    let manager3 = GateManager::new(all_gates_config(), specs_dir.clone());

    let action = manager3.check_gate(Phase::QaReview, spec_id);
    assert!(matches!(action, GateAction::Pause { .. }), "Should still pause after restart 2");
    eprintln!("✓ Still paused after restart 2");

    // Verify timestamp still preserved
    let state = load_gate_state_file(&specs_dir, spec_id)
        .expect("State file should exist");
    assert_eq!(state.triggered_at, original_triggered_at,
        "Timestamp should be preserved across multiple restarts");
    eprintln!("✓ Original timestamp preserved across {} restarts", 2);

    // Restart 3 - finally make decision
    drop(manager3);
    eprintln!("=== Restart 3 - Making decision ===");
    let manager4 = GateManager::new(all_gates_config(), specs_dir.clone());

    let decision = GateDecision::Approved {
        feedback: Some("Approved after 3 restarts".to_string()),
    };
    manager4.record_decision(spec_id, Phase::QaReview, decision);
    eprintln!("✓ Decision recorded after 3 restarts");

    let action = manager4.check_gate(Phase::QaReview, spec_id);
    assert!(matches!(action, GateAction::Continue), "Should continue after decision");
    eprintln!("✓ Pipeline continues after decision (post-multiple-restarts)");

    eprintln!("\n✓ Test passed: Multiple restarts handled correctly");
    cleanup_dir(&test_dir);
}

/// Test persistence across different gate phases.
///
/// Verifies:
/// - Gate state persists for all gate phases (Planning, Executing, QaReview)
/// - Phase information is correctly preserved
/// - Decisions work correctly for all phases after restart
#[test]
fn test_persistence_all_phases() {
    let test_dir = temp_test_dir("persistence_all_phases");
    let specs_dir = test_dir.join("specs");
    fs::create_dir_all(&specs_dir).expect("Failed to create specs dir");

    // Test each phase
    for phase in [Phase::Planning, Phase::Executing, Phase::QaReview] {
        eprintln!("\n=== Testing phase: {:?} ===", phase);

        let spec_id = SpecId::new();
        let spec_dir = specs_dir.join(spec_id.to_string());
        fs::create_dir_all(&spec_dir).expect("Failed to create spec dir");

        // Trigger gate
        let manager1 = GateManager::new(all_gates_config(), specs_dir.clone());
        manager1.trigger_gate(spec_id, phase);
        eprintln!("✓ Gate triggered at {:?}", phase);

        // Verify state file
        let state = load_gate_state_file(&specs_dir, spec_id)
            .expect("State file should exist");
        assert_eq!(state.phase, phase, "Phase should match");

        // Simulate restart
        drop(manager1);
        let manager2 = GateManager::new(all_gates_config(), specs_dir.clone());
        eprintln!("✓ Pipeline restarted");

        // Verify state persisted
        let state_after = load_gate_state_file(&specs_dir, spec_id)
            .expect("State file should exist after restart");
        assert_eq!(state_after.phase, phase, "Phase should be preserved");
        assert_eq!(state_after.triggered_at, state.triggered_at, "Timestamp should be preserved");

        // Should still pause
        let action = manager2.check_gate(phase, spec_id);
        assert!(matches!(action, GateAction::Pause { .. }),
            "Should pause after restart for phase {:?}", phase);
        eprintln!("✓ Gate still paused after restart for {:?}", phase);
    }

    eprintln!("\n✓ Test passed: Persistence works for all gate phases");
    cleanup_dir(&test_dir);
}

/// Test GATE_STATE.json file format and structure.
///
/// Verifies:
/// - GATE_STATE.json has correct JSON structure
/// - All required fields are present
/// - File can be parsed by external tools
#[test]
fn test_gate_state_file_format() {
    let test_dir = temp_test_dir("gate_state_format");
    let specs_dir = test_dir.join("specs");
    fs::create_dir_all(&specs_dir).expect("Failed to create specs dir");

    let spec_id = SpecId::new();
    let spec_dir = specs_dir.join(spec_id.to_string());
    fs::create_dir_all(&spec_dir).expect("Failed to create spec dir");

    // Trigger gate
    let manager = GateManager::new(all_gates_config(), specs_dir.clone());
    manager.trigger_gate(spec_id, Phase::Planning);

    // Read raw JSON
    let state_path = spec_dir.join("GATE_STATE.json");
    assert!(state_path.exists(), "GATE_STATE.json should exist");

    let json_content = fs::read_to_string(&state_path)
        .expect("Should be able to read GATE_STATE.json");
    eprintln!("GATE_STATE.json content:\n{}", json_content);

    // Parse as generic JSON
    let json: serde_json::Value = serde_json::from_str(&json_content)
        .expect("Should be valid JSON");

    // Verify required fields
    assert!(json.get("phase").is_some(), "Should have 'phase' field");
    assert!(json.get("triggered_at").is_some(), "Should have 'triggered_at' field");
    assert!(json.get("decision").is_some(), "Should have 'decision' field");
    assert!(json.get("decided_at").is_some(), "Should have 'decided_at' field");
    eprintln!("✓ All required fields present");

    // Verify types
    assert!(json["triggered_at"].is_number(), "'triggered_at' should be a number");
    assert!(json["decision"].is_null(), "'decision' should be null initially");
    assert!(json["decided_at"].is_null(), "'decided_at' should be null initially");
    eprintln!("✓ Field types correct");

    // Verify phase format
    let phase_str = json["phase"].as_str().expect("phase should be a string");
    assert!(["Planning", "Executing", "QaReview", "SpecCreation"].contains(&phase_str),
        "phase should be a valid phase name");
    eprintln!("✓ Phase format valid");

    eprintln!("\n✓ Test passed: GATE_STATE.json format is correct");
    cleanup_dir(&test_dir);
}

/// Test concurrent pipeline instances reading same gate state.
///
/// Verifies:
/// - Multiple GateManager instances can read the same gate state
/// - State remains consistent across concurrent reads
/// - Decision is written to GATE_STATE.json for persistence
/// - DECISION.json is consumed by first check (one-time consumption model)
#[test]
fn test_concurrent_gate_state_access() {
    let test_dir = temp_test_dir("concurrent_gate_access");
    let specs_dir = test_dir.join("specs");
    fs::create_dir_all(&specs_dir).expect("Failed to create specs dir");

    let spec_id = SpecId::new();
    let spec_dir = specs_dir.join(spec_id.to_string());
    fs::create_dir_all(&spec_dir).expect("Failed to create spec dir");

    // Trigger gate
    let manager1 = GateManager::new(all_gates_config(), specs_dir.clone());
    manager1.trigger_gate(spec_id, Phase::Planning);
    eprintln!("✓ Gate triggered");

    // Create multiple managers (simulating concurrent pipeline instances)
    let manager2 = GateManager::new(all_gates_config(), specs_dir.clone());
    let manager3 = GateManager::new(all_gates_config(), specs_dir.clone());
    eprintln!("✓ Created 3 concurrent GateManager instances");

    // All should see the same paused state
    let action1 = manager1.check_gate(Phase::Planning, spec_id);
    let action2 = manager2.check_gate(Phase::Planning, spec_id);
    let action3 = manager3.check_gate(Phase::Planning, spec_id);

    assert!(matches!(action1, GateAction::Pause { .. }), "Manager 1 should see Pause");
    assert!(matches!(action2, GateAction::Pause { .. }), "Manager 2 should see Pause");
    assert!(matches!(action3, GateAction::Pause { .. }), "Manager 3 should see Pause");
    eprintln!("✓ All managers see consistent paused state");

    // One manager records decision
    let decision = GateDecision::Approved {
        feedback: Some("Approved by manager 2".to_string()),
    };
    manager2.record_decision(spec_id, Phase::Planning, decision);
    eprintln!("✓ Manager 2 recorded approval decision");

    // First check consumes the DECISION.json file (load_decision removes it)
    let action1_after = manager1.check_gate(Phase::Planning, spec_id);
    assert!(matches!(action1_after, GateAction::Continue), "Manager 1 should see Continue (consumed decision)");
    eprintln!("✓ Manager 1 consumed decision and continues");

    // Subsequent checks won't see decision file (it was consumed), but gate state shows decision was made
    // Verify decision is recorded in GATE_STATE.json
    let final_state = load_gate_state_file(&specs_dir, spec_id)
        .expect("State file should exist");
    assert!(final_state.decision.is_some(), "Decision should be recorded in gate state");
    assert!(final_state.decided_at.is_some(), "Decision timestamp should be recorded");
    eprintln!("✓ Decision persisted in GATE_STATE.json for other managers to see");

    eprintln!("\n✓ Test passed: Concurrent access handled correctly");
    cleanup_dir(&test_dir);
}
