//! Captured page entries must not override a newer durable control.
use super::*;
mod accepted_dispatch;
#[path = "../../../surge-orchestrator/tests/fixtures/mock_bridge.rs"]
mod mock_bridge;
mod planned_capacity;
mod positive_projection;
use crate::runtime_home_fixture::FixtureHome;
use std::path::Path;
use surge_core::{
    execution_recovery::{PendingStagePhase, SuspensionFence, SuspensionReason},
    id::{StageInvocationId, WorkItemOperationId},
    work_item::{WorkItemRequirements, WorkItemWorkspace},
};
use surge_orchestrator::engine::{Engine, EngineConfig, tools::worktree::WorktreeToolDispatcher};
use surge_persistence::runs::Storage;

async fn reserve(
    home: &Path,
    project: &Path,
) -> (Arc<Storage>, WorkItemAttempt, WorkItemWorkspace) {
    reserve_with_policy(home, project, None).await
}

async fn reserve_with_policy(
    home: &Path,
    project: &Path,
    policy: Option<surge_persistence::work_items::recovery_cycles::FrozenQuotaPolicy>,
) -> (Arc<Storage>, WorkItemAttempt, WorkItemWorkspace) {
    let graph =
        toml::from_str(include_str!("../../../../examples/flow_minimal_agent.toml")).unwrap();
    reserve_graph(home, project, graph, policy).await
}

async fn reserve_graph(
    home: &Path,
    project: &Path,
    graph: surge_core::Graph,
    policy: Option<surge_persistence::work_items::recovery_cycles::FrozenQuotaPolicy>,
) -> (Arc<Storage>, WorkItemAttempt, WorkItemWorkspace) {
    let (storage, attempt, workspace, _) =
        reserve_graph_with_start(home, project, graph, policy).await;
    (storage, attempt, workspace)
}

async fn reserve_graph_with_start(
    home: &Path,
    project: &Path,
    graph: surge_core::Graph,
    policy: Option<surge_persistence::work_items::recovery_cycles::FrozenQuotaPolicy>,
) -> (
    Arc<Storage>,
    WorkItemAttempt,
    WorkItemWorkspace,
    WorkItemCommand,
) {
    let repo = git2::Repository::init(project).unwrap();
    let signature = git2::Signature::now("Fixture", "fixture@example.com").unwrap();
    let tree_id = repo.index().unwrap().write_tree().unwrap();
    let tree = repo.find_tree(tree_id).unwrap();
    repo.commit(Some("HEAD"), &signature, &signature, "base", &tree, &[])
        .unwrap();
    let storage = Storage::open(home).await.unwrap();
    let workspace =
        surge_git::task_workspace::plan(project, &home.join("work-items/workspaces"), RunId::new())
            .unwrap();
    let store = storage.work_items();
    let create = WorkItemCommand::Create {
        operation_id: WorkItemOperationId::new(),
        project: project.into(),
        title: "stale page".into(),
        requirements: WorkItemRequirements::new(
            "Retain suspension".into(),
            vec!["No stale launch".into()],
        )
        .unwrap(),
    };
    let WorkItemResult::Detail(detail) = store
        .mutate(&create, Some(&workspace), None, "human", 1)
        .unwrap()
    else {
        panic!("detail")
    };
    let start = WorkItemCommand::Start {
        operation_id: WorkItemOperationId::new(),
        item: detail.item.id,
        expected_version: detail.item.version,
        graph: Box::new(graph),
        quota_recovery: policy
            .as_ref()
            .map(|policy| serde_json::to_value(policy).unwrap()),
    };
    let config = serde_json::to_string(&EngineRunConfig {
        quota_recovery: policy.unwrap_or_default(),
        ..EngineRunConfig::default()
    })
    .unwrap();
    let WorkItemResult::Attempt(attempt) = store
        .mutate(&start, None, Some(&config), "host", 2)
        .unwrap()
    else {
        panic!("attempt")
    };
    (storage, *attempt, workspace, start)
}

