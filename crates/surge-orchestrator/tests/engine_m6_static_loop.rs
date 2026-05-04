//! M6: 3-iteration static loop completes; event log has 3×LoopIterationStarted +
//! 3×LoopIterationCompleted + 1×LoopCompleted.

mod fixtures;

use std::collections::BTreeMap;
use std::sync::Arc;
use surge_acp::bridge::facade::BridgeFacade;
use surge_core::edge::{Edge, EdgeKind, EdgePolicy, PortRef};
use surge_core::graph::{Graph, GraphMetadata, SCHEMA_VERSION, Subgraph};
use surge_core::id::RunId;
use surge_core::keys::{EdgeKey, NodeKey, OutcomeKey, SubgraphKey};
use surge_core::loop_config::{
    ExitCondition, FailurePolicy, IterableSource, LoopConfig, ParallelismMode,
};
use surge_core::node::{Node, NodeConfig, Position};
use surge_core::run_event::EventPayload;
use surge_core::terminal_config::{TerminalConfig, TerminalKind};
use surge_orchestrator::engine::tools::worktree::WorktreeToolDispatcher;
use surge_orchestrator::engine::{Engine, EngineConfig, EngineRunConfig, RunOutcome};
use surge_persistence::runs::Storage;
use surge_persistence::runs::seq::EventSeq;

fn build_static_loop_graph() -> Graph {
    // Nodes:
    //   loop_1 (Loop: 3 static items, body = "body_sg") -> on completion -> end
    //   end (Terminal::Success)
    //
    // Subgraph "body_sg":
    //   body_end (Terminal::Success)
    //
    // The Loop node routes its `completed` outcome to `end`.

    let loop_key = NodeKey::try_from("loop_1").unwrap();
    let end_key = NodeKey::try_from("end").unwrap();
    let body_sg_key = SubgraphKey::try_from("body_sg").unwrap();
    let body_end_key = NodeKey::try_from("body_end").unwrap();
    let done_outcome = OutcomeKey::try_from("completed").unwrap();

    let loop_node = Node {
        id: loop_key.clone(),
        position: Position::default(),
        declared_outcomes: vec![],
        config: NodeConfig::Loop(LoopConfig {
            iterates_over: IterableSource::Static(vec![
                toml::Value::Integer(1),
                toml::Value::Integer(2),
                toml::Value::Integer(3),
            ]),
            body: body_sg_key.clone(),
            iteration_var_name: "item".into(),
            exit_condition: ExitCondition::AllItems,
            on_iteration_failure: FailurePolicy::Abort,
            parallelism: ParallelismMode::Sequential,
            gate_after_each: false,
        }),
    };

    let end_node = Node {
        id: end_key.clone(),
        position: Position::default(),
        declared_outcomes: vec![],
        config: NodeConfig::Terminal(TerminalConfig {
            kind: TerminalKind::Success,
            message: Some("loop done".into()),
        }),
    };

    let body_end_node = Node {
        id: body_end_key.clone(),
        position: Position::default(),
        declared_outcomes: vec![],
        config: NodeConfig::Terminal(TerminalConfig {
            kind: TerminalKind::Success,
            message: None,
        }),
    };

    let edge_loop_to_end = Edge {
        id: EdgeKey::try_from("e_loop_done").unwrap(),
        from: PortRef {
            node: loop_key.clone(),
            outcome: done_outcome,
        },
        to: end_key.clone(),
        kind: EdgeKind::Forward,
        policy: EdgePolicy::default(),
    };

    let mut nodes = BTreeMap::new();
    nodes.insert(loop_key.clone(), loop_node);
    nodes.insert(end_key, end_node);

    let mut body_nodes = BTreeMap::new();
    body_nodes.insert(body_end_key.clone(), body_end_node);

    let mut subgraphs = BTreeMap::new();
    subgraphs.insert(
        body_sg_key,
        Subgraph {
            start: body_end_key,
            nodes: body_nodes,
            edges: vec![],
        },
    );

    Graph {
        schema_version: SCHEMA_VERSION,
        metadata: GraphMetadata {
            name: "static_loop_3".into(),
            description: None,
            template_origin: None,
            created_at: chrono::Utc::now(),
            author: None,
        },
        start: loop_key,
        nodes,
        edges: vec![edge_loop_to_end],
        subgraphs,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn three_iteration_static_loop_completes() {
    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::open(dir.path()).await.unwrap();
    let bridge = Arc::new(fixtures::mock_bridge::MockBridge::new()) as Arc<dyn BridgeFacade>;
    let dispatcher = Arc::new(WorktreeToolDispatcher::new(dir.path().to_path_buf()));

    let engine = Engine::new(bridge, storage.clone(), dispatcher, EngineConfig::default());

    let run_id = RunId::new();
    let handle = engine
        .start_run(
            run_id,
            build_static_loop_graph(),
            dir.path().to_path_buf(),
            EngineRunConfig::default(),
        )
        .await
        .expect("start_run");

    let outcome = handle.await_completion().await.expect("await_completion");
    match outcome {
        RunOutcome::Completed { .. } => {},
        other => panic!("expected Completed, got {other:?}"),
    }

    // Read the full event log.
    let reader = storage.open_run_reader(run_id).await.unwrap();
    let events = reader
        .read_events(EventSeq::ZERO..EventSeq(i64::MAX as u64))
        .await
        .unwrap();

    let payloads: Vec<&EventPayload> = events.iter().map(|e| e.payload.payload()).collect();

    let started_count = payloads
        .iter()
        .filter(|p| matches!(p, EventPayload::LoopIterationStarted { .. }))
        .count();
    let completed_count = payloads
        .iter()
        .filter(|p| matches!(p, EventPayload::LoopIterationCompleted { .. }))
        .count();
    let loop_done_count = payloads
        .iter()
        .filter(|p| matches!(p, EventPayload::LoopCompleted { .. }))
        .count();

    assert_eq!(started_count, 3, "expected 3 LoopIterationStarted events");
    assert_eq!(
        completed_count, 3,
        "expected 3 LoopIterationCompleted events"
    );
    assert_eq!(loop_done_count, 1, "expected 1 LoopCompleted event");
}
