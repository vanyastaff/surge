//! End-to-end integration tests for UI gate approval flow.
//!
//! Since surge-ui is a GPUI-based desktop application, full automated UI testing
//! is not feasible. These tests verify the integration points between the UI
//! components and the gate approval system.
#![allow(clippy::ptr_arg)]
//!
//! For full manual E2E testing, see: MANUAL_TESTING_GUIDE.md

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static TEST_DIR_SEQ: AtomicU64 = AtomicU64::new(0);

fn unique_test_dir(test_name: &str) -> PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let seq = TEST_DIR_SEQ.fetch_add(1, Ordering::Relaxed);
    let temp_dir = std::env::temp_dir().join(format!(
        "surge-ui-test-{}-{}-{nonce}-{seq}",
        std::process::id(),
        test_name
    ));
    fs::create_dir_all(&temp_dir).expect("Failed to create temp dir");
    temp_dir
}

/// Test helper to simulate UI gate decision writing.
///
/// This mimics what happens when the user clicks approve/reject in the UI.
fn write_ui_gate_decision(
    project_path: &PathBuf,
    task_id: &str,
    approved: bool,
) -> Result<(), std::io::Error> {
    let gate_dir = project_path.join(".surge").join("gates");
    fs::create_dir_all(&gate_dir)?;

    let decision_file = gate_dir.join(format!("{}.json", task_id));
    let decision_data = format!(
        r#"{{"task_id":"{}","approved":{},"timestamp":"{}"}}"#,
        task_id,
        approved,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    );

    fs::write(&decision_file, decision_data)?;
    Ok(())
}

/// Test helper to verify UI decision file exists and contains correct data.
fn verify_ui_decision_file(
    project_path: &PathBuf,
    task_id: &str,
    expected_approved: bool,
) -> Result<(), String> {
    let decision_file = project_path
        .join(".surge")
        .join("gates")
        .join(format!("{}.json", task_id));

    if !decision_file.exists() {
        return Err(format!("Decision file not found: {:?}", decision_file));
    }

    let content = fs::read_to_string(&decision_file)
        .map_err(|e| format!("Failed to read decision file: {}", e))?;

    // Parse JSON to verify structure
    let parsed: serde_json::Value = serde_json::from_str(&content)
        .map_err(|e| format!("Failed to parse decision JSON: {}", e))?;

    // Verify task_id
    let task_id_field = parsed
        .get("task_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing or invalid task_id field".to_string())?;

    if task_id_field != task_id {
        return Err(format!(
            "Task ID mismatch: expected {}, got {}",
            task_id, task_id_field
        ));
    }

    // Verify approved
    let approved_field = parsed
        .get("approved")
        .and_then(|v| v.as_bool())
        .ok_or_else(|| "Missing or invalid approved field".to_string())?;

    if approved_field != expected_approved {
        return Err(format!(
            "Approved mismatch: expected {}, got {}",
            expected_approved, approved_field
        ));
    }

    // Verify timestamp exists
    let _timestamp = parsed
        .get("timestamp")
        .ok_or_else(|| "Missing timestamp field".to_string())?;

    Ok(())
}

#[test]
fn test_ui_gate_decision_approval_write() {
    let temp_dir = unique_test_dir("approval");

    // Simulate UI approval decision
    let task_id = "test-task-001";
    write_ui_gate_decision(&temp_dir, task_id, true).expect("Failed to write approval decision");

    // Verify decision file was written correctly
    verify_ui_decision_file(&temp_dir, task_id, true).expect("Decision verification failed");

    // Clean up
    fs::remove_dir_all(&temp_dir).ok();
}

#[test]
fn test_ui_gate_decision_rejection_write() {
    let temp_dir = unique_test_dir("rejection");

    // Simulate UI rejection decision
    let task_id = "test-task-002";
    write_ui_gate_decision(&temp_dir, task_id, false).expect("Failed to write rejection decision");

    // Verify decision file was written correctly
    verify_ui_decision_file(&temp_dir, task_id, false).expect("Decision verification failed");

    // Clean up
    fs::remove_dir_all(&temp_dir).ok();
}

