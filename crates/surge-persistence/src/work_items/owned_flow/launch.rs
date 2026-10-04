//! Real stable launch ownership and host-only runtime hydration capability.
use super::{OwnedFlowSourceSnapshot, private_inputs};
use crate::work_items::start_preparation::secure_lock::{PreparationLock, PreparationLockKey};
use crate::work_items::{
    self, LaunchLock, Result, WorkItemError, WorkItemLaunchClaim, WorkItemStore,
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use std::{path::Path, sync::Arc};
use surge_core::{RunId, id::WorkItemOperationId, mcp_config::McpServerRef, work_item::*};

/// Host-only verified effective inputs. No Deserialize or public constructor exists.
#[derive(Clone)]
pub struct AuthenticatedOwnedFlowInputs {
    manifest: OwnedFlowInputsManifest,
    servers: Vec<McpServerRef>,
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
impl WorkItemStore {
    pub(in crate::work_items) fn claim_owned_flow(
        &self,
        run: RunId,
    ) -> Result<WorkItemLaunchClaim> {
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
        let mut conn = self.pool.get()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = stored(&tx, run)?;
        if current.operation != value.operation || current.identity != value.identity {
            return Err(WorkItemError::Conflict(
                "owned Flow launch ownership changed".into(),
            ));
        }
        let attempt = association(&tx, &current, run)?;
        if current.state == "pending" {
            pending_permission(&tx, &current, run)?;
        } else if current.state != "started" || !attempt.state.is_active() {
            return Err(WorkItemError::Conflict(
                "owned Flow launch is inactive".into(),
            ));
        }
        guard.verify()?;
        let token = RunId::new().to_string();
        tx.execute(
            "UPDATE work_item_attempts SET claim_token=? WHERE run=? AND generation=?",
            params![token, run.to_string(), attempt.binding.generation],
        )?;
        tx.commit()?;
        Ok(WorkItemLaunchClaim {
            lock: LaunchLock::OwnedFlow {
                guard: Arc::new(guard),
                operation: value.operation,
                identity: value.identity,
            },
            run,
            token,
            binding: attempt.binding,
        })
    }
    pub(in crate::work_items) fn validate_owned_flow_association(
        &self,
        conn: &Connection,
        claim: &WorkItemLaunchClaim,
    ) -> Result<()> {
        let LaunchLock::OwnedFlow {
            operation,
            identity,
            ..
        } = &claim.lock
        else {
            return Err(WorkItemError::Conflict(
                "owned Flow requires stable launch ownership".into(),
            ));
        };
        let value = stored(conn, claim.run)?;
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
        claim.verify_lock()?;
        let mut conn = self.pool.get()?;
        let tx = conn.transaction()?;
        self.validate_claim_in(&tx, claim)?;
        pending_permission(&tx, &stored(&tx, claim.run)?, claim.run)?;
        claim.verify_lock()?;
        tx.commit()?;
        Ok(())
    }
    /// Recheck owned Flow intent and the latest generation control at an actual effect.
    /// Legacy task claims keep their existing execution-control path.
    pub fn validate_owned_flow_effect(&self, claim: &WorkItemLaunchClaim) -> Result<()> {
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
        claim.verify_lock()?;
        tx.commit()?;
        Ok(())
    }
    /// Whether this exact Reserved attempt has a real uncanceled durable launch intent.
    pub fn has_pending_owned_flow(&self, run: RunId) -> Result<bool> {
        let conn = self.pool.get()?;
        let exists:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM owned_flow_operations WHERE run=? AND state='accepted' AND intent_state='pending')",[run.to_string()],|r|r.get(0))?;
        if !exists {
            return Ok(false);
        }
        Ok(pending_permission(&conn, &stored(&conn, run)?, run).is_ok())
    }
    /// Authenticated public manifest for exact journal comparison; no effect capability.
    pub fn owned_flow_manifest(&self, run: RunId) -> Result<Option<OwnedFlowInputsManifest>> {
        let conn = self.pool.get()?;
        let exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM owned_flow_operations WHERE run=? AND state='accepted')",
            [run.to_string()],
            |r| r.get(0),
        )?;
        if exists {
            Ok(Some(stored(&conn, run)?.inputs))
        } else {
            Ok(None)
        }
    }
    /// Hydrate only under a current real stable claim and accepted immutable association.
    pub fn hydrate_owned_flow_inputs(
        &self,
        claim: &WorkItemLaunchClaim,
    ) -> Result<Option<AuthenticatedOwnedFlowInputs>> {
        if !matches!(claim.lock, LaunchLock::OwnedFlow { .. }) {
            return Ok(None);
        }
        self.validate_claim(claim)?;
        let value = stored(&*self.pool.get()?, claim.run)?;
        let servers = private_inputs::hydrate(&self.home, &value.inputs)?;
        self.validate_claim(claim)?;
        let current = stored(&*self.pool.get()?, claim.run)?;
        if current.inputs != value.inputs {
            return Err(WorkItemError::PrivateInputsCorrupt);
        }
        Ok(Some(AuthenticatedOwnedFlowInputs {
            manifest: value.inputs,
            servers,
        }))
    }
    /// Mark startup after the owning host has verified the actual unique journal manifest.
    pub fn acknowledge_owned_flow_startup(
        &self,
        claim: &WorkItemLaunchClaim,
        manifest: &OwnedFlowInputsManifest,
    ) -> Result<()> {
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
        claim.verify_lock()?;
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
