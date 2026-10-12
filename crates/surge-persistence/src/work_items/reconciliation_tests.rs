//! Storage CAS oracles. Runtime suspension evidence is exercised by the daemon's journal test.
use super::recovery_cycles::{
    AccountEvidence, QuotaObservation, RecoveryCandidate, RecoveryWake, WakeOrigin,
};
use super::*;
use crate::runtime_home_fixture::FixtureHome;
use surge_core::{
    execution_recovery::{
        ExecutionControlState, PendingStagePhase, SuspensionFence, SuspensionReason,
    },
    id::WorkItemOperationId,
};

const INVOCATION: &str = "invocation-01ARZ3NDEKTSV4RRFFQ69G5FAV";
const PROJECTIONS: [ReconciliationProjection; 4] = [
    ReconciliationProjection::Launched,
    ReconciliationProjection::Completed,
    ReconciliationProjection::Failed,
    ReconciliationProjection::Aborted,
];

async fn fixture() -> (FixtureHome, WorkItemStore, WorkItemLaunchClaim) {
    let home = FixtureHome::new().unwrap();
    let storage = crate::runs::Storage::open(home.path()).await.unwrap();
    let store = storage.work_items();
    let workspace = WorkItemWorkspace {
        repository: home.path().join("repo/.git"),
        checkout: home.path().join("repo"),
        path: home.path().join("retained"),
        ownership: RunId::new().to_string(),
        branch: "retained".into(),
        base_commit: "a".repeat(40),
    };
    let create = WorkItemCommand::Create {
        operation_id: WorkItemOperationId::new(),
        project: workspace.checkout.clone(),
        title: "Reconciliation CAS".into(),
        requirements: WorkItemRequirements::new(
            "Original".into(),
            vec!["Preserve authority".into()],
        )
        .unwrap(),
    };
    let WorkItemResult::Detail(detail) = store
        .mutate(&create, Some(&workspace), None, "human", 1)
        .unwrap()
    else {
        panic!("detail")
    };
    let command = WorkItemCommand::Start {
        operation_id: WorkItemOperationId::new(),
        item: detail.item.id,
        expected_version: detail.item.version,
        graph: Box::new(
            toml::from_str(include_str!("../../../../examples/flow_terminal_only.toml")).unwrap(),
        ),
        quota_recovery: None,
    };
    let WorkItemResult::Attempt(attempt) =
        store.mutate(&command, None, Some("{}"), "host", 2).unwrap()
    else {
        panic!("attempt")
    };
    let claim = store.claim(attempt.run).unwrap();
    arm_wake(&store, &claim);
    drop(storage);
    (home, store, claim)
}

fn arm_wake(store: &WorkItemStore, claim: &WorkItemLaunchClaim) {
    let cycle = store.begin_recovery_cycle(claim, INVOCATION, 0).unwrap();
    let candidate = RecoveryCandidate::new("claude".into(), AccountEvidence::Unknown).unwrap();
    let reservation = store
        .reserve_recovery_candidate(claim, &cycle, &candidate)
        .unwrap();
    let cycle = store.recovery_cycle(claim.run(), INVOCATION, 1).unwrap();
    let cycle = store
        .record_recovery_observation(
            claim,
            &cycle,
            &reservation.receipt,
            &QuotaObservation::unsupported(),
        )
        .unwrap();
    let wake =
        RecoveryWake::new("retained-wake".into(), WakeOrigin::PolicyBackoff, 100, 1100).unwrap();
    let cycle = store
        .arm_recovery_wake(claim, &cycle, &reservation.receipt, &wake)
        .unwrap();
    assert_eq!(cycle.wake.as_ref().unwrap().identity(), "retained-wake");
}

fn request_suspend(
    store: &WorkItemStore,
    claim: &WorkItemLaunchClaim,
) -> surge_core::execution_recovery::WorkItemExecutionControl {
    let detail = store.show(claim.binding().item).unwrap();
    let command = WorkItemCommand::Suspend {
        operation_id: WorkItemOperationId::new(),
        item: detail.item.id,
        expected_version: detail.item.version,
    };
    let WorkItemResult::Control(control) = store.mutate(&command, None, None, "human", 3).unwrap()
    else {
        panic!("control")
    };
    *control
}

