//! Registry control intents and generation-fenced acknowledgements.
use super::*;
use surge_core::execution_recovery::{
    ExecutionControlState as State, SuspensionFence, WorkItemExecutionControl as Control,
};

/// Whether an observed generic-recovery result still belongs to current authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconciliationSettlement {
    /// The unchanged eligible authority received the requested result.
    Settled,
    /// Authority changed or requires explicit action; no record was written.
    Superseded,
}

/// Closed set of positive projections admitted by generic reconciliation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconciliationProjection {
    /// The current journal or retained engine owner establishes a launched attempt.
    Launched,
    /// The inspected journal establishes successful completion.
    Completed,
    /// The inspected journal establishes failure.
    Failed,
    /// The inspected journal establishes an aborted run.
    Aborted,
}

impl From<ReconciliationProjection> for WorkItemAttemptState {
    fn from(projection: ReconciliationProjection) -> Self {
        match projection {
            ReconciliationProjection::Launched => Self::Launched,
            ReconciliationProjection::Completed => Self::Completed,
            ReconciliationProjection::Failed => Self::Failed,
            ReconciliationProjection::Aborted => Self::Aborted,
        }
    }
}

#[derive(Clone, Copy)]
enum ReconciliationUpdate<'a> {
    Error(&'a str),
    Projection(ReconciliationProjection),
}

impl ReconciliationUpdate<'_> {
    fn state(self) -> WorkItemAttemptState {
        match self {
            Self::Error(_) => WorkItemAttemptState::Attention,
            Self::Projection(projection) => projection.into(),
        }
    }
}

pub(super) fn read_control(
    conn: &Connection,
    run: RunId,
    generation: Option<u64>,
) -> Result<Option<Control>> {
    let payload = if let Some(generation) = generation {
        conn.query_row(
            "SELECT payload FROM work_item_execution_controls WHERE run=? AND generation=?",
            params![run.to_string(), generation],
            |row| row.get::<_, String>(0),
        )
        .optional()?
    } else {
        conn.query_row("SELECT payload FROM work_item_execution_controls WHERE run=? ORDER BY generation DESC LIMIT 1", [run.to_string()], |row|row.get::<_,String>(0)).optional()?
    };
    payload
        .map(|value| serde_json::from_str(&value).map_err(WorkItemError::from))
        .transpose()
}
pub(super) fn write_control(conn: &Connection, control: &Control) -> Result<()> {
    let state = serde_json::to_string(&control.state)?;
    conn.execute(
        "UPDATE work_item_execution_controls SET state=?,payload=? WHERE run=? AND generation=?",
        params![
            state.trim_matches('"'),
            serde_json::to_string(control)?,
            control.run.to_string(),
            control.generation
        ],
    )?;
    Ok(())
}
impl WorkItemStore {
    /// Conditionally settle a generic reconciliation failure under its original retained claim.
    ///
    /// The entire attempt projection and optional control must still match, including absence,
    /// operation, state, fence and diagnostic. A stale token/binding is an error; a newer state
    /// under the same claim is a benign no-write result, including terminal assignment release.
    pub fn settle_reconciliation_error(
        &self,
        claim: &WorkItemLaunchClaim,
        observed: &WorkItemAttempt,
        observed_control: Option<&Control>,
        diagnostic: &str,
    ) -> Result<ReconciliationSettlement> {
        self.settle_reconciliation_update(
            claim,
            observed,
            observed_control,
            ReconciliationUpdate::Error(diagnostic),
        )
    }

    /// Apply an inspected positive result only to the exact still-current generic authority.
    /// Terminal projections atomically release the assignment and advance the item version.
    pub fn settle_reconciliation_projection(
        &self,
        claim: &WorkItemLaunchClaim,
        observed: &WorkItemAttempt,
        observed_control: Option<&Control>,
        projection: ReconciliationProjection,
    ) -> Result<ReconciliationSettlement> {
        self.settle_reconciliation_update(
            claim,
            observed,
            observed_control,
            ReconciliationUpdate::Projection(projection),
        )
    }

