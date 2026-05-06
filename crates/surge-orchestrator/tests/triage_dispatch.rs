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
use surge_orchestrator::triage::{dispatch_triage, TriageInput, TriageOptions};
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

    // Drive task: wait for dispatcher to subscribe + create scratch dir,
    // then write artifact and emit OutcomeReported.
    let scratch_root = tmp.path().to_path_buf();
    let bridge_for_drive = Arc::clone(&bridge);
    let drive = tokio::spawn(async move {
        // Sleep enough for dispatcher to call subscribe() + open_session() + send_message().
        // The mock open_session is synchronous returning the pinned id, so 80ms is plenty.
        tokio::time::sleep(Duration::from_millis(80)).await;

        let scratch = std::fs::read_dir(&scratch_root)
            .unwrap()
            .filter_map(|e| e.ok())
            .find(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .expect("dispatcher should have created scratch subdir")
            .path();

        let decision = r#"{"decision":"enqueued","priority":"high","priority_reasoning":"prod crash","summary":"Fix panic"}"#;
        let summary = "## Fix parser panic\n\nStack overflow at depth 16.";
        std::fs::write(scratch.join("triage_decision.json"), decision).unwrap();
        std::fs::write(scratch.join("inbox_summary.md"), summary).unwrap();
        bridge_for_drive
            .enqueue_event(BridgeEvent::OutcomeReported {
                session,
                outcome: OutcomeKey::from_str("enqueued").unwrap(),
                summary: "agent picked enqueued".into(),
                artifacts_produced: vec!["triage_decision.json".into(), "inbox_summary.md".into()],
            })
            .await;
        bridge_for_drive.pump_scripted_events().await;
    });

    let result = dispatch_triage(
        Arc::clone(&bridge) as Arc<dyn BridgeFacade>,
        input(),
        opts,
    )
    .await
    .expect("dispatch_triage should succeed");

    drive.await.unwrap();

    match result {
        TriageDecision::Enqueued { priority, .. } => {
            assert_eq!(priority, Priority::High);
        }
        other => panic!("expected Enqueued, got {other:?}"),
    }
}
