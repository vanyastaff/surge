//! Fixed actual split-reader oracle, authored before its next consumer edit.
use super::*;
use surge_core::execution_recovery::semantic_position::{
    ContinuationSeedId, SeedLineage, SemanticPosition, SourceEventRef,
};
mod runtime_home {
    #[cfg(windows)]
    use surge_persistence::RuntimeHomeOwner;
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../scripts/test-support/runtime_home.rs"
    ));
}
#[tokio::test]
async fn actual_split_reader_rejects_source_900_and_accepts_only_local_4_after_entry_3() {
    let home = runtime_home::FixtureHome::new().unwrap();
    let store = surge_persistence::artifacts::ArtifactStore::new(home.path().join("runs"));
    let run = RunId::new();
    let source = RunId::new();
    let seed = ContinuationSeedId::new(surge_core::ContentHash::compute(b"split-lineage"));
    let node = surge_core::NodeKey::try_from("split").unwrap();
    let bytes = b"schema_version = 1\n[[tasks]]\nid = 'child-b'\ntitle = 'First retained child'\nacceptance_criteria = ['First behavior']\n[[tasks]]\nid = 'child-a'\ntitle = 'Second retained child'\nacceptance_criteria = ['Second behavior']\n";
    let mut artifact = store.put(run, "discovered-tasks", bytes).await.unwrap();
    artifact.produced_by = node.clone();
    artifact.produced_at_seq = Some(SemanticPosition::inherited(
        seed,
        SourceEventRef::new(source, 900).unwrap(),
    ));
    let mut memory = RunMemory {
        semantic_lineage: Some(SeedLineage::new(
            seed,
            SourceEventRef::new(source, 1000).unwrap(),
        )),
        ..RunMemory::default()
    };
    memory
        .artifacts_by_node
        .insert(node.clone(), vec![artifact.clone()]);
    assert!(
        read_split_tasks(
            &store,
            run,
            &memory,
            &node,
            surge_persistence::runs::EventSeq(3)
        )
        .await
        .is_err(),
        "large source sequence cannot satisfy a successor-local stage entry"
    );
    artifact.produced_at_seq = Some(SemanticPosition::local(4).unwrap());
    memory
        .artifacts_by_node
        .insert(node.clone(), vec![artifact]);
    let tasks = read_split_tasks(
        &store,
        run,
        &memory,
        &node,
        surge_persistence::runs::EventSeq(3),
    )
    .await
    .unwrap();
    assert_eq!(
        tasks
            .tasks
            .iter()
            .map(|task| task.id.as_str())
            .collect::<Vec<_>>(),
        ["child-b", "child-a"]
    );
    assert_eq!(
        store
            .open(run, surge_core::ContentHash::compute(bytes))
            .await
            .unwrap(),
        bytes
    );
    drop(store);
    home.close().unwrap();
}

use crate::engine::stage::predicate_mock_bridge;

fn predicate_caller_graph() -> Graph {
    use surge_core::branch_config::{BranchArm, BranchConfig, Predicate};
    use surge_core::edge::{Edge, EdgeKind, EdgePolicy, PortRef};
    let mut graph: Graph =
        toml::from_str(include_str!("../../../../examples/flow_minimal_agent.toml")).unwrap();
    let branch = surge_core::NodeKey::try_from("decide").unwrap();
    let trap = surge_core::NodeKey::try_from("impl_1").unwrap();
    if let NodeConfig::Agent(config) = &mut graph.nodes.get_mut(&trap).unwrap().config {
        config.bindings.clear();
    }
    graph.start = branch.clone();
    graph.nodes.insert(
        branch.clone(),
        surge_core::node::Node {
            id: branch.clone(),
            position: surge_core::node::Position::default(),
            declared_outcomes: ["selected", "fallback"]
                .into_iter()
                .map(|id| surge_core::node::OutcomeDecl {
                    id: OutcomeKey::try_from(id).unwrap(),
                    description: id.into(),
                    edge_kind_hint: EdgeKind::Forward,
                    is_terminal: false,
                    ledger_effect: surge_core::LedgerEffect::default(),
                })
                .collect(),
            config: NodeConfig::Branch(BranchConfig {
                predicates: vec![BranchArm {
                    condition: Predicate::OutcomeMatches {
                        node: surge_core::NodeKey::try_from("producer").unwrap(),
                        outcome: OutcomeKey::try_from("passed").unwrap(),
                    },
                    outcome: OutcomeKey::try_from("selected").unwrap(),
                }],
                default_outcome: OutcomeKey::try_from("fallback").unwrap(),
            }),
        },
    );
    for outcome in ["selected", "fallback"] {
        graph.edges.push(Edge {
            id: surge_core::keys::EdgeKey::try_from(format!("branch_{outcome}")).unwrap(),
            from: PortRef {
                node: branch.clone(),
                outcome: OutcomeKey::try_from(outcome).unwrap(),
            },
            to: trap.clone(),
            kind: EdgeKind::Forward,
            policy: EdgePolicy::default(),
        });
    }
    graph
}

