//! Accepted engine work retains real completion supervision when projection fails.
use super::*;
use std::time::Duration;
use surge_core::{EventPayload, execution_recovery::ExecutionControlState};
use surge_persistence::work_items::ReconciliationSettlement;

#[derive(Clone, Copy)]
enum ProjectionFailure {
    Superseded,
    StorageError,
}

async fn accepted_terminal_run_is_supervised(failure: ProjectionFailure) {
    let home = FixtureHome::new().unwrap();
    let project = tempfile::tempdir().unwrap();
    let graph = toml::from_str(include_str!(
        "../../../../../examples/flow_terminal_only.toml"
    ))
    .unwrap();
    let (storage, attempt, workspace) =
        reserve_graph(home.path(), project.path(), graph, None).await;
    let store = storage.work_items();
    let claim = store.claim(attempt.run).unwrap();
    surge_git::task_workspace::prepare(
        &workspace,
        surge_git::run_worktree::ReconcilePhase::BeforeExecution,
    )
    .unwrap();
    store.mark_workspace_prepared(&claim).unwrap();
    let authority = ReconciliationAuthority {
        attempt: store.validate_claim(&claim).unwrap(),
        control: store.execution_control(attempt.run).unwrap(),
    };
    assert!(reconciliation_eligible(&authority));
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
    assert!(admission.try_admit_no_queue(attempt.run).await);
    let publisher = broadcasts.register(attempt.run).await;
    let control_events = engine.subscribe_tap();
    let mut terminal_events = engine.subscribe_tap();
    let run = tracking.task_run(&claim, false).await.unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let event = terminal_events.recv().await.unwrap();
            if event.run_id == attempt.run
                && matches!(
                    event.event.payload.payload,
                    EventPayload::RunCompleted { .. }
                )
            {
                break;
            }
        }
    })
    .await
    .expect("actual terminal-only engine must persist RunCompleted");
    assert_eq!(admission.snapshot().await.active, 1);

    match failure {
        ProjectionFailure::Superseded => {
            let detail = store.show(attempt.item).unwrap();
            let WorkItemResult::Control(control) = store
                .mutate(
                    &WorkItemCommand::Suspend {
                        operation_id: WorkItemOperationId::new(),
                        item: attempt.item,
                        expected_version: detail.item.version,
                    },
                    None,
                    None,
                    "human",
                    3,
                )
                .unwrap()
            else {
                panic!("manual Suspend must reserve its control")
            };
            assert_eq!(control.state, ExecutionControlState::SuspendRequested);
        },
        ProjectionFailure::StorageError => {
            storage
                .acquire_registry_conn()
                .unwrap()
                .execute_batch(
                    "CREATE TRIGGER reject_launched_projection \
                     BEFORE UPDATE ON work_item_attempts WHEN NEW.state='launched' \
                     BEGIN SELECT RAISE(FAIL, 'fixture rejects Launched projection'); END;",
                )
                .unwrap();
        },
    }
    let projected = record_launch_projection(
        &store,
        &claim,
        &authority.attempt,
        LaunchPurpose::Reconciliation(&authority),
        LaunchProjection::Launched,
    );
    match failure {
        ProjectionFailure::Superseded => {
            assert_eq!(projected.unwrap(), ReconciliationSettlement::Superseded);
        },
        ProjectionFailure::StorageError => {
            assert!(
                matches!(
                    projected,
                    Err(WorkItemError::Sql(rusqlite::Error::SqliteFailure(_, Some(ref message))))
                        if message == "fixture rejects Launched projection"
                ),
                "the real SQLite trigger must reject the production projection"
            );
        },
    }
    // Handoff retries the same rejected projection, then transfers this actual
    // accepted run and its original retained claim to the real supervisor.
    let supervisor = handoff_accepted_dispatch(
        &authority.attempt,
        AcceptedDispatch {
            run,
            claim,
            engine: engine.clone(),
            storage: storage.clone(),
            publisher,
            control_events,
        },
        LaunchPurpose::Reconciliation(&authority),
        &admission,
        &broadcasts,
    );
    tokio::time::timeout(Duration::from_secs(10), supervisor)
        .await
        .expect("accepted run supervision must settle")
        .unwrap();
    let history = storage
        .inspect_folded_run(attempt.run)
        .await
        .unwrap()
        .database
        .unwrap();
    let journal_terminal = terminal(&history, &attempt.binding).unwrap();
    let persisted = store.for_run(attempt.run).unwrap().unwrap();
    let item = store.show(attempt.item).unwrap().item;
    let admission_active = admission.snapshot().await.active;
    let active_runs = engine.snapshot_active_runs().await;
    let provider_calls_empty = bridge.recorded_calls.lock().await.is_empty();
    let control = store.execution_control(attempt.run).unwrap();
    drop(tracking);
    drop(engine);
    drop(store);
    drop(storage);
    home.close().unwrap();
    assert_eq!(journal_terminal, Some(WorkItemAttemptState::Completed));
    assert_eq!(persisted.state, WorkItemAttemptState::Completed);
    assert_eq!(item.active_run, None);
    assert_eq!(admission_active, 0);
    assert!(active_runs.is_empty());
    assert!(provider_calls_empty);
    if matches!(failure, ProjectionFailure::Superseded) {
        assert_eq!(control.unwrap().state, ExecutionControlState::Terminal);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn accepted_dispatch_keeps_supervisor_after_superseded_launched_projection() {
    accepted_terminal_run_is_supervised(ProjectionFailure::Superseded).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn accepted_dispatch_keeps_supervisor_after_launched_projection_storage_error() {
    accepted_terminal_run_is_supervised(ProjectionFailure::StorageError).await;
}
