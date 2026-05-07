//! M6: Graph with 2 edges from the same (node, outcome) port fails validation
//! with EngineError::GraphInvalid whose message contains "multiple edges" and
//! "M8" or "Parallel".

mod fixtures;

use std::collections::BTreeMap;
use std::sync::Arc;
use surge_acp::bridge::facade::BridgeFacade;
use surge_core::edge::{Edge, EdgeKind, EdgePolicy, PortRef};
use surge_core::graph::{Graph, GraphMetadata, SCHEMA_VERSION};
use surge_core::id::RunId;
use surge_core::keys::{EdgeKey, NodeKey, OutcomeKey};
use surge_core::node::{Node, NodeConfig, Position};
use surge_core::terminal_config::{TerminalConfig, TerminalKind};
use surge_orchestrator::engine::tools::worktree::WorktreeToolDispatcher;
use surge_orchestrator::engine::{Engine, EngineConfig, EngineError, EngineRunConfig};
use surge_persistence::runs::Storage;

fn build_multi_edge_graph() -> Graph {
    // Three nodes: a, b, c.
    // Two edges from (a, "done") to b AND to c — this is multi-edge fanout
    // which is M8+ and must be rejected by validate_for_m6.
    let n_a = NodeKey::try_from("a").unwrap();
    let n_b = NodeKey::try_from("b").unwrap();
    let n_c = NodeKey::try_from("c").unwrap();
    let done_outcome = OutcomeKey::try_from("done").unwrap();

    let mut nodes = BTreeMap::new();
    for k in [&n_a, &n_b, &n_c] {
        nodes.insert(
            k.clone(),
            Node {
                id: k.clone(),
                position: Position::default(),
                declared_outcomes: vec![],
                config: NodeConfig::Terminal(TerminalConfig {
                    kind: TerminalKind::Success,
                    message: None,
                }),
            },
        );
    }

    let shared_port = PortRef {
        node: n_a.clone(),
        outcome: done_outcome,
    };

    let edges = vec![
        Edge {
            id: EdgeKey::try_from("e1").unwrap(),
            from: shared_port.clone(),
            to: n_b,
            kind: EdgeKind::Forward,
            policy: EdgePolicy::default(),
        },
        Edge {
            id: EdgeKey::try_from("e2").unwrap(),
            from: shared_port,
            to: n_c,
            kind: EdgeKind::Forward,
            policy: EdgePolicy::default(),
        },
    ];

    Graph {
        schema_version: SCHEMA_VERSION,
        metadata: GraphMetadata {
            name: "multi_edge".into(),
            description: None,
            template_origin: None,
            created_at: chrono::Utc::now(),
            author: None,
            archetype: None,
        },
        start: n_a,
        nodes,
        edges,
        subgraphs: BTreeMap::new(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn multi_edge_same_port_rejected_with_m8_pointer() {
    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::open(dir.path()).await.unwrap();
    let bridge = Arc::new(fixtures::mock_bridge::MockBridge::new()) as Arc<dyn BridgeFacade>;
    let dispatcher = Arc::new(WorktreeToolDispatcher::new(dir.path().to_path_buf()));

    let engine = Engine::new(bridge, storage, dispatcher, EngineConfig::default());

    let run_id = RunId::new();
    let result = engine
        .start_run(
            run_id,
            build_multi_edge_graph(),
            dir.path().to_path_buf(),
            EngineRunConfig::default(),
        )
        .await;
    let err = match result {
        Err(e) => e,
        Ok(_) => panic!("start_run should fail with GraphInvalid but succeeded"),
    };

    match err {
        EngineError::GraphInvalid(msg) => {
            assert!(
                msg.contains("multiple edges") || msg.contains("multi-edge"),
                "error message should mention multiple edges: {msg}"
            );
            assert!(
                msg.contains("M8") || msg.contains("Parallel"),
                "error message should mention M8 or Parallel: {msg}"
            );
        },
        other => panic!("expected GraphInvalid, got {other:?}"),
    }
}