fn predicate_git_project(path: &std::path::Path) {
    std::fs::create_dir(path).unwrap();
    std::fs::write(path.join("README.md"), "fixed predicate fixture\n").unwrap();
    for args in [
        vec!["init"],
        vec!["config", "user.name", "Predicate fixture"],
        vec!["config", "user.email", "fixture@example.invalid"],
        vec!["config", "core.hooksPath", "."],
        vec!["config", "commit.gpgSign", "false"],
        vec!["config", "maintenance.auto", "false"],
        vec!["config", "gc.auto", "0"],
        vec!["add", "README.md"],
        vec!["commit", "-m", "fixture base"],
    ] {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(path)
            .output()
            .unwrap();
        assert!(output.status.success(), "fixture git failed: {output:?}");
    }
}

fn predicate_caller_prefix(project: &std::path::Path) -> Vec<VersionedEventPayload> {
    let graph = predicate_caller_graph();
    let config = surge_core::run_event::RunConfig {
        bootstrap_edit_loop_cap: None,
        sandbox_default: surge_core::sandbox::SandboxMode::WorkspaceWrite,
        approval_default: surge_core::approvals::ApprovalPolicy::OnRequest,
        auto_pr: false,
        mcp_servers: Vec::new(),
        budget: surge_core::budget::BudgetGuard::default(),
    };
    let mut prefix = vec![
        VersionedEventPayload::new(EventPayload::RunStarted {
            pipeline_template: None,
            project_path: project.to_path_buf(),
            initial_prompt: "predicate fixture".into(),
            config,
        }),
        VersionedEventPayload::new(EventPayload::PipelineMaterialized {
            graph_hash: graph.identity().unwrap(),
            graph: Box::new(graph),
        }),
    ];
    for _ in 3..=6 {
        prefix.push(VersionedEventPayload::new(
            EventPayload::RequirementRevised {
                node: surge_core::NodeKey::try_from("producer").unwrap(),
                task: None,
                text: "fixed prior history".into(),
            },
        ));
    }
    prefix.push(VersionedEventPayload::new(EventPayload::OutcomeReported {
        node: surge_core::NodeKey::try_from("producer").unwrap(),
        outcome: OutcomeKey::try_from("passed").unwrap(),
        summary: "actual local event seven".into(),
    }));
    prefix
}

async fn predicate_caller_execution(
    storage: &Arc<surge_persistence::runs::Storage>,
    writer: &RunWriter,
    project: &std::path::Path,
    run_id: RunId,
    case: &crate::engine::predicates::provenance_test_support::Case,
) -> (
    crate::engine::execution_state::ExecutionState,
    surge_persistence::runs::RunReader,
) {
    writer
        .append_events(predicate_caller_prefix(project))
        .await
        .unwrap();
    let reader = storage.open_run_reader(run_id).await.unwrap();
    let mut execution = crate::engine::execution_state::load(
        &reader,
        crate::engine::execution_state::Position::Latest,
    )
    .await
    .unwrap();
    assert_eq!(execution.position, 7);
    let memory =
        crate::engine::predicates::provenance_test_support::memory(case, &project.join("evidence"));
    // Private structural injection, never an ordinary seeded-start acceptance claim.
    execution.memory.outcomes = memory.outcomes;
    execution.memory.artifacts = memory.artifacts;
    execution.memory.semantic_lineage = memory.semantic_lineage;
    (execution, reader)
}

