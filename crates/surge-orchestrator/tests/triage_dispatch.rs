//! Unit tests for `triage::dispatch_triage` against `MockBridge`.

#[path = "fixtures/mod.rs"]
mod fixtures;

use chrono::Utc;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use surge_acp::bridge::event::BridgeEvent;
use surge_acp::bridge::facade::BridgeFacade;
use surge_core::{OutcomeKey, SessionId};
use surge_intake::types::{Priority, TaskDetails, TaskId, TriageDecision};
use surge_orchestrator::triage::{TriageInput, TriageOptions, dispatch_triage};
use tempfile::TempDir;

fn task_details(id: &str) -> TaskDetails {
    TaskDetails {
        task_id: TaskId::try_new(id).unwrap(),
        source_id: "mock:t".into(),
        title: "Fix parser panic".into(),
        description: "Stack overflow on nested JSON".into(),
        status: "open".into(),
        labels: vec!["surge:enabled".into()],
        url: format!("https://x/{id}"),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        assignee: None,
        raw_payload: serde_json::json!({}),
    }
}

fn input() -> TriageInput {
    TriageInput {
        task: task_details("mock:t#1"),
        candidates: vec![],
        active_runs: vec![],
    }
}

/// Reusable drive task: sleep, find the scratch sub-dir, write the
/// supplied JSON (and optional summary), enqueue an OutcomeReported
/// matching `outcome_key`, then pump events.
async fn drive_one_attempt(
    bridge: Arc<fixtures::mock_bridge::MockBridge>,
    scratch_root: std::path::PathBuf,
    session: SessionId,
    outcome_key: &'static str,
    decision_json: &'static str,
    summary_md: &'static str,
) {
    tokio::time::sleep(Duration::from_millis(80)).await;
    let scratch = std::fs::read_dir(&scratch_root)
        .unwrap()
        .filter_map(|e| e.ok())
        .find(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .expect("dispatcher should have created scratch subdir")
        .path();

    std::fs::write(scratch.join("triage_decision.json"), decision_json).unwrap();
    if !summary_md.is_empty() {
        std::fs::write(scratch.join("inbox_summary.md"), summary_md).unwrap();
    }
    bridge
        .enqueue_event(BridgeEvent::OutcomeReported {
            session,
            outcome: OutcomeKey::from_str(outcome_key).unwrap(),
            summary: format!("agent picked {outcome_key}"),
            artifacts_produced: vec!["triage_decision.json".into(), "inbox_summary.md".into()],
        })
        .await;
    bridge.pump_scripted_events().await;
}

/// Drive multiple sequential attempts. The dispatcher reuses the same
/// scratch dir across retries (per spec), so we don't need to find a
/// new sub-dir each time — just overwrite `triage_decision.json` and
/// emit a fresh `OutcomeReported` per attempt.
async fn drive_n_attempts(
    bridge: Arc<fixtures::mock_bridge::MockBridge>,
    scratch_root: std::path::PathBuf,
    sessions: Vec<SessionId>,
    outcomes_and_artifacts: Vec<(&'static str, &'static str)>,
) {
    assert_eq!(sessions.len(), outcomes_and_artifacts.len());
    for (i, (outcome_key, decision_json)) in outcomes_and_artifacts.iter().enumerate() {
        // Wait for the dispatcher to (re)create scratch and (re)open session.
        // First attempt: dispatcher creates the per-call scratch sub-dir.
        // Later attempts: scratch dir already exists, dispatcher opens new session.
        tokio::time::sleep(Duration::from_millis(80)).await;
        let scratch = std::fs::read_dir(&scratch_root)
            .unwrap()
            .filter_map(|e| e.ok())
            .find(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .expect("dispatcher created scratch subdir")
            .path();
        std::fs::write(scratch.join("triage_decision.json"), decision_json).unwrap();
        bridge
            .enqueue_event(BridgeEvent::OutcomeReported {
                session: sessions[i],
                outcome: OutcomeKey::from_str(outcome_key).unwrap(),
                summary: format!("attempt {} → {outcome_key}", i + 1),
                artifacts_produced: vec!["triage_decision.json".into()],
            })
            .await;
        bridge.pump_scripted_events().await;
    }
}

#[tokio::test]
async fn enqueued_happy_path() {
    let bridge = Arc::new(fixtures::mock_bridge::MockBridge::new());
    let session = SessionId::new();
    bridge.pin_next_session_id(session).await;

    let tmp = TempDir::new().unwrap();
    let opts = TriageOptions {
        claude_binary: Some(std::path::PathBuf::from("/dev/null")),
        attempt_timeout: Duration::from_secs(2),
        max_attempts: 1,
        scratch_root: tmp.path().to_path_buf(),
        keep_scratch_on_failure: false,
    };

    let drive = tokio::spawn(drive_one_attempt(
        Arc::clone(&bridge),
        tmp.path().to_path_buf(),
        session,
        "enqueued",
        r#"{"decision":"enqueued","priority":"high","priority_reasoning":"prod crash","summary":"Fix panic"}"#,
        "## Fix parser panic\n\nStack overflow at depth 16.",
    ));

    let result = dispatch_triage(Arc::clone(&bridge) as Arc<dyn BridgeFacade>, input(), opts)
        .await
        .expect("dispatch_triage should succeed");

    drive.await.unwrap();

    match result {
        TriageDecision::Enqueued { priority, .. } => {
            assert_eq!(priority, Priority::High);
        },
        other => panic!("expected Enqueued, got {other:?}"),
    }
}

#[tokio::test]
async fn duplicate_happy_path() {
    let bridge = Arc::new(fixtures::mock_bridge::MockBridge::new());
    let session = SessionId::new();
    bridge.pin_next_session_id(session).await;

    let tmp = TempDir::new().unwrap();
    let opts = TriageOptions {
        claude_binary: Some(std::path::PathBuf::from("/dev/null")),
        attempt_timeout: Duration::from_secs(2),
        max_attempts: 1,
        scratch_root: tmp.path().to_path_buf(),
        keep_scratch_on_failure: false,
    };

    let drive = tokio::spawn(drive_one_attempt(
        Arc::clone(&bridge),
        tmp.path().to_path_buf(),
        session,
        "duplicate",
        r#"{"decision":"duplicate","duplicate_of":"mock:t#42","priority":"high","priority_reasoning":"same code path"}"#,
        "",
    ));

    let result = dispatch_triage(Arc::clone(&bridge) as Arc<dyn BridgeFacade>, input(), opts)
        .await
        .unwrap();
    drive.await.unwrap();

    match result {
        TriageDecision::Duplicate { of, .. } => {
            assert_eq!(of.as_str(), "mock:t#42");
        },
        other => panic!("expected Duplicate, got {other:?}"),
    }
}

#[tokio::test]
async fn out_of_scope_happy_path() {
    let bridge = Arc::new(fixtures::mock_bridge::MockBridge::new());
    let session = SessionId::new();
    bridge.pin_next_session_id(session).await;

    let tmp = TempDir::new().unwrap();
    let opts = TriageOptions {
        claude_binary: Some(std::path::PathBuf::from("/dev/null")),
        attempt_timeout: Duration::from_secs(2),
        max_attempts: 1,
        scratch_root: tmp.path().to_path_buf(),
        keep_scratch_on_failure: false,
    };

    let drive = tokio::spawn(drive_one_attempt(
        Arc::clone(&bridge),
        tmp.path().to_path_buf(),
        session,
        "out_of_scope",
        r#"{"decision":"out_of_scope","priority":"low","priority_reasoning":"hiring task"}"#,
        "",
    ));

    let result = dispatch_triage(Arc::clone(&bridge) as Arc<dyn BridgeFacade>, input(), opts)
        .await
        .unwrap();
    drive.await.unwrap();

    assert!(matches!(result, TriageDecision::OutOfScope { .. }));
}

#[tokio::test]
async fn unclear_happy_path() {
    let bridge = Arc::new(fixtures::mock_bridge::MockBridge::new());
    let session = SessionId::new();
    bridge.pin_next_session_id(session).await;

    let tmp = TempDir::new().unwrap();
    let opts = TriageOptions {
        claude_binary: Some(std::path::PathBuf::from("/dev/null")),
        attempt_timeout: Duration::from_secs(2),
        max_attempts: 1,
        scratch_root: tmp.path().to_path_buf(),
        keep_scratch_on_failure: false,
    };

    let drive = tokio::spawn(drive_one_attempt(
        Arc::clone(&bridge),
        tmp.path().to_path_buf(),
        session,
        "unclear",
        r#"{"decision":"unclear","priority":"medium","question":"What does X mean here?"}"#,
        "",
    ));

    let result = dispatch_triage(Arc::clone(&bridge) as Arc<dyn BridgeFacade>, input(), opts)
        .await
        .unwrap();
    drive.await.unwrap();

    match result {
        TriageDecision::Unclear { question } => {
            assert!(question.contains("What does X mean here"));
        },
        other => panic!("expected Unclear, got {other:?}"),
    }
}

#[tokio::test]
async fn bad_json_then_recovers() {
    let bridge = Arc::new(fixtures::mock_bridge::MockBridge::new());
    let session_a = SessionId::new();
    let session_b = SessionId::new();
    bridge.pin_session_ids(vec![session_a, session_b]).await;

    let tmp = TempDir::new().unwrap();
    let opts = TriageOptions {
        claude_binary: Some(std::path::PathBuf::from("/dev/null")),
        attempt_timeout: Duration::from_secs(2),
        max_attempts: 3,
        scratch_root: tmp.path().to_path_buf(),
        keep_scratch_on_failure: false,
    };

    let drive = tokio::spawn(drive_n_attempts(
        Arc::clone(&bridge),
        tmp.path().to_path_buf(),
        vec![session_a, session_b],
        vec![
            ("enqueued", "{ this is not valid json"),
            (
                "enqueued",
                r#"{"decision":"enqueued","priority":"medium","priority_reasoning":"ok","summary":"fixed"}"#,
            ),
        ],
    ));

    let result = dispatch_triage(Arc::clone(&bridge) as Arc<dyn BridgeFacade>, input(), opts)
        .await
        .unwrap();
    drive.await.unwrap();

    assert!(matches!(result, TriageDecision::Enqueued { .. }));
}

#[tokio::test]
async fn exhaust_retries_yields_unclear() {
    let bridge = Arc::new(fixtures::mock_bridge::MockBridge::new());
    let s1 = SessionId::new();
    let s2 = SessionId::new();
    let s3 = SessionId::new();
    bridge.pin_session_ids(vec![s1, s2, s3]).await;

    let tmp = TempDir::new().unwrap();
    let opts = TriageOptions {
        claude_binary: Some(std::path::PathBuf::from("/dev/null")),
        attempt_timeout: Duration::from_secs(2),
        max_attempts: 3,
        scratch_root: tmp.path().to_path_buf(),
        keep_scratch_on_failure: true,
    };

    let drive = tokio::spawn(drive_n_attempts(
        Arc::clone(&bridge),
        tmp.path().to_path_buf(),
        vec![s1, s2, s3],
        vec![
            ("enqueued", "{ bad 1"),
            ("enqueued", "{ bad 2"),
            ("enqueued", "{ bad 3"),
        ],
    ));

    let result = dispatch_triage(Arc::clone(&bridge) as Arc<dyn BridgeFacade>, input(), opts)
        .await
        .unwrap();
    drive.await.unwrap();

    match result {
        TriageDecision::Unclear { question } => {
            assert!(
                question.contains("Triage failed after 3 attempts"),
                "expected diagnostic text in question, got: {question}"
            );
        },
        other => panic!("expected Unclear, got {other:?}"),
    }
}
