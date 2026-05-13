//! Daemon smoke test: end-to-end wiring of dispatch_triage. Uses
//! MockBridge + MockTaskSource to validate the TriageDecision
//! constructed for an Enqueued decision carries a non-Medium
//! (LLM-derived) priority.
//!
//! This test does NOT exercise the consumer task in `main.rs`
//! directly (it's tightly coupled to daemon lifecycle); instead we
//! reproduce the dispatch_triage call against a mock bridge with the
//! same TriageInput shape the daemon would build. The point is
//! signature-drift detection between dispatch_triage and the
//! daemon's expected inputs.

use async_trait::async_trait;
use chrono::Utc;
use std::collections::VecDeque;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use surge_acp::bridge::error::{
    BridgeError, CloseSessionError, OpenSessionError, ReplyToToolError, SendMessageError,
};
use surge_acp::bridge::event::{BridgeEvent, ToolResultPayload};
use surge_acp::bridge::facade::BridgeFacade;
use surge_acp::bridge::session::{MessageContent, SessionConfig, SessionState};
use surge_core::{OutcomeKey, SessionId};
use surge_intake::testing::MockTaskSource;
use surge_intake::types::{Priority, TaskDetails, TaskId, TriageDecision};
use tempfile::TempDir;
use tokio::sync::{Mutex, broadcast};

/// Minimal MockBridge fixture for testing dispatch_triage.
/// (Simplified variant of surge-orchestrator::tests::fixtures::mock_bridge::MockBridge.)
struct MockBridge {
    scripted_events: Mutex<VecDeque<BridgeEvent>>,
    tx: broadcast::Sender<BridgeEvent>,
    pinned_session_ids: Mutex<VecDeque<SessionId>>,
}

impl MockBridge {
    fn new() -> Self {
        let (tx, _rx) = broadcast::channel(256);
        Self {
            scripted_events: Mutex::new(VecDeque::new()),
            tx,
            pinned_session_ids: Mutex::new(VecDeque::new()),
        }
    }

    async fn pin_next_session_id(&self, id: SessionId) {
        self.pinned_session_ids.lock().await.push_back(id);
    }

    async fn enqueue_event(&self, event: BridgeEvent) {
        self.scripted_events.lock().await.push_back(event);
    }

    async fn pump_scripted_events(&self) {
        let mut q = self.scripted_events.lock().await;
        while let Some(ev) = q.pop_front() {
            let _ = self.tx.send(ev);
        }
    }
}

#[async_trait]
impl BridgeFacade for MockBridge {
    async fn open_session(&self, _config: SessionConfig) -> Result<SessionId, OpenSessionError> {
        let id = self
            .pinned_session_ids
            .lock()
            .await
            .pop_front()
            .unwrap_or_else(SessionId::new);
        Ok(id)
    }

    async fn send_message(
        &self,
        _session: SessionId,
        _content: MessageContent,
    ) -> Result<(), SendMessageError> {
        Ok(())
    }

    async fn reply_to_tool(
        &self,
        _session: SessionId,
        _call_id: String,
        _payload: ToolResultPayload,
    ) -> Result<(), ReplyToToolError> {
        Ok(())
    }

    async fn reply_to_permission(
        &self,
        _session: SessionId,
        _request_id: String,
        _response: surge_acp::bridge::RequestPermissionResponse,
    ) -> Result<(), surge_acp::bridge::ReplyToPermissionError> {
        Ok(())
    }

    async fn session_state(&self, _session: SessionId) -> Result<SessionState, BridgeError> {
        Err(BridgeError::WorkerDead)
    }

    async fn close_session(&self, _session: SessionId) -> Result<(), CloseSessionError> {
        Ok(())
    }

    fn subscribe(&self) -> broadcast::Receiver<BridgeEvent> {
        self.tx.subscribe()
    }
}

#[tokio::test]
async fn triage_enqueued_yields_real_priority() {
    // Arrange: mock task source with one task (unused in this test, but kept for symmetry).
    let _src = Arc::new(MockTaskSource::new("mock:t", "mock"));
    let task = TaskDetails {
        task_id: TaskId::try_new("mock:t#1").unwrap(),
        source_id: "mock:t".into(),
        title: "Fix parser panic".into(),
        description: "Stack overflow on invalid UTF-8".into(),
        status: "open".into(),
        labels: vec!["surge:enabled".into()],
        url: "https://example.com/issues/1".into(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        assignee: None,
        raw_payload: serde_json::json!({}),
    };

    // Mock bridge scripted to return Enqueued{priority: Urgent}.
    let bridge = Arc::new(MockBridge::new());
    let session = surge_core::SessionId::new();
    bridge.pin_next_session_id(session).await;

    let tmp = TempDir::new().unwrap();
    let scratch_root = tmp.path().to_path_buf();
    let bridge_drive = Arc::clone(&bridge);
    let drive = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(80)).await;
        // Find the scratch subdirectory created by dispatch_triage.
        let scratch = std::fs::read_dir(&scratch_root)
            .unwrap()
            .filter_map(|e| e.ok())
            .find(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .expect("dispatcher created scratch subdir")
            .path();
        // Write the triage decision JSON with Urgent priority.
        std::fs::write(
            scratch.join("triage_decision.json"),
            r#"{"decision":"enqueued","priority":"urgent","priority_reasoning":"prod crash","summary":"hot fix"}"#,
        )
        .unwrap();
        // Emit the OutcomeReported event to signal completion.
        bridge_drive
            .enqueue_event(BridgeEvent::OutcomeReported {
                session,
                outcome: OutcomeKey::from_str("enqueued").unwrap(),
                summary: "agent completed triage".into(),
                artifacts_produced: vec!["triage_decision.json".into()],
            })
            .await;
        bridge_drive.pump_scripted_events().await;
    });

    let mut opts = surge_orchestrator::triage::TriageOptions::with_scratch_root(
        tmp.path().to_path_buf(),
        Some(std::path::PathBuf::from("/dev/null")),
    );
    opts.attempt_timeout = Duration::from_secs(2);
    opts.max_attempts = 1;
    opts.keep_scratch_on_failure = false;
    let input = surge_orchestrator::triage::TriageInput {
        task: task.clone(),
        candidates: vec![],
        active_runs: vec![],
    };

    let result = surge_orchestrator::triage::dispatch_triage(
        Arc::clone(&bridge) as Arc<dyn BridgeFacade>,
        input,
        opts,
    )
    .await
    .unwrap();
    drive.await.unwrap();

    // Assert: not Medium, and priority round-trips into the decision.
    match result {
        TriageDecision::Enqueued { priority, .. } => {
            assert_eq!(priority, Priority::Urgent);
            assert_ne!(
                priority,
                Priority::Medium,
                "must NOT regress to Medium placeholder"
            );
        },
        other => panic!("expected Enqueued, got {other:?}"),
    }
}