    fn settle_reconciliation_update(
        &self,
        claim: &WorkItemLaunchClaim,
        observed: &WorkItemAttempt,
        observed_control: Option<&Control>,
        update: ReconciliationUpdate<'_>,
    ) -> Result<ReconciliationSettlement> {
        claim.verify_lock()?;
        let mut conn = self.pool.get()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = attempt(&tx, claim.run())?;
        self.validate_reconciliation_owner(&tx, claim, &current, observed)?;
        let control = read_control(&tx, claim.run(), None)?;
        for value in control.as_ref().into_iter().chain(observed_control) {
            if value.run != claim.run()
                || value.item != claim.binding().item
                || value.attempt_generation != claim.binding().generation
            {
                return Err(WorkItemError::Conflict(
                    "reconciliation control binding changed".into(),
                ));
            }
        }
        claim.verify_lock()?;
        if serde_json::to_value(&current)? != serde_json::to_value(observed)?
            || control.as_ref() != observed_control
            || !matches!(
                current.state,
                WorkItemAttemptState::Reserved | WorkItemAttemptState::Launched
            )
            || control.as_ref().is_some_and(|value| {
                !matches!(value.state, State::Executing | State::SuspendRequested)
            })
        {
            return Ok(ReconciliationSettlement::Superseded);
        }
        self.validate_claim_in(&tx, claim)?;
        write_reconciliation_result(&tx, claim, current, control, update)?;
        claim.verify_lock()?;
        tx.commit()?;
        Ok(ReconciliationSettlement::Settled)
    }