async fn predicate_caller_params(
    home: &std::path::Path,
    case: &crate::engine::predicates::provenance_test_support::Case,
) -> (RunTaskParams, Arc<predicate_mock_bridge::MockBridge>) {
    use crate::engine::capacity::{PersistentCapacityLedger, RunHistoryWorkEstimator};
    let project = home.join("project");
    predicate_git_project(&project);
    let storage = surge_persistence::runs::Storage::open(home).await.unwrap();
    let run_id = RunId::new();
    let writer = storage.create_run(run_id, &project, None).await.unwrap();
    let (execution, reader) =
        predicate_caller_execution(&storage, &writer, &project, run_id, case).await;
    let bridge = Arc::new(predicate_mock_bridge::MockBridge::new());
    bridge
        .fail_next_send_message(
            surge_acp::bridge::error::SendMessageError::SessionNotFound {
                session: surge_core::SessionId::new(),
            },
        )
        .await;
    let capacity = crate::engine::quota_recovery::RunCapacity::new(
        Arc::new(PersistentCapacityLedger::new(storage.clone())),
        Arc::new(RunHistoryWorkEstimator::new(reader)),
        crate::engine::EngineConfig::default().capacity,
        Vec::new(),
        None,
        None,
    );
    let params = RunTaskParams {
        work_item_claim: None,
        pending_suspension: None,
        run_id,
        writer,
        artifact_store: surge_persistence::artifacts::ArtifactStore::new(home.join("runs")),
        bridge: bridge.clone(),
        tool_dispatcher: Arc::new(crate::engine::tools::worktree::WorktreeToolDispatcher::new(
            project.clone(),
        )),
        notify_deliverer: Arc::new(surge_notify::MultiplexingNotifier::new()),
        execution,
        resumed: false,
        worktree_path: project,
        run_config: EngineRunConfig {
            memory_store_path: Some(home.join("memory.db")),
            stage_timeout_override: Some(std::time::Duration::from_secs(2)),
            ..EngineRunConfig::default()
        },
        event_tx: broadcast::channel(64).0,
        cancel: CancellationToken::new(),
        gate_resolutions: Arc::default(),
        tool_resolutions: Arc::default(),
        roadmap_amendments: mpsc::channel(1).1,
        pending_steers: crate::engine::steer::new_queue(),
        pending_elevations: crate::engine::elevation::PendingElevations::new(),
        mcp_registry: None,
        mcp_servers: Vec::new(),
        profile_registry: None,
        agent_registry: None,
        capacity,
        escalation: surge_core::escalation::EscalationConfig::default(),
        storage,
    };
    (params, bridge)
}

async fn predicate_caller_events(
    params: &RunTaskParams,
) -> Vec<surge_persistence::runs::ReadEvent> {
    let last = params.writer.current_seq().await.unwrap();
    params
        .writer
        .read_events(surge_persistence::runs::EventSeq::ZERO..last.next())
        .await
        .unwrap()
}

async fn predicate_checkpoint_cursors(params: &RunTaskParams) -> Vec<String> {
    let reader = params.storage.open_run_reader(params.run_id).await.unwrap();
    let mut cursors = Vec::new();
    for seq in reader.list_snapshots().await.unwrap() {
        let (stored_seq, blob) = reader
            .latest_snapshot_at_or_before(seq)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored_seq, seq);
        let snapshot = crate::engine::snapshot::EngineSnapshot::deserialize(&blob).unwrap();
        cursors.push(snapshot.cursor.node);
    }
    cursors
}

