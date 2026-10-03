use super::*;
use crate::runs::{MockClock, registry::open_registry_pool};
use surge_core::bootstrap_operation::{BootstrapCaptureFields, BootstrapContentRef};
use surge_core::budget::BudgetGuard;

fn intent(root: &std::path::Path, text: &str) -> BootstrapIntent {
    BootstrapIntent::new(root.to_path_buf(), text.into(), BudgetGuard::default()).unwrap()
}

fn capture(root: &std::path::Path) -> BootstrapCapture {
    BootstrapCaptureFields {
        version: 1,
        planning_run: RunId::new(),
        implementation_run: RunId::new(),
        repository: root.to_path_buf(),
        git_common_dir: root.to_path_buf(),
        base_commit: "a".repeat(40),
        planning_worktree: root.join("planning"),
        implementation_worktree: root.join("implementation"),
        planning_branch: "surge/planning".into(),
        implementation_branch: "surge/implementation".into(),
        graph: BootstrapContentRef {
            name: "bootstrap@1".into(),
            digest: ContentHash::compute(b"graph"),
        },
        project_context: None,
        bootstrap_edit_loop_cap: None,
        runtime_identity: ContentHash::compute(b"runtime"),
        configuration: vec![],
        credential_refs: vec![],
    }
    .try_into()
    .unwrap()
}

fn open(root: &std::path::Path) -> BootstrapOperationStore {
    BootstrapOperationStore::new(open_registry_pool(root, &MockClock::new(1)).unwrap())
}

#[test]
fn accepted_identity_payload_and_queue_survive_reopen() {
    let temp = tempfile::tempdir().unwrap();
    let store = open(temp.path());
    let first = RunId::new();
    let original = intent(temp.path(), "  exact prompt\n");
    let pins = capture(temp.path());
    let record = store.insert_if_absent(first, &original, &pins).unwrap();
    let second = store
        .insert_if_absent(RunId::new(), &original, &capture(temp.path()))
        .unwrap();
    drop(store);
    let store = open(temp.path());
    let loaded = store.lookup(first, &original).unwrap().unwrap();
    assert_eq!(loaded, record);
    assert_eq!(
        store.pending_admission_runs().unwrap(),
        vec![record.status.planning_run, second.status.planning_run]
    );
    assert_eq!(store.queued().unwrap(), vec![record.clone(), second]);
    // Changed capture is irrelevant to a duplicate accepted intent.
    assert_eq!(
        store
            .insert_if_absent(first, &original, &capture(temp.path()))
            .unwrap(),
        record
    );
    assert!(matches!(
        store.lookup(first, &intent(temp.path(), "different")),
        Err(BootstrapStoreError::IntentConflict)
    ));
    assert_eq!(store.get(first).unwrap().unwrap(), record);
}

#[test]
fn cancellation_is_monotonic_and_claim_is_compare_and_swap() {
    let temp = tempfile::tempdir().unwrap();
    let store = open(temp.path());
    let id = RunId::new();
    let accepted = store
        .insert_if_absent(id, &intent(temp.path(), "x"), &capture(temp.path()))
        .unwrap();
    let claimed = store.claim(id, accepted.status.revision).unwrap();
    assert_eq!(
        claimed.status.state,
        BootstrapState::Pending {
            phase: BootstrapPhase::PreparingPlanning
        }
    );
    assert!(matches!(
        store.claim(id, accepted.status.revision),
        Err(BootstrapStoreError::StaleRevision)
    ));
    assert_eq!(
        store.pending_admission_runs().unwrap(),
        vec![claimed.status.planning_run]
    );
    let cancelled = store.request_cancel(id).unwrap();
    assert!(store.pending_admission_runs().unwrap().is_empty());
    assert!(cancelled.status.cancel_requested);
    assert_eq!(
        cancelled.status.state,
        BootstrapState::Cancelling {
            phase: BootstrapPhase::PreparingPlanning
        }
    );
    assert_eq!(store.request_cancel(id).unwrap(), cancelled);
    assert!(store.claim(id, cancelled.status.revision).is_err());
    drop(store);
    assert_eq!(open(temp.path()).get(id).unwrap().unwrap(), cancelled);
}

