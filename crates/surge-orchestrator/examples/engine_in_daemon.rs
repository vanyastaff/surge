//! Example: engine constructed in a hypothetical daemon-style host.
//!
//! Runs two simple terminal-only graphs sequentially against one engine
//! instance. Demonstrates: cheap construction, RunHandle await pattern,
//! repeat usage. Not a real daemon — just shows the API ergonomics.
//!
//! Run with: `cargo run -p surge-orchestrator --example engine_in_daemon`

use std::collections::BTreeMap;
use std::sync::Arc;

use surge_acp::bridge::error::{
    BridgeError, CloseSessionError, OpenSessionError, ReplyToToolError, SendMessageError,
};
use surge_acp::bridge::event::{BridgeEvent, ToolResultPayload};
use surge_acp::bridge::facade::BridgeFacade;
use surge_acp::bridge::session::{MessageContent, SessionConfig, SessionState};
use surge_core::graph::{Graph, GraphMetadata, SCHEMA_VERSION};
use surge_core::id::{RunId, SessionId};
use surge_core::keys::NodeKey;
use surge_core::node::{Node, NodeConfig, Position};
use surge_core::terminal_config::{TerminalConfig, TerminalKind};
use surge_orchestrator::engine::tools::ToolDispatcher;
use surge_orchestrator::engine::tools::worktree::WorktreeToolDispatcher;
use surge_orchestrator::engine::{Engine, EngineConfig, EngineRunConfig};
use surge_persistence::runs::Storage;
use tokio::sync::broadcast;

/// No-op bridge for the example — production daemons construct AcpBridge.
struct NoOpBridge;

#[async_trait::async_trait]
impl BridgeFacade for NoOpBridge {
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
    ) -> Result<(), surge_acp::bridge::ReplyToPermissionError> {
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
        std::mem::forget(tx);
        rx
    }
}

fn terminal_only_graph(name: &str) -> Graph {
    let end = NodeKey::try_from("end").unwrap();
    let mut nodes = BTreeMap::new();
    nodes.insert(
        end.clone(),
        Node {
            id: end.clone(),
            position: Position::default(),
            declared_outcomes: vec![],
            config: NodeConfig::Terminal(TerminalConfig {
                kind: TerminalKind::Success,
                message: None,
            }),
        },
    );
    Graph {
        schema_version: SCHEMA_VERSION,
        metadata: GraphMetadata {
            name: name.into(),
            description: None,
            template_origin: None,
            created_at: chrono::Utc::now(),
            author: None,
            archetype: None,
        },
        start: end,
        nodes,
        edges: vec![],
        subgraphs: BTreeMap::new(),
    }
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let storage = Storage::open(dir.path()).await?;
    let bridge: Arc<dyn BridgeFacade> = Arc::new(NoOpBridge);
    let dispatcher =
        Arc::new(WorktreeToolDispatcher::new(dir.path().to_path_buf())) as Arc<dyn ToolDispatcher>;

    let engine = Engine::new(bridge, storage, dispatcher, EngineConfig::default());

    for i in 0..2 {
        let g = terminal_only_graph(&format!("daemon-run-{i}"));
        let run_id = RunId::new();
        let h = engine
            .start_run(
                run_id,
                g,
                dir.path().to_path_buf(),
                EngineRunConfig::default(),
            )
            .await?;
        let outcome = h.await_completion().await?;
        println!("run {i} → {outcome:?}");
    }

    Ok(())
}