    fn validate_reconciliation_owner(
        &self,
        conn: &Connection,
        claim: &WorkItemLaunchClaim,
        current: &WorkItemAttempt,
        observed: &WorkItemAttempt,
    ) -> Result<()> {
        let token: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM work_item_attempts WHERE run=? AND generation=? AND claim_token=?)",
            params![claim.run().to_string(), claim.binding().generation, claim.token],
            |row| row.get(0),
        )?;
        if !token
            || current.run != claim.run()
            || observed.run != claim.run()
            || current.item != claim.binding().item
            || observed.item != claim.binding().item
            || current.binding != *claim.binding()
            || observed.binding != *claim.binding()
        {
            return Err(WorkItemError::Conflict(
                "stale reconciliation claim or binding".into(),
            ));
        }
        if matches!(claim.lock, LaunchLock::OwnedFlow { .. }) {
            self.validate_owned_flow_association(conn, claim)?;
        }
        Ok(())
    }

    /// Latest exact generation control; no database guard survives this return.
    pub fn execution_control(&self, run: RunId) -> Result<Option<Control>> {
        read_control(&*self.pool.get()?, run, None)
    }
    pub(super) fn request_control(
        &self,
        tx: &Transaction<'_>,
        command: &WorkItemCommand,
    ) -> Result<OperationResult> {
        let (item, expected, continue_requested, new_session) = match command {
            WorkItemCommand::Suspend {
                item,
                expected_version,
                ..
            } => (*item, *expected_version, false, false),
            WorkItemCommand::Continue {
                item,
                expected_version,
                new_session,
                ..
            } => (*item, *expected_version, true, *new_session),
            _ => return Err(WorkItemError::Invalid("not an execution control".into())),
        };
        let record = record(tx, item)?;
        check(&record, expected, false)?;
        let run = record.active_run.ok_or_else(|| {
            WorkItemError::Conflict("task has no assigned nonterminal attempt".into())
        })?;
        let attempt = attempt(tx, run)?;
        if !attempt.state.is_active() || attempt.binding.generation != record.generation {
            return Err(WorkItemError::Conflict("assigned attempt changed".into()));
        }
        let prior = read_control(tx, run, None)?;
        if continue_requested
            && !prior.as_ref().is_some_and(|control| {
                matches!(control.state, State::Suspended | State::Attention)
                    && control
                        .fence
                        .as_ref()
                        .is_some_and(|fence| fence.cleanup_confirmed)
            })
        {
            return Err(WorkItemError::Conflict(
                "Continue requires a confirmed suspension fence and cleanup".into(),
            ));
        }
        if !continue_requested
            && prior.as_ref().is_some_and(|control| {
                control.state == State::SuspendRequested
                    || (control.state == State::Suspended
                        && !control.fence.as_ref().is_some_and(|fence| {
                            matches!(
                                fence.reason,
                                surge_core::execution_recovery::SuspensionReason::Capacity { .. }
                            )
                        }))
            })
        {
            return Err(WorkItemError::Conflict(
                "execution control is already pending".into(),
            ));
        }
        let generation = prior.as_ref().map_or(Ok(1), |control| {
            control.generation.checked_add(1).ok_or_else(|| {
                WorkItemError::Conflict("execution control generation exhausted".into())
            })
        })?;
        let operation = command
            .operation_id()
            .ok_or_else(|| WorkItemError::Invalid("control identity missing".into()))?;
        let control = Control {
            item,
            run,
            attempt_generation: record.generation,
            generation,
            operation,
            state: if continue_requested {
                State::ContinueReserved
            } else {
                State::SuspendRequested
            },
            fence: prior.and_then(|control| control.fence),
            allow_new_session: new_session,
            diagnostic: None,
        };
        let state = serde_json::to_string(&control.state)?;
        tx.execute("INSERT INTO work_item_execution_controls(run,generation,item,attempt_generation,operation,state,payload) VALUES(?,?,?,?,?,?,?)",params![run.to_string(),generation,item.to_string(),record.generation,operation.to_string(),state.trim_matches('"'),serde_json::to_string(&control)?])?;
        tx.execute("UPDATE work_items SET version=version+1 WHERE id=? AND version=? AND active_run=? AND generation=?",params![item.to_string(),expected,run.to_string(),record.generation])?;
        if !continue_requested {
            recovery_cycles::invalidate_recovery_wakes(tx, item, run, record.generation)?;
        }
        Ok(OperationResult::Control { run, generation })
    }
    /// Confirm only trusted host-inspected journal evidence under the retained execution claim.
    pub fn confirm_suspension(
        &self,
        claim: &WorkItemLaunchClaim,
        fence: &SuspensionFence,
    ) -> Result<Control> {
        let path = self
            .home
            .join("runs")
            .join(claim.run().to_string())
            .join("events.sqlite");
        let history = crate::runs::inspection::read_folded_events(&path, claim.run())
            .map_err(|error| WorkItemError::Invalid(error.to_string()))?;
        if !matches!(&history.state,surge_core::RunState::Pipeline { memory,.. } if memory.suspension.as_ref()==Some(fence))
        {
            return Err(WorkItemError::Conflict(
                "suspension fence is absent from authoritative nonterminal journal".into(),
            ));
        }
        let connection = crate::runs::connection::RetainedConnection::read_only(&path)?;
        let snapshot: Vec<u8> = connection.query_row(
            "SELECT snapshot FROM graph_snapshots WHERE at_seq=?",
            [fence.snapshot_seq],
            |row| row.get(0),
        )?;
        let snapshot: serde_json::Value = serde_json::from_slice(&snapshot)?;
        if snapshot.get("at_seq").and_then(serde_json::Value::as_u64) != Some(fence.snapshot_seq)
            || snapshot.get("pending_stage") != Some(&serde_json::to_value(&fence.pending_stage)?)
        {
            return Err(WorkItemError::Conflict(
                "suspension snapshot does not match its journal fence".into(),
            ));
        }
        drop(connection);
        self.validate_claim(claim)?;
        let mut conn = self.pool.get()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut control = read_control(&tx, claim.run(), Some(fence.control_generation))?
            .ok_or(WorkItemError::NotFound)?;
        let item = record(&tx, control.item)?;
        if item.active_run != Some(claim.run())
            || item.generation != claim.binding().generation
            || read_control(&tx, claim.run(), None)?
                .as_ref()
                .map(|latest| latest.generation)
                != Some(control.generation)
        {
            return Err(WorkItemError::Conflict(
                "suspension acknowledgement belongs to an obsolete assignment/control".into(),
            ));
        }
        if control.state == State::Suspended {
            return Ok(control);
        }
        if control.state != State::SuspendRequested {
            return Err(WorkItemError::Conflict(
                "control is not waiting for suspension".into(),
            ));
        }
        control.state = if fence.cleanup_confirmed {
            State::Suspended
        } else {
            State::Attention
        };
        control.fence = Some(fence.clone());
        write_control(&tx, &control)?;
        let mut attempt = attempt(&tx, claim.run())?;
        attempt.state = if fence.cleanup_confirmed {
            WorkItemAttemptState::Suspended
        } else {
            WorkItemAttemptState::Attention
        };
        tx.execute(
            "UPDATE work_item_attempts SET state=?,payload=? WHERE run=? AND generation=?",
            params![
                if fence.cleanup_confirmed {
                    "suspended"
                } else {
                    "attention"
                },
                serde_json::to_string(&attempt)?,
                claim.run().to_string(),
                claim.binding().generation
            ],
        )?;
        tx.commit()?;
        Ok(control)
    }
    /// Confirm continued execution only after its matching journal authorization.
    pub fn confirm_continued(
        &self,
        claim: &WorkItemLaunchClaim,
        generation: u64,
    ) -> Result<Control> {
        let path = self
            .home
            .join("runs")
            .join(claim.run().to_string())
            .join("events.sqlite");
        let history = crate::runs::inspection::read_folded_events(&path, claim.run())
            .map_err(|error| WorkItemError::Invalid(error.to_string()))?;
        if !matches!(&history.state,surge_core::RunState::Pipeline { memory,.. } if memory.control_generation==generation && memory.suspension.is_none())
        {
            return Err(WorkItemError::Conflict(
                "continued generation is not durably authorized in the journal".into(),
            ));
        }
        self.validate_claim(claim)?;
        let mut conn = self.pool.get()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut control =
            read_control(&tx, claim.run(), Some(generation))?.ok_or(WorkItemError::NotFound)?;
        if read_control(&tx, claim.run(), None)?
            .as_ref()
            .map(|latest| latest.generation)
            != Some(generation)
            || control.attempt_generation != claim.binding().generation
            || !matches!(control.state, State::ContinueReserved | State::Executing)
        {
            return Err(WorkItemError::Conflict(
                "obsolete Continue acknowledgement".into(),
            ));
        }
        let item = record(&tx, control.item)?;
        if item.active_run != Some(claim.run()) || item.generation != claim.binding().generation {
            return Err(WorkItemError::Conflict(
                "continued assignment is obsolete".into(),
            ));
        }
        control.state = State::Executing;
        write_control(&tx, &control)?;
        tx.commit()?;
        Ok(control)
    }
}