#[test]
fn attention_requires_explicit_retry_with_original_pins() {
    let temp = tempfile::tempdir().unwrap();
    let store = open(temp.path());
    let id = RunId::new();
    let pins = capture(temp.path());
    let accepted = store
        .insert_if_absent(id, &intent(temp.path(), "x"), &pins)
        .unwrap();
    let blocked = store
        .block(
            id,
            accepted.status.revision,
            BootstrapAttentionReason::ConfigurationChanged,
        )
        .unwrap();
    assert!(store.queued().unwrap().is_empty());
    assert!(store.pending_admission_runs().unwrap().is_empty());
    assert!(store.claim(id, blocked.status.revision).is_err());
    assert!(matches!(
        store.retry(id, blocked.status.revision, &capture(temp.path())),
        Err(BootstrapStoreError::PinnedInputsChanged)
    ));
    let retried = store.retry(id, blocked.status.revision, &pins).unwrap();
    assert_eq!(retried.status.state, accepted.status.state);
    assert!(retried.status.revision > blocked.status.revision);
    let cancelling = store.request_cancel(id).unwrap();
    assert!(store.retry(id, cancelling.status.revision, &pins).is_err());
}

#[test]
fn unknown_versions_are_inspectable_but_not_claimable() {
    let temp = tempfile::tempdir().unwrap();
    let store = open(temp.path());
    let id = RunId::new();
    store
        .insert_if_absent(id, &intent(temp.path(), "x"), &capture(temp.path()))
        .unwrap();
    store
        .pool
        .get()
        .unwrap()
        .execute(
            "UPDATE bootstrap_operations SET payload_version=99, capture_json='future data'",
            [],
        )
        .unwrap();
    assert!(store.queued().unwrap().is_empty());
    assert!(store.pending_admission_runs().unwrap().is_empty());
    let record = store.get(id).unwrap().unwrap();
    assert!(matches!(
        record.payload,
        BootstrapStoredPayload::Unsupported { version: 99 }
    ));
    assert!(matches!(
        store.claim(id, record.status.revision),
        Err(BootstrapStoreError::UnsupportedPayload)
    ));
}

#[test]
fn tampered_fingerprint_is_not_accepted_as_existing_intent() {
    let temp = tempfile::tempdir().unwrap();
    let store = open(temp.path());
    let id = RunId::new();
    store
        .insert_if_absent(id, &intent(temp.path(), "x"), &capture(temp.path()))
        .unwrap();
    store
        .pool
        .get()
        .unwrap()
        .execute(
            "UPDATE bootstrap_operations SET intent_fingerprint=?",
            [ContentHash::compute(b"tamper").to_string()],
        )
        .unwrap();
    assert!(matches!(
        store.get(id),
        Err(BootstrapStoreError::CorruptRecord)
    ));
}

#[test]
fn failed_cancel_transaction_never_reports_acknowledgement() {
    let temp = tempfile::tempdir().unwrap();
    let store = open(temp.path());
    let id = RunId::new();
    let before = store
        .insert_if_absent(id, &intent(temp.path(), "x"), &capture(temp.path()))
        .unwrap();
    // Deferred foreign-key failure happens at COMMIT, after the state UPDATE
    // and the in-transaction reread have both succeeded.
    store.pool.get().unwrap().execute_batch(
        "CREATE TABLE cancel_parent (id TEXT PRIMARY KEY);
         CREATE TABLE cancel_child (id TEXT REFERENCES cancel_parent(id) DEFERRABLE INITIALLY DEFERRED);
         CREATE TRIGGER reject_cancel AFTER UPDATE ON bootstrap_operations
         BEGIN INSERT INTO cancel_child VALUES ('missing-parent'); END;"
    ).unwrap();
    assert!(matches!(
        store.request_cancel(id),
        Err(BootstrapStoreError::Sqlite(_))
    ));
    assert_eq!(store.get(id).unwrap().unwrap(), before);
}

