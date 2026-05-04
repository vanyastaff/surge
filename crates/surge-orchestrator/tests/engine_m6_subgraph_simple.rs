//! M6: Subgraph node with single inner Terminal emits SubgraphEntered +
//! SubgraphExited; run completes via outer Terminal.
//!
//! Graph layout:
//!   sg_node (Subgraph: inner = "inner_sg") -- on "completed" --> end
//!   end (Terminal::Success)
//!
//! Subgraph "inner_sg":
//!   inner_end (Terminal::Success)
//!
//! SubgraphConfig::outputs maps inner_end's success to outer outcome "completed".

mod fixtures;

use std::collections::BTreeMap;
use std::sync::Arc;
use surge_acp::bridge::facade::BridgeFacade;
use surge_core::agent_config::ArtifactSource;
use surge_core::edge::{Edge, EdgeKind, EdgePolicy, PortRef};
use surge_core::graph::{Graph, GraphMetadata, Subgraph, SCHEMA_VERSION};
use surge_core::id::RunId;
use surge_core::keys::{EdgeKey, NodeKey, OutcomeKey, SubgraphKey};
use surge_core::node::{Node, NodeConfig, Position};
use surge_core::run_event::EventPayload;
use surge_core::subgraph_config::{SubgraphConfig, SubgraphOutput};
use surge_core::terminal_config::{TerminalConfig, TerminalKind};
use surge_orchestrator::engine::tools::worktree::WorktreeToolDispatcher;
use surge_orchestrator::engine::{Engine, EngineConfig, EngineRunConfig, RunOutcome};
use surge_persistence::runs::seq::EventSeq;
use surge_persistence::runs::Storage;

fn build_subgraph_graph() -> Graph {
    let sg_node_key = NodeKey::try_from("sg_node").unwrap();
    let end_key = NodeKey::try_from("end").unwrap();
    let inner_sg_key = SubgraphKey::try_from("inner_sg").unwrap();
    let inner_end_key = NodeKey::try_from("inner_end").unwrap();
    let completed_outcome = OutcomeKey::try_from("completed").unwrap();

    // The inner subgraph terminal node is Success, so on_subgraph_done
    // projects it to the outer outcome via SubgraphConfig::outputs.
    let sg_config = SubgraphConfig {
        inner: inner_sg_key.clone(),
        inputs: vec![],
        outputs: vec![SubgraphOutput {
            // ArtifactSource::Static always resolves (no actual artifact needed
            // because the engine uses it only for artifact projection, not for
            // gate decisions when the inner graph completes).
            inner_artifact: ArtifactSource::Static {
                content: "ok".into(),
            },
            outer_outcome: completed_outcome.clone(),
        }],
    };

    let sg_node = Node {
        id: sg_node_key.clone(),
        position: Position::default(),
        declared_outcomes: vec![],
        config: NodeConfig::Subgraph(sg_config),
    };

    let end_node = Node {
        id: end_key.clone(),
        position: Position::default(),
        declared_outcomes: vec![],
        config: NodeConfig::Terminal(TerminalConfig {
            kind: TerminalKind::Success,
            message: None,
        }),
    };

    let inner_end_node = Node {
        id: inner_end_key.clone(),
        position: Position::default(),
        declared_outcomes: vec![],
        config: NodeConfig::Terminal(TerminalConfig {
            kind: TerminalKind::Success,
            message: None,
        }),
    };

    let edge_sg_to_end = Edge {
        id: EdgeKey::try_from("e_sg_done").unwrap(),
        from: PortRef {
            node: sg_node_key.clone(),
            outcome: completed_outcome,
        },
        to: end_key.clone(),
        kind: EdgeKind::Forward,
        policy: EdgePolicy::default(),
    };

    let mut nodes = BTreeMap::new();
    nodes.insert(sg_node_key.clone(), sg_node);
    nodes.insert(end_key, end_node);

    let mut inner_nodes = BTreeMap::new();
    inner_nodes.insert(inner_end_key.clone(), inner_end_node);

    let mut subgraphs = BTreeMap::new();
    subgraphs.insert(
        inner_sg_key,
        Subgraph {
            start: inner_end_key,
            nodes: inner_nodes,
            edges: vec![],
        },
    );

    Graph {
        schema_version: SCHEMA_VERSION,
        metadata: GraphMetadata {
            name: "subgraph_simple".into(),
            description: None,
            template_origin: None,
            created_at: chrono::Utc::now(),
            author: None,
        },
        start: sg_node_key,
        nodes,
        edges: vec![edge_sg_to_end],
        subgraphs,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn subgraph_emits_entered_and_exited_then_completes() {
    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::open(dir.path()).await.unwrap();
    let bridge = Arc::new(fixtures::mock_bridge::MockBridge::new()) as Arc<dyn BridgeFacade>;
    let dispatcher = Arc::new(WorktreeToolDispatcher::new(dir.path().to_path_buf()));

    let engine = Engine::new(bridge, storage.clone(), dispatcher, EngineConfig::default());

    let run_id = RunId::new();
    let handle = engine
        .start_run(
            run_id,
            build_subgraph_graph(),
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

    let reader = storage.open_run_reader(run_id).await.unwrap();
    let events = reader
        .read_events(EventSeq::ZERO..EventSeq(i64::MAX as u64))
        .await
        .unwrap();

    let payloads: Vec<&EventPayload> = events.iter().map(|e| e.payload.payload()).collect();

    let entered = payloads
        .iter()
        .filter(|p| matches!(p, EventPayload::SubgraphEntered { .. }))
        .count();
    let exited = payloads
        .iter()
        .filter(|p| matches!(p, EventPayload::SubgraphExited { .. }))
        .count();

    assert_eq!(entered, 1, "expected 1 SubgraphEntered event");
    assert_eq!(exited, 1, "expected 1 SubgraphExited event");
}