fn snapshot(store: &WorkItemStore) -> Vec<Vec<Vec<rusqlite::types::Value>>> {
    let conn = store.pool.get().unwrap();
    [
        "work_items",
        "work_item_attempts",
        "work_item_execution_controls",
        "work_item_quota_cycles",
        "work_item_quota_candidates",
        "work_item_operations",
    ]
    .map(|table| {
        let mut query = conn
            .prepare(&format!("SELECT * FROM {table} ORDER BY 1,2"))
            .unwrap();
        let columns = query.column_count();
        query
            .query_map([], |row| (0..columns).map(|index| row.get(index)).collect())
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    })
    .into()
}

fn assert_superseded(
    store: &WorkItemStore,
    claim: &WorkItemLaunchClaim,
    observed: &WorkItemAttempt,
    control: Option<&surge_core::execution_recovery::WorkItemExecutionControl>,
) {
    let before = snapshot(store);
    assert_eq!(
        store
            .settle_reconciliation_error(claim, observed, control, "stale refusal")
            .unwrap(),
        ReconciliationSettlement::Superseded
    );
    assert_eq!(
        snapshot(store),
        before,
        "superseded refusal wrote authority or wake records"
    );
    for projection in PROJECTIONS {
        assert_eq!(
            store
                .settle_reconciliation_projection(claim, observed, control, projection)
                .unwrap(),
            ReconciliationSettlement::Superseded
        );
        assert_eq!(
            snapshot(store),
            before,
            "superseded {projection:?} wrote authority or wake records"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reconciliation_none_to_new_control_preserves_all_authority_and_wake() {
    let (home, store, claim) = fixture().await;
    let observed = store.for_run(claim.run()).unwrap().unwrap();
    assert!(store.execution_control(claim.run()).unwrap().is_none());
    let control = request_suspend(&store, &claim);
    assert_eq!(control.state, ExecutionControlState::SuspendRequested);
    assert_superseded(&store, &claim, &observed, None);
    drop(claim);
    drop(store);
    home.close().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reconciliation_same_generation_control_payload_changes_are_fenced() {
    let (home, store, claim) = fixture().await;
    let original = request_suspend(&store, &claim);
    let observed = store.for_run(claim.run()).unwrap().unwrap();
    let mut state = original.clone();
    state.state = ExecutionControlState::Executing;
    let mut fence = original.clone();
    fence.fence = Some(SuspensionFence {
        control_generation: original.generation,
        snapshot_seq: 23,
        pending_stage: PendingStagePhase::BetweenStages,
        cleanup_confirmed: true,
        reason: SuspensionReason::Manual,
    });
    let mut diagnostic = original.clone();
    diagnostic.diagnostic = Some("new owner diagnostic".into());
    let mut session = original.clone();
    session.allow_new_session = true;
    for changed in [state, fence, diagnostic, session] {
        assert_eq!(changed.operation, original.operation);
        assert_eq!(changed.generation, original.generation);
        // Direct storage-unit input isolates full-control CAS from the attempt-state guard.
        // This is deliberately not evidence that a runtime suspension was authorized.
        control::write_control(&store.pool.get().unwrap(), &changed).unwrap();
        assert_eq!(
            serde_json::to_value(store.for_run(claim.run()).unwrap().unwrap()).unwrap(),
            serde_json::to_value(&observed).unwrap()
        );
        assert_superseded(&store, &claim, &observed, Some(&original));
    }
    drop(claim);
    drop(store);
    home.close().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reconciliation_current_failure_sets_exact_diagnostic_without_changing_wake() {
    for with_control in [false, true] {
        let (home, store, claim) = fixture().await;
        let control = with_control.then(|| request_suspend(&store, &claim));
        let observed = store.for_run(claim.run()).unwrap().unwrap();
        let before = snapshot(&store);
        assert_eq!(
            store
                .settle_reconciliation_error(
                    &claim,
                    &observed,
                    control.as_ref(),
                    "exact current failure"
                )
                .unwrap(),
            ReconciliationSettlement::Settled
        );
        let current = store.for_run(claim.run()).unwrap().unwrap();
        assert_eq!(current.state, WorkItemAttemptState::Attention);
        assert_eq!(current.diagnostic.as_deref(), Some("exact current failure"));
        let mut expected = observed;
        expected.state = WorkItemAttemptState::Attention;
        expected.diagnostic = Some("exact current failure".into());
        assert_eq!(
            serde_json::to_value(current).unwrap(),
            serde_json::to_value(expected).unwrap()
        );
        let expected_control = control.map(|mut control| {
            control.state = ExecutionControlState::Attention;
            control.diagnostic = Some("exact current failure".into());
            control
        });
        assert_eq!(
            store.execution_control(claim.run()).unwrap(),
            expected_control
        );
        let after = snapshot(&store);
        for index in [0, 3, 4, 5] {
            assert_eq!(after[index], before[index]);
        }
        drop(claim);
        drop(store);
        home.close().unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reconciliation_rejects_stale_token_foreign_claim_and_changed_binding() {
    let (home, store, claim) = fixture().await;
    let (foreign_home, foreign_store, foreign) = fixture().await;
    let observed = store.for_run(claim.run()).unwrap().unwrap();
    let mut stale = claim.clone();
    stale.token = "obsolete owner token".into();
    let mut changed = claim.clone();
    changed.binding.generation += 1;
    for invalid in [&stale, &changed, &foreign] {
        let before = snapshot(&store);
        assert!(
            store
                .settle_reconciliation_error(invalid, &observed, None, "invalid owner")
                .is_err()
        );
        assert_eq!(snapshot(&store), before);
        assert_invalid_projection(&store, invalid, &observed);
    }
    drop((stale, changed, claim, foreign));
    drop((store, foreign_store));
    home.close().unwrap();
    foreign_home.close().unwrap();
}

fn assert_invalid_projection(
    store: &WorkItemStore,
    claim: &WorkItemLaunchClaim,
    observed: &WorkItemAttempt,
) {
    let before = snapshot(store);
    for projection in PROJECTIONS {
        assert!(
            store
                .settle_reconciliation_projection(claim, observed, None, projection)
                .is_err()
        );
        assert_eq!(
            snapshot(store),
            before,
            "invalid claim changed authority through {projection:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reconciliation_current_launched_projection_preserves_assignment_and_control() {
    for with_control in [false, true] {
        let (home, store, claim) = fixture().await;
        let control = with_control.then(|| request_suspend(&store, &claim));
        store
            .settle(
                claim.run(),
                claim.binding().generation,
                WorkItemAttemptState::Reserved,
                Some("previous diagnostic".into()),
            )
            .unwrap();
        let observed = store.for_run(claim.run()).unwrap().unwrap();
        let before = snapshot(&store);
        assert_eq!(
            store
                .settle_reconciliation_projection(
                    &claim,
                    &observed,
                    control.as_ref(),
                    ReconciliationProjection::Launched
                )
                .unwrap(),
            ReconciliationSettlement::Settled
        );
        let mut expected = observed;
        expected.state = WorkItemAttemptState::Launched;
        expected.diagnostic = None;
        assert_eq!(
            serde_json::to_value(store.for_run(claim.run()).unwrap().unwrap()).unwrap(),
            serde_json::to_value(expected).unwrap()
        );
        assert_eq!(store.execution_control(claim.run()).unwrap(), control);
        let after = snapshot(&store);
        for index in [0, 2, 3, 4, 5] {
            assert_eq!(after[index], before[index]);
        }
        drop(claim);
        drop(store);
        home.close().unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reconciliation_current_terminal_projections_release_exact_assignment_once() {
    for (projection, state) in [
        (
            ReconciliationProjection::Completed,
            WorkItemAttemptState::Completed,
        ),
        (
            ReconciliationProjection::Failed,
            WorkItemAttemptState::Failed,
        ),
        (
            ReconciliationProjection::Aborted,
            WorkItemAttemptState::Aborted,
        ),
    ] {
        assert_current_terminal(projection, state, false).await;
        assert_current_terminal(projection, state, true).await;
    }
}

async fn assert_current_terminal(
    projection: ReconciliationProjection,
    state: WorkItemAttemptState,
    with_control: bool,
) {
    let (home, store, claim) = fixture().await;
    let control = with_control.then(|| request_suspend(&store, &claim));
    if let Some(control) = &control {
        store
            .record_control_diagnostic(control, "retained control history")
            .unwrap();
    }
    let control = store.execution_control(claim.run()).unwrap();
    let observed = store.for_run(claim.run()).unwrap().unwrap();
    let mut expected_item = store.show(claim.binding().item).unwrap().item;
    let before = snapshot(&store);
    assert_eq!(
        store
            .settle_reconciliation_projection(&claim, &observed, control.as_ref(), projection)
            .unwrap(),
        ReconciliationSettlement::Settled
    );
    let mut expected = observed.clone();
    expected.state = state;
    expected.diagnostic = None;
    assert_eq!(
        serde_json::to_value(store.for_run(claim.run()).unwrap().unwrap()).unwrap(),
        serde_json::to_value(expected).unwrap()
    );
    expected_item.active_run = None;
    expected_item.version += 1;
    assert_eq!(
        serde_json::to_value(store.show(claim.binding().item).unwrap().item).unwrap(),
        serde_json::to_value(expected_item).unwrap()
    );
    let expected_control = control.clone().map(|mut value| {
        value.state = ExecutionControlState::Terminal;
        value
    });
    assert_eq!(
        store.execution_control(claim.run()).unwrap(),
        expected_control
    );
    let after = snapshot(&store);
    for index in [3, 4, 5] {
        assert_eq!(after[index], before[index]);
    }
    assert_superseded(&store, &claim, &observed, control.as_ref());
    drop(claim);
    drop(store);
    home.close().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reconciliation_terminal_under_same_claim_is_superseded() {
    let (home, store, claim) = fixture().await;
    let observed = store.for_run(claim.run()).unwrap().unwrap();
    store
        .settle(
            claim.run(),
            claim.binding().generation,
            WorkItemAttemptState::Completed,
            None,
        )
        .unwrap();
    assert!(
        store
            .show(claim.binding().item)
            .unwrap()
            .item
            .active_run
            .is_none()
    );
    assert_superseded(&store, &claim, &observed, None);
    drop(claim);
    drop(store);
    home.close().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reconciliation_changed_eligible_attempt_is_superseded() {
    let (home, store, claim) = fixture().await;
    let observed = store.for_run(claim.run()).unwrap().unwrap();
    store
        .settle(
            claim.run(),
            claim.binding().generation,
            WorkItemAttemptState::Launched,
            Some("new dispatch owner".into()),
        )
        .unwrap();
    assert_superseded(&store, &claim, &observed, None);
    drop(claim);
    drop(store);
    home.close().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reconciliation_current_continue_settles_failure_and_projection() {
    for (attempt_state, terminal) in [
        (WorkItemAttemptState::Suspended, false),
        (WorkItemAttemptState::Suspended, true),
        (WorkItemAttemptState::Attention, false),
        (WorkItemAttemptState::Attention, true),
    ] {
        let (home, store, claim) = fixture().await;
        let mut pending = request_suspend(&store, &claim);
        pending.state = ExecutionControlState::ContinueReserved;
        // Storage CAS input only; daemon integration supplies the actual
        // suspension journal and accepted human Continue authorization.
        control::write_control(&store.pool.get().unwrap(), &pending).unwrap();
        let mut observed = store.for_run(claim.run()).unwrap().unwrap();
        observed.state = attempt_state;
        store
            .pool
            .get()
            .unwrap()
            .execute(
                "UPDATE work_item_attempts SET state=?,payload=? WHERE run=?",
                rusqlite::params![
                    serde_json::to_value(attempt_state)
                        .unwrap()
                        .as_str()
                        .unwrap(),
                    serde_json::to_string(&observed).unwrap(),
                    claim.run().to_string()
                ],
            )
            .unwrap();
        let stale = pending.clone();
        pending = request_suspend(&store, &claim);
        pending.state = ExecutionControlState::ContinueReserved;
        control::write_control(&store.pool.get().unwrap(), &pending).unwrap();
        assert_superseded(&store, &claim, &observed, Some(&stale));
        let settled = if terminal {
            store.settle_reconciliation_projection(
                &claim,
                &observed,
                Some(&pending),
                ReconciliationProjection::Completed,
            )
        } else {
            store.settle_reconciliation_error(
                &claim,
                &observed,
                Some(&pending),
                "continue recovery refused",
            )
        }
        .unwrap();
        assert_eq!(settled, ReconciliationSettlement::Settled);
        let current = store.for_run(claim.run()).unwrap().unwrap();
        assert_eq!(
            current.state,
            if terminal {
                WorkItemAttemptState::Completed
            } else {
                WorkItemAttemptState::Attention
            }
        );
        let control = store.execution_control(claim.run()).unwrap().unwrap();
        assert_eq!(
            control.state,
            if terminal {
                ExecutionControlState::Terminal
            } else {
                ExecutionControlState::Attention
            }
        );
        assert_eq!(
            control.diagnostic.as_deref(),
            if terminal {
                None
            } else {
                Some("continue recovery refused")
            }
        );
        drop(claim);
        drop(store);
        home.close().unwrap();
    }
}