#[test]
fn cancelling_terminal_record_does_not_relabel_or_increment_it() {
    let temp = tempfile::tempdir().unwrap();
    let store = open(temp.path());
    let id = RunId::new();
    store
        .insert_if_absent(id, &intent(temp.path(), "x"), &capture(temp.path()))
        .unwrap();
    for state in [
        BootstrapState::Completed,
        BootstrapState::Failed,
        BootstrapState::Cancelled,
    ] {
        seed_terminal(&store, id, state);
        let before = store.get(id).unwrap().unwrap();
        assert_eq!(store.request_cancel(id).unwrap(), before);
    }
}

#[test]
fn competing_insert_and_claim_cannot_duplicate_identity() {
    let temp = tempfile::tempdir().unwrap();
    let store = open(temp.path());
    let id = RunId::new();
    let original = intent(temp.path(), "x");
    let pins = capture(temp.path());
    let results = std::thread::scope(|scope| {
        let a = scope.spawn(|| store.insert_if_absent(id, &original, &pins).unwrap());
        let b = scope.spawn(|| store.insert_if_absent(id, &original, &pins).unwrap());
        [a.join().unwrap(), b.join().unwrap()]
    });
    assert_eq!(results[0], results[1]);
    let revision = results[0].status.revision;
    let results = std::thread::scope(|scope| {
        let a = scope.spawn(|| store.claim(id, revision));
        let b = scope.spawn(|| store.request_cancel(id));
        [a.join().unwrap(), b.join().unwrap()]
    });
    assert!(results[1].is_ok());
    let final_record = store.get(id).unwrap().unwrap();
    assert!(final_record.status.cancel_requested);
    assert!(matches!(
        final_record.status.state,
        BootstrapState::Cancelling { .. }
    ));
}

#[test]
fn corrupt_supported_payload_stops_queue_enumeration() {
    let temp = tempfile::tempdir().unwrap();
    let store = open(temp.path());
    store
        .insert_if_absent(
            RunId::new(),
            &intent(temp.path(), "x"),
            &capture(temp.path()),
        )
        .unwrap();
    store
        .pool
        .get()
        .unwrap()
        .execute("UPDATE bootstrap_operations SET capture_json='{}'", [])
        .unwrap();
    assert!(matches!(
        store.queued(),
        Err(BootstrapStoreError::Serialization(_))
    ));
}

#[test]
fn incoherent_state_is_rejected_before_scheduling() {
    let temp = tempfile::tempdir().unwrap();
    let store = open(temp.path());
    let id = RunId::new();
    store
        .insert_if_absent(id, &intent(temp.path(), "x"), &capture(temp.path()))
        .unwrap();
    store
        .pool
        .get()
        .unwrap()
        .execute("UPDATE bootstrap_operations SET cancel_requested=1", [])
        .unwrap();
    assert!(matches!(
        store.get(id),
        Err(BootstrapStoreError::CorruptRecord)
    ));
}

#[test]
fn attention_cannot_point_at_an_unrelated_run() {
    let temp = tempfile::tempdir().unwrap();
    let store = open(temp.path());
    let id = RunId::new();
    store
        .insert_if_absent(id, &intent(temp.path(), "x"), &capture(temp.path()))
        .unwrap();
    let state = BootstrapState::NeedsAttention {
        phase: BootstrapPhase::QueuedPlanning,
        run_id: Some(RunId::new()),
        reason: BootstrapAttentionReason::ConfigurationChanged,
    };
    store
        .pool
        .get()
        .unwrap()
        .execute(
            "UPDATE bootstrap_operations SET state_json=?",
            [serde_json::to_string(&state).unwrap()],
        )
        .unwrap();
    assert!(matches!(
        store.queued(),
        Err(BootstrapStoreError::CorruptRecord)
    ));
}

