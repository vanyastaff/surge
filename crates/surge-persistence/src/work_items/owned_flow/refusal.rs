//! Manifest-only quota-wake rejection with pretransaction original-guard ownership.
use super::{
    launch, private_unavailable,
    refusal_owner::{DeliveryResources, PreparedRefusal},
};
use crate::work_items::{
    self, LaunchLock, Result, WorkItemError, WorkItemLaunchClaim, WorkItemStore,
    recovery_cycles::{self, RecoveryCycle},
    start_preparation::secure_lock::{PreparationLock, PreparationLockKey},
};
use rusqlite::{Connection, TransactionBehavior, params};
use std::sync::Arc;
use surge_core::{
    ContentHash, RunId,
    execution_recovery::{ExecutionControlState as ControlState, WorkItemExecutionControl},
    work_item::*,
};
use surge_process::owner_panic::{
    abort_on_owner_panic as protected, install_owner_panic_protection,
};

/// Host quota entry classification; receipts alone never convey a launch claim.
pub enum OwnedFlowWakeAdmission {
    /// Legacy task keeps its existing claim path.
    Legacy,
    /// Exact accepted snapshot is coherent under this retained original guard.
    Ready(Box<WorkItemLaunchClaim>),
    /// Immutable refusal and Attention committed, with an independent delivery owner.
    Refused,
    /// Exact private unavailability committed Attention; no claim or permanent receipt.
    PrivateInputsUnavailable,
    /// Operator/current assignment changed; no refusal mutation was made.
    Obsolete,
}
struct WakeHint {
    cycle: RecoveryCycle,
    control: WorkItemExecutionControl,
    version: u64,
    registry_wake: Option<(String, Option<i64>)>,
}
enum Decision {
    Ready(WorkItemAttempt),
    Obsolete,
    Mismatch(Box<OwnedFlowWakeRefusalReceipt>),
}
fn hint(store: &WorkItemStore, run: RunId, now: i64) -> Result<Option<WakeHint>> {
    let cycles = store.due_recovery_wakes_for_run(run, now, 100)?;
    let Some(cycle) = cycles.into_iter().next() else {
        return Ok(None);
    };
    let conn = store.pool.get()?;
    let Some(control) = work_items::control::read_control(&conn, run, None)? else {
        return Ok(None);
    };
    let item = work_items::record(&conn, control.item)?;
    let registry_wake = private_unavailable::registry(&conn, run)?;
    Ok(Some(WakeHint {
        cycle,
        control,
        version: item.version,
        registry_wake,
    }))
}
fn classify(
    conn: &Connection,
    accepted: &launch::AcceptedLaunch,
    run: RunId,
    expected: &WakeHint,
    now: i64,
) -> Result<Decision> {
    let attempt = launch::independent_binding(conn, accepted, run)?;
    let item = work_items::record(conn, attempt.item)?;
    let control = work_items::control::read_control(conn, run, None)?;
    if item.archived_at_ms.is_some()
        || item.active_run != Some(run)
        || item.generation != attempt.binding.generation
        || item.accepted_revision != attempt.binding.revision
        || item.workspace != accepted.source.workspace
        || item.version != expected.version
        || attempt.state != WorkItemAttemptState::Suspended
        || accepted.state != "started"
        || control.as_ref() != Some(&expected.control)
        || expected.control.state != ControlState::Suspended
        || expected.control.item != attempt.item
        || expected.control.run != run
        || expected.control.attempt_generation != attempt.binding.generation
    {
        return Ok(Decision::Obsolete);
    }
    if private_unavailable::registry(conn, run)? != expected.registry_wake {
        return Ok(Decision::Obsolete);
    }
    let cycle = recovery_cycles::read_cycle(
        conn,
        run,
        &expected.cycle.invocation,
        expected.cycle.generation,
    )?;
    if cycle != expected.cycle
        || cycle.closed
        || !cycle
            .wake
            .as_ref()
            .is_some_and(|wake| wake.due_at_ms() <= now)
        || (cycle.control_generation != expected.control.generation
            && !recovery_cycles::validate_capacity_transfer(
                conn,
                run,
                &attempt.binding,
                &cycle,
                expected.control.generation,
            )?)
    {
        return Ok(Decision::Obsolete);
    }
    let manifest: OwnedFlowInputsManifest = serde_json::from_str(&accepted.manifest)?;
    if manifest.operation() == accepted.operation
        && manifest.run() == run
        && manifest.binding() == &attempt.binding
        && manifest.workspace() == &accepted.source.workspace
        && manifest.workspace_owner() == accepted.receipt.workspace_owner
    {
        return Ok(Decision::Ready(attempt));
    }
    let wake = cycle.wake.as_ref().ok_or(WorkItemError::NotFound)?;
    let receipt = OwnedFlowWakeRefusalReceipt::new(
        run,
        accepted.operation,
        attempt.binding,
        OwnedFlowWakeLineage {
            invocation: cycle
                .invocation
                .parse()
                .map_err(|_| WorkItemError::IndependentAcceptedDataInvalid)?,
            control_generation: expected.control.generation,
            cycle_generation: cycle.generation,
            source_revision: cycle.revision,
            wake_identity: wake.identity().to_owned(),
        },
        OwnedFlowWakeRefusalReason::InputsAssociationMismatch,
    )
    .map_err(|_| WorkItemError::IndependentAcceptedDataInvalid)?;
    Ok(Decision::Mismatch(Box::new(receipt)))
}
fn commit_refusal(
    conn: &Connection,
    receipt: &OwnedFlowWakeRefusalReceipt,
    expected: &WakeHint,
) -> Result<ContentHash> {
    let hash = receipt.hash()?;
    let binding = receipt.binding();
    let lineage = receipt.lineage();
    conn.execute("INSERT INTO owned_flow_wake_refusals(key,hash,body,run,operation,item,attempt_generation,control_generation,invocation,cycle_generation,source_revision,wake_identity) VALUES(?,?,?,?,?,?,?,?,?,?,?,?)",
        params![hash.to_string(), hash.to_string(), serde_json::to_string(receipt)?, receipt.run().to_string(), receipt.operation().to_string(), binding.item.to_string(), binding.generation, lineage.control_generation, lineage.invocation.to_string(), lineage.cycle_generation, lineage.source_revision, lineage.wake_identity])?;
    conn.execute(
        "INSERT INTO owned_flow_wake_refusal_outbox(key,hash) VALUES(?,?)",
        params![hash.to_string(), hash.to_string()],
    )?;
    let attempt = work_items::attempt(conn, receipt.run())?;
    private_unavailable::attention(
        conn,
        &attempt,
        Some(&expected.control),
        expected.version,
        "owned_flow_inputs_association_mismatch",
    )?;
    disarm_private_wake(conn, receipt.run(), expected)?;
    Ok(hash)
}
fn original_guard(
    store: &WorkItemStore,
    observed: &launch::AcceptedLaunch,
    run: RunId,
) -> Result<Arc<PreparationLock>> {
    let guard = Arc::new(PreparationLock::acquire_stable(
        &store.home,
        PreparationLockKey::FlowLaunch {
            operation: observed.operation,
            run,
        },
    )?);
    if guard.identity()? != observed.identity {
        return Err(WorkItemError::IndependentAcceptedDataInvalid);
    }
    Ok(guard)
}
pub(super) fn reserve_refusal(
    store: &WorkItemStore,
    guard: Arc<PreparationLock>,
    run: RunId,
) -> Result<PreparedRefusal> {
    PreparedRefusal::reserve(
        guard,
        DeliveryResources {
            pool: store.pool.clone(),
            home: store.home.clone(),
            clock: store.clock.clone(),
            writers: store.writers.clone(),
        },
        run,
    )
}
fn revalidate_private(
    authenticated: Result<Option<Arc<super::PrivateOwner>>>,
    conn: &Connection,
) -> (Option<Arc<super::PrivateOwner>>, Option<WorkItemError>) {
    match authenticated {
        Ok(owner) => {
            let error = owner.as_ref().and_then(|owner| owner.verify_in(conn).err());
            (owner, error)
        },
        Err(error) => (None, Some(error)),
    }
}
fn disarm_private_wake(conn: &Connection, run: RunId, expected: &WakeHint) -> Result<()> {
    let cycles = conn.execute("UPDATE work_item_quota_cycles SET wake_at_ms=NULL WHERE run=? AND invocation=? AND cycle_generation=? AND revision=? AND wake_at_ms IS ?", params![run.to_string(),expected.cycle.invocation,expected.cycle.generation,expected.cycle.revision,expected.cycle.wake.as_ref().map(|wake|wake.due_at_ms())])?;
    if cycles != 1 {
        return Err(WorkItemError::Conflict(
            "private recovery wake changed".into(),
        ));
    }
    if let Some((status, wake)) = &expected.registry_wake {
        let changed = conn.execute(
            "UPDATE runs SET wake_at=NULL WHERE id=? AND status=? AND wake_at IS ?",
            params![run.to_string(), status, wake],
        )?;
        if changed != 1 {
            return Err(WorkItemError::Conflict(
                "private registry wake changed".into(),
            ));
        }
    }
    Ok(())
}
impl WorkItemStore {
    /// Inspect a due owned Flow before normal claim hydration can reject a damaged manifest.
    /// A permanent mismatch owns its original guard through actual informational delivery.
    pub fn claim_owned_flow_quota_wake(
        &self,
        run: RunId,
        now: i64,
    ) -> Result<OwnedFlowWakeAdmission> {
        install_owner_panic_protection();
        protected(|| self.claim_quota_wake_inner(run, now))
    }
    fn claim_quota_wake_inner(&self, run: RunId, now: i64) -> Result<OwnedFlowWakeAdmission> {
        super::purpose::refuse_execution(self, run)?;
        let exists: bool = self.pool.get()?.query_row(
            "SELECT EXISTS(SELECT 1 FROM owned_flow_operations WHERE run=? AND state='accepted')",
            [run.to_string()],
            |row| row.get(0),
        )?;
        if !exists {
            return Ok(OwnedFlowWakeAdmission::Legacy);
        }
        let Some(hint) = hint(self, run, now)? else {
            return Ok(OwnedFlowWakeAdmission::Obsolete);
        };
        let observed = launch::accepted_launch(&*self.pool.get()?, run)?;
        let guard = original_guard(self, &observed, run)?;
        let private_auth = serde_json::from_str::<OwnedFlowInputsManifest>(&observed.manifest)
            .map_err(|_| WorkItemError::PrivateInputsCorrupt)
            .and_then(|manifest| super::private_inputs::hydrate(self, &manifest))
            .map(|(_, owner)| owner);
        let prepared = reserve_refusal(self, guard.clone(), run)?;
        // Whole journal/artifact IO remains outside the registry write transaction.
        super::refusal_delivery::validate_original_startup(&self.home, &observed)?;
        // Ready has completed before any registry write transaction is opened.
        let mut conn = self.pool.get()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let accepted = launch::accepted_launch(&tx, run)?;
        if accepted.operation != observed.operation
            || accepted.identity != observed.identity
            || accepted.source != observed.source
            || accepted.config != observed.config
            || accepted.receipt != observed.receipt
        {
            return Err(WorkItemError::IndependentAcceptedDataInvalid);
        }
        let decision = classify(&tx, &accepted, run, &hint, now)?;
        guard.verify()?;
        match decision {
            Decision::Obsolete => {
                tx.rollback()?;
                drop(prepared);
                Ok(OwnedFlowWakeAdmission::Obsolete)
            },
            Decision::Ready(attempt) => {
                if accepted.manifest != observed.manifest {
                    return Err(WorkItemError::PrivateInputsCorrupt);
                }
                let (private_owner, private_error) = revalidate_private(private_auth, &tx);
                match private_error {
                    Some(error) if private_unavailable::known(&error) => {
                        private_unavailable::attention(
                            &tx,
                            &attempt,
                            Some(&hint.control),
                            hint.version,
                            "owned_flow_private_inputs_unavailable",
                        )?;
                        disarm_private_wake(&tx, run, &hint)?;
                        guard.verify()?;
                        tx.commit()?;
                        guard.verify()?;
                        drop(prepared);
                        return Ok(OwnedFlowWakeAdmission::PrivateInputsUnavailable);
                    },
                    Some(error) => return Err(error),
                    None => {},
                }
                let token = RunId::new().to_string();
                tx.execute(
                    "UPDATE work_item_attempts SET claim_token=? WHERE run=? AND generation=?",
                    params![token, run.to_string(), attempt.binding.generation],
                )?;
                guard.verify()?;
                tx.commit()?;
                drop(prepared);
                Ok(OwnedFlowWakeAdmission::Ready(Box::new(
                    WorkItemLaunchClaim {
                        lock: LaunchLock::OwnedFlow {
                            guard,
                            operation: accepted.operation,
                            identity: accepted.identity,
                            private_owner,
                        },
                        run,
                        token,
                        binding: attempt.binding,
                    },
                )))
            },
            Decision::Mismatch(receipt) => {
                let hash = commit_refusal(&tx, &receipt, &hint)?;
                guard.verify()?;
                let committed = tx.commit();
                prepared.publish(hash, hash, committed.is_err());
                committed?;
                Ok(OwnedFlowWakeAdmission::Refused)
            },
        }
    }
}