async fn write_startup(
    storage: &Arc<Storage>,
    attempt: &surge_core::work_item::WorkItemAttempt,
    workspace: &surge_core::work_item::WorkItemWorkspace,
) {
    use surge_core::{
        ContentHash, EventPayload as E, VersionedEventPayload as V, approvals::ApprovalPolicy,
        run_event::RunConfig, sandbox::SandboxMode, work_item::WorkItemContext,
    };
    let claim = storage.work_items().claim(attempt.run).unwrap();
    surge_git::task_workspace::prepare(
        workspace,
        surge_git::run_worktree::ReconcilePhase::BeforeExecution,
    )
    .unwrap();
    storage
        .work_items()
        .mark_workspace_prepared(&claim)
        .unwrap();
    let requirements = storage
        .work_items()
        .requirements(attempt)
        .unwrap()
        .origin
        .requirements()
        .unwrap()
        .clone();
    let context = WorkItemContext::new(attempt.binding.clone(), requirements).unwrap();
    let writer = storage
        .create_run(attempt.run, &workspace.path, None)
        .await
        .unwrap();
    let artifact = surge_persistence::artifacts::ArtifactStore::new(storage.home().join("runs"))
        .put(
            attempt.run,
            "accepted_requirements",
            &serde_json::to_vec(context.requirements().unwrap()).unwrap(),
        )
        .await
        .unwrap();
    let mut events = vec![
        E::RunStarted {
            pipeline_template: None,
            project_path: workspace.path.clone(),
            initial_prompt: context.prompt(),
            config: RunConfig {
                bootstrap_edit_loop_cap: Some(
                    serde_json::from_str::<surge_orchestrator::engine::EngineRunConfig>(
                        &attempt.config,
                    )
                    .unwrap()
                    .bootstrap
                    .edit_loop_cap,
                ),
                sandbox_default: SandboxMode::WorkspaceWrite,
                approval_default: ApprovalPolicy::OnRequest,
                auto_pr: false,
                mcp_servers: vec![],
                budget: surge_core::budget::BudgetGuard::default(),
            },
        },
        E::PipelineMaterialized {
            graph: attempt.graph.clone(),
            graph_hash: ContentHash::compute(&serde_json::to_vec(&attempt.graph).unwrap()),
        },
        E::ArtifactProduced {
            node: "task_requirements".parse().unwrap(),
            artifact: artifact.hash,
            path: artifact.path,
            name: "accepted_requirements".into(),
            source_path: None,
        },
    ];
    let prompt = context.prompt();
    std::fs::create_dir_all(workspace.path.join(".surge")).unwrap();
    std::fs::write(workspace.path.join(".surge/user_prompt.txt"), &prompt).unwrap();
    {
        events.push(E::ArtifactProduced {
            node: "initial_prompt_seed".parse().unwrap(),
            artifact: ContentHash::compute(prompt.as_bytes()),
            path: ".surge/user_prompt.txt".into(),
            name: "user_prompt".into(),
            source_path: None,
        });
    }
    {
        events.push(E::WorkItemAttemptBound { context });
    }
    writer
        .append_events(events.into_iter().map(V::new).collect())
        .await
        .unwrap();
    writer.close().await.unwrap();
}
async fn suspend(storage: &Arc<Storage>, attempt: &WorkItemAttempt) {
    let store = storage.work_items();
    let claim = store.claim(attempt.run).unwrap();
    suspend_claimed(storage, attempt, &claim).await;
    drop(claim);
}

