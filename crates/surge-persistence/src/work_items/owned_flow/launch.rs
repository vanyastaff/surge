//! Real stable launch ownership and host-only runtime hydration capability.
use super::{OwnedFlowSourceSnapshot, private_inputs, private_unavailable, refusal};
use crate::work_items::start_preparation::secure_lock::{PreparationLock, PreparationLockKey};
use crate::work_items::{
    self, LaunchLock, ReconciliationObservation, Result, WorkItemError, WorkItemLaunchClaim,
    WorkItemStore,
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use std::{path::Path, sync::Arc};
use surge_core::{RunId, id::WorkItemOperationId, mcp_config::McpServerRef, work_item::*};

/// Host-only verified effective inputs. No Deserialize or public constructor exists.
#[derive(Clone)]
pub struct AuthenticatedOwnedFlowInputs {
    manifest: OwnedFlowInputsManifest,
    servers: Vec<McpServerRef>,
    private_owner: Option<Arc<super::PrivateOwner>>,
}
impl std::fmt::Debug for AuthenticatedOwnedFlowInputs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthenticatedOwnedFlowInputs")
            .field("run", &self.manifest.run())
            .field("server_count", &self.servers.len())
            .finish()
    }
}
impl AuthenticatedOwnedFlowInputs {
    /// Frozen public startup projection; this projection alone cannot hydrate inputs.
    #[must_use]
    pub fn manifest(&self) -> &OwnedFlowInputsManifest {
        &self.manifest
    }
    /// Exact effective servers, after host registry and private authentication checks.
    #[must_use]
    pub fn servers(&self) -> &[McpServerRef] {
        &self.servers
    }
    /// Require the real run, accepted binding and retained workspace at runtime use.
    pub fn validate_for(
        &self,
        run: RunId,
        binding: &WorkItemBinding,
        workspace: &Path,
    ) -> Result<()> {
        if let Some(owner) = &self.private_owner {
            owner.verify()?;
        }
        if self.manifest.run() != run
            || self.manifest.binding() != binding
            || self.manifest.workspace().path != workspace
        {
            return Err(WorkItemError::PrivateInputsCorrupt);
        }
        Ok(())
    }
}
/// Independent accepted ownership loader deliberately does not decode the manifest.
pub(super) struct AcceptedLaunch {
    pub(super) operation: WorkItemOperationId,
    pub(super) identity: String,
    pub(super) source: OwnedFlowSourceSnapshot,
    pub(super) config: String,
    pub(super) manifest: String,
    pub(super) receipt: OwnedFlowReceipt,
    pub(super) state: String,
}
pub(super) fn accepted_launch(conn: &Connection, run: RunId) -> Result<AcceptedLaunch> {
    conn.query_row("SELECT operation,launch_lock_identity,source_snapshot,config_snapshot,inputs_manifest,receipt,intent_state FROM owned_flow_operations WHERE run=? AND state='accepted'",[run.to_string()],|r| {
        let operation: String = r.get(0)?;
        Ok(AcceptedLaunch { operation:operation.parse().map_err(|e|rusqlite::Error::FromSqlConversionFailure(0,rusqlite::types::Type::Text,Box::new(e)))?,identity:r.get(1)?,source:work_items::json(r.get(2)?)?,config:r.get(3)?,manifest:r.get(4)?,receipt:work_items::json(r.get(5)?)?,state:r.get(6)? })
    }).map_err(WorkItemError::Sql)
}
/// Immutable accepted facts, before any current permission or manifest comparison.
pub(super) fn independent_binding(
    conn: &Connection,
    value: &AcceptedLaunch,
    run: RunId,
) -> Result<WorkItemAttempt> {
    let attempt = work_items::attempt(conn, run)?;
    let accepted = work_items::revision(conn, attempt.item, attempt.binding.revision)?;
    if accepted.origin.flow() != Some(&value.source.contract)
        || accepted.hash != attempt.binding.requirements_hash
        || attempt.graph.as_ref() != value.source.contract.graph()
        || attempt.config != value.config
        || value.receipt.run != run
        || value.receipt.operation_id != value.operation
        || value.receipt.item != attempt.item
        || value.receipt.binding != attempt.binding
        || value.source.workspace.ownership != value.receipt.workspace_owner.to_string()
    {
        return Err(WorkItemError::IndependentAcceptedDataInvalid);
    }
    Ok(attempt)
}
#[derive(PartialEq)]
struct StoredLaunch {
    operation: WorkItemOperationId,
    identity: String,
    source: OwnedFlowSourceSnapshot,
    config: String,
    inputs: OwnedFlowInputsManifest,
    receipt: OwnedFlowReceipt,
    state: String,
}
fn stored(conn: &Connection, run: RunId) -> Result<StoredLaunch> {
    conn.query_row("SELECT operation,launch_lock_identity,source_snapshot,config_snapshot,inputs_manifest,receipt,intent_state FROM owned_flow_operations WHERE run=? AND state='accepted'",[run.to_string()],|r| {
        let operation:String=r.get(0)?;
        Ok(StoredLaunch { operation:operation.parse().map_err(|e|rusqlite::Error::FromSqlConversionFailure(0,rusqlite::types::Type::Text,Box::new(e)))?,identity:r.get(1)?,source:work_items::json(r.get(2)?)?,config:r.get(3)?,inputs:work_items::json(r.get(4)?)?,receipt:work_items::json(r.get(5)?)?,state:r.get(6)? })
    }).map_err(|error|match error { rusqlite::Error::QueryReturnedNoRows=>WorkItemError::Conflict("owned Flow launch binding missing".into()),error=>WorkItemError::Sql(error) })
}
fn association(conn: &Connection, value: &StoredLaunch, run: RunId) -> Result<WorkItemAttempt> {
    let attempt = work_items::attempt(conn, run)?;
    let item = work_items::record(conn, attempt.item)?;
    let accepted = work_items::revision(conn, attempt.item, attempt.binding.revision)?;
    let valid = accepted
        .origin
        .flow()
        .is_some_and(|flow| flow == &value.source.contract);
    if !valid
        || item.archived_at_ms.is_some()
        || item.active_run != Some(run)
        || item.generation != attempt.binding.generation
        || item.accepted_revision != attempt.binding.revision
        || accepted.hash != attempt.binding.requirements_hash
        || item.workspace != value.source.workspace
        || attempt.graph.as_ref() != value.source.contract.graph()
        || attempt.config != value.config
        || value.receipt.run != run
        || value.receipt.operation_id != value.operation
        || value.receipt.item != attempt.item
        || value.receipt.binding != attempt.binding
        || value.inputs.operation() != value.operation
        || value.inputs.run() != run
        || value.inputs.binding() != &attempt.binding
        || value.inputs.workspace() != &item.workspace
        || value.inputs.workspace_owner() != value.receipt.workspace_owner
        || value.state == "canceled"
    {
        return Err(WorkItemError::Conflict(
            "owned Flow accepted association changed".into(),
        ));
    }
    Ok(attempt)
}
fn pending_permission(conn: &Connection, value: &StoredLaunch, run: RunId) -> Result<()> {
    let attempt = association(conn, value, run)?;
    if value.state != "pending"
        || attempt.state != WorkItemAttemptState::Reserved
        || work_items::control::read_control(conn, run, None)?.is_some()
    {
        return Err(WorkItemError::Conflict(
            "owned Flow launch intent is not dispatchable".into(),
        ));
    }
    Ok(())
}
fn confirmed_continuation(
    conn: &Connection,
    control: &surge_core::execution_recovery::WorkItemExecutionControl,
) -> Result<bool> {
    use surge_core::execution_recovery::ExecutionControlState;
    let Some(fence) = &control.fence else {
        return Ok(false);
    };
    if !fence.cleanup_confirmed
        || fence.control_generation.checked_add(1) != Some(control.generation)
    {
        return Ok(false);
    }
    let Some(prior) =
        work_items::control::read_control(conn, control.run, Some(fence.control_generation))?
    else {
        return Ok(false);
    };
    Ok(prior.item == control.item
        && prior.run == control.run
        && prior.attempt_generation == control.attempt_generation
        && prior.generation == fence.control_generation
        && prior.state == ExecutionControlState::Suspended
        && prior.fence.as_ref() == Some(fence))
}
#[cfg(all(test, unix))]
static CLAIM_GATES: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<RunId, Arc<super::refusal_owner::ReadyGate>>>,
> = std::sync::OnceLock::new();
#[cfg(all(test, unix))]
pub(super) fn claim_gate(run: RunId) -> Arc<super::refusal_owner::ReadyGate> {
    let gate = Arc::new(super::refusal_owner::ReadyGate::default());
    CLAIM_GATES
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .insert(run, gate.clone());
    gate
}
#[cfg(all(test, unix))]
fn pause_after_hydration(run: RunId) {
    let gate = CLAIM_GATES
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .remove(&run);
    if let Some(gate) = gate {
        gate.enter();
    }
}
fn preclaim_snapshot(
    store: &WorkItemStore,
    value: &StoredLaunch,
    run: RunId,
    observation: Option<ReconciliationObservation<'_>>,
) -> Result<Option<(u64, private_unavailable::Wakes)>> {
    let conn = store.pool.get()?;
    if let Some(observed) = observation
        && !observed.matches(&conn, &work_items::attempt(&conn, run)?)?
    {
        return Ok(None);
    }
    let attempt = association(&conn, value, run)?;
    Ok(Some((
        work_items::record(&conn, attempt.item)?.version,
        private_unavailable::Wakes::read(&conn, run)?,
    )))
}
fn settle_private_unavailable(
    store: &WorkItemStore,
    observed: &StoredLaunch,
    guard: Arc<PreparationLock>,
    observation: Option<ReconciliationObservation<'_>>,
    snapshot: (u64, private_unavailable::Wakes),
    error: WorkItemError,
) -> Result<Option<WorkItemLaunchClaim>> {
    let Some(observation) = observation else {
        return Err(error);
    };
    let run = observation.attempt.run;
    let prepared = refusal::reserve_refusal(store, guard.clone(), run)?;
    let mut conn = store.pool.get()?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let attempt = work_items::attempt(&tx, run)?;
    if !observation.matches(&tx, &attempt)?
        || work_items::record(&tx, attempt.item)?.version != snapshot.0
        || private_unavailable::Wakes::read(&tx, run)? != snapshot.1
    {
        return Ok(None);
    }
    let current = stored(&tx, run)?;
    if &current != observed {
        return Err(WorkItemError::IndependentAcceptedDataInvalid);
    }
    association(&tx, &current, run)?;
    guard.verify()?;
    private_unavailable::attention(
        &tx,
        &attempt,
        observation.control,
        snapshot.0,
        "owned_flow_private_inputs_unavailable",
    )?;
    snapshot.1.disarm(&tx, run)?;
    guard.verify()?;
    tx.commit()?;
    guard.verify()?;
    drop(prepared);
    Err(error)
}
impl WorkItemStore {
    pub(in crate::work_items) fn claim_owned_flow_matching(
        &self,
        run: RunId,
        observation: Option<ReconciliationObservation<'_>>,
    ) -> Result<Option<WorkItemLaunchClaim>> {
        super::purpose::refuse_execution(self, run)?;
        let value = stored(&*self.pool.get()?, run)?;
        let guard = PreparationLock::acquire_stable(
            &self.home,
            PreparationLockKey::FlowLaunch {
                operation: value.operation,
                run,
            },
        )?;
        // Compare the original object before changing any claimant token.
        if guard.identity()? != value.identity {
            return Err(WorkItemError::Conflict(
                "owned Flow launch object replaced".into(),
            ));
        }
        let Some(snapshot) = preclaim_snapshot(self, &value, run, observation)? else {
            return Ok(None);
        };
        let private_owner = match private_inputs::hydrate(self, &value.inputs) {
            Ok((_, owner)) => owner,
            Err(error) if private_unavailable::known(&error) && observation.is_some() => {
                return settle_private_unavailable(
                    self,
                    &value,
                    Arc::new(guard),
                    observation,
                    snapshot,
                    error,
                );
            },
            Err(error) => return Err(error),
        };
        #[cfg(all(test, unix))]
        if observation.is_some() {
            pause_after_hydration(run);
        }
        let mut conn = self.pool.get()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = stored(&tx, run)?;
        if current != value {
            return Err(WorkItemError::Conflict(
                "owned Flow launch ownership changed".into(),
            ));
        }
        let attempt = association(&tx, &current, run)?;
        if let Some(observation) = observation
            && !observation.matches(&tx, &attempt)?
        {
            return Ok(None);
        }
        if current.state == "pending" {
            pending_permission(&tx, &current, run)?;
        } else if current.state != "started" || !attempt.state.is_active() {
            return Err(WorkItemError::Conflict(
                "owned Flow launch is inactive".into(),
            ));
        }
        guard.verify()?;
        if let Some(owner) = &private_owner
            && let Err(error) = owner.verify_in(&tx)
        {
            tx.rollback()?;
            drop(conn);
            if observation.is_some() && private_unavailable::known(&error) {
                return settle_private_unavailable(
                    self,
                    &value,
                    Arc::new(guard),
                    observation,
                    snapshot,
                    error,
                );
            }
            return Err(error);
        }
        let token = RunId::new().to_string();
        tx.execute(
            "UPDATE work_item_attempts SET claim_token=? WHERE run=? AND generation=?",
            params![token, run.to_string(), attempt.binding.generation],
        )?;
        tx.commit()?;
        Ok(Some(WorkItemLaunchClaim {
            lock: LaunchLock::OwnedFlow {
                guard: Arc::new(guard),
                operation: value.operation,
                identity: value.identity,
                private_owner,
            },
            run,
            token,
            binding: attempt.binding,
        }))
    }
    pub(in crate::work_items) fn validate_owned_flow_association(
        &self,
        conn: &Connection,
        claim: &WorkItemLaunchClaim,
    ) -> Result<()> {
        let LaunchLock::OwnedFlow {
            operation,
            identity,
            private_owner,
            ..
        } = &claim.lock
        else {
            return Err(WorkItemError::Conflict(
                "owned Flow requires stable launch ownership".into(),
            ));
        };
        super::purpose::refuse_execution_in(conn, claim.run, private_owner.as_ref())?;
        let value = stored(conn, claim.run)?;
        if matches!(value.inputs.mcp(), FrozenOwnedFlowMcp::Private { .. })
            || matches!(
                value.inputs.request_identity(),
                OwnedFlowRequestIdentity::Hmac { .. }
            )
        {
            let owner = private_owner
                .as_ref()
                .ok_or(WorkItemError::PrivateInputsRecoveryRequired)?;
            owner.verify_in(conn)?;
        }
        if *operation != value.operation
            || *identity != value.identity
            || association(conn, &value, claim.run)?.binding != claim.binding
        {
            return Err(WorkItemError::Conflict(
                "owned Flow stable association changed".into(),
            ));
        }
        Ok(())
    }
    /// Effect-time permission for a genuinely pending owned ordinary Flow.
    pub fn validate_owned_flow_pending(&self, claim: &WorkItemLaunchClaim) -> Result<()> {
        super::purpose::refuse_execution(self, claim.run)?;
        claim.verify_lock()?;
        let mut conn = self.pool.get()?;
        let tx = conn.transaction()?;
        self.validate_claim_in(&tx, claim)?;
        pending_permission(&tx, &stored(&tx, claim.run)?, claim.run)?;
        claim.verify_lock_in(&tx)?;
        tx.commit()?;
        Ok(())
    }
    /// Recheck owned Flow intent and the latest generation control at an actual effect.
    /// Legacy task claims keep their existing execution-control path.
    pub fn validate_owned_flow_effect(&self, claim: &WorkItemLaunchClaim) -> Result<()> {
        super::purpose::refuse_execution(self, claim.run)?;
        claim.verify_lock()?;
        let mut conn = self.pool.get()?;
        let tx = conn.transaction()?;
        let attempt = self.validate_claim_in(&tx, claim)?;
        if matches!(claim.lock, LaunchLock::OwnedFlow { .. }) {
            let value = stored(&tx, claim.run)?;
            if value.state != "started" {
                return Err(WorkItemError::Conflict(
                    "owned Flow startup intent is not acknowledged".into(),
                ));
            }
            let control = work_items::control::read_control(&tx, claim.run, None)?;
            let allowed = match control {
                None => matches!(
                    attempt.state,
                    WorkItemAttemptState::Reserved | WorkItemAttemptState::Launched
                ),
                Some(control)
                    if control.item == claim.binding.item
                        && control.run == claim.run
                        && control.attempt_generation == claim.binding.generation =>
                {
                    match control.state {
                        surge_core::execution_recovery::ExecutionControlState::Executing => {
                            matches!(
                                attempt.state,
                                WorkItemAttemptState::Reserved | WorkItemAttemptState::Launched
                            )
                        },
                        surge_core::execution_recovery::ExecutionControlState::ContinueReserved => {
                            confirmed_continuation(&tx, &control)?
                        },
                        _ => false,
                    }
                },
                Some(_) => false,
            };
            if !allowed {
                return Err(WorkItemError::Conflict(
                    "owned Flow current control forbids effects".into(),
                ));
            }
        }
        claim.verify_lock_in(&tx)?;
        tx.commit()?;
        Ok(())
    }
    /// Whether this exact Reserved attempt has a real uncanceled durable launch intent.
    pub fn has_pending_owned_flow(&self, run: RunId) -> Result<bool> {
        super::purpose::refuse_execution(self, run)?;
        let conn = self.pool.get()?;
        let exists:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM owned_flow_operations WHERE run=? AND state='accepted' AND intent_state='pending')",[run.to_string()],|r|r.get(0))?;
        if !exists {
            return Ok(false);
        }
        Ok(pending_permission(&conn, &stored(&conn, run)?, run).is_ok())
    }
    /// Authenticated public manifest for exact journal comparison; no effect capability.
    pub fn owned_flow_manifest(&self, run: RunId) -> Result<Option<OwnedFlowInputsManifest>> {
        super::purpose::refuse_execution(self, run)?;
        let conn = self.pool.get()?;
        let exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM owned_flow_operations WHERE run=? AND state='accepted')",
            [run.to_string()],
            |r| r.get(0),
        )?;
        if exists {
            let inputs = stored(&conn, run)?.inputs;
            drop(conn);
            let (_, owner) = private_inputs::hydrate(self, &inputs)?;
            if let Some(owner) = owner {
                owner.verify()?;
            }
            Ok(Some(inputs))
        } else {
            Ok(None)
        }
    }
    /// Hydrate only under a current real stable claim and accepted immutable association.
    pub fn hydrate_owned_flow_inputs(
        &self,
        claim: &WorkItemLaunchClaim,
    ) -> Result<Option<AuthenticatedOwnedFlowInputs>> {
        super::purpose::refuse_execution(self, claim.run)?;
        if !matches!(claim.lock, LaunchLock::OwnedFlow { .. }) {
            return Ok(None);
        }
        self.validate_claim(claim)?;
        let value = stored(&*self.pool.get()?, claim.run)?;
        let (servers, private_owner) = private_inputs::hydrate(self, &value.inputs)?;
        self.validate_claim(claim)?;
        let current = stored(&*self.pool.get()?, claim.run)?;
        if current.inputs != value.inputs {
            return Err(WorkItemError::PrivateInputsCorrupt);
        }
        Ok(Some(AuthenticatedOwnedFlowInputs {
            manifest: value.inputs,
            servers,
            private_owner,
        }))
    }
    /// Mark startup after the owning host has verified the actual unique journal manifest.
    pub fn acknowledge_owned_flow_startup(
        &self,
        claim: &WorkItemLaunchClaim,
        manifest: &OwnedFlowInputsManifest,
    ) -> Result<()> {
        super::purpose::refuse_execution(self, claim.run)?;
        claim.verify_lock()?;
        let mut conn = self.pool.get()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        self.validate_claim_in(&tx, claim)?;
        let value = stored(&tx, claim.run)?;
        if &value.inputs != manifest {
            return Err(WorkItemError::PrivateInputsCorrupt);
        }
        pending_permission(&tx, &value, claim.run)?;
        tx.execute("UPDATE owned_flow_operations SET intent_state='started' WHERE run=? AND intent_state='pending'",[claim.run.to_string()])?;
        claim.verify_lock_in(&tx)?;
        tx.commit()?;
        Ok(())
    }
    /// Cancel a queued owned intent even when the process-local queue is empty.
    pub fn cancel_pending_owned_flow(&self, run: RunId) -> Result<bool> {
        let mut conn = self.pool.get()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let state: Option<String> = tx
            .query_row(
                "SELECT intent_state FROM owned_flow_operations WHERE run=? AND state='accepted'",
                [run.to_string()],
                |r| r.get(0),
            )
            .optional()?;
        if state.as_deref() != Some("pending") {
            return Ok(false);
        }
        tx.execute("UPDATE owned_flow_operations SET intent_state='canceled' WHERE run=? AND intent_state='pending'",[run.to_string()])?;
        let value = work_items::attempt(&tx, run)?;
        if value.state == WorkItemAttemptState::Reserved {
            tx.execute("UPDATE work_item_attempts SET state='aborted',claim_token=NULL WHERE run=? AND generation=?",params![run.to_string(),value.binding.generation])?;
            tx.execute("UPDATE work_items SET active_run=NULL,version=version+1 WHERE id=? AND active_run=? AND generation=?",params![value.item.to_string(),run.to_string(),value.binding.generation])?;
        }
        tx.commit()?;
        Ok(true)
    }
}
