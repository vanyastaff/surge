//! Real ACP subprocess adapter; only launch arguments are controlled by the fixture.
use agent_client_protocol::schema::v1::RequestPermissionResponse;
use std::path::PathBuf;
use surge_acp::bridge::error::{
    BridgeError, CloseSessionError, OpenSessionError, ReplyToPermissionError, ReplyToToolError,
    SendMessageError,
};
use surge_acp::bridge::facade::BridgeFacade;
use surge_acp::bridge::{
    AcpBridge, BridgeEvent, MessageContent, SessionConfig, SessionState, ToolResultPayload,
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
impl WireBridge {
    /// Configure the actual recipe before the engine's admission decorator sees it.
    pub fn engine_config(
        &self,
        home: &std::path::Path,
    ) -> surge_orchestrator::engine::EngineConfig {
        use surge_orchestrator::profile_loader::{DiskProfileSet, ProfileRegistry};
        let profiles = home.join("wire-fixture-profiles");
        std::fs::create_dir_all(&profiles).unwrap();
        std::fs::write(
            profiles.join("implementer-1.0.toml"),
            r#"
schema_version = 1
[role]
id = "implementer"
version = "1.0.0"
display_name = "Wire fixture"
category = "agents"
description = "Actual ACP fixture recipe"
when_to_use = "Tests"
[runtime]
recommended_model = "sonnet"
agent_id = "wire-fixture"
[[outcomes]]
id = "done"
description = "Success"
edge_kind_hint = "forward"
[prompt]
system = "Report done using the supplied stage outcome tool."
"#,
        )
        .unwrap();
        let binary = std::env::var_os("SURGE_TEST_MOCK_ACP_BINARY")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!(
                    "../../target/debug/mock_acp_agent{}",
                    std::env::consts::EXE_SUFFIX
                ))
            });
        let agent = serde_json::from_value(serde_json::json!({
            "command": binary, "args": self.flags
        }))
        .unwrap();
        surge_orchestrator::engine::EngineConfig {
            profile_registry: Some(std::sync::Arc::new(ProfileRegistry::new(
                DiskProfileSet::scan(&profiles).unwrap(),
            ))),
            agent_registry: Some(std::sync::Arc::new(surge_acp::Registry::from_config(
                std::collections::HashMap::from([("wire-fixture".to_owned(), agent)]),
            ))),
            ..Default::default()
        }
    }
}
#[async_trait::async_trait]
impl BridgeFacade for WireBridge {
    async fn open_session(
        &self,
        mut config: SessionConfig,
    ) -> Result<surge_core::execution_recovery::OpenedSession, OpenSessionError> {
        if let Some(proxy) = std::env::var_os("SURGE_TEST_CONTINUATION_MCP_PROXY") {
            let python = std::env::var("SURGE_TEST_CONTINUATION_PYTHON").unwrap();
            let script = std::env::var("SURGE_TEST_CONTINUATION_SCRIPT").unwrap();
            let stage = config
                .stage_mcp
                .as_mut()
                .expect("actual stage MCP descriptor");
            let mut args = vec![
                PathBuf::from(proxy).display().to_string(),
                script,
                stage.session.to_string(),
                stage.server.command.display().to_string(),
            ];
            args.append(&mut stage.server.args);
            stage.server.command = python.into();
            stage.server.args = args;
        }
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
        let selected = if let Ok(node) = std::env::var("SURGE_TEST_CLOSE_NODE") {
            let (storage, run) = self
                .session_commit_check
                .as_ref()
                .expect("selected close journal");
            let inspected = storage.inspect_run(*run).await.unwrap();
            let surge_persistence::runs::inspection::RunDatabaseInspection::Present { events } =
                inspected.database
            else {
                panic!("missing selected close journal")
            };
            events.iter().any(|event| matches!(event.payload.payload(), surge_core::EventPayload::SessionOpened { session, node: opened_node, .. } if *session == id && opened_node.as_str() == node))
        } else {
            true
        };
        if selected && let Some(barrier) = std::env::var_os("SURGE_TEST_CLOSE_BARRIER") {
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