fn write_reconciliation_result(
    tx: &Transaction<'_>,
    claim: &WorkItemLaunchClaim,
    mut current: WorkItemAttempt,
    control: Option<Control>,
    update: ReconciliationUpdate<'_>,
) -> Result<()> {
    current.state = update.state();
    current.diagnostic = match update {
        ReconciliationUpdate::Error(diagnostic) => Some(diagnostic.to_owned()),
        ReconciliationUpdate::Projection(_) => None,
    };
    let state = serde_json::to_string(&current.state)?;
    let changed = tx.execute(
        "UPDATE work_item_attempts SET state=?,payload=? WHERE run=? AND generation=? AND claim_token=?",
        params![state.trim_matches('"'), serde_json::to_string(&current)?, claim.run().to_string(), claim.binding().generation, claim.token],
    )?;
    if changed != 1 {
        return Err(WorkItemError::Conflict(
            "reconciliation owner changed".into(),
        ));
    }
    if let Some(mut control) = control {
        match update {
            ReconciliationUpdate::Error(diagnostic) => {
                control.state = State::Attention;
                control.diagnostic = Some(diagnostic.to_owned());
                write_control(tx, &control)?;
            },
            ReconciliationUpdate::Projection(ReconciliationProjection::Launched) => {},
            ReconciliationUpdate::Projection(_) => {
                control.state = State::Terminal;
                write_control(tx, &control)?;
            },
        }
    }
    if !current.state.is_active() {
        let released = tx.execute(
            "UPDATE work_items SET active_run=NULL,version=version+1 WHERE id=? AND active_run=? AND generation=? AND archived_at_ms IS NULL",
            params![current.item.to_string(), claim.run().to_string(), claim.binding().generation],
        )?;
        if released != 1 {
            return Err(WorkItemError::Conflict(
                "reconciliation assignment changed".into(),
            ));
        }
    }
    Ok(())
}
