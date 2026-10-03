//! Real ACP subprocess adapter; only launch arguments are controlled by the fixture.
use agent_client_protocol::schema::v1::RequestPermissionResponse;
use std::path::PathBuf;
use surge_acp::bridge::error::{
    BridgeError, CloseSessionError, OpenSessionError, ReplyToPermissionError, ReplyToToolError,
    SendMessageError,
};
use surge_acp::bridge::facade::BridgeFacade;
use surge_acp::bridge::{
    AcpBridge, AgentKind, BridgeEvent, MessageContent, SessionConfig, SessionState,
    ToolResultPayload,
};
use surge_core::SessionId;
pub struct WireBridge {
    pub bridge: AcpBridge,
    pub flags: Vec<String>,
    pub session_commit_check: Option<(
        std::sync::Arc<surge_persistence::runs::Storage>,
        surge_core::RunId,
    )>,
}
#[async_trait::async_trait]
impl BridgeFacade for WireBridge {
    async fn open_session(
        &self,
        mut config: SessionConfig,
    ) -> Result<surge_core::execution_recovery::OpenedSession, OpenSessionError> {
        config.agent_kind = AgentKind::Custom {
            binary: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!(
                "../../target/debug/mock_acp_agent{}",
                std::env::consts::EXE_SUFFIX
            )),
            args: self.flags.clone(),
        };
        self.bridge.open_session(config).await
    }
    async fn send_message(
        &self,
        id: SessionId,
        content: MessageContent,
    ) -> Result<(), SendMessageError> {
        if let Some((storage, run)) = &self.session_commit_check {
            let inspected = storage
                .inspect_run(*run)
                .await
                .expect("fixture read-only journal");
            let surge_persistence::runs::inspection::RunDatabaseInspection::Present { events } =
                inspected.database
            else {
                panic!("provider prompt before journal creation")
            };
            let opened=events.iter().find(|row|matches!(&row.payload.payload,surge_core::EventPayload::SessionOpened { session,opened:Some(opened),.. } if *session==id && opened.session==id));
            assert!(
                opened.is_some(),
                "real first ACP prompt must wait for durable actual provider identity"
            );
        }
        self.bridge.send_message(id, content).await
    }
    async fn session_state(&self, id: SessionId) -> Result<SessionState, BridgeError> {
        self.bridge.session_state(id).await
    }
    async fn close_session(&self, id: SessionId) -> Result<(), CloseSessionError> {
        if let Some(barrier) = std::env::var_os("SURGE_TEST_CLOSE_BARRIER") {
            let barrier = PathBuf::from(barrier);
            std::fs::write(barrier.with_extension("ready"), b"close reached").unwrap();
            tokio::time::timeout(std::time::Duration::from_secs(20), async {
                while !barrier.exists() {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("isolated fixture close barrier must be released or host killed");
        }
        self.bridge.close_session(id).await
    }
    async fn reply_to_tool(
        &self,
        id: SessionId,
        call: String,
        payload: ToolResultPayload,
    ) -> Result<(), ReplyToToolError> {
        self.bridge.reply_to_tool(id, call, payload).await
    }
    async fn reply_to_permission(
        &self,
        id: SessionId,
        request: String,
        response: RequestPermissionResponse,
    ) -> Result<(), ReplyToPermissionError> {
        self.bridge.reply_to_permission(id, request, response).await
    }
    fn subscribe(&self) -> tokio::sync::broadcast::Receiver<BridgeEvent> {
        self.bridge.subscribe()
    }
}
