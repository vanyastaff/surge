//! Sanity test: three concurrent single-terminal-node runs each complete
//! independently without interfering with one another.

mod fixtures;

use std::collections::BTreeMap;
use std::sync::Arc;
use surge_acp::bridge::facade::BridgeFacade;
use surge_core::graph::{Graph, GraphMetadata, SCHEMA_VERSION};
use surge_core::id::RunId;
use surge_core::keys::NodeKey;
use surge_core::node::{Node, NodeConfig, Position};
use surge_core::terminal_config::{TerminalConfig, TerminalKind};
use surge_orchestrator::engine::tools::worktree::WorktreeToolDispatcher;
use surge_orchestrator::engine::tools::ToolDispatcher;
use surge_orchestrator::engine::{Engine, EngineConfig, EngineRunConfig, RunOutcome};
use surge_persistence::runs::Storage;

fn minimal_graph(name: &str) -> Graph {
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
        },
        start: end,
        nodes,
        edges: vec![],
        subgraphs: BTreeMap::new(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn three_concurrent_runs_complete_independently() {
    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::open(dir.path()).await.unwrap();
    let bridge = Arc::new(fixtures::mock_bridge::MockBridge::new()) as Arc<dyn BridgeFacade>;
    let dispatcher =
        Arc::new(WorktreeToolDispatcher::new(dir.path().to_path_buf())) as Arc<dyn ToolDispatcher>;

    let engine = Arc::new(Engine::new(
        bridge,
        storage,
        dispatcher,
        EngineConfig::default(),
    ));

    let mut handles = vec![];
    for i in 0..3 {
        let eng = engine.clone();
        let dir_path = dir.path().to_path_buf();
        handles.push(tokio::spawn(async move {
            let g = minimal_graph(&format!("run-{i}"));
            let h = eng
                .start_run(RunId::new(), g, dir_path, EngineRunConfig::default())
                .await
                .unwrap();
            h.await_completion().await.unwrap()
        }));
    }

    for h in handles {
        let outcome = h.await.unwrap();
        assert!(matches!(outcome, RunOutcome::Completed { .. }));
    }
}
