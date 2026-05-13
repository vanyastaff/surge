//! Task 5.1 — drive every bundled `examples/flow_*.toml` archetype through
//! the engine against a deterministic mock ACP bridge.
//!
//! The bridge chooses the first declared outcome for every agent session.
//! The examples are authored so those first outcomes follow the happy path,
//! which gives this suite a deterministic terminal run for every archetype
//! without requiring an external ACP binary.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use surge_acp::bridge::error::{
    BridgeError, CloseSessionError, OpenSessionError, ReplyToToolError, SendMessageError,
};
use surge_acp::bridge::event::{BridgeEvent, ToolResultPayload};
use surge_acp::bridge::facade::BridgeFacade;
use surge_acp::bridge::session::{MessageContent, SessionConfig, SessionState};
use surge_core::graph::Graph;
use surge_core::id::{RunId, SessionId};
use surge_core::keys::OutcomeKey;
use surge_core::run_event::EventPayload;
use surge_orchestrator::engine::tools::ToolDispatcher;
use surge_orchestrator::engine::tools::worktree::WorktreeToolDispatcher;
use surge_orchestrator::engine::{Engine, EngineConfig, EngineRunConfig, RunOutcome};
use surge_persistence::runs::Storage;
use surge_persistence::runs::seq::EventSeq;
use tokio::sync::{Mutex, broadcast};

const ARCHETYPES: &[&str] = &[
    "flow_terminal_only.toml",
    "flow_minimal_agent.toml",
    "flow_linear_3.toml",
    "flow_single_loop.toml",
    "flow_multi_milestone.toml",
    "flow_bug_fix.toml",
    "flow_refactor.toml",
    "flow_spike.toml",
];

fn examples_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("examples")
}

fn load_archetype(name: &str) -> Graph {
    let path = examples_dir().join(name);
    let toml_s =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    toml::from_str(&toml_s).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

struct DeterministicMockBridge {
    tx: broadcast::Sender<BridgeEvent>,
    outcomes: Mutex<HashMap<SessionId, OutcomeKey>>,
}

impl DeterministicMockBridge {
    fn new() -> Self {
        let (tx, _) = broadcast::channel(64);
        Self {
            tx,
            outcomes: Mutex::new(HashMap::new()),
        }
    }
}

#[async_trait]
impl BridgeFacade for DeterministicMockBridge {
    async fn open_session(&self, config: SessionConfig) -> Result<SessionId, OpenSessionError> {
        let session = SessionId::new();
        let outcome = config
            .declared_outcomes
            .first()
            .cloned()
            .unwrap_or_else(|| OutcomeKey::try_from("done").expect("'done' is a valid outcome"));
        self.outcomes.lock().await.insert(session, outcome);
        Ok(session)
    }

    async fn send_message(
        &self,
        session: SessionId,
        _content: MessageContent,
    ) -> Result<(), SendMessageError> {
        let outcome = self
            .outcomes
            .lock()
            .await
            .get(&session)
            .cloned()
            .unwrap_or_else(|| OutcomeKey::try_from("done").expect("'done' is a valid outcome"));
        let _ = self.tx.send(BridgeEvent::OutcomeReported {
            session,
            outcome,
            summary: "deterministic mock outcome".into(),
            artifacts_produced: vec![],
        });
        Ok(())
    }

    async fn session_state(&self, _session: SessionId) -> Result<SessionState, BridgeError> {
        Err(BridgeError::WorkerDead)
    }

    async fn close_session(&self, _session: SessionId) -> Result<(), CloseSessionError> {
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
        _response: agent_client_protocol::RequestPermissionResponse,
    ) -> Result<(), surge_acp::bridge::ReplyToPermissionError> {
        Ok(())
    }

    fn subscribe(&self) -> broadcast::Receiver<BridgeEvent> {
        self.tx.subscribe()
    }
}

async fn run_archetype(name: &str) -> Vec<surge_persistence::runs::reader::ReadEvent> {
    let dir = tempfile::tempdir().expect("tempdir");
    let storage = Storage::open(dir.path()).await.expect("storage");
    let bridge = Arc::new(DeterministicMockBridge::new()) as Arc<dyn BridgeFacade>;
    let dispatcher =
        Arc::new(WorktreeToolDispatcher::new(dir.path().to_path_buf())) as Arc<dyn ToolDispatcher>;
    let engine = Engine::new(bridge, storage.clone(), dispatcher, EngineConfig::default());

    let run_id = RunId::new();
    let handle = engine
        .start_run(
            run_id,
            load_archetype(name),
            dir.path().to_path_buf(),
            EngineRunConfig::default(),
        )
        .await
        .unwrap_or_else(|e| panic!("{name}: start_run failed: {e}"));

    let outcome = tokio::time::timeout(Duration::from_secs(30), handle.await_completion())
        .await
        .unwrap_or_else(|_| panic!("{name}: run hung > 30s"))
        .expect("await_completion");
    match outcome {
        RunOutcome::Completed { .. } => {},
        other => panic!("{name}: expected Completed, got {other:?}"),
    }
    drop(engine);

    let reader = storage.open_run_reader(run_id).await.unwrap();
    let last = reader.current_seq().await.unwrap();
    reader
        .read_events(EventSeq(0)..EventSeq(last.0 + 1))
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn all_archetypes_complete_against_deterministic_mock_bridge() {
    for name in ARCHETYPES {
        let events = run_archetype(name).await;
        assert!(
            events.iter().all(|ev| !matches!(
                ev.payload.payload,
                EventPayload::StageFailed { .. } | EventPayload::RunFailed { .. }
            )),
            "{name}: run contained StageFailed/RunFailed: {events:?}"
        );
        assert!(
            events
                .iter()
                .any(|ev| matches!(ev.payload.payload, EventPayload::RunCompleted { .. })),
            "{name}: missing RunCompleted event"
        );
    }
}