async fn suspend_claimed(
    storage: &Arc<Storage>,
    attempt: &WorkItemAttempt,
    claim: &WorkItemLaunchClaim,
) {
    let store = storage.work_items();
    let detail = store.show(attempt.item).unwrap();
    let request = WorkItemCommand::Suspend {
        operation_id: WorkItemOperationId::new(),
        item: attempt.item,
        expected_version: detail.item.version,
    };
    let WorkItemResult::Control(control) = store.mutate(&request, None, None, "human", 3).unwrap()
    else {
        panic!("control")
    };
    let writer = storage.open_run_writer(attempt.run).await.unwrap();
    let prefix = writer.current_seq().await.unwrap().as_u64();
    let pending = PendingStagePhase::Interrupted {
        node: attempt.graph.start.clone(),
        invocation: StageInvocationId::new(),
    };
    let fence = SuspensionFence {
        control_generation: control.generation,
        snapshot_seq: prefix,
        pending_stage: pending.clone(),
        cleanup_confirmed: true,
        reason: SuspensionReason::Manual,
    };
    let snapshot =
        serde_json::to_vec(&serde_json::json!({"at_seq": prefix, "pending_stage": pending}))
            .unwrap();
    writer
        .seal_suspension(fence.clone(), snapshot)
        .await
        .unwrap();
    store.confirm_suspension(claim, &fence).unwrap();
    writer.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stale_reconciliation_page_preserves_confirmed_suspension() {
    let home = FixtureHome::new().unwrap();
    let project = tempfile::tempdir().unwrap();
    let (storage, attempt, workspace) = reserve(home.path(), project.path()).await;
    write_startup(&storage, &attempt, &workspace).await;
    let store = storage.work_items();
    store
        .settle(
            attempt.run,
            attempt.binding.generation,
            WorkItemAttemptState::Launched,
            None,
        )
        .unwrap();
    let captured = store.for_run(attempt.run).unwrap().unwrap();
    suspend(&storage, &attempt).await;
    let before = store.for_run(attempt.run).unwrap().unwrap();
    let control = store.execution_control(attempt.run).unwrap();
    assert_eq!(before.state, WorkItemAttemptState::Suspended);
    let bridge = Arc::new(mock_bridge::MockBridge::new());
    let engine = Arc::new(Engine::new(
        bridge.clone(),
        storage.clone(),
        Arc::new(WorktreeToolDispatcher::new(project.path().into())),
        EngineConfig::default(),
    ));
    let claim = store.claim(attempt.run).unwrap();
    engine.validate_work_item_startup(&claim).await.unwrap();
    drop(claim);
    let journal_before = journal(&storage, attempt.run).await;
    let claim_before = registry_claim_token(&storage, attempt.run);
    let tracking = TrackingContext::new(engine.clone(), storage.clone());
    reconcile_candidate(
        captured,
        &tracking,
        &Arc::new(AdmissionController::new(1, 1)),
        &Arc::new(BroadcastRegistry::new()),
    )
    .await
    .unwrap();
    assert_eq!(
        registry_claim_token(&storage, attempt.run),
        claim_before,
        "confirmed manual suspension must not receive a generic launch claim"
    );
    let after = store.for_run(attempt.run).unwrap().unwrap();
    assert_eq!(
        after.state,
        before.state,
        "stale candidate changed confirmed suspension; attempt diagnostic={:?}; control before={control:?}; control after={:?}",
        after.diagnostic,
        store.execution_control(attempt.run).unwrap()
    );
    assert_eq!(store.execution_control(attempt.run).unwrap(), control);
    assert_eq!(
        serde_json::to_value(&after).unwrap(),
        serde_json::to_value(&before).unwrap()
    );
    assert_eq!(journal(&storage, attempt.run).await, journal_before);
    assert!(
        bridge.recorded_calls.lock().await.is_empty(),
        "stale page cannot contact a provider"
    );
    drop(tracking);
    drop(engine);
    drop(store);
    drop(storage);
    home.close().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reconciliation_does_not_claim_unstarted_reservation_before_explicit_start() {
    for empty_journal in [false, true] {
        let home = FixtureHome::new().unwrap();
        unstarted_reservation_requires_explicit_start(home.path(), empty_journal).await;
        home.close().unwrap();
    }
}

async fn unstarted_reservation_requires_explicit_start(home: &Path, empty_journal: bool) {
    let project = tempfile::tempdir().unwrap();
    let graph =
        toml::from_str(include_str!("../../../../examples/flow_terminal_only.toml")).unwrap();
    let (storage, attempt, workspace, start) =
        reserve_graph_with_start(home, project.path(), graph, None).await;
    if empty_journal {
        storage
            .create_run(attempt.run, &workspace.path, None)
            .await
            .unwrap()
            .close()
            .await
            .unwrap();
    }
    let bridge = Arc::new(mock_bridge::MockBridge::new());
    let engine = Arc::new(Engine::new(
        bridge.clone(),
        storage.clone(),
        Arc::new(WorktreeToolDispatcher::new(project.path().into())),
        EngineConfig::default(),
    ));
    let tracking = TrackingContext::new(engine.clone(), storage.clone());
    let admission = Arc::new(AdmissionController::new(1, 1));
    let broadcasts = Arc::new(BroadcastRegistry::new());
    let token_before = registry_claim_token(&storage, attempt.run);
    reconcile_candidate(attempt.clone(), &tracking, &admission, &broadcasts)
        .await
        .unwrap();
    assert_eq!(
        registry_claim_token(&storage, attempt.run),
        token_before,
        "startup reconciliation must not claim a reservation requiring explicit Start; empty={empty_journal}"
    );
    assert_eq!(
        serde_json::to_value(storage.work_items().for_run(attempt.run).unwrap().unwrap()).unwrap(),
        serde_json::to_value(&attempt).unwrap()
    );
    assert!(bridge.recorded_calls.lock().await.is_empty());
    let result = execute(&start, &tracking, &admission, &broadcasts)
        .await
        .unwrap();
    assert!(matches!(result, WorkItemResult::Attempt(ref actual) if actual.run == attempt.run));
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while storage
            .work_items()
            .for_run(attempt.run)
            .unwrap()
            .unwrap()
            .state
            != WorkItemAttemptState::Completed
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("explicit Start must complete the same terminal-only run");
    drop(tracking);
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while Arc::strong_count(&engine) != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("actual supervisor must release engine before fixture cleanup");
    drop(engine);
}

fn registry_claim_token(storage: &Storage, run: RunId) -> Option<String> {
    storage
        .acquire_registry_conn()
        .unwrap()
        .query_row(
            "SELECT claim_token FROM work_item_attempts WHERE run=?",
            [run.to_string()],
            |row| row.get(0),
        )
        .unwrap()
}

async fn journal(storage: &Arc<Storage>, run: RunId) -> Vec<(u64, serde_json::Value)> {
    let surge_persistence::runs::inspection::RunDatabaseInspection::Present { events } =
        storage.inspect_run(run).await.unwrap().database
    else {
        panic!("journal")
    };
    events
        .into_iter()
        .map(|row| (row.seq.0, serde_json::to_value(row.payload).unwrap()))
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stale_reconciliation_page_preserves_attention_and_reason() {
    let home = FixtureHome::new().unwrap();
    let project = tempfile::tempdir().unwrap();
    let (storage, attempt, workspace) = reserve(home.path(), project.path()).await;
    write_startup(&storage, &attempt, &workspace).await;
    let store = storage.work_items();
    store
        .settle(
            attempt.run,
            attempt.binding.generation,
            WorkItemAttemptState::Launched,
            None,
        )
        .unwrap();
    let captured = store.for_run(attempt.run).unwrap().unwrap();
    store
        .settle(
            attempt.run,
            attempt.binding.generation,
            WorkItemAttemptState::Attention,
            Some("operator decision required".into()),
        )
        .unwrap();
    let before = store.for_run(attempt.run).unwrap().unwrap();
    let prefix = journal(&storage, attempt.run).await;
    let bridge = Arc::new(mock_bridge::MockBridge::new());
    let engine = Arc::new(Engine::new(
        bridge.clone(),
        storage.clone(),
        Arc::new(WorktreeToolDispatcher::new(project.path().into())),
        EngineConfig::default(),
    ));
    let tracking = TrackingContext::new(engine.clone(), storage.clone());
    reconcile_candidate(
        captured,
        &tracking,
        &Arc::new(AdmissionController::new(1, 1)),
        &Arc::new(BroadcastRegistry::new()),
    )
    .await
    .unwrap();
    let after = store.for_run(attempt.run).unwrap().unwrap();
    assert_eq!(
        serde_json::to_value(after).unwrap(),
        serde_json::to_value(before).unwrap()
    );
    assert_eq!(journal(&storage, attempt.run).await, prefix);
    assert!(bridge.recorded_calls.lock().await.is_empty());
    drop(tracking);
    drop(engine);
    drop(store);
    drop(storage);
    home.close().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn current_reconciliation_reports_authentic_startup_refusal() {
    let home = FixtureHome::new().unwrap();
    let project = tempfile::tempdir().unwrap();
    let (storage, attempt, workspace) = reserve(home.path(), project.path()).await;
    write_startup(&storage, &attempt, &workspace).await;
    std::fs::write(workspace.path.join(".surge/user_prompt.txt"), "changed").unwrap();
    let store = storage.work_items();
    let bridge = Arc::new(mock_bridge::MockBridge::new());
    let engine = Arc::new(Engine::new(
        bridge.clone(),
        storage.clone(),
        Arc::new(WorktreeToolDispatcher::new(project.path().into())),
        EngineConfig::default(),
    ));
    let claim = store.claim(attempt.run).unwrap();
    let expected = engine
        .validate_work_item_startup(&claim)
        .await
        .unwrap_err()
        .to_string();
    drop(claim);
    let tracking = TrackingContext::new(engine.clone(), storage.clone());
    reconcile_candidate(
        attempt.clone(),
        &tracking,
        &Arc::new(AdmissionController::new(1, 1)),
        &Arc::new(BroadcastRegistry::new()),
    )
    .await
    .unwrap();
    let after = store.for_run(attempt.run).unwrap().unwrap();
    assert_eq!(after.state, WorkItemAttemptState::Attention);
    assert_eq!(
        after.diagnostic,
        Some(WorkItemError::Conflict(expected).to_string())
    );
    assert!(bridge.recorded_calls.lock().await.is_empty());
    drop(tracking);
    drop(engine);
    drop(store);
    drop(storage);
    home.close().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn suspension_after_eligibility_survives_inner_dispatch_refusal() {
    let home = FixtureHome::new().unwrap();
    let project = tempfile::tempdir().unwrap();
    let (storage, attempt, workspace) = reserve(home.path(), project.path()).await;
    write_startup(&storage, &attempt, &workspace).await;
    let store = storage.work_items();
    store
        .settle(
            attempt.run,
            attempt.binding.generation,
            WorkItemAttemptState::Launched,
            None,
        )
        .unwrap();
    let claim = store.claim(attempt.run).unwrap();
    let authority = ReconciliationAuthority {
        attempt: store.validate_claim(&claim).unwrap(),
        control: store.execution_control(attempt.run).unwrap(),
    };
    assert!(reconciliation_eligible(&authority));
    suspend_claimed(&storage, &attempt, &claim).await;
    let before = store.for_run(attempt.run).unwrap().unwrap();
    let control = store.execution_control(attempt.run).unwrap();
    let prefix = journal(&storage, attempt.run).await;
    let bridge = Arc::new(mock_bridge::MockBridge::new());
    let engine = Arc::new(Engine::new(
        bridge.clone(),
        storage.clone(),
        Arc::new(WorktreeToolDispatcher::new(project.path().into())),
        EngineConfig::default(),
    ));
    engine.validate_work_item_startup(&claim).await.unwrap();
    let tracking = TrackingContext::new(engine.clone(), storage.clone());
    launch_claimed_attempt(
        &authority.attempt,
        claim,
        LaunchPurpose::Reconciliation(&authority),
        &tracking,
        &Arc::new(AdmissionController::new(1, 1)),
        &Arc::new(BroadcastRegistry::new()),
    )
    .await
    .unwrap();
    let after = store.for_run(attempt.run).unwrap().unwrap();
    assert_eq!(
        serde_json::to_value(after).unwrap(),
        serde_json::to_value(before).unwrap()
    );
    assert_eq!(store.execution_control(attempt.run).unwrap(), control);
    assert_eq!(journal(&storage, attempt.run).await, prefix);
    assert!(bridge.recorded_calls.lock().await.is_empty());
    drop(tracking);
    drop(engine);
    drop(store);
    drop(storage);
    home.close().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn suspended_attempt_requires_a_durable_continue_for_reconciliation() {
    use surge_core::execution_recovery::{
        ExecutionControlState as State, WorkItemExecutionControl,
    };
    let home = FixtureHome::new().unwrap();
    let project = tempfile::tempdir().unwrap();
    let (storage, mut attempt, _) = reserve(home.path(), project.path()).await;
    attempt.state = WorkItemAttemptState::Suspended;
    let mut authority = ReconciliationAuthority {
        attempt,
        control: None,
    };
    assert!(!reconciliation_eligible(&authority));
    for state in [
        State::Executing,
        State::SuspendRequested,
        State::Suspended,
        State::Attention,
        State::ContinueReserved,
    ] {
        authority.control = Some(WorkItemExecutionControl {
            item: authority.attempt.item,
            run: authority.attempt.run,
            attempt_generation: authority.attempt.binding.generation,
            generation: 2,
            operation: WorkItemOperationId::new(),
            state,
            fence: None,
            allow_new_session: false,
            diagnostic: None,
        });
        for attempt_state in [
            WorkItemAttemptState::Suspended,
            WorkItemAttemptState::Attention,
        ] {
            authority.attempt.state = attempt_state;
            assert_eq!(
                reconciliation_eligible(&authority),
                state == State::ContinueReserved
            );
        }
    }
    drop(storage);
    home.close().unwrap();
}
