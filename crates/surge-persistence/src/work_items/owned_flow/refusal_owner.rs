//! One-purpose runtime-independent delivery owner; no execution capability is minted here.
use super::refusal_delivery;
use crate::runs::{clock::Clock, writer_slot::ActiveWriters};
use crate::work_items::{Result, WorkItemError, start_preparation::secure_lock::PreparationLock};
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use std::{
    path::PathBuf,
    sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock},
    thread::JoinHandle,
};
use surge_core::{ContentHash, RunId};
use surge_process::owner_panic::{
    abort_on_owner_panic as protected, install_owner_panic_protection,
};

#[derive(Clone)]
pub(super) struct DeliveryResources {
    pub(super) pool: Pool<SqliteConnectionManager>,
    pub(super) home: PathBuf,
    pub(super) clock: Arc<dyn Clock>,
    pub(super) writers: Arc<ActiveWriters>,
}
#[derive(Clone, Copy)]
enum State {
    Preparing,
    Ready,
    Committed { key: ContentHash, hash: ContentHash },
    CommitUnknown { key: ContentHash, hash: ContentHash },
    Aborted,
    Delivered,
}
struct Slot {
    guard: Arc<PreparationLock>,
    resources: DeliveryResources,
    run_hint: RunId,
    state: Mutex<State>,
    changed: Condvar,
    #[cfg(test)]
    ready_gate: Option<Arc<ReadyGate>>,
}
struct Entry {
    slot: Arc<Slot>,
    handle: Option<JoinHandle<()>>,
}
#[derive(Default)]
struct Registry {
    closed: bool,
    entries: Vec<Entry>,
}
#[cfg(test)]
static READY_GATES: OnceLock<Mutex<std::collections::HashMap<RunId, Arc<ReadyGate>>>> =
    OnceLock::new();
#[cfg(test)]
pub(super) fn ready_gate(run: RunId) -> Arc<ReadyGate> {
    let gate = Arc::new(ReadyGate::default());
    lock(READY_GATES.get_or_init(Mutex::default)).insert(run, gate.clone());
    gate
}
static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
static REGISTRY_CHANGED: Condvar = Condvar::new();
static JOINING: (Mutex<bool>, Condvar) = (Mutex::new(false), Condvar::new());
fn registry() -> &'static Mutex<Registry> {
    REGISTRY.get_or_init(Mutex::default)
}
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(_) => std::process::abort(),
    }
}
fn wait<'a, T>(condition: &Condvar, guard: MutexGuard<'a, T>) -> MutexGuard<'a, T> {
    match condition.wait(guard) {
        Ok(guard) => guard,
        Err(_) => std::process::abort(),
    }
}
struct JoinDomain;
impl Drop for JoinDomain {
    fn drop(&mut self) {
        protected(|| {
            *lock(&JOINING.0) = false;
            JOINING.1.notify_all();
        });
    }
}
fn join_domain() -> JoinDomain {
    let mut joining = lock(&JOINING.0);
    while *joining {
        joining = wait(&JOINING.1, joining);
    }
    *joining = true;
    JoinDomain
}
fn join_slot(slot: &Arc<Slot>) {
    let _domain = join_domain();
    join_slot_in_domain(slot);
}
fn join_slot_in_domain(slot: &Arc<Slot>) {
    let handle = {
        let mut registry = lock(registry());
        loop {
            match registry
                .entries
                .iter_mut()
                .find(|entry| Arc::ptr_eq(&entry.slot, slot))
            {
                Some(entry) if entry.handle.is_some() => break entry.handle.take(),
                Some(_) => registry = wait(&REGISTRY_CHANGED, registry),
                None => break None,
            }
        }
    };
    if let Some(handle) = handle {
        if handle.join().is_err() {
            std::process::abort()
        }
        // The outer slot remains registered until the actual worker has been joined.
        lock(registry())
            .entries
            .retain(|entry| !Arc::ptr_eq(&entry.slot, slot));
    }
}
fn reap_finished() {
    let domain = {
        let mut joining = lock(&JOINING.0);
        if *joining {
            return;
        }
        *joining = true;
        JoinDomain
    };
    let slots: Vec<_> = lock(registry())
        .entries
        .iter()
        .filter(|entry| entry.handle.as_ref().is_some_and(JoinHandle::is_finished))
        .map(|entry| entry.slot.clone())
        .collect();
    for slot in slots {
        join_slot_in_domain(&slot);
    }
    drop(domain);
}