impl WorkItemStore {
    pub(super) fn resume_pending_refusals(&self) -> Result<usize> {
        install_owner_panic_protection();
        protected(|| {
            let pending: Vec<(String, String, String)> = {
                let conn = self.pool.get()?;
                let mut statement = conn.prepare("SELECT r.key,r.hash,r.body FROM owned_flow_wake_refusals r JOIN owned_flow_wake_refusal_outbox o ON o.key=r.key AND o.hash=r.hash WHERE o.delivered_event_seq IS NULL ORDER BY r.run")?;
                let rows =
                    statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
                rows.collect::<rusqlite::Result<_>>()?
            };
            let mut count = 0;
            for (key, hash, body) in pending {
                let receipt: OwnedFlowWakeRefusalReceipt = serde_json::from_str(&body)?;
                let expected = receipt.hash()?;
                if key != expected.to_string() || hash != key {
                    return Err(WorkItemError::IndependentAcceptedDataInvalid);
                }
                let independent = launch::accepted_launch(&*self.pool.get()?, receipt.run())?;
                let accepted =
                    launch::independent_binding(&*self.pool.get()?, &independent, receipt.run())?;
                if receipt.operation() != independent.operation
                    || receipt.binding() != &accepted.binding
                {
                    return Err(WorkItemError::IndependentAcceptedDataInvalid);
                }
                let guard = Arc::new(PreparationLock::acquire_stable(
                    &self.home,
                    PreparationLockKey::FlowLaunch {
                        operation: independent.operation,
                        run: receipt.run(),
                    },
                )?);
                if guard.identity()? != independent.identity {
                    return Err(WorkItemError::IndependentAcceptedDataInvalid);
                }
                let resources = DeliveryResources {
                    pool: self.pool.clone(),
                    home: self.home.clone(),
                    clock: self.clock.clone(),
                    writers: self.writers.clone(),
                };
                let prepared = PreparedRefusal::reserve(guard, resources, receipt.run())?;
                prepared.publish(expected, expected, false);
                count += 1;
            }
            Ok(count)
        })
    }
}
impl crate::runs::Storage {
    /// Reclaim pending informational delivery before any launch-capable startup work.
    /// Original receipt/source/descriptor identity are checked without changing claim tokens.
    pub fn resume_pending_owned_flow_refusals(&self) -> Result<usize> {
        self.work_items().resume_pending_refusals()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runs::Storage;
    use crate::runtime_home_fixture::FixtureHome;
    use surge_core::{EventPayload as E, VersionedEventPayload as V};

    fn wake_accepted(
        store: &WorkItemStore,
        home: &std::path::Path,
        private: bool,
    ) -> (OwnedFlowReceipt, WorkItemLaunchClaim) {
        let servers = if private {
            vec![surge_core::mcp_config::McpServerRef::new(
                "fixed-private-server".into(),
                surge_core::mcp_config::McpTransportConfig::stdio(
                    "unlaunched-private-provider".into(),
                    Vec::new(),
                    Default::default(),
                ),
                None,
                std::time::Duration::from_secs(7),
                false,
            )]
        } else {
            Vec::new()
        };
        super::super::tests::accepted_with_servers(
            store,
            home,
            surge_core::id::WorkItemOperationId::new(),
            &servers,
        )
    }
    async fn storage_wake_fixture() -> (
        FixtureHome,
        (Arc<Storage>, OwnedFlowReceipt, std::path::PathBuf),
    ) {
        storage_wake_fixture_with_private(false).await
    }
    async fn storage_wake_fixture_with_private(
        private: bool,
    ) -> (
        FixtureHome,
        (Arc<Storage>, OwnedFlowReceipt, std::path::PathBuf),
    ) {
        let home = FixtureHome::new().unwrap();
        let storage = Storage::open(home.path()).await.unwrap();
        let store = storage.work_items();
        let (receipt, claim) = wake_accepted(&store, home.path(), private);
        let accepted = launch::accepted_launch(&store.pool.get().unwrap(), receipt.run).unwrap();
        let writer = storage
            .create_run(receipt.run, &accepted.source.workspace.path, None)
            .await
            .unwrap();
        let manifest = store.owned_flow_manifest(receipt.run).unwrap().unwrap();
        writer
            .append_event(V::new(E::RunStarted {
                pipeline_template: None,
                project_path: accepted.source.workspace.path.clone(),
                initial_prompt: accepted.source.contract.raw_prompt().into(),
                config: surge_core::run_event::RunConfig {
                    bootstrap_edit_loop_cap: None,
                    sandbox_default: surge_core::sandbox::SandboxMode::WorkspaceWrite,
                    approval_default: Default::default(),
                    auto_pr: false,
                    mcp_servers: Vec::new(),
                    budget: Default::default(),
                },
            }))
            .await
            .unwrap();
        writer
            .append_event(V::new(E::PipelineMaterialized {
                graph: Box::new(accepted.source.contract.graph().clone()),
                graph_hash: accepted.source.contract.graph_hash(),
            }))
            .await
            .unwrap();
        writer
            .append_event(V::new(E::WorkItemAttemptBound {
                context: WorkItemContext::new_flow(
                    receipt.binding.clone(),
                    accepted.source.contract.clone(),
                )
                .unwrap(),
            }))
            .await
            .unwrap();
        writer
            .append_event(V::new(E::OwnedFlowInputsBound {
                manifest: Box::new(manifest.clone()),
            }))
            .await
            .unwrap();
        let artifact = writer
            .store_artifact(
                "accepted_flow_contract",
                &accepted.source.contract.canonical_bytes().unwrap(),
            )
            .await
            .unwrap();
        let artifact_path = artifact.path.clone();
        writer
            .append_event(V::new(E::ArtifactProduced {
                node: accepted.source.contract.graph().start.clone(),
                artifact: artifact.id,
                path: artifact.path,
                name: "accepted_flow_contract".into(),
                source_path: None,
            }))
            .await
            .unwrap();
        writer.close().await.unwrap();
        store
            .acknowledge_owned_flow_startup(&claim, &manifest)
            .unwrap();
        let command = WorkItemCommand::Suspend {
            operation_id: surge_core::id::WorkItemOperationId::new(),
            item: receipt.item,
            expected_version: store.show(receipt.item).unwrap().item.version,
        };
        store
            .mutate(&command, None, None, "storage-wake-fixture", 10)
            .unwrap();
        let invocation = surge_core::id::StageInvocationId::new();
        let mut control = store.execution_control(receipt.run).unwrap().unwrap();
        control.state = ControlState::Suspended;
        // Storage-only model of the persisted eligibility predicate. This boolean is
        // not a Closed certificate or native/provider cleanup proof; this test checks
        // public-entry transaction ordering, not capacity-transfer acceptance.
        control.fence = Some(surge_core::execution_recovery::SuspensionFence {
            control_generation: control.generation,
            snapshot_seq: 1,
            pending_stage: surge_core::execution_recovery::PendingStagePhase::Interrupted {
                node: accepted.source.contract.graph().start.clone(),
                invocation,
            },
            cleanup_confirmed: true,
            reason: surge_core::execution_recovery::SuspensionReason::Capacity {
                wake_at_ms: Some(100),
            },
        });
        let conn = store.pool.get().unwrap();
        work_items::control::write_control(&conn, &control).unwrap();
        let wake = recovery_cycles::RecoveryWake::new(
            "actual-observed-wake".into(),
            recovery_cycles::WakeOrigin::PolicyBackoff,
            1,
            100,
        )
        .unwrap();
        conn.execute("INSERT INTO work_item_quota_cycles(run,invocation,cycle_generation,item,attempt_generation,control_generation,revision,probe_count,wake,wake_at_ms) VALUES(?,?,1,?,?,?,1,0,?,100)",
            params![receipt.run.to_string(), invocation.to_string(), receipt.item.to_string(), receipt.binding.generation, control.generation, serde_json::to_string(&wake).unwrap()]).unwrap();
        drop(conn);
        store
            .settle(
                receipt.run,
                receipt.binding.generation,
                WorkItemAttemptState::Suspended,
                None,
            )
            .unwrap();
        storage.set_run_parked(&receipt.run, 100).await.unwrap();
        drop(claim);
        (home, (storage, receipt, artifact_path))
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn production_entry_waits_ready_before_transaction_then_observes_real_newer_stop() {
        let (home, owners) = storage_wake_fixture().await;
        {
            let (storage, receipt, _artifact) = owners;
            let store = storage.work_items();
            let allowed = store.claim_owned_flow_quota_wake(receipt.run, 100).unwrap();
            assert!(matches!(allowed, OwnedFlowWakeAdmission::Ready(_)));
            drop(allowed);
            let before = store
                .pool
                .get()
                .unwrap()
                .query_row(
                    "SELECT claim_token FROM work_item_attempts WHERE run=?",
                    [receipt.run.to_string()],
                    |row| row.get::<_, String>(0),
                )
                .unwrap();
            let gate = super::super::refusal_owner::ready_gate(receipt.run);
            let waking_store = store.clone();
            let waking = std::thread::spawn(move || {
                waking_store.claim_owned_flow_quota_wake(receipt.run, 100)
            });
            gate.wait_entered();
            let command = WorkItemCommand::Suspend {
                operation_id: surge_core::id::WorkItemOperationId::new(),
                item: receipt.item,
                expected_version: store.show(receipt.item).unwrap().item.version,
            };
            // Production operator operation on a second pooled connection commits while the real entry waits Ready.
            store
                .mutate(&command, None, None, "newer-real-stop", 200)
                .unwrap();
            let newest = store.execution_control(receipt.run).unwrap().unwrap();
            gate.release();
            let result = waking.join().unwrap().unwrap();
            assert!(matches!(result, OwnedFlowWakeAdmission::Obsolete));
            assert_eq!(
                store.execution_control(receipt.run).unwrap().unwrap(),
                newest
            );
            let conn = store.pool.get().unwrap();
            let token: String = conn
                .query_row(
                    "SELECT claim_token FROM work_item_attempts WHERE run=?",
                    [receipt.run.to_string()],
                    |row| row.get(0),
                )
                .unwrap();
            let receipts: u64 = conn
                .query_row("SELECT COUNT(*) FROM owned_flow_wake_refusals", [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(token, before);
            assert_eq!(receipts, 0);
            assert_eq!(newest.state, ControlState::SuspendRequested);
        }
        home.close().expect("close runtime home");
    }
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn production_startup_artifact_io_cannot_hold_registry_transaction_against_newer_stop() {
        use std::{io::Write, os::unix::fs::OpenOptionsExt};
        let (home, owners) = storage_wake_fixture().await;
        let (storage, receipt, artifact) = owners;
        let store = storage.work_items();
        let allowed = store.claim_owned_flow_quota_wake(receipt.run, 100).unwrap();
        assert!(matches!(allowed, OwnedFlowWakeAdmission::Ready(_)));
        drop(allowed);
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
        let bytes = std::fs::read(&artifact).unwrap();
        let saved = artifact.with_extension("original-retained");
        std::fs::rename(&artifact, &saved).unwrap();
        nix::unistd::mkfifo(
            &artifact,
            nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR,
        )
        .unwrap();
        let reading_store = store.clone();
        let reading =
            std::thread::spawn(move || reading_store.claim_owned_flow_quota_wake(receipt.run, 100));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        // A nonblocking writer can open only after the real filesystem reader arrived.
        let mut writer = loop {
            match std::fs::OpenOptions::new()
                .write(true)
                .custom_flags(nix::libc::O_NONBLOCK)
                .open(&artifact)
            {
                Ok(writer) => break writer,
                Err(error) if error.raw_os_error() == Some(nix::libc::ENXIO) => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "production preflight never reached actual artifact IO"
                    );
                    std::thread::sleep(std::time::Duration::from_millis(5));
                },
                Err(error) => panic!("fixture artifact writer failed: {error}"),
            }
        };
        let command = WorkItemCommand::Suspend {
            operation_id: surge_core::id::WorkItemOperationId::new(),
            item: receipt.item,
            expected_version: store.show(receipt.item).unwrap().item.version,
        };
        let stopping_store = store.clone();
        let (send, receive) = std::sync::mpsc::channel();
        let stopping = std::thread::spawn(move || {
            let result =
                stopping_store.mutate(&command, None, None, "stop-during-real-artifact-io", 200);
            send.send(result).unwrap();
        });
        // Release IO even if the assertion would fail, then join both actual operations.
        let committed_without_io = receive
            .recv_timeout(std::time::Duration::from_millis(250))
            .ok();
        writer.write_all(&bytes).unwrap();
        drop(writer);
        let admission = reading.join().unwrap().unwrap();
        stopping.join().unwrap();
        std::fs::remove_file(&artifact).unwrap();
        std::fs::rename(saved, artifact).unwrap();
        assert!(
            committed_without_io.is_some_and(|result| result.is_ok()),
            "original startup IO held a registry write transaction against operator Stop"
        );
        assert!(matches!(admission, OwnedFlowWakeAdmission::Obsolete));
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
        assert_eq!(current, token);
        assert_eq!(rows, 0);
        assert_eq!(
            store.execution_control(receipt.run).unwrap().unwrap().state,
            ControlState::SuspendRequested
        );
        drop(admission);
        drop(store);
        drop(storage);
        home.close().expect("close runtime home");
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn private_unavailable_wake_commits_attention_without_permanent_refusal_or_claim() {
        let (home, (storage, receipt, _)) = storage_wake_fixture_with_private(true).await;
        let facts = {
            let store = storage.work_items();
            let ready = store.claim_owned_flow_quota_wake(receipt.run, 100).unwrap();
            let healthy = matches!(ready, OwnedFlowWakeAdmission::Ready(_));
            drop(ready);
            let original = store.execution_control(receipt.run).unwrap().unwrap();
            let version = store.show(receipt.item).unwrap().item.version;
            let before = super::super::tests::operation_snapshot(&store, receipt.operation_id);
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
            std::fs::remove_file(
                home.path()
                    .join("work-items/private-flow-inputs/host-key-v1"),
            )
            .unwrap();
            let result = store.claim_owned_flow_quota_wake(receipt.run, 100).unwrap();
            let attempt = store.for_run(receipt.run).unwrap().unwrap();
            let mut expected = original;
            expected.state = ControlState::Attention;
            expected.diagnostic = Some("owned_flow_private_inputs_unavailable".into());
            let conn = store.pool.get().unwrap();
            let after_token: String = conn
                .query_row(
                    "SELECT claim_token FROM work_item_attempts WHERE run=?",
                    [receipt.run.to_string()],
                    |row| row.get(0),
                )
                .unwrap();
            let counts:(u64,u64,u64)=conn.query_row("SELECT (SELECT COUNT(*) FROM owned_flow_wake_refusals),(SELECT COUNT(*) FROM owned_flow_wake_refusal_outbox),(SELECT COUNT(*) FROM work_item_quota_cycles WHERE wake_at_ms IS NOT NULL)",[],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).unwrap();
            let registry: Option<i64> = conn
                .query_row(
                    "SELECT wake_at FROM runs WHERE id=?",
                    [receipt.run.to_string()],
                    |row| row.get(0),
                )
                .unwrap();
            drop(conn);
            (
                healthy,
                matches!(result, OwnedFlowWakeAdmission::PrivateInputsUnavailable),
                attempt.state == WorkItemAttemptState::Attention,
                store.execution_control(receipt.run).unwrap() == Some(expected),
                store.show(receipt.item).unwrap().item.version == version + 1,
                token == after_token,
                super::super::tests::operation_snapshot(&store, receipt.operation_id) == before,
                counts == (0, 0, 0) && registry.is_none(),
            )
        };
        drop(storage);
        home.close().unwrap();
        assert_eq!(facts, (true, true, true, true, true, true, true, true));
    }
    async fn private_wake_race_case(fault: &str) -> (bool, bool, bool) {
        let (home, (storage, receipt, _)) = storage_wake_fixture_with_private(true).await;
        let facts = {
            let store = storage.work_items();
            std::fs::remove_file(
                home.path()
                    .join("work-items/private-flow-inputs/host-key-v1"),
            )
            .unwrap();
            let gate = super::super::refusal_owner::ready_gate(receipt.run);
            let worker_store = store.clone();
            let worker = std::thread::spawn(move || {
                worker_store.claim_owned_flow_quota_wake(receipt.run, 100)
            });
            gate.wait_entered();
            match fault {
                "item-version" => {
                    store
                        .pool
                        .get()
                        .unwrap()
                        .execute(
                            "UPDATE work_items SET version=version+1 WHERE id=?",
                            [receipt.item.to_string()],
                        )
                        .unwrap();
                },
                "cycle-revision" => {
                    store
                        .pool
                        .get()
                        .unwrap()
                        .execute(
                            "UPDATE work_item_quota_cycles SET revision=revision+1 WHERE run=?",
                            [receipt.run.to_string()],
                        )
                        .unwrap();
                },
                "cycle-closed" => {
                    store
                        .pool
                        .get()
                        .unwrap()
                        .execute(
                            "UPDATE work_item_quota_cycles SET closed=1 WHERE run=?",
                            [receipt.run.to_string()],
                        )
                        .unwrap();
                },
                "registry-wake" => storage.set_run_parked(&receipt.run, 200).await.unwrap(),
                _ => unreachable!(),
            }
            let before = crate::work_items::reconciliation_tests::snapshot(&store);
            let registry =
                private_unavailable::registry(&store.pool.get().unwrap(), receipt.run).unwrap();
            gate.release();
            let result = worker.join().unwrap();
            (
                matches!(result, Ok(OwnedFlowWakeAdmission::Obsolete)),
                crate::work_items::reconciliation_tests::snapshot(&store) == before,
                private_unavailable::registry(&store.pool.get().unwrap(), receipt.run).unwrap()
                    == registry,
            )
        };
        drop(storage);
        home.close().unwrap();
        facts
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn private_unavailable_wake_never_overwrites_changed_item_cycle_or_registry_wake() {
        let mut outcomes = Vec::new();
        for fault in [
            "item-version",
            "cycle-revision",
            "cycle-closed",
            "registry-wake",
        ] {
            let facts = private_wake_race_case(fault).await;
            outcomes.push((fault, facts));
        }
        assert!(
            outcomes
                .iter()
                .all(|(_, facts)| *facts == (true, true, true)),
            "{outcomes:?}"
        );
    }
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn private_key_lost_after_hydration_before_ready_commits_attention_without_claim() {
        let (home, (storage, receipt, _)) = storage_wake_fixture_with_private(true).await;
        let facts = {
            let store = storage.work_items();
            let token_before: String = store
                .pool
                .get()
                .unwrap()
                .query_row(
                    "SELECT claim_token FROM work_item_attempts WHERE run=?",
                    [receipt.run.to_string()],
                    |row| row.get(0),
                )
                .unwrap();
            let gate = super::super::refusal_owner::ready_gate(receipt.run);
            let worker_store = store.clone();
            let worker = std::thread::spawn(move || {
                worker_store.claim_owned_flow_quota_wake(receipt.run, 100)
            });
            gate.wait_entered();
            std::fs::remove_file(
                home.path()
                    .join("work-items/private-flow-inputs/host-key-v1"),
            )
            .unwrap();
            gate.release();
            let result = worker.join().unwrap();
            let attempt = store.for_run(receipt.run).unwrap().unwrap();
            let token_after: String = store
                .pool
                .get()
                .unwrap()
                .query_row(
                    "SELECT claim_token FROM work_item_attempts WHERE run=?",
                    [receipt.run.to_string()],
                    |row| row.get(0),
                )
                .unwrap();
            let no_claim = token_before == token_after;
            (
                matches!(result, Ok(OwnedFlowWakeAdmission::PrivateInputsUnavailable)),
                attempt.state == WorkItemAttemptState::Attention,
                store.execution_control(receipt.run).unwrap().unwrap().state
                    == ControlState::Attention,
                no_claim,
                format!("state={:?}", attempt.state),
            )
        };
        drop(storage);
        home.close().unwrap();
        assert!(facts.0 && facts.1 && facts.2 && facts.3, "{facts:?}");
    }
}