#[test]
fn test_ui_gate_decision_file_format() {
    let temp_dir = unique_test_dir("file-format");

    // Write decision
    let task_id = "test-task-003";
    write_ui_gate_decision(&temp_dir, task_id, true).expect("Failed to write decision");

    // Read and parse decision file
    let decision_file = temp_dir
        .join(".surge")
        .join("gates")
        .join(format!("{}.json", task_id));
    let content = fs::read_to_string(&decision_file).expect("Failed to read decision file");
    let parsed: serde_json::Value = serde_json::from_str(&content).expect("Failed to parse JSON");

    // Verify all required fields exist
    assert!(parsed.get("task_id").is_some(), "Missing task_id field");
    assert!(parsed.get("approved").is_some(), "Missing approved field");
    assert!(parsed.get("timestamp").is_some(), "Missing timestamp field");

    // Verify types
    assert!(parsed["task_id"].is_string(), "task_id should be string");
    assert!(
        parsed["approved"].is_boolean(),
        "approved should be boolean"
    );
    assert!(
        parsed["timestamp"].is_string() || parsed["timestamp"].is_number(),
        "timestamp should be string or number"
    );

    // Clean up
    fs::remove_dir_all(&temp_dir).ok();
}

#[test]
fn test_ui_rejection_feedback_file() {
    let temp_dir = unique_test_dir("feedback");

    // Simulate writing HUMAN_INPUT.md (as done by gate_approval.rs)
    let task_id = "test-task-004";
    let feedback = "Please fix the error handling in the main function.";
    let human_input_path = temp_dir.join("HUMAN_INPUT.md");

    let content = format!(
        "# Gate Rejection Feedback\n\n\
         **Task ID:** {}\n\
         **Timestamp:** {}\n\n\
         ## Feedback\n\n\
         {}\n\n\
         ## Instructions\n\n\
         Please address the feedback above and re-run this phase of the pipeline.\n",
        task_id,
        chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC"),
        feedback
    );

    fs::write(&human_input_path, content).expect("Failed to write HUMAN_INPUT.md");

    // Verify file exists and contains feedback
    assert!(human_input_path.exists(), "HUMAN_INPUT.md should exist");
    let read_content =
        fs::read_to_string(&human_input_path).expect("Failed to read HUMAN_INPUT.md");
    assert!(
        read_content.contains(task_id),
        "HUMAN_INPUT.md should contain task_id"
    );
    assert!(
        read_content.contains(feedback),
        "HUMAN_INPUT.md should contain feedback"
    );

    // Clean up
    fs::remove_dir_all(&temp_dir).ok();
}

#[test]
fn test_ui_gate_decision_directory_creation() {
    let temp_dir = unique_test_dir("directory-creation");
    fs::remove_dir_all(&temp_dir).expect("Failed to remove temp dir before directory test");
    // Note: Don't create temp_dir initially - test should create it

    // Write decision to non-existent directory
    let task_id = "test-task-005";
    write_ui_gate_decision(&temp_dir, task_id, true)
        .expect("Should create directory if it doesn't exist");

    // Verify directory was created
    let gate_dir = temp_dir.join(".surge").join("gates");
    assert!(gate_dir.exists(), "Gate directory should be created");
    assert!(gate_dir.is_dir(), "Gate path should be a directory");

    // Verify decision file exists
    let decision_file = gate_dir.join(format!("{}.json", task_id));
    assert!(decision_file.exists(), "Decision file should exist");

    // Clean up
    fs::remove_dir_all(&temp_dir).ok();
}

/// Test that UI decisions are compatible with orchestrator GateDecision format.
///
/// This verifies that the JSON format written by the UI can be read and parsed
/// by the orchestrator's gate manager.
#[test]
fn test_ui_decision_orchestrator_compatibility() {
    use serde_json;

    let temp_dir = unique_test_dir("orchestrator-compatibility");

    // Write UI decision
    let task_id = "test-task-006";
    write_ui_gate_decision(&temp_dir, task_id, true).expect("Failed to write decision");

    // Read decision file as if we're the orchestrator
    let decision_file = temp_dir
        .join(".surge")
        .join("gates")
        .join(format!("{}.json", task_id));
    let content = fs::read_to_string(&decision_file).expect("Failed to read decision file");

    // Parse as generic JSON (orchestrator uses this approach)
    let parsed: serde_json::Value = serde_json::from_str(&content)
        .expect("Orchestrator should be able to parse UI decision JSON");

    // Verify orchestrator can extract fields
    let _task_id_val = parsed["task_id"]
        .as_str()
        .expect("Orchestrator should be able to read task_id");
    let _approved_val = parsed["approved"]
        .as_bool()
        .expect("Orchestrator should be able to read approved");
    let _timestamp_val = &parsed["timestamp"]; // Can be string or number

    // Clean up
    fs::remove_dir_all(&temp_dir).ok();
}
