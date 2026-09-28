//! Unit test: a successful linear run writes one snapshot per stage boundary.

mod fixtures;

use std::collections::BTreeMap;
use std::sync::Arc;
use surge_acp::bridge::facade::BridgeFacade;
use surge_core::branch_config::{BranchArm, BranchConfig, CompareOp, Predicate};
use surge_core::edge::{Edge, EdgeKind, EdgePolicy, PortRef};
use surge_core::graph::{Graph, GraphMetadata, SCHEMA_VERSION};
use surge_core::id::RunId;
use surge_core::keys::{EdgeKey, NodeKey, OutcomeKey};
use surge_core::node::{Node, NodeConfig, OutcomeDecl, Position};
use surge_core::terminal_config::{TerminalConfig, TerminalKind};
use surge_orchestrator::engine::tools::ToolDispatcher;
use surge_orchestrator::engine::tools::worktree::WorktreeToolDispatcher;
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
            archetype: None,
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
    assert_eq!(
        snapshot_count, 0,
        "single-stage run should have 0 snapshots"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn three_node_branch_run_writes_two_snapshots() {
    let dir = tempfile::tempdir().unwrap();
    let worktree = dir.path().join("project");
    std::fs::create_dir(&worktree).unwrap();
    let init = std::process::Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(&worktree)
        .output()
        .unwrap();
    assert!(init.status.success());
    std::fs::write(worktree.join("app.txt"), "boundary version").unwrap();
    let storage = Storage::open(dir.path()).await.unwrap();
    let bridge = Arc::new(fixtures::mock_bridge::MockBridge::new()) as Arc<dyn BridgeFacade>;
    let dispatcher =
        Arc::new(WorktreeToolDispatcher::new(worktree.clone())) as Arc<dyn ToolDispatcher>;

    let engine = Engine::new(bridge, storage.clone(), dispatcher, EngineConfig::default());

    let b1 = NodeKey::try_from("b1").unwrap();
    let b2 = NodeKey::try_from("b2").unwrap();
    let end = NodeKey::try_from("end").unwrap();

    // A single predicate arm routing to the same outcome as
    // `default_outcome` — `surge_core::validate`'s `BranchWithoutArms` rule
    // requires at least one arm, and matching or not changes nothing
    // observable here since both paths lead to "done".
    let done_arm = || BranchArm {
        condition: Predicate::EnvVar {
            name: "SURGE_ENGINE_SNAPSHOT_TEST_UNSET".into(),
            op: CompareOp::Eq,
            value: String::new(),
        },
        outcome: OutcomeKey::try_from("done").unwrap(),
    };
    let done_outcome_decl = || OutcomeDecl {
        id: OutcomeKey::try_from("done").unwrap(),
        description: "branch routed to done".into(),
        edge_kind_hint: EdgeKind::Forward,
        is_terminal: false,
        ledger_effect: Default::default(),
    };

    let mut nodes = BTreeMap::new();
    nodes.insert(
        b1.clone(),
        Node {
            id: b1.clone(),
            position: Position::default(),
            declared_outcomes: vec![done_outcome_decl()],
            config: NodeConfig::Branch(BranchConfig {
                predicates: vec![done_arm()],
                default_outcome: OutcomeKey::try_from("done").unwrap(),
            }),
        },
    );
    nodes.insert(
        b2.clone(),
        Node {
            id: b2.clone(),
            position: Position::default(),
            declared_outcomes: vec![done_outcome_decl()],
            config: NodeConfig::Branch(BranchConfig {
                predicates: vec![done_arm()],
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
            archetype: None,
        },
        start: b1,
        nodes,
        edges,
        subgraphs: BTreeMap::new(),
    };

    let run_id = RunId::new();
    let handle = engine
        .start_run(run_id, graph, worktree.clone(), EngineRunConfig::default())
        .await
        .unwrap();
    let outcome = handle.await_completion().await.unwrap();
    assert!(matches!(outcome, RunOutcome::Completed { .. }));

    let snapshot_count = count_snapshots(&storage, run_id).await;
    assert_eq!(
        snapshot_count, 2,
        "3-node graph (2 transitions) → 2 snapshots"
    );
    let reader = storage.open_run_reader(run_id).await.unwrap();
    let (boundary_seq, blob) = reader
        .latest_snapshot_at_or_before(reader.current_seq().await.unwrap())
        .await
        .unwrap()
        .unwrap();
    let snapshot =
        surge_orchestrator::engine::snapshot::EngineSnapshot::deserialize(&blob).unwrap();
    assert_eq!(snapshot.root_traversal_counts.get("e1"), Some(&1));
    assert_eq!(snapshot.root_traversal_counts.get("e2"), Some(&1));
    let replayed = surge_orchestrator::engine::replay::replay(&reader)
        .await
        .unwrap();
    assert_eq!(
        replayed
            .root_traversal_counts
            .get(&EdgeKey::try_from("e1").unwrap()),
        Some(&1)
    );
    let checkpoint = snapshot
        .workspace_checkpoint
        .expect("boundary records workspace checkpoint");
    std::fs::write(worktree.join("app.txt"), "later version").unwrap();
    let historical = std::process::Command::new("git")
        .arg("--git-dir")
        .arg(&checkpoint.git_common_dir)
        .args(["show", &format!("{}:app.txt", checkpoint.commit)])
        .output()
        .unwrap();
    assert!(historical.status.success());
    assert_eq!(historical.stdout, b"boundary version");
    let child = RunId::new();
    let destination = dir.path().canonicalize().unwrap().join("fork");
    surge_orchestrator::engine::fork::fork(
        &storage,
        surge_orchestrator::engine::fork::ForkRequest::new(run_id, child, boundary_seq.as_u64())
            .with_worktree(destination.clone()),
    )
    .await
    .unwrap();
    assert_eq!(
        std::fs::read(destination.join("app.txt")).unwrap(),
        b"boundary version"
    );
    let child_reader = storage.open_run_reader(child).await.unwrap();
    let events = child_reader
        .read_events(surge_persistence::runs::EventSeq(1)..surge_persistence::runs::EventSeq(2))
        .await
        .unwrap();
    assert!(matches!(&events[0].payload.payload,
        surge_core::run_event::EventPayload::RunStarted { project_path, .. }
            if project_path == &destination));
    let resumed = engine.resume_run(child, destination.clone()).await.unwrap();
    assert!(matches!(
        resumed.await_completion().await.unwrap(),
        RunOutcome::Completed { .. }
    ));
    std::fs::write(destination.join("app.txt"), "child edit").unwrap();
    assert_eq!(
        std::fs::read(worktree.join("app.txt")).unwrap(),
        b"later version"
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
            archetype: None,
        },
        start: end,
        nodes,
        edges: vec![],
        subgraphs: BTreeMap::new(),
    };

    let run_id = RunId::new();
    let h = engine
        .start_run(
            run_id,
            graph,
            dir.path().to_path_buf(),
            EngineRunConfig::default(),
        )
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
