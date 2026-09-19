//! Integration test: run-level pause (T14).
//!
//! A two-stage run is paused while stage 1 waits for its scripted outcome,
//! and the pause is proven to hold *between* the stages: stage 2 must not
//! open a session until the operator resumes. This is the engine-side half
//! of the acceptance criterion "`surge task pause` stops new dispatch and
//! halts the running run at its next stage boundary" — the queue-side half
//! (`TaskScheduler` not dispatching) is `ato_outer_test`'s.

mod fixtures;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use surge_acp::bridge::event::BridgeEvent;
use surge_acp::bridge::facade::BridgeFacade;
use surge_core::SessionId;
use surge_core::agent_config::{AgentConfig, NodeLimits};
use surge_core::edge::{Edge, EdgeKind, EdgePolicy, PortRef};
use surge_core::graph::{Graph, GraphMetadata, SCHEMA_VERSION};
use surge_core::id::RunId;
use surge_core::keys::{EdgeKey, NodeKey, OutcomeKey, ProfileKey};
use surge_core::node::{Node, NodeConfig, OutcomeDecl, Position};
use surge_core::terminal_config::{TerminalConfig, TerminalKind};
use surge_orchestrator::engine::tools::ToolDispatcher;
use surge_orchestrator::engine::tools::worktree::WorktreeToolDispatcher;
use surge_orchestrator::engine::{Engine, EngineConfig, EngineError, EngineRunConfig, RunOutcome};
use surge_persistence::runs::Storage;

use fixtures::mock_bridge::{MockBridge, RecordedCall};

fn agent_node(id: &str) -> Node {
    Node {
        id: NodeKey::try_from(id).unwrap(),
        position: Position::default(),
        declared_outcomes: vec![OutcomeDecl {
            id: OutcomeKey::try_from("done").unwrap(),
            description: "done".into(),
            edge_kind_hint: EdgeKind::Forward,
            is_terminal: false,
            ledger_effect: Default::default(),
        }],
        config: NodeConfig::Agent(AgentConfig {
            profile: ProfileKey::try_from("implementer@1.0").unwrap(),
            prompt_overrides: None,
            tool_overrides: None,
            sandbox_override: None,
            approvals_override: None,
            bindings: vec![],
            rules_overrides: None,
            limits: NodeLimits::default(),
            hooks: vec![],
            custom_fields: Default::default(),
        }),
    }
}

fn terminal(id: &str) -> Node {
    Node {
        id: NodeKey::try_from(id).unwrap(),
        position: Position::default(),
        declared_outcomes: vec![],
        config: NodeConfig::Terminal(TerminalConfig {
            kind: TerminalKind::Success,
            message: None,
        }),
    }
}

fn edge(id: &str, from_node: &str, from_outcome: &str, to: &str) -> Edge {
    Edge {
        id: EdgeKey::try_from(id).unwrap(),
        from: PortRef {
            node: NodeKey::try_from(from_node).unwrap(),
            outcome: OutcomeKey::try_from(from_outcome).unwrap(),
        },
        to: NodeKey::try_from(to).unwrap(),
        kind: EdgeKind::Forward,
        policy: EdgePolicy::default(),
    }
}

fn two_stage_graph() -> Graph {
    let mut nodes = BTreeMap::new();
    nodes.insert(NodeKey::try_from("stage_1").unwrap(), agent_node("stage_1"));
    nodes.insert(NodeKey::try_from("stage_2").unwrap(), agent_node("stage_2"));
    nodes.insert(NodeKey::try_from("end").unwrap(), terminal("end"));
    Graph {
        schema_version: SCHEMA_VERSION,
        metadata: GraphMetadata {
            name: "pause-two-stage".into(),
            description: None,
            template_origin: None,
            created_at: chrono::Utc::now(),
            author: None,
            archetype: None,
            when_to_use: None,
            autonomy: None,
        },
        start: NodeKey::try_from("stage_1").unwrap(),
        nodes,
        edges: vec![
            edge("e1", "stage_1", "done", "stage_2"),
            edge("e2", "stage_2", "done", "end"),
        ],
        subgraphs: BTreeMap::new(),
    }
}

async fn open_session_count(mock: &MockBridge) -> usize {
    mock.recorded_calls
        .lock()
        .await
        .iter()
        .filter(|call| matches!(call, RecordedCall::OpenSession))
        .count()
}