fn predicate_failure_diagnostics(entered: bool) -> Vec<EventPayload> {
    let node = surge_core::NodeKey::try_from("decide").unwrap();
    let reason =
        "stage error at decide: semantic history belongs to another seed or source boundary";
    let mut events = Vec::new();
    if entered {
        events.push(EventPayload::StageEntered {
            node: node.clone(),
            attempt: 1,
        });
    }
    events.push(EventPayload::StageFailed {
        node,
        reason: reason.into(),
        retry_available: false,
    });
    events.push(EventPayload::RunFailed {
        error: reason.into(),
    });
    events
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn predicate_branch_actual_dispatch_and_resolution_refuse_without_state_or_route_effects() {
    use surge_core::execution_recovery::semantic_position::PositionError;
    let home = runtime_home::FixtureHome::new().unwrap();
    let facts = {
        let case = crate::engine::predicates::provenance_test_support::cases(false)
            .into_iter()
            .find(|case| case.label == "foreign run")
            .unwrap();
        let (params, bridge) = predicate_caller_params(home.path(), &case).await;
        let mut state = initial_execution_state(&params);
        let before_memory = state.memory.clone();
        let before_cursor = state.cursor.clone();
        let before_frames = state.frames.clone();
        let node = state.routing_graph.nodes[&state.cursor.node].clone();
        let hooks = run_on_error_hooks_owned(
            &state.hook_executor,
            &node,
            &state.cursor.node,
            "provenance refusal",
            &params.worktree_path,
            None,
            Some(&params.writer),
        )
        .await;
        let dispatched = dispatch_node_stage(&params, &mut state, &node).await;
        let StageDispatch::StageResult(result) = dispatched else {
            panic!("Branch must use the real StageResult resolver")
        };
        let typed = matches!(
            &result,
            Err(StageError::SemanticPosition(PositionError::ForeignLineage))
        );
        let result_debug = match &result {
            Ok(StageOutcome::Routed(outcome)) => format!("Routed({outcome})"),
            Ok(StageOutcome::Terminal(_)) => "Terminal".into(),
            Err(error) => format!("Err({error:?})"),
        };
        let resolution = resolve_stage_result(&params, &mut state, &node, result).await;
        let failed = matches!(&resolution, Err(RunOutcome::Failed { error }) if error == "stage error at decide: semantic history belongs to another seed or source boundary");
        let events = predicate_caller_events(&params).await;
        let suffix: Vec<_> = events
            .iter()
            .skip(7)
            .map(|event| event.payload.payload.clone())
            .collect();
        let checkpoint_cursors = predicate_checkpoint_cursors(&params).await;
        let clean = state.memory == before_memory
            && state.cursor == before_cursor
            && state.frames == before_frames
            && state.root_traversal_counts.is_empty();
        let no_hooks = hooks.outcome.is_none() && hooks.records.is_empty();
        let no_calls = bridge.recorded_calls.lock().await.is_empty();
        params.writer.close().await.unwrap();
        (
            typed
                && failed
                && clean
                && no_hooks
                && no_calls
                && checkpoint_cursors.iter().all(|cursor| cursor == "decide")
                && suffix == predicate_failure_diagnostics(false),
            result_debug,
            suffix,
        )
    };
    home.close().unwrap();
    assert!(
        facts.0,
        "actual dispatch/resolution leaked a decision or changed state: result={}, suffix={:#?}",
        facts.1, facts.2
    );
}

async fn predicate_run_loop_control(label: &str, refusal: bool) {
    let home = runtime_home::FixtureHome::new().unwrap();
    let facts = {
        let case = crate::engine::predicates::provenance_test_support::cases(false)
            .into_iter()
            .find(|case| case.label == label)
            .unwrap();
        let (mut params, bridge) = predicate_caller_params(home.path(), &case).await;
        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            Box::pin(execute_inner(&mut params)),
        )
        .await
        .unwrap();
        let events = predicate_caller_events(&params).await;
        let suffix: Vec<_> = events
            .iter()
            .skip(7)
            .map(|event| event.payload.payload.clone())
            .collect();
        let checkpoint_cursors = predicate_checkpoint_cursors(&params).await;
        let calls = bridge.recorded_calls.lock().await.clone();
        let correct = if refusal {
            matches!(&outcome, RunOutcome::Failed { error } if error == "stage error at decide: semantic history belongs to another seed or source boundary")
                && suffix == predicate_failure_diagnostics(true)
                && checkpoint_cursors.iter().all(|cursor| cursor == "decide")
                && calls.is_empty()
        } else {
            calls.iter().any(|call| matches!(call, predicate_mock_bridge::RecordedCall::OpenSession)) && calls.iter().any(|call| matches!(call, predicate_mock_bridge::RecordedCall::SendMessage { .. })) && checkpoint_cursors.iter().any(|cursor| cursor == "impl_1") && suffix.iter().any(|event| matches!(event, EventPayload::OutcomeReported { node, outcome, .. } if node.as_str() == "decide" && outcome.as_str() == "selected")) && suffix.iter().any(|event| matches!(event, EventPayload::EdgeTraversed { from, to, .. } if from.as_str() == "decide" && to.as_str() == "impl_1"))
        };
        params.writer.close().await.unwrap();
        (
            correct,
            format!("{outcome:?}"),
            suffix,
            format!("{calls:?}"),
        )
    };
    home.close().unwrap();
    assert!(
        facts.0,
        "run-loop {label}: outcome={}, suffix={:#?}, bridge={}",
        facts.1, facts.2, facts.3
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn predicate_branch_actual_run_loop_refusal_never_dispatches_downstream() {
    predicate_run_loop_control("foreign run", true).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn predicate_branch_actual_run_loop_local_control_reaches_mock_trap() {
    predicate_run_loop_control("local7", false).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn predicate_branch_actual_run_loop_inherited_control_reaches_mock_trap() {
    predicate_run_loop_control("source87", false).await;
}
