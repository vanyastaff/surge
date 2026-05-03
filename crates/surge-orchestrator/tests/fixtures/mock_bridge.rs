//! `MockBridge` — scripted `BridgeFacade` impl for unit tests.
//!
//! Records every call against the bridge into `recorded_calls` (so tests can
//! assert order/content). Emits scripted events from `scripted_events` on
//! every call to `subscribe()` — each subscriber receives the same script.

use async_trait::async_trait;
use std::collections::VecDeque;
use std::sync::Arc;
use surge_acp::bridge::error::{
    BridgeError, CloseSessionError, OpenSessionError, ReplyToToolError, SendMessageError,
};
use surge_acp::bridge::event::{BridgeEvent, ToolResultPayload};
use surge_acp::bridge::facade::BridgeFacade;
use surge_acp::bridge::session::{MessageContent, SessionConfig, SessionState};
use surge_core::SessionId;
use tokio::sync::{broadcast, Mutex};

/// Calls recorded against `MockBridge`, in order received.
#[derive(Debug, Clone)]
#[allow(dead_code)] // fields exist for test assertion via pattern matching
pub enum RecordedCall {
    OpenSession,
    SendMessage { session: SessionId },
    ReplyToTool { session: SessionId, call_id: String, payload: ToolResultPayload },
    SessionState { session: SessionId },
    CloseSession(SessionId),
    Subscribe,
}

pub struct MockBridge {
    /// Events to broadcast — each call to `pump_scripted_events()` drains the queue.
    scripted_events: Mutex<VecDeque<BridgeEvent>>,
    /// Calls recorded for assertion.
    pub recorded_calls: Arc<Mutex<Vec<RecordedCall>>>,
    /// Broadcast channel.
    tx: broadcast::Sender<BridgeEvent>,
}

impl MockBridge {
    pub fn new() -> Self {
        let (tx, _rx) = broadcast::channel(256);
        Self {
            scripted_events: Mutex::new(VecDeque::new()),
            recorded_calls: Arc::new(Mutex::new(Vec::new())),
            tx,
        }
    }

    /// Queue an event to be broadcast on the next `pump_scripted_events()`.
    pub async fn enqueue_event(&self, event: BridgeEvent) {
        self.scripted_events.lock().await.push_back(event);
    }

    /// Drain the scripted-event queue and broadcast each event to subscribers.
    /// Tests typically call this after `bridge.subscribe()` returns to ensure
    /// the receiver is alive.
    pub async fn pump_scripted_events(&self) {
        let mut q = self.scripted_events.lock().await;
        while let Some(ev) = q.pop_front() {
            // Ignore SendError (no subscribers) — test may not have subscribed.
            let _ = self.tx.send(ev);
        }
    }
}

impl Default for MockBridge {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl BridgeFacade for MockBridge {
    async fn open_session(
        &self,
        _config: SessionConfig,
    ) -> Result<SessionId, OpenSessionError> {
        self.recorded_calls.lock().await.push(RecordedCall::OpenSession);
        Ok(SessionId::new())
    }

    async fn send_message(
        &self,
        session: SessionId,
        _content: MessageContent,
    ) -> Result<(), SendMessageError> {
        self.recorded_calls.lock().await.push(RecordedCall::SendMessage { session });
        Ok(())
    }

    async fn reply_to_tool(
        &self,
        session: SessionId,
        call_id: String,
        payload: ToolResultPayload,
    ) -> Result<(), ReplyToToolError> {
        self.recorded_calls
            .lock()
            .await
            .push(RecordedCall::ReplyToTool { session, call_id, payload });
        Ok(())
    }

    async fn session_state(
        &self,
        session: SessionId,
    ) -> Result<SessionState, BridgeError> {
        self.recorded_calls.lock().await.push(RecordedCall::SessionState { session });
        // Best-effort default; tests that need a specific state should override.
        Err(BridgeError::WorkerDead)
    }

    async fn close_session(
        &self,
        session: SessionId,
    ) -> Result<(), CloseSessionError> {
        self.recorded_calls.lock().await.push(RecordedCall::CloseSession(session));
        Ok(())
    }

    fn subscribe(&self) -> broadcast::Receiver<BridgeEvent> {
        // Cannot be async; record the call without locking.
        let recorded = self.recorded_calls.clone();
        tokio::spawn(async move {
            recorded.lock().await.push(RecordedCall::Subscribe);
        });
        self.tx.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::str::FromStr;
    use surge_acp::bridge::event::SessionEndReason;
    use surge_acp::bridge::sandbox::AlwaysAllowSandbox;
    use surge_acp::bridge::session::AgentKind;
    use surge_acp::client::PermissionPolicy;
    use surge_core::OutcomeKey;

    fn minimal_session_config() -> SessionConfig {
        SessionConfig {
            agent_kind: AgentKind::Mock { args: vec![] },
            working_dir: PathBuf::from("/tmp/wt"),
            system_prompt: "sys".into(),
            declared_outcomes: vec![OutcomeKey::from_str("done").unwrap()],
            allows_escalation: false,
            tools: vec![],
            sandbox: Box::new(AlwaysAllowSandbox),
            permission_policy: PermissionPolicy::default(),
            bindings: BTreeMap::new(),
        }
    }

    #[tokio::test]
    async fn records_open_session() {
        let m = MockBridge::new();
        let _id = m.open_session(minimal_session_config()).await;
        let calls = m.recorded_calls.lock().await;
        assert!(matches!(calls[0], RecordedCall::OpenSession));
    }

    #[tokio::test]
    async fn records_reply_to_tool() {
        let m = MockBridge::new();
        let session = SessionId::new();
        let _ = m
            .reply_to_tool(
                session,
                "call-1".into(),
                ToolResultPayload::Ok { result_json: "{}".into() },
            )
            .await;
        let calls = m.recorded_calls.lock().await;
        match &calls[0] {
            RecordedCall::ReplyToTool { call_id, .. } => assert_eq!(call_id, "call-1"),
            other => panic!("expected ReplyToTool, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn pumps_scripted_event() {
        let m = MockBridge::new();
        let mut rx = m.subscribe();
        let session = SessionId::new();
        let ev = BridgeEvent::SessionEnded {
            session,
            reason: SessionEndReason::Normal,
        };
        m.enqueue_event(ev).await;
        // Yield to let subscribe task land Subscribe call (best-effort).
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        m.pump_scripted_events().await;
        let received = rx.recv().await.unwrap();
        assert!(matches!(received, BridgeEvent::SessionEnded { .. }));
    }
}