#[test]
fn terminal_intent_conflict_cannot_overwrite_the_original_record() {
    let temp = tempfile::tempdir().unwrap();
    let store = open(temp.path());
    let id = RunId::new();
    let original = intent(temp.path(), "first");
    let pins = capture(temp.path());
    store.insert_if_absent(id, &original, &pins).unwrap();
    seed_terminal(&store, id, BootstrapState::Completed);
    let before = store.get(id).unwrap().unwrap();
    assert!(matches!(
        store.insert_if_absent(id, &intent(temp.path(), "different"), &capture(temp.path())),
        Err(BootstrapStoreError::IntentConflict)
    ));
    assert_eq!(store.get(id).unwrap().unwrap(), before);
}

#[test]
fn cancellation_attention_preserves_stop_intent_and_cannot_retry() {
    let temp = tempfile::tempdir().unwrap();
    let store = open(temp.path());
    let id = RunId::new();
    let pins = capture(temp.path());
    store
        .insert_if_absent(id, &intent(temp.path(), "x"), &pins)
        .unwrap();
    let claimed = store.claim(id, 0).unwrap();
    let cancelled = store.request_cancel(id).unwrap();
    let blocked = store
        .block(
            id,
            cancelled.status.revision,
            BootstrapAttentionReason::StorageUnconfirmed,
        )
        .unwrap();
    assert!(blocked.status.cancel_requested);
    assert_eq!(store.request_cancel(id).unwrap(), blocked);
    assert_eq!(
        blocked.status.state,
        BootstrapState::NeedsAttention {
            phase: BootstrapPhase::PreparingPlanning,
            run_id: Some(claimed.status.planning_run),
            reason: BootstrapAttentionReason::StorageUnconfirmed,
        }
    );
    assert!(store.queued().unwrap().is_empty());
    assert!(store.pending_admission_runs().unwrap().is_empty());
    assert!(matches!(
        store.claim(id, blocked.status.revision),
        Err(BootstrapStoreError::InvalidTransition)
    ));
    assert!(matches!(
        store.retry(id, blocked.status.revision, &pins),
        Err(BootstrapStoreError::InvalidTransition)
    ));
    drop(store);
    assert_eq!(open(temp.path()).get(id).unwrap().unwrap(), blocked);
}

