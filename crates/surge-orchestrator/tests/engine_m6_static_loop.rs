//! M6: 3-iteration static loop completes; event log has 3×LoopIterationStarted +
//! 3×LoopIterationCompleted + 1×LoopCompleted.

mod fixtures;

use std::sync::Arc;
use surge_acp::bridge::facade::BridgeFacade;
use surge_core::edge::{Edge, EdgeKind, EdgePolicy, PortRef};
use surge_core::id::RunId;
use surge_core::keys::{EdgeKey, NodeKey, OutcomeKey};
use surge_core::loop_config::IterableSource;
use surge_core::node::{Node, NodeConfig, OutcomeDecl, Position};
use surge_core::run_event::EventPayload;
use surge_orchestrator::engine::tools::worktree::WorktreeToolDispatcher;
use surge_orchestrator::engine::{Engine, EngineConfig, EngineRunConfig, RunOutcome};
use surge_persistence::runs::Storage;
use surge_persistence::runs::seq::EventSeq;

#[path = "fixtures/static_loop_graph.rs"]
mod static_loop_graph;
use static_loop_graph::build_static_loop_graph;

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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn approved_run_artifact_drives_loop_without_planner() {
    use surge_orchestrator::engine::config::RunSeedArtifact;

    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::open(dir.path()).await.unwrap();
    let mock = Arc::new(fixtures::mock_bridge::MockBridge::new());
    let bridge = mock.clone() as Arc<dyn BridgeFacade>;
    let engine = Engine::new(
        bridge,
        storage.clone(),
        Arc::new(WorktreeToolDispatcher::new(dir.path().into())),
        EngineConfig::default(),
    );
    let mut graph = build_static_loop_graph();
    let NodeConfig::Loop(config) = &mut graph.nodes.get_mut(&graph.start).unwrap().config else {
        panic!("expected loop");
    };
    config.iterates_over = IterableSource::RunArtifact {
        name: "roadmap".into(),
        jsonpath: "milestones".into(),
    };
    let missing = engine
        .start_run(
            RunId::new(),
            graph.clone(),
            dir.path().into(),
            EngineRunConfig::default(),
        )
        .await;
    assert!(matches!(
        missing,
        Err(surge_orchestrator::engine::EngineError::GraphInvalid(_))
    ));
    let seed = RunSeedArtifact::new(
        "roadmap", "roadmap.toml",
        "[[milestones]]\nid = 'timer'\n[[milestones.tasks]]\nid = 'pause'\n[[milestones]]\nid = 'accessibility'\ntasks = []\n",
        "bootstrap_parent",
    ).unwrap();
    let body = graph.subgraphs.values_mut().next().unwrap();
    let agent: Node = toml::from_str(
        r#"
id = "worker"
[[declared_outcomes]]
id = "done"
description = "Iteration done"
edge_kind_hint = "forward"
is_terminal = false
[config]
node_kind = "agent"
profile = "implementer@1.0"
[config.prompt_overrides]
system = "Process the current iteration."
"#,
    )
    .unwrap();
    body.edges.push(Edge {
        id: EdgeKey::try_from("worker_done").unwrap(),
        from: PortRef {
            node: agent.id.clone(),
            outcome: OutcomeKey::try_from("done").unwrap(),
        },
        to: body.start.clone(),
        kind: EdgeKind::Forward,
        policy: EdgePolicy::default(),
    });
    body.start = agent.id.clone();
    body.nodes.insert(agent.id.clone(), agent);
    let sessions = vec![surge_core::SessionId::new(), surge_core::SessionId::new()];
    mock.pin_session_ids(sessions.clone()).await;
    let pumping = mock.clone();
    let pump = tokio::spawn(async move {
        for (index, session) in sessions.into_iter().enumerate() {
            pumping.wait_for_subscribe_count(index + 1).await;
            pumping
                .enqueue_event(surge_acp::bridge::event::BridgeEvent::OutcomeReported {
                    session,
                    outcome: OutcomeKey::try_from("done").unwrap(),
                    summary: "iteration complete".into(),
                    artifacts_produced: vec![],
                })
                .await;
            pumping.pump_scripted_events().await;
        }
    });
    let id = RunId::new();
    let handle = engine
        .start_run(
            id,
            graph,
            dir.path().into(),
            EngineRunConfig {
                seed_artifacts: vec![seed],
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let outcome =
        tokio::time::timeout(std::time::Duration::from_secs(5), handle.await_completion())
            .await
            .unwrap()
            .unwrap();
    assert!(
        matches!(outcome, RunOutcome::Completed { .. }),
        "{outcome:?}"
    );
    pump.await.unwrap();
    let prompt = mock.last_prompt().await.unwrap();
    assert!(prompt.contains("accessibility"));
    assert!(!prompt.contains("pause"));
    let reader = storage.open_run_reader(id).await.unwrap();
    let events = reader
        .read_events(EventSeq::ZERO..EventSeq(i64::MAX as u64))
        .await
        .unwrap();
    let items: Vec<_> = events
        .iter()
        .filter_map(|event| match event.payload.payload() {
            EventPayload::LoopIterationStarted { item, .. } => Some(item.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["id"].as_str(), Some("timer"));
    assert_eq!(items[0]["tasks"][0]["id"].as_str(), Some("pause"));
    assert_eq!(items[1]["id"].as_str(), Some("accessibility"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fork_inside_loop_keeps_iteration_position_and_remaining_items() {
    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::open(dir.path()).await.unwrap();
    let bridge = Arc::new(fixtures::mock_bridge::MockBridge::new()) as Arc<dyn BridgeFacade>;
    let engine = Engine::new(
        bridge,
        storage.clone(),
        Arc::new(WorktreeToolDispatcher::new(dir.path().into())),
        EngineConfig::default(),
    );
    let mut graph = build_static_loop_graph();
    let body = graph.subgraphs.values_mut().next().unwrap();
    let step = NodeKey::try_from("body_step").unwrap();
    let done = OutcomeKey::try_from("done").unwrap();
    body.nodes.insert(
        step.clone(),
        Node {
            id: step.clone(),
            position: Position::default(),
            declared_outcomes: vec![OutcomeDecl {
                id: done.clone(),
                description: "continue".into(),
                edge_kind_hint: EdgeKind::Forward,
                is_terminal: false,
                ledger_effect: Default::default(),
            }],
            config: NodeConfig::Branch(surge_core::branch_config::BranchConfig {
                predicates: vec![surge_core::branch_config::BranchArm {
                    condition: surge_core::branch_config::Predicate::FileExists {
                        path: "unused".into(),
                    },
                    outcome: done.clone(),
                }],
                default_outcome: done.clone(),
            }),
        },
    );
    body.edges.push(Edge {
        id: EdgeKey::try_from("body_next").unwrap(),
        from: PortRef {
            node: step.clone(),
            outcome: done,
        },
        to: body.start.clone(),
        kind: EdgeKind::Forward,
        policy: EdgePolicy::default(),
    });
    body.start = step.clone();
    let parent = RunId::new();
    let handle = engine
        .start_run(parent, graph, dir.path().into(), EngineRunConfig::default())
        .await
        .unwrap();
    assert!(matches!(
        handle.await_completion().await.unwrap(),
        RunOutcome::Completed { .. }
    ));
    let reader = storage.open_run_reader(parent).await.unwrap();
    let events = reader
        .read_events(EventSeq(1)..EventSeq(reader.current_seq().await.unwrap().as_u64() + 1))
        .await
        .unwrap();
    let boundary = events
        .iter()
        .find(|event| {
            matches!(&event.payload.payload,
        EventPayload::StageCompleted { node, .. } if node == &step)
        })
        .unwrap()
        .seq;
    let (_, blob) = reader
        .latest_snapshot_at_or_before(boundary)
        .await
        .unwrap()
        .unwrap();
    let snapshot =
        surge_orchestrator::engine::snapshot::EngineSnapshot::deserialize(&blob).unwrap();
    assert_eq!(
        snapshot.frames.len(),
        1,
        "active loop must survive the boundary"
    );
    let child = RunId::new();
    surge_orchestrator::engine::fork::fork(
        &storage,
        surge_orchestrator::engine::fork::ForkRequest::new(parent, child, boundary.as_u64()),
    )
    .await
    .unwrap();
    let resumed = engine.resume_run(child, dir.path().into()).await.unwrap();
    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        resumed.await_completion(),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(
        matches!(outcome, RunOutcome::Completed { .. }),
        "{outcome:?}"
    );
    let reader = storage.open_run_reader(child).await.unwrap();
    let events = reader
        .read_events(EventSeq(1)..EventSeq(reader.current_seq().await.unwrap().as_u64() + 1))
        .await
        .unwrap();
    let indices: Vec<_> = events
        .iter()
        .filter_map(|event| match &event.payload.payload {
            EventPayload::LoopIterationStarted { index, .. } => Some(*index),
            _ => None,
        })
        .collect();
    assert_eq!(indices, vec![0, 1, 2]);
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(
                event.payload.payload,
                EventPayload::LoopIterationCompleted { .. }
            ))
            .count(),
        3
    );
}