fn outcome_event(session: SessionId) -> BridgeEvent {
    BridgeEvent::OutcomeReported {
        session,
        outcome: OutcomeKey::try_from("done").unwrap(),
        summary: "done".into(),
        artifacts_produced: vec![],
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pause_holds_the_run_between_stages_and_resume_completes_it() {
    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::open(dir.path()).await.unwrap();
    let mock = Arc::new(MockBridge::new());
    let bridge: Arc<dyn BridgeFacade> = mock.clone();
    let dispatcher =
        Arc::new(WorktreeToolDispatcher::new(dir.path().to_path_buf())) as Arc<dyn ToolDispatcher>;
    let engine = Engine::new(bridge, storage, dispatcher, EngineConfig::default());

    let session_1 = SessionId::new();
    let session_2 = SessionId::new();
    mock.pin_session_ids(vec![session_1, session_2]).await;
    mock.enqueue_event(outcome_event(session_1)).await;

    let run_id = RunId::new();
    let handle = engine
        .start_run(
            run_id,
            two_stage_graph(),
            dir.path().to_path_buf(),
            EngineRunConfig::default(),
        )
        .await
        .expect("start_run");

    // Stage 1 has subscribed and is waiting for its scripted outcome.
    mock.wait_for_subscribe_count(1).await;
    assert_eq!(
        open_session_count(&mock).await,
        1,
        "stage 1 opened its session"
    );

    // Pause while stage 1 is still in flight, then let it finish: the run
    // must stop at the boundary and never open stage 2's session.
    engine.pause_run(run_id).await.expect("pause_run");
    assert!(engine.is_run_paused(run_id).await.unwrap());
    mock.pump_scripted_events().await;

    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        open_session_count(&mock).await,
        1,
        "a paused run must not open the next stage's session"
    );

    // Resume, feed stage 2 its outcome, and the run completes.
    mock.enqueue_event(outcome_event(session_2)).await;
    let mock_for_pump = mock.clone();
    let pump = tokio::spawn(async move {
        mock_for_pump.pump_after_subscribe(2).await;
    });
    engine.unpause_run(run_id).await.expect("unpause_run");
    assert!(!engine.is_run_paused(run_id).await.unwrap());

    let outcome = tokio::time::timeout(Duration::from_secs(10), handle.await_completion())
        .await
        .expect("run timed out")
        .expect("run join");
    pump.await.unwrap();
    assert_eq!(
        open_session_count(&mock).await,
        2,
        "stage 2 ran after resume"
    );
    match outcome {
        RunOutcome::Completed { terminal } => assert_eq!(terminal.as_ref(), "end"),
        other => panic!("expected Completed after resume, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stopping_a_paused_run_wins_over_the_pause() {
    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::open(dir.path()).await.unwrap();
    let mock = Arc::new(MockBridge::new());
    let bridge: Arc<dyn BridgeFacade> = mock.clone();
    let dispatcher =
        Arc::new(WorktreeToolDispatcher::new(dir.path().to_path_buf())) as Arc<dyn ToolDispatcher>;
    let engine = Engine::new(bridge, storage, dispatcher, EngineConfig::default());

    let session_1 = SessionId::new();
    mock.pin_session_ids(vec![session_1]).await;
    mock.enqueue_event(outcome_event(session_1)).await;

    let run_id = RunId::new();
    let handle = engine
        .start_run(
            run_id,
            two_stage_graph(),
            dir.path().to_path_buf(),
            EngineRunConfig::default(),
        )
        .await
        .expect("start_run");

    mock.wait_for_subscribe_count(1).await;
    engine.pause_run(run_id).await.expect("pause_run");
    mock.pump_scripted_events().await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Stop wins: the paused run must abort rather than wait forever.
    engine
        .stop_run(run_id, "operator stopped".into())
        .await
        .expect("stop_run");
    let outcome = tokio::time::timeout(Duration::from_secs(10), handle.await_completion())
        .await
        .expect("run timed out")
        .expect("run join");
    assert!(
        matches!(outcome, RunOutcome::Aborted { .. }),
        "expected Aborted, got {outcome:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pause_and_resume_of_an_unknown_run_is_run_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::open(dir.path()).await.unwrap();
    let bridge = Arc::new(fixtures::mock_bridge::MockBridge::new()) as Arc<dyn BridgeFacade>;
    let dispatcher =
        Arc::new(WorktreeToolDispatcher::new(dir.path().to_path_buf())) as Arc<dyn ToolDispatcher>;
    let engine = Engine::new(bridge, storage, dispatcher, EngineConfig::default());

    let unknown = RunId::new();
    assert!(matches!(
        engine.pause_run(unknown).await,
        Err(EngineError::RunNotFound(_))
    ));
    assert!(matches!(
        engine.unpause_run(unknown).await,
        Err(EngineError::RunNotFound(_))
    ));
}
