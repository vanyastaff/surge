//! Unit test: a successful linear run writes one snapshot per stage boundary.

mod fixtures;

use std::collections::BTreeMap;
use std::sync::Arc;
use surge_acp::bridge::facade::BridgeFacade;
use surge_core::branch_config::BranchConfig;
use surge_core::edge::{Edge, EdgeKind, EdgePolicy, PortRef};
use surge_core::graph::{Graph, GraphMetadata, SCHEMA_VERSION};
use surge_core::id::RunId;
use surge_core::keys::{EdgeKey, NodeKey, OutcomeKey};
use surge_core::node::{Node, NodeConfig, Position};
use surge_core::terminal_config::{TerminalConfig, TerminalKind};
use surge_orchestrator::engine::tools::worktree::WorktreeToolDispatcher;
use surge_orchestrator::engine::tools::ToolDispatcher;
use surge_orchestrator::engine::{Engine, EngineConfig, EngineRunConfig, RunOutcome};
use surge_persistence::runs::Storage;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn single_terminal_run_has_no_stage_boundary_snapshots() {
    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::open(dir.path()).await.unwrap();
    let bridge = Arc::new(fixtures::mock_bridge::MockBridge::new()) as Arc<dyn BridgeFacade>;
    let dispatcher =
        Arc::new(WorktreeToolDispatcher::new(dir.path().to_path_buf())) as Arc<dyn ToolDispatcher>;

    let engine = Engine::new(bridge, storage.clone(), dispatcher, EngineConfig::default());

    // Single Terminal::Success node — one stage, zero boundary snapshots.
    // Snapshots happen *between* stages; a 1-stage run has no boundary.
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
    let graph = Graph {
        schema_version: SCHEMA_VERSION,
        metadata: GraphMetadata {
            name: "single".into(),
            description: None,
            template_origin: None,
            created_at: chrono::Utc::now(),
            author: None,
        },
        start: end,
        nodes,
        edges: vec![],
        subgraphs: BTreeMap::new(),
    };

    let run_id = RunId::new();
    let handle = engine
        .start_run(
            run_id,
            graph,
            dir.path().to_path_buf(),
            EngineRunConfig::default(),
        )
        .await
        .unwrap();
    let _ = handle.await_completion().await.unwrap();

    // Single-stage runs have zero stage boundaries.
    let snapshot_count = count_snapshots(&storage, run_id).await;
    assert_eq!(snapshot_count, 0, "single-stage run should have 0 snapshots");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn three_node_branch_run_writes_two_snapshots() {
    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::open(dir.path()).await.unwrap();
    let bridge = Arc::new(fixtures::mock_bridge::MockBridge::new()) as Arc<dyn BridgeFacade>;
    let dispatcher =
        Arc::new(WorktreeToolDispatcher::new(dir.path().to_path_buf())) as Arc<dyn ToolDispatcher>;

    let engine = Engine::new(bridge, storage.clone(), dispatcher, EngineConfig::default());

    let b1 = NodeKey::try_from("b1").unwrap();
    let b2 = NodeKey::try_from("b2").unwrap();
    let end = NodeKey::try_from("end").unwrap();

    let mut nodes = BTreeMap::new();
    nodes.insert(
        b1.clone(),
        Node {
            id: b1.clone(),
            position: Position::default(),
            declared_outcomes: vec![],
            config: NodeConfig::Branch(BranchConfig {
                predicates: vec![],
                default_outcome: OutcomeKey::try_from("done").unwrap(),
            }),
        },
    );
    nodes.insert(
        b2.clone(),
        Node {
            id: b2.clone(),
            position: Position::default(),
            declared_outcomes: vec![],
            config: NodeConfig::Branch(BranchConfig {
                predicates: vec![],
                default_outcome: OutcomeKey::try_from("done").unwrap(),
            }),
        },
    );
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

    let edges = vec![
        Edge {
            id: EdgeKey::try_from("e1").unwrap(),
            from: PortRef {
                node: b1.clone(),
                outcome: OutcomeKey::try_from("done").unwrap(),
            },
            to: b2.clone(),
            kind: EdgeKind::Forward,
            policy: EdgePolicy::default(),
        },
        Edge {
            id: EdgeKey::try_from("e2").unwrap(),
            from: PortRef {
                node: b2.clone(),
                outcome: OutcomeKey::try_from("done").unwrap(),
            },
            to: end.clone(),
            kind: EdgeKind::Forward,
            policy: EdgePolicy::default(),
        },
    ];

    let graph = Graph {
        schema_version: SCHEMA_VERSION,
        metadata: GraphMetadata {
            name: "branch_seq".into(),
            description: None,
            template_origin: None,
            created_at: chrono::Utc::now(),
            author: None,
        },
        start: b1,
        nodes,
        edges,
        subgraphs: BTreeMap::new(),
    };

    let run_id = RunId::new();
    let handle = engine
        .start_run(
            run_id,
            graph,
            dir.path().to_path_buf(),
            EngineRunConfig::default(),
        )
        .await
        .unwrap();
    let outcome = handle.await_completion().await.unwrap();
    assert!(matches!(outcome, RunOutcome::Completed { .. }));

    let snapshot_count = count_snapshots(&storage, run_id).await;
    assert_eq!(
        snapshot_count, 2,
        "3-node graph (2 transitions) → 2 snapshots"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resume_after_completion_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::open(dir.path()).await.unwrap();
    let bridge = Arc::new(fixtures::mock_bridge::MockBridge::new()) as Arc<dyn BridgeFacade>;
    let dispatcher =
        Arc::new(WorktreeToolDispatcher::new(dir.path().to_path_buf())) as Arc<dyn ToolDispatcher>;

    let engine = Engine::new(bridge, storage.clone(), dispatcher, EngineConfig::default());

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
    let graph = Graph {
        schema_version: SCHEMA_VERSION,
        metadata: GraphMetadata {
            name: "rs".into(),
            description: None,
            template_origin: None,
            created_at: chrono::Utc::now(),
            author: None,
        },
        start: end,
        nodes,
        edges: vec![],
        subgraphs: BTreeMap::new(),
    };

    let run_id = RunId::new();
    let h = engine
        .start_run(run_id, graph, dir.path().to_path_buf(), EngineRunConfig::default())
        .await
        .unwrap();
    let _ = h.await_completion().await.unwrap();

    // Resume should detect a terminal-state run and exit cleanly.
    let r = engine
        .resume_run(run_id, dir.path().to_path_buf())
        .await
        .unwrap();
    let outcome = r.await_completion().await.unwrap();
    match outcome {
        RunOutcome::Completed { .. } => {},
        other => panic!("expected Completed on resume, got {other:?}"),
    }
}

/// Count how many graph snapshots were written for a completed run.
async fn count_snapshots(storage: &Arc<Storage>, run_id: RunId) -> usize {
    let reader = storage
        .open_run_reader(run_id)
        .await
        .expect("open_run_reader");
    let seqs = reader.list_snapshots().await.expect("list_snapshots");
    seqs.len()
}