fn child_intent(parent: RunId) -> BootstrapContinuation {
    BootstrapContinuation::new(
        parent,
        BootstrapContentRef {
            name: "materialized:flow".into(),
            digest: ContentHash::compute(b"graph"),
        },
        ["description", "roadmap", "flow"]
            .into_iter()
            .map(|name| surge_core::run_state::ArtifactRef {
                hash: ContentHash::compute(name.as_bytes()),
                path: name.into(),
                name: name.into(),
                produced_by: surge_core::keys::NodeKey::try_new("generator").unwrap(),
                produced_at_seq: 5,
            })
            .collect(),
        BudgetGuard {
            limits: surge_core::budget::BudgetLimits {
                tokens: Some(7),
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .unwrap()
}

#[test]
fn child_launch_commit_survives_reopen_and_cancel_fences_continuation() {
    let temp = tempfile::tempdir().unwrap();
    let store = open(temp.path());
    for cancelled in [false, true] {
        let id = RunId::new();
        let captured = capture(temp.path());
        let record = store
            .insert_if_absent(id, &intent(temp.path(), "app"), &captured)
            .unwrap();
        let planning = BootstrapState::Pending {
            phase: BootstrapPhase::Planning,
        };
        store
            .pool
            .get()
            .unwrap()
            .execute(
                "UPDATE bootstrap_operations SET state_json=? WHERE operation_id=?",
                params![serde_json::to_string(&planning).unwrap(), id.to_string()],
            )
            .unwrap();
        let child = child_intent(record.status.planning_run);
        if cancelled {
            store.request_cancel(id).unwrap();
            assert!(
                store
                    .continue_planning(id, record.status.revision, &child)
                    .is_err()
            );
            assert!(open(temp.path()).get(id).unwrap().unwrap().child.is_none());
        } else {
            let committed = store
                .continue_planning(id, record.status.revision, &child)
                .unwrap();
            assert_eq!(
                committed.status.state,
                BootstrapState::Pending {
                    phase: BootstrapPhase::QueuedImplementation
                }
            );
            assert_eq!(
                open(temp.path()).get(id).unwrap().unwrap().child,
                Some(child.clone())
            );
            assert!(
                store
                    .continue_planning(id, committed.status.revision, &child)
                    .is_err()
            );
        }
    }
}

fn seed_terminal(store: &BootstrapOperationStore, id: RunId, state: BootstrapState) {
    let record = store.get(id).unwrap().unwrap();
    let (result, child) = match state {
        BootstrapState::Completed => (
            BootstrapTerminal::Completed {
                run_id: record.status.implementation_run,
                terminal: surge_core::keys::NodeKey::try_new("end").unwrap(),
            },
            Some(child_intent(record.status.planning_run)),
        ),
        BootstrapState::Failed => (
            BootstrapTerminal::Failed {
                run_id: record.status.planning_run,
                reason: surge_core::bootstrap_continuation::BootstrapFailure::RunFailed,
            },
            None,
        ),
        BootstrapState::Cancelled => (BootstrapTerminal::Cancelled, None),
        _ => panic!("terminal fixture"),
    };
    store.pool.get().unwrap().execute(
        "UPDATE bootstrap_operations SET state_json=?1,cancel_requested=?2,result_json=?3,child_json=?4 WHERE operation_id=?5",
        params![serde_json::to_string(&state).unwrap(), matches!(state, BootstrapState::Cancelled), serde_json::to_string(&result).unwrap(), child.as_ref().map(|child| serde_json::to_string(child).unwrap()), id.to_string()],
    ).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn read_only_capture_inspection_uses_run_identity_and_checks_integrity() {
    let root = tempfile::tempdir().unwrap();
    let store = open(root.path());
    let pins = capture(root.path());
    let operation = RunId::new();
    store
        .insert_if_absent(operation, &intent(root.path(), "create"), &pins)
        .unwrap();
    for run in [pins.fields().planning_run, pins.fields().implementation_run] {
        let found = crate::runs::Storage::inspect_existing_run_capture(root.path().into(), run)
            .await
            .unwrap();
        assert_eq!(found, Some(pins.clone()));
    }
    assert!(
        crate::runs::Storage::inspect_existing_run_capture(root.path().into(), RunId::new())
            .await
            .unwrap()
            .is_none()
    );
    store
        .pool
        .get()
        .unwrap()
        .execute(
            "UPDATE bootstrap_operations SET capture_fingerprint = ?",
            ["0".repeat(64)],
        )
        .unwrap();
    assert!(
        crate::runs::Storage::inspect_existing_run_capture(
            root.path().into(),
            pins.fields().implementation_run
        )
        .await
        .is_err()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn read_only_capture_inspection_preserves_absence_and_rejects_future_payload() {
    let root = tempfile::tempdir().unwrap();
    let missing = root.path().join("absent");
    assert!(
        crate::runs::Storage::inspect_existing_run_capture(missing.clone(), RunId::new())
            .await
            .unwrap()
            .is_none()
    );
    assert!(!missing.exists());
    let store = open(root.path());
    let pins = capture(root.path());
    store
        .insert_if_absent(RunId::new(), &intent(root.path(), "create"), &pins)
        .unwrap();
    store
        .pool
        .get()
        .unwrap()
        .execute("UPDATE bootstrap_operations SET payload_version = 999", [])
        .unwrap();
    assert!(matches!(
        crate::runs::Storage::inspect_existing_run_capture(
            root.path().into(),
            pins.fields().planning_run
        )
        .await,
        Err(crate::runs::StorageError::BootstrapJournal(
            BootstrapStoreError::UnsupportedPayload
        ))
    ));
}