pub(super) struct PreparedRefusal {
    slot: Arc<Slot>,
    published: bool,
}
impl PreparedRefusal {
    pub(super) fn reserve(
        guard: Arc<PreparationLock>,
        resources: DeliveryResources,
        run_hint: RunId,
    ) -> Result<Self> {
        Self::reserve_inner(
            guard,
            resources,
            run_hint,
            #[cfg(test)]
            lock(READY_GATES.get_or_init(Mutex::default)).remove(&run_hint),
            #[cfg(test)]
            None,
        )
    }
    fn reserve_inner(
        guard: Arc<PreparationLock>,
        resources: DeliveryResources,
        run_hint: RunId,
        #[cfg(test)] ready_gate: Option<Arc<ReadyGate>>,
        #[cfg(test)] creation_gate: Option<Arc<ReadyGate>>,
    ) -> Result<Self> {
        install_owner_panic_protection();
        protected(|| {
            if lock(registry()).closed {
                return Err(WorkItemError::Busy);
            }
            reap_finished();
            let slot = Arc::new(Slot {
                guard,
                resources,
                run_hint,
                state: Mutex::new(State::Preparing),
                changed: Condvar::new(),
                #[cfg(test)]
                ready_gate,
            });
            {
                let mut registry = lock(registry());
                if registry.closed || registry.entries.len() >= 8 {
                    return Err(WorkItemError::Busy);
                }
                registry.entries.push(Entry {
                    slot: slot.clone(),
                    handle: None,
                });
            }
            #[cfg(test)]
            if let Some(gate) = &creation_gate {
                gate.enter();
            }
            let worker_slot = slot.clone();
            let handle = std::thread::Builder::new()
                .name("owned-flow-refusal".into())
                .stack_size(512 * 1024)
                .spawn(move || protected(|| worker(&worker_slot)));
            match handle {
                Ok(handle) => {
                    let mut registry = lock(registry());
                    let Some(entry) = registry
                        .entries
                        .iter_mut()
                        .find(|entry| Arc::ptr_eq(&entry.slot, &slot))
                    else {
                        std::process::abort()
                    };
                    entry.handle = Some(handle);
                    REGISTRY_CHANGED.notify_all();
                },
                Err(error) => {
                    lock(registry())
                        .entries
                        .retain(|entry| !Arc::ptr_eq(&entry.slot, &slot));
                    REGISTRY_CHANGED.notify_all();
                    return Err(WorkItemError::Io(error));
                },
            }
            let mut state = lock(&slot.state);
            while matches!(*state, State::Preparing) {
                state = wait(&slot.changed, state);
            }
            let ready = matches!(*state, State::Ready);
            drop(state);
            let reservation = Self {
                slot,
                published: false,
            };
            if !ready {
                return Err(WorkItemError::Busy);
            }
            Ok(reservation)
        })
    }
    pub(super) fn publish(mut self, key: ContentHash, hash: ContentHash, uncertain: bool) {
        protected(|| {
            *lock(&self.slot.state) = if uncertain {
                State::CommitUnknown { key, hash }
            } else {
                State::Committed { key, hash }
            };
            self.published = true;
            self.slot.changed.notify_all();
        });
    }
}
impl Drop for PreparedRefusal {
    fn drop(&mut self) {
        protected(|| {
            if !self.published {
                *lock(&self.slot.state) = State::Aborted;
                self.slot.changed.notify_all();
                join_slot(&self.slot);
            }
        });
    }
}
fn worker(slot: &Arc<Slot>) {
    #[cfg(test)]
    if let Some(gate) = &slot.ready_gate {
        gate.enter();
    }
    let mut state = lock(&slot.state);
    if matches!(*state, State::Preparing) {
        *state = State::Ready;
    }
    slot.changed.notify_all();
    while matches!(*state, State::Ready | State::Preparing) {
        state = wait(&slot.changed, state);
    }
    let selected = *state;
    drop(state);
    let (key, hash, uncertain) = match selected {
        State::Committed { key, hash } => (key, hash, false),
        State::CommitUnknown { key, hash } => (key, hash, true),
        _ => return,
    };
    let mut journal_owner = None;
    loop {
        match refusal_delivery::deliver(
            &slot.resources,
            &slot.guard,
            slot.run_hint,
            key,
            hash,
            uncertain,
            &mut journal_owner,
        ) {
            Ok(()) => {
                *lock(&slot.state) = State::Delivered;
                slot.changed.notify_all();
                return;
            },
            Err(_) => std::thread::sleep(std::time::Duration::from_millis(100)),
        }
    }
}
/// Close process admission before consuming the asynchronous runtime.
pub(crate) fn close_admission() {
    install_owner_panic_protection();
    protected(|| {
        let mut registry = lock(registry());
        registry.closed = true;
    });
}
/// Join each actual informational delivery owner after asynchronous runtime teardown.
pub(crate) fn join_all() {
    protected(|| {
        let slots: Vec<_> = lock(registry())
            .entries
            .iter()
            .map(|entry| entry.slot.clone())
            .collect();
        for slot in slots {
            join_slot(&slot);
        }
    });
}
#[cfg(test)]
#[derive(Default)]
pub(super) struct ReadyGate {
    state: Mutex<(bool, bool)>,
    changed: Condvar,
}
#[cfg(test)]
impl ReadyGate {
    fn enter(&self) {
        let mut state = lock(&self.state);
        state.0 = true;
        self.changed.notify_all();
        while !state.1 {
            state = wait(&self.changed, state);
        }
    }
    pub(super) fn wait_entered(&self) {
        let mut state = lock(&self.state);
        while !state.0 {
            state = wait(&self.changed, state);
        }
    }
    pub(super) fn release(&self) {
        lock(&self.state).1 = true;
        self.changed.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::work_items::{LaunchLock, WorkItemLaunchClaim};
    use surge_core::{id::WorkItemOperationId, work_item::WorkItemCommand};

    fn original_guard(claim: &WorkItemLaunchClaim) -> Arc<PreparationLock> {
        match &claim.lock {
            LaunchLock::OwnedFlow { guard, .. } => guard.clone(),
            _ => panic!("real owned launch"),
        }
    }
    fn resources(storage: &crate::runs::Storage) -> DeliveryResources {
        DeliveryResources {
            pool: storage.registry_pool.clone(),
            home: storage.home.clone(),
            clock: storage.clock.clone(),
            writers: storage.active_writers.clone(),
        }
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn ready_wait_is_outside_registry_transaction_and_newer_stop_wins() {
        let home = tempfile::tempdir().unwrap();
        let storage = crate::runs::Storage::open(home.path()).await.unwrap();
        let store = storage.work_items();
        let (receipt, claim) =
            super::super::tests::accepted(&store, home.path(), WorkItemOperationId::new());
        let guard = original_guard(&claim);
        let original_token: String = store
            .pool
            .get()
            .unwrap()
            .query_row(
                "SELECT claim_token FROM work_item_attempts WHERE run=?",
                [receipt.run.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        let gate = Arc::new(ReadyGate::default());
        let worker_gate = gate.clone();
        let worker_resources = resources(&storage);
        let pending = std::thread::spawn(move || {
            PreparedRefusal::reserve_inner(
                guard,
                worker_resources,
                receipt.run,
                Some(worker_gate),
                None,
            )
        });
        gate.wait_entered();
        // This is a second actual connection and normal immutable operator operation.
        let current = store.show(receipt.item).unwrap();
        let command = WorkItemCommand::Suspend {
            operation_id: WorkItemOperationId::new(),
            item: receipt.item,
            expected_version: current.item.version,
        };
        let stopped = store
            .mutate(&command, None, None, "ready-barrier", 100)
            .unwrap();
        assert!(matches!(
            stopped,
            surge_core::work_item::WorkItemResult::Control(_)
        ));
        let observed = store.execution_control(receipt.run).unwrap().unwrap();
        assert_eq!(
            observed.state,
            surge_core::execution_recovery::ExecutionControlState::SuspendRequested
        );
        gate.release();
        let prepared = pending.join().unwrap().unwrap();
        let mut conn = store.pool.get().unwrap();
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .unwrap();
        let fresh = crate::work_items::control::read_control(&tx, receipt.run, None)
            .unwrap()
            .unwrap();
        assert_eq!(fresh, observed);
        let receipts: u64 = tx
            .query_row("SELECT COUNT(*) FROM owned_flow_wake_refusals", [], |row| {
                row.get(0)
            })
            .unwrap();
        let token: String = tx
            .query_row(
                "SELECT claim_token FROM work_item_attempts WHERE run=?",
                [receipt.run.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        tx.rollback().unwrap();
        drop(prepared); // actual aborted owner join only after the transaction was released.
        assert_eq!(receipts, 0);
        assert_eq!(token, original_token);
        assert_eq!(
            store.execution_control(receipt.run).unwrap().unwrap(),
            observed
        );
        assert!(matches!(store.claim(receipt.run), Err(WorkItemError::Busy)));
        drop(claim);
        let released = PreparationLock::acquire_stable(
            home.path(),
            crate::work_items::start_preparation::secure_lock::PreparationLockKey::FlowLaunch {
                operation: receipt.operation_id,
                run: receipt.run,
            },
        )
        .unwrap();
        released.verify().unwrap();
        drop(released);
        // A newer Stop is still not dispatchable merely because its real lease released.
        assert!(matches!(
            store.claim(receipt.run),
            Err(WorkItemError::Conflict(_))
        ));
    }

    fn wait_for(condition: impl Fn() -> bool) -> bool {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !condition() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        condition()
    }
    fn owned_fixture() -> (
        tempfile::TempDir,
        Arc<crate::runs::Storage>,
        tokio::runtime::Runtime,
    ) {
        let home = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let storage = runtime
            .block_on(crate::runs::Storage::open(home.path()))
            .unwrap();
        (home, storage, runtime)
    }
    fn closed_admission_does_not_wait_behind_pending_join() {
        let (home, storage, _runtime) = owned_fixture();
        let store = storage.work_items();
        let (first, claim_a) =
            super::super::tests::accepted(&store, home.path(), WorkItemOperationId::new());
        let a = PreparedRefusal::reserve(original_guard(&claim_a), resources(&storage), first.run)
            .unwrap();
        drop(claim_a);
        let (second, claim_b) =
            super::super::tests::accepted(&store, home.path(), WorkItemOperationId::new());
        let b = PreparedRefusal::reserve(original_guard(&claim_b), resources(&storage), second.run)
            .unwrap();
        drop(claim_b);
        // An actual ambiguous-commit absence resolution creates a finished but unreaped B.
        let absent = surge_core::work_item::OwnedFlowWakeRefusalReceipt::new(
            second.run,
            second.operation_id,
            second.binding.clone(),
            surge_core::work_item::OwnedFlowWakeLineage {
                invocation: surge_core::id::StageInvocationId::new(),
                control_generation: 1,
                cycle_generation: 1,
                source_revision: 1,
                wake_identity: "rolled-back-wake".into(),
            },
            surge_core::work_item::OwnedFlowWakeRefusalReason::InputsAssociationMismatch,
        )
        .unwrap();
        let absent_hash = absent.hash().unwrap();
        b.publish(absent_hash, absent_hash, true);
        let finished_b = wait_for(|| {
            lock(registry()).entries.iter().any(|entry| {
                entry.slot.run_hint == second.run
                    && entry.handle.as_ref().is_some_and(JoinHandle::is_finished)
            })
        });
        close_admission();
        let joining = std::thread::spawn(join_all);
        let blocked_a = wait_for(|| {
            lock(registry())
                .entries
                .iter()
                .any(|entry| entry.slot.run_hint == first.run && entry.handle.is_none())
        });
        let (send, receive) = std::sync::mpsc::channel();
        let probe_guard = PreparationLock::acquire_stable(
            home.path(),
            crate::work_items::start_preparation::secure_lock::PreparationLockKey::FlowLaunch {
                operation: WorkItemOperationId::new(),
                run: RunId::new(),
            },
        )
        .unwrap();
        let probe_resources = resources(&storage);
        let probe = std::thread::spawn(move || {
            let denied = matches!(
                PreparedRefusal::reserve(Arc::new(probe_guard), probe_resources, RunId::new()),
                Err(WorkItemError::Busy)
            );
            send.send(denied).unwrap();
        });
        let immediate_busy = receive
            .recv_timeout(std::time::Duration::from_millis(250))
            .ok();
        let original_excluded =
            matches!(PreparationLock::acquire_stable(home.path(),
            crate::work_items::start_preparation::secure_lock::PreparationLockKey::FlowLaunch {
                operation: first.operation_id, run: first.run,
            }), Err(WorkItemError::Busy));
        drop(a); // resolves the original admitted Ready reservation, never erased by close.
        joining.join().unwrap();
        probe.join().unwrap();
        assert!(finished_b);
        assert!(blocked_a);
        assert!(original_excluded);
        assert_eq!(immediate_busy, Some(true));
        assert!(lock(registry()).entries.is_empty());
    }
    fn registered_creation_is_counted_until_actual_handle_and_resolution() {
        let (home, storage, _runtime) = owned_fixture();
        let store = storage.work_items();
        let (receipt, claim) =
            super::super::tests::accepted(&store, home.path(), WorkItemOperationId::new());
        let guard = original_guard(&claim);
        drop(claim);
        let creation = Arc::new(ReadyGate::default());
        let creator_gate = creation.clone();
        let delivery_resources = resources(&storage);
        let creating = std::thread::spawn(move || {
            PreparedRefusal::reserve_inner(
                guard,
                delivery_resources,
                receipt.run,
                None,
                Some(creator_gate),
            )
        });
        creation.wait_entered();
        close_admission();
        let (send, receive) = std::sync::mpsc::channel();
        let joining = std::thread::spawn(move || {
            join_all();
            send.send(()).unwrap();
        });
        let entered = wait_for(|| *lock(&JOINING.0));
        let premature = receive
            .recv_timeout(std::time::Duration::from_millis(100))
            .is_ok();
        creation.release();
        let prepared = creating.join().unwrap().unwrap();
        let still_waiting = receive
            .recv_timeout(std::time::Duration::from_millis(100))
            .is_err();
        drop(prepared);
        joining.join().unwrap();
        assert!(entered);
        assert!(!premature);
        assert!(still_waiting);
        assert!(lock(registry()).entries.is_empty());
        let _released = PreparationLock::acquire_stable(
            home.path(),
            crate::work_items::start_preparation::secure_lock::PreparationLockKey::FlowLaunch {
                operation: receipt.operation_id,
                run: receipt.run,
            },
        )
        .unwrap();
    }
    fn ambiguous_lookup_unavailable_retains_until_actual_absence() {
        let (home, storage, _runtime) = owned_fixture();
        let store = storage.work_items();
        let (receipt, claim) =
            super::super::tests::accepted(&store, home.path(), WorkItemOperationId::new());
        let token: String = store
            .pool
            .get()
            .unwrap()
            .query_row(
                "SELECT claim_token FROM work_item_attempts WHERE run=?",
                [receipt.run.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        let guard = original_guard(&claim);
        drop(claim);
        // Isolated damaged-storage model: querying the exact table really fails,
        // rather than returning a made-up None or intercepting production reads.
        store
            .pool
            .get()
            .unwrap()
            .execute(
                "ALTER TABLE owned_flow_wake_refusals RENAME TO unavailable_refusals",
                [],
            )
            .unwrap();
        let prepared = PreparedRefusal::reserve(guard, resources(&storage), receipt.run).unwrap();
        let absent = surge_core::work_item::OwnedFlowWakeRefusalReceipt::new(
            receipt.run,
            receipt.operation_id,
            receipt.binding.clone(),
            surge_core::work_item::OwnedFlowWakeLineage {
                invocation: surge_core::id::StageInvocationId::new(),
                control_generation: 1,
                cycle_generation: 1,
                source_revision: 1,
                wake_identity: "uncommitted-observed-wake".into(),
            },
            surge_core::work_item::OwnedFlowWakeRefusalReason::InputsAssociationMismatch,
        )
        .unwrap()
        .hash()
        .unwrap();
        prepared.publish(absent, absent, true);
        std::thread::sleep(std::time::Duration::from_millis(200));
        let actual_lookup_failed = store
            .pool
            .get()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM owned_flow_wake_refusals", [], |row| {
                row.get::<_, u64>(0)
            })
            .is_err();
        close_admission();
        let (send, receive) = std::sync::mpsc::channel();
        let joining = std::thread::spawn(move || {
            join_all();
            send.send(()).unwrap();
        });
        let entered = wait_for(|| *lock(&JOINING.0));
        let premature = receive
            .recv_timeout(std::time::Duration::from_millis(200))
            .is_ok();
        let original_excluded =
            matches!(PreparationLock::acquire_stable(home.path(),
            crate::work_items::start_preparation::secure_lock::PreparationLockKey::FlowLaunch {
                operation: receipt.operation_id, run: receipt.run,
            }), Err(WorkItemError::Busy));
        store
            .pool
            .get()
            .unwrap()
            .execute(
                "ALTER TABLE unavailable_refusals RENAME TO owned_flow_wake_refusals",
                [],
            )
            .unwrap();
        assert!(
            receive
                .recv_timeout(std::time::Duration::from_secs(2))
                .is_ok()
        );
        joining.join().unwrap();
        let current_token: String = store
            .pool
            .get()
            .unwrap()
            .query_row(
                "SELECT claim_token FROM work_item_attempts WHERE run=?",
                [receipt.run.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(current_token, token);
        assert!(actual_lookup_failed);
        assert!(entered);
        assert!(
            !premature,
            "unavailable receipt lookup was treated as confirmed absence"
        );
        assert!(original_excluded);
        assert!(lock(registry()).entries.is_empty());
        let _released = PreparationLock::acquire_stable(
            home.path(),
            crate::work_items::start_preparation::secure_lock::PreparationLockKey::FlowLaunch {
                operation: receipt.operation_id,
                run: receipt.run,
            },
        )
        .unwrap();
    }

    fn ninth_live_reservation_is_busy_without_refusal_mutation() {
        let (home_a, storage_a, _runtime_a) = owned_fixture();
        let (home, storage, _runtime) = owned_fixture();
        let store = storage.work_items();
        let mut reservations = Vec::new();
        for index in 0..8 {
            let (selected_home, selected_storage) = if index % 2 == 0 {
                (&home_a, &storage_a)
            } else {
                (&home, &storage)
            };
            let (receipt, claim) = super::super::tests::accepted(
                &selected_storage.work_items(),
                selected_home.path(),
                WorkItemOperationId::new(),
            );
            let prepared = PreparedRefusal::reserve(
                original_guard(&claim),
                resources(selected_storage),
                receipt.run,
            )
            .unwrap();
            drop(claim);
            reservations.push(prepared);
        }
        let (receipt, claim) =
            super::super::tests::accepted(&store, home.path(), WorkItemOperationId::new());
        let token: String = store
            .pool
            .get()
            .unwrap()
            .query_row(
                "SELECT claim_token FROM work_item_attempts WHERE run=?",
                [receipt.run.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        let ninth_busy = matches!(
            PreparedRefusal::reserve(original_guard(&claim), resources(&storage), receipt.run),
            Err(WorkItemError::Busy)
        );
        let current: String = store
            .pool
            .get()
            .unwrap()
            .query_row(
                "SELECT claim_token FROM work_item_attempts WHERE run=?",
                [receipt.run.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        let rows: u64 = store
            .pool
            .get()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM owned_flow_wake_refusals", [], |row| {
                row.get(0)
            })
            .unwrap();
        let rows_a: u64 = storage_a
            .work_items()
            .pool
            .get()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM owned_flow_wake_refusals", [], |row| {
                row.get(0)
            })
            .unwrap();
        let actual_live = lock(registry()).entries.len();
        drop(claim);
        drop(reservations); // Each unused reservation aborts and joins its actual worker.
        close_admission();
        join_all();
        assert!(ninth_busy);
        assert_eq!(actual_live, 8);
        assert_eq!(current, token);
        assert_eq!(rows, 0);
        assert_eq!(rows_a, 0);
        assert!(lock(registry()).entries.is_empty());
    }

    #[test]
    fn lifecycle_probe() {
        match std::env::var("SURGE_REFUSAL_LIFECYCLE_PROBE").as_deref() {
            Ok("closed") => closed_admission_does_not_wait_behind_pending_join(),
            Ok("creation") => registered_creation_is_counted_until_actual_handle_and_resolution(),
            Ok("unknown") => ambiguous_lookup_unavailable_retains_until_actual_absence(),
            Ok("capacity") => ninth_live_reservation_is_busy_without_refusal_mutation(),
            _ => {},
        }
    }
    #[test]
    fn actual_shutdown_and_creation_barriers_in_isolated_processes() {
        for case in ["closed", "creation", "unknown", "capacity"] {
            let mut child = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "work_items::owned_flow::refusal_owner::tests::lifecycle_probe",
                    "--nocapture",
                ])
                .env("SURGE_REFUSAL_LIFECYCLE_PROBE", case)
                .spawn()
                .unwrap();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
            let status = loop {
                if let Some(status) = child.try_wait().unwrap() {
                    break Some(status);
                }
                if std::time::Instant::now() >= deadline {
                    child.kill().unwrap();
                    child.wait().unwrap();
                    break None;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            };
            assert!(
                status.is_some_and(|status| status.success()),
                "actual refusal lifecycle child failed"
            );
        }
    }
}
