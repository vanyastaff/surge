//! Registry projection ordering, not a claim about live engine suspension timing.
use super::*;
use surge_core::{EventPayload, VersionedEventPayload, execution_recovery::ExecutionControlState};
use surge_persistence::work_items::{ReconciliationSettlement, recovery_cycles::RecoveryCycle};

#[derive(Debug, PartialEq)]
struct ProjectionSnapshot {
    attempt: serde_json::Value,
    control: serde_json::Value,
    item: serde_json::Value,
    wakes: Vec<RecoveryCycle>,
    journal: Vec<(u64, serde_json::Value)>,
}

async fn snapshot(storage: &Arc<Storage>, attempt: &WorkItemAttempt) -> ProjectionSnapshot {
    let store = storage.work_items();
    ProjectionSnapshot {
        attempt: serde_json::to_value(store.for_run(attempt.run).unwrap().unwrap()).unwrap(),
        control: serde_json::to_value(store.execution_control(attempt.run).unwrap()).unwrap(),
        item: serde_json::to_value(store.show(attempt.item).unwrap()).unwrap(),
        wakes: store
            .due_recovery_wakes_for_run(attempt.run, i64::MAX, 100)
            .unwrap(),
        journal: journal(storage, attempt.run).await,
    }
}

fn request_suspend(store: &WorkItemStore, attempt: &WorkItemAttempt) {
    let item = store.show(attempt.item).unwrap().item;
    let command = WorkItemCommand::Suspend {
        operation_id: WorkItemOperationId::new(),
        item: item.id,
        expected_version: item.version,
    };
    let WorkItemResult::Control(control) = store.mutate(&command, None, None, "human", 3).unwrap()
    else {
        panic!("manual Suspend must return its durable control")
    };
    assert_eq!(control.state, ExecutionControlState::SuspendRequested);
}

#[derive(Clone, Copy)]
enum Case {
    ReservedThenSuspendRequested,
    AuthenticatedCompletedThenSuspendRequested,
    LaunchedThenConfirmedSuspension,
}

async fn check_projection_order(case: Case) {
    let home = FixtureHome::new().unwrap();
    let project = tempfile::tempdir().unwrap();
    let (storage, attempt, workspace) = reserve(home.path(), project.path()).await;
    write_startup(&storage, &attempt, &workspace).await;
    let store = storage.work_items();
    if !matches!(case, Case::ReservedThenSuspendRequested) {
        store
            .settle(
                attempt.run,
                attempt.binding.generation,
                WorkItemAttemptState::Launched,
                None,
            )
            .unwrap();
    }
    let claim = store.claim(attempt.run).unwrap();
    let authority = ReconciliationAuthority {
        attempt: store.validate_claim(&claim).unwrap(),
        control: store.execution_control(attempt.run).unwrap(),
    };
    assert!(reconciliation_eligible(&authority));
    assert!(authority.control.is_none());

    let projection = match case {
        Case::ReservedThenSuspendRequested => {
            assert_eq!(authority.attempt.state, WorkItemAttemptState::Reserved);
            request_suspend(&store, &attempt);
            LaunchProjection::Launched
        },
        Case::AuthenticatedCompletedThenSuspendRequested => {
            let writer = storage.open_run_writer(attempt.run).await.unwrap();
            writer
                .append_event(VersionedEventPayload::new(EventPayload::RunCompleted {
                    terminal_node: "end".parse().unwrap(),
                }))
                .await
                .unwrap();
            writer.close().await.unwrap();
            let history = storage
                .inspect_folded_run(attempt.run)
                .await
                .unwrap()
                .database
                .unwrap();
            let authenticated = terminal(&history, &attempt.binding).unwrap().unwrap();
            assert_eq!(authenticated, WorkItemAttemptState::Completed);
            // The actual terminal evidence was inspected before this independent
            // manual intent. An older projection cannot retire the new control.
            request_suspend(&store, &attempt);
            terminal_projection(authenticated).unwrap()
        },
        Case::LaunchedThenConfirmedSuspension => {
            suspend_claimed(&storage, &attempt, &claim).await;
            assert_eq!(
                store.for_run(attempt.run).unwrap().unwrap().state,
                WorkItemAttemptState::Suspended
            );
            assert_eq!(
                store.execution_control(attempt.run).unwrap().unwrap().state,
                ExecutionControlState::Suspended
            );
            LaunchProjection::Launched
        },
    };
    let before = snapshot(&storage, &attempt).await;
    let result = record_launch_projection(
        &store,
        &claim,
        &authority.attempt,
        LaunchPurpose::Reconciliation(&authority),
        projection,
    )
    .unwrap();
    let after = snapshot(&storage, &attempt).await;
    drop(claim);
    drop(store);
    drop(storage);
    home.close().unwrap();
    assert!(
        after == before,
        "older positive projection changed newer authority: attempt {:?} -> {:?}, control {:?} -> {:?}, item version {:?} -> {:?}, wake count {} -> {}",
        before.attempt["state"],
        after.attempt["state"],
        before.control["state"],
        after.control["state"],
        before.item["item"]["version"],
        after.item["item"]["version"],
        before.wakes.len(),
        after.wakes.len(),
    );
    assert_eq!(result, ReconciliationSettlement::Superseded);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn launched_projection_preserves_suspend_requested_after_reserved_observation() {
    check_projection_order(Case::ReservedThenSuspendRequested).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn completed_projection_preserves_suspend_requested_after_terminal_inspection() {
    check_projection_order(Case::AuthenticatedCompletedThenSuspendRequested).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn launched_projection_preserves_confirmed_suspension_after_launched_observation() {
    check_projection_order(Case::LaunchedThenConfirmedSuspension).await;
}
