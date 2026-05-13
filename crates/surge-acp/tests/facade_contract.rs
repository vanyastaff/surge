//! Property test: `AcpBridge` and a minimal mock must satisfy the same
//! `BridgeFacade` trait surface for an open→close scenario. Catches signature
//! drift if either implementation diverges.

use std::str::FromStr;
use surge_acp::bridge::acp_bridge::AcpBridge;
use surge_acp::bridge::error::{
    BridgeError, CloseSessionError, OpenSessionError, ReplyToPermissionError, ReplyToToolError,
    SendMessageError,
};
use surge_acp::bridge::event::{BridgeEvent, ToolResultPayload};
use surge_acp::bridge::facade::BridgeFacade;
use surge_acp::bridge::sandbox::AlwaysAllowSandbox;
use surge_acp::bridge::session::{AgentKind, MessageContent, SessionConfig, SessionState};
use surge_acp::client::PermissionPolicy;
use surge_core::{OutcomeKey, SessionId};
use tokio::sync::broadcast;

// Minimal mock that just compiles against the trait — exercise of the
// contract surface, not behavior. Richer MockBridge lives in
// surge-orchestrator/tests/fixtures.
struct MinimalMock;

#[async_trait::async_trait]
impl BridgeFacade for MinimalMock {
    async fn open_session(&self, _: SessionConfig) -> Result<SessionId, OpenSessionError> {
        Ok(SessionId::new())
    }
    async fn send_message(&self, _: SessionId, _: MessageContent) -> Result<(), SendMessageError> {
        Ok(())
    }
    async fn reply_to_tool(
        &self,
        _: SessionId,
        _: String,
        _: ToolResultPayload,
    ) -> Result<(), ReplyToToolError> {
        Ok(())
    }
    async fn reply_to_permission(
        &self,
        _: SessionId,
        _: String,
        _: agent_client_protocol::RequestPermissionResponse,
    ) -> Result<(), ReplyToPermissionError> {
        Ok(())
    }
    async fn session_state(&self, _: SessionId) -> Result<SessionState, BridgeError> {
        Err(BridgeError::WorkerDead)
    }
    async fn close_session(&self, _: SessionId) -> Result<(), CloseSessionError> {
        Ok(())
    }
    fn subscribe(&self) -> broadcast::Receiver<BridgeEvent> {
        let (tx, rx) = broadcast::channel(1);
        std::mem::forget(tx); // keep alive for test
        rx
    }
}

fn minimal_session_config() -> SessionConfig {
    SessionConfig {
        agent_kind: AgentKind::Mock { args: vec![] },
        working_dir: std::path::PathBuf::from("/tmp/wt"),
        system_prompt: "sys".into(),
        declared_outcomes: vec![OutcomeKey::from_str("done").unwrap()],
        allows_escalation: false,
        tools: vec![],
        sandbox: Box::new(AlwaysAllowSandbox),
        permission_policy: PermissionPolicy::default(),
        bindings: Default::default(),
    }
}

/// Generic contract: any BridgeFacade impl can be opened and closed.
async fn open_and_close<B: BridgeFacade>(b: &B) -> bool {
    match b.open_session(minimal_session_config()).await {
        Ok(id) => b.close_session(id).await.is_ok(),
        Err(_) => false,
    }
}

#[tokio::test]
async fn minimal_mock_satisfies_facade_contract() {
    let mock = MinimalMock;
    assert!(open_and_close(&mock).await);
}

// Real-bridge test — requires the worker thread to actually run. Spawning the
// agent subprocess will fail (no mock_acp_agent on PATH in plain test runs),
// so open_session returns AgentSpawnFailed; close_session is then trivially
// a no-op for an unknown session id. We only assert the calls don't deadlock
// or panic — i.e., the facade trait is wired correctly.
#[tokio::test(flavor = "multi_thread")]
async fn real_acp_bridge_satisfies_facade_contract() {
    let bridge = AcpBridge::with_defaults().expect("AcpBridge::with_defaults");
    // open_session returns Err(AgentSpawnFailed) — that's fine for contract
    // shape verification. We just need the call to complete.
    let _ = bridge.open_session(minimal_session_config()).await;
    // close_session on a never-opened id; expect Err but no panic/deadlock.
    let _ = bridge.close_session(SessionId::new()).await;
    bridge.shutdown().await.unwrap();
}
