//! Shared bootstrap-driver test harness.

#![allow(dead_code)]

use std::sync::Arc;
use std::time::Duration;

use surge_acp::bridge::event::BridgeEvent;
use surge_acp::bridge::facade::BridgeFacade;
use surge_core::id::{RunId, SessionId};
use surge_core::keys::OutcomeKey;
use surge_core::run_event::EventPayload;
use surge_orchestrator::bootstrap_driver::{
    BootstrapError, MaterializedRun, run_bootstrap_in_worktree,
};
use surge_orchestrator::engine::tools::ToolDispatcher;
use surge_orchestrator::engine::tools::worktree::WorktreeToolDispatcher;
use surge_orchestrator::engine::{Engine, EngineConfig};
use surge_persistence::runs::{EventSeq, ReadEvent, Storage};
use tokio::task::JoinHandle;

use super::mock_bridge::{MockBridge, RecordedCall};

pub struct BootstrapHarness {
    pub dir: tempfile::TempDir,
    pub storage: Arc<Storage>,
    pub mock: Arc<MockBridge>,
    pub engine: Arc<Engine>,
    pub run_id: RunId,
    pub sessions: Vec<SessionId>,
}

impl BootstrapHarness {
    pub async fn new(session_count: usize) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();
        let mock = Arc::new(MockBridge::new());
        let bridge: Arc<dyn BridgeFacade> = mock.clone();
        let dispatcher = Arc::new(WorktreeToolDispatcher::new(dir.path().to_path_buf()))
            as Arc<dyn ToolDispatcher>;
        let engine = Arc::new(Engine::new(
            bridge,
            storage.clone(),
            dispatcher,
            EngineConfig::default(),
        ));
        let sessions = (0..session_count)
            .map(|_| SessionId::new())
            .collect::<Vec<_>>();
        mock.pin_session_ids(sessions.clone()).await;

        Self {
            dir,
            storage,
            mock,
            engine,
            run_id: RunId::new(),
            sessions,
        }
    }

    pub fn start(&self, prompt: &str) -> JoinHandle<Result<MaterializedRun, BootstrapError>> {
        let engine = self.engine.clone();
        let prompt = prompt.to_owned();
        let run_id = self.run_id;
        let worktree = self.dir.path().to_path_buf();
        tokio::spawn(async move {
            run_bootstrap_in_worktree(engine.as_ref(), prompt, run_id, worktree).await
        })
    }

    pub async fn wait_for_subscribe_count(&self, expected: usize) {
        for _ in 0..100 {
            let count = self
                .mock
                .recorded_calls
                .lock()
                .await
                .iter()
                .filter(|call| matches!(call, RecordedCall::Subscribe))
                .count();
            if count >= expected {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("timed out waiting for {expected} bridge subscribers");
    }

    pub async fn complete_agent_with_artifact(
        &self,
        session_index: usize,
        file_name: &str,
        content: &str,
    ) {
        tokio::fs::write(self.dir.path().join(file_name), content)
            .await
            .unwrap();
        self.report_agent_outcome(session_index, "drafted", file_name)
            .await;
    }

    pub async fn report_agent_outcome(&self, session_index: usize, outcome: &str, artifact: &str) {
        self.mock
            .enqueue_event(BridgeEvent::OutcomeReported {
                session: self.sessions[session_index],
                outcome: OutcomeKey::try_from(outcome).unwrap(),
                summary: "scripted".into(),
                artifacts_produced: vec![artifact.into()],
            })
            .await;
        self.mock.pump_scripted_events().await;
    }

    pub async fn approve_next_gate(&self) {
        self.resolve_next_gate("approve", "ok").await;
    }

    pub async fn edit_next_gate(&self, comment: &str) {
        self.resolve_next_gate("edit", comment).await;
    }

    async fn resolve_next_gate(&self, outcome: &str, comment: &str) {
        for _ in 0..100 {
            let result = self
                .engine
                .resolve_human_input(
                    self.run_id,
                    None,
                    serde_json::json!({ "outcome": outcome, "comment": comment }),
                )
                .await;
            if result.is_ok() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("timed out waiting for pending bootstrap HumanGate");
    }

    pub async fn read_events(&self) -> Vec<ReadEvent> {
        let reader = self.storage.open_run_reader(self.run_id).await.unwrap();
        let max_seq = reader.current_seq().await.unwrap();
        reader
            .read_events(EventSeq(1)..EventSeq(max_seq.as_u64().saturating_add(1)))
            .await
            .unwrap()
    }
}

pub fn event_payloads(events: &[ReadEvent]) -> Vec<&EventPayload> {
    events
        .iter()
        .map(|event| &event.payload.payload)
        .collect::<Vec<_>>()
}

pub fn bundled_flow_toml(name: &str) -> String {
    let graph = surge_core::BundledFlows::by_name_latest(name)
        .unwrap_or_else(|| panic!("bundled flow {name} missing"))
        .graph;
    toml::to_string(&graph).unwrap()
}
