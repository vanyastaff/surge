//! Internal stop receipts preserve their distinct quota/cleanup purpose.
use super::*;
use surge_core::execution_recovery::{ExecutionControlState, WorkItemExecutionControl};

/// Historical purpose of an internal stop; inspection is not wake authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapacityStopKind {
    /// Every policy candidate was considered; a separately validated wake may exist.
    CandidatesExhausted,
    /// The selected provider failed, but its complete writer cleanup is unknown.
    CleanupUnknown,
}
impl CapacityStopKind {
    fn tag(self) -> &'static str {
        match self {
            Self::CandidatesExhausted => "quota_capacity_suspend_v1",
            Self::CleanupUnknown => "quota_cleanup_stop_v1",
        }
    }
}
fn intent_hash(
    kind: CapacityStopKind,
    association: &CapacityControlAssociation,
) -> Result<ContentHash> {
    let mut original = association.clone();
    original.sequence = 0;
    let bytes = serde_json::to_vec(&serde_json::json!({
        "internal":kind.tag(),"association":original,
    }))?;
    Ok(ContentHash::compute(&bytes))
}

impl WorkItemStore {
    /// Decode an immutable internal operation purpose, never a caller boolean.
    pub fn capacity_stop_kind(
        &self,
        association: &CapacityControlAssociation,
    ) -> Result<CapacityStopKind> {
        let connection = self.pool.get()?;
        read_stop_kind(&connection, association)
    }
}
pub(crate) fn read_stop_kind(
    conn: &Connection,
    association: &CapacityControlAssociation,
) -> Result<CapacityStopKind> {
    let target = control::read_control(conn, association.run, Some(association.target_control))?
        .ok_or(WorkItemError::NotFound)?;
    let attempt = attempt(conn, association.run)?;
    if target.item != attempt.item
        || target.attempt_generation != attempt.binding.generation
        || target.run != association.run
        || target.generation != association.target_control
        || association.source_control.checked_add(1) != Some(association.target_control)
    {
        return Err(WorkItemError::Invalid(
            "stop target assignment differs".into(),
        ));
    }
    let (hash, result): (String, String) = conn.query_row(
        "SELECT body_hash,result FROM work_item_operations WHERE operation=?",
        [target.operation.to_string()],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let result: OperationResult = serde_json::from_str(&result)?;
    if !matches!(result,OperationResult::Control {run,generation} if run==target.run && generation==target.generation)
    {
        return Err(WorkItemError::Invalid(
            "stop operation result differs".into(),
        ));
    }
    for kind in [
        CapacityStopKind::CandidatesExhausted,
        CapacityStopKind::CleanupUnknown,
    ] {
        if hash == intent_hash(kind, association)?.to_string() {
            if kind == CapacityStopKind::CleanupUnknown && association.wake_identity.is_some() {
                return Err(WorkItemError::Invalid(
                    "cleanup-unknown stop cannot own a wake".into(),
                ));
            }
            return Ok(kind);
        }
    }
    Err(WorkItemError::Invalid(
        "stop operation has no recognized internal purpose".into(),
    ))
}

pub(crate) fn insert_capacity_intent(
    tx: &Transaction<'_>,
    claim: &WorkItemLaunchClaim,
    current: &RecoveryCycle,
    reservation: &str,
    kind: CapacityStopKind,
) -> Result<CapacityControlAssociation> {
    let generation = current
        .control_generation
        .checked_add(1)
        .filter(|value| *value <= i64::MAX as u64)
        .ok_or_else(|| WorkItemError::Conflict("capacity control generation exhausted".into()))?;
    let operation = WorkItemOperationId::new();
    let mut association = CapacityControlAssociation {
        sequence: 0,
        run: claim.run,
        invocation: current.invocation.clone(),
        cycle_generation: current.generation,
        source_revision: current.revision,
        source_control: current.control_generation,
        target_control: generation,
        reservation: reservation.into(),
        wake_identity: if kind == CapacityStopKind::CleanupUnknown {
            None
        } else {
            current.wake.as_ref().map(|wake| wake.identity().into())
        },
    };
    let body = serde_json::to_string(&association)?;
    tx.execute(
        "INSERT INTO work_item_operations(operation,body_hash,result) VALUES(?,?,?)",
        params![
            operation.to_string(),
            intent_hash(kind, &association)?.to_string(),
            serde_json::to_string(&OperationResult::Control {
                run: claim.run,
                generation
            })?
        ],
    )?;
    let control = WorkItemExecutionControl {
        item: claim.binding.item,
        run: claim.run,
        attempt_generation: claim.binding.generation,
        generation,
        operation,
        state: ExecutionControlState::SuspendRequested,
        fence: None,
        allow_new_session: false,
        diagnostic: if kind == CapacityStopKind::CleanupUnknown {
            Some("provider quota rejection retained; complete writer cleanup is unconfirmed".into())
        } else {
            None
        },
    };
    tx.execute("INSERT INTO work_item_execution_controls(run,generation,item,attempt_generation,operation,state,payload) VALUES(?,?,?,?,?,'suspend_requested',?)",
        params![claim.run.to_string(),generation,claim.binding.item.to_string(),claim.binding.generation,operation.to_string(),serde_json::to_string(&control)?])?;
    tx.execute("INSERT INTO work_item_capacity_controls(run,item,attempt_generation,invocation,cycle_generation,source_revision,source_control,target_control,reservation,wake_identity,body) VALUES(?,?,?,?,?,?,?,?,?,?,?)",
        params![claim.run.to_string(),claim.binding.item.to_string(),claim.binding.generation,current.invocation,current.generation,current.revision,current.control_generation,generation,reservation,association.wake_identity,body])?;
    association.sequence = u64::try_from(tx.last_insert_rowid())
        .map_err(|_| WorkItemError::Invalid("invalid capacity association cursor".into()))?;
    tx.execute(
        "UPDATE work_items SET version=version+1 WHERE id=? AND active_run=? AND generation=?",
        params![
            claim.binding.item.to_string(),
            claim.run.to_string(),
            claim.binding.generation
        ],
    )?;
    if kind == CapacityStopKind::CleanupUnknown {
        // Retain existing wake JSON only as audit. No timer can act on this halt.
        tx.execute("UPDATE work_item_quota_cycles SET wake_at_ms=NULL WHERE run=? AND invocation=? AND cycle_generation=?",
            params![claim.run.to_string(),current.invocation,current.generation])?;
    }
    Ok(association)
}
