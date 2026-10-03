//! Persisted opening epochs: inspection is never an opening capability.
use super::*;
use surge_core::{EventPayload, SessionId, execution_recovery::ExecutionControlState};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct HandoffBody {
    pub operation: WorkItemOperationId,
    pub run: RunId,
    pub item: WorkItemId,
    pub attempt_generation: u64,
    pub logical_invocation: String,
    pub source_cycle: u64,
    pub source_revision: u64,
    pub source_control: u64,
    pub wake_identity: Option<String>,
    pub target_cycle: u64,
    pub target_revision: u64,
    pub target_control: u64,
    pub reservation: String,
    pub launch: QuotaLaunchContract,
}

impl HandoffBody {
    pub(super) fn validate(&self) -> Result<()> {
        let automatic = self.wake_identity.is_some();
        let transition = if automatic {
            self.source_cycle.checked_add(1) == Some(self.target_cycle)
                && self.source_control.checked_add(1) == Some(self.target_control)
        } else {
            self.source_cycle == self.target_cycle
                && self.source_control == self.target_control
                && self.source_revision.checked_add(1) == Some(self.target_revision)
        };
        if self.operation.as_ulid() == Default::default()
            || self.run.as_ulid() == Default::default()
            || self.item.as_ulid() == Default::default()
            || self.attempt_generation == 0
            || self.source_cycle == 0
            || self.source_revision == 0
            || self.target_revision == 0
            || self.target_revision > i64::MAX as u64
            || self.target_cycle > i64::MAX as u64
            || self.target_control > i64::MAX as u64
            || !transition
            || invocation_key(&self.logical_invocation)? != self.logical_invocation
            || !identifier(&self.reservation)
            || self
                .wake_identity
                .as_ref()
                .is_some_and(|value| !identifier(value))
        {
            return Err(WorkItemError::Invalid(
                "invalid immutable quota handoff".into(),
            ));
        }
        // Reconstruct through the validator even for host-created values.
        QuotaLaunchContract::new(
            self.launch.candidate.clone(),
            self.launch.provider_invocation,
            self.launch.mode,
            self.launch.saved.clone(),
        )?;
        Ok(())
    }
    pub(super) fn operation_hash(&self) -> Result<ContentHash> {
        let bytes = serde_json::to_vec(&serde_json::json!({
            "internal": "quota_handoff_v1", "handoff": self
        }))?;
        Ok(ContentHash::compute(&bytes))
    }
}

/// Read-only current state of an immutable opening epoch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotaHandoffInspection {
    body: HandoffBody,
    state: QuotaHandoffState,
    opened_seq: Option<u64>,
    internal_session: Option<SessionId>,
    prompt_authorization_seq: Option<u64>,
}

/// Atomic reservation created by one confirmed due quota wake.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotaWakeReservation {
    cycle: RecoveryCycle,
    reservation: CandidateReservation,
    handoff: QuotaHandoffInspection,
}
impl QuotaWakeReservation {
    /// New cycle bound to the reserved Continue control.
    pub fn cycle(&self) -> &RecoveryCycle {
        &self.cycle
    }
    /// Candidate receipt; replayed receipts never authorize another opening.
    pub fn reservation(&self) -> &CandidateReservation {
        &self.reservation
    }
    /// Immutable provider-opening epoch.
    pub fn handoff(&self) -> &QuotaHandoffInspection {
        &self.handoff
    }
}
impl QuotaHandoffInspection {
    /// Opening epoch, distinct from the provider invocation.
    pub fn operation(&self) -> WorkItemOperationId {
        self.body.operation
    }
    /// Logical graph invocation whose quota cycle owns this provider opening.
    pub fn logical_invocation(&self) -> &str {
        &self.body.logical_invocation
    }
    /// Target recovery-cycle ordinal reserved by this opening epoch.
    pub fn cycle_generation(&self) -> u64 {
        self.body.target_cycle
    }
    /// Immutable candidate reservation receipt.
    pub fn reservation_receipt(&self) -> &str {
        &self.body.reservation
    }
    /// Whether this opening epoch was reserved by a scheduled quota wake.
    pub fn is_automatic_wake(&self) -> bool {
        self.body.wake_identity.is_some()
    }
    /// Immutable selected launch recipe, without dispatch authority.
    pub fn launch(&self) -> &QuotaLaunchContract {
        &self.body.launch
    }
    /// Durable admission state.
    pub fn state(&self) -> QuotaHandoffState {
        self.state
    }
    /// Exact control generation admitting this epoch.
    pub fn control_generation(&self) -> u64 {
        self.body.target_control
    }
    /// Fresh bridge connection confirmed for this epoch, when established.
    pub fn internal_session(&self) -> Option<SessionId> {
        self.internal_session
    }
    /// Exact durable journal event authorizing this quota handoff's prompt.
    pub fn prompt_authorization_seq(&self) -> Option<u64> {
        self.prompt_authorization_seq
    }
}

struct StoredHandoff {
    operation: String,
    run: String,
    item: String,
    attempt: u64,
    logical: String,
    source_cycle: u64,
    source_control: u64,
    wake: Option<String>,
    target_cycle: u64,
    target_control: u64,
    reservation: String,
    provider_invocation: String,
    launch_hash: String,
    state: String,
    body_hash: String,
    body: String,
    opened_seq: Option<u64>,
    session: Option<String>,
    prompt_authorization_seq: Option<u64>,
}

pub(super) fn read_handoff(
    conn: &Connection,
    operation: WorkItemOperationId,
) -> Result<QuotaHandoffInspection> {
    let row = conn.query_row(
        "SELECT operation,run,item,attempt_generation,logical_invocation,source_cycle,source_control,wake_identity,target_cycle,target_control,reservation,provider_invocation,launch_hash,state,body_hash,body,opened_seq,internal_session,prompt_authorization_seq FROM work_item_quota_handoffs WHERE operation=?",
        [operation.to_string()], |row| Ok(StoredHandoff {
            operation: row.get(0)?, run: row.get(1)?, item: row.get(2)?, attempt: row.get(3)?,
            logical: row.get(4)?, source_cycle: row.get(5)?, source_control: row.get(6)?,
            wake: row.get(7)?, target_cycle: row.get(8)?, target_control: row.get(9)?,
            reservation: row.get(10)?, provider_invocation: row.get(11)?,
            launch_hash: row.get(12)?, state: row.get(13)?, body_hash: row.get(14)?,
            body: row.get(15)?, opened_seq: row.get(16)?, session: row.get(17)?, prompt_authorization_seq: row.get(18)?,
        }),
    )?;
    let body: HandoffBody = serde_json::from_str(&row.body)?;
    body.validate()?;
    let same = row.operation == body.operation.to_string()
        && body.operation == operation
        && row.run == body.run.to_string()
        && row.item == body.item.to_string()
        && row.attempt == body.attempt_generation
        && row.logical == body.logical_invocation
        && row.source_cycle == body.source_cycle
        && row.source_control == body.source_control
        && row.wake == body.wake_identity
        && row.target_cycle == body.target_cycle
        && row.target_control == body.target_control
        && row.reservation == body.reservation
        && row.provider_invocation == body.launch.provider_invocation.to_string()
        && row.launch_hash == body.launch.candidate.launch_hash().to_string()
        && row.body_hash == ContentHash::compute(row.body.as_bytes()).to_string();
    if !same {
        return Err(WorkItemError::Invalid(
            "quota handoff columns disagree with body".into(),
        ));
    }
    validate_handoff_links(conn, &body)?;
    let state = match row.state.as_str() {
        "reserved" => QuotaHandoffState::Reserved,
        "opening_unknown" => QuotaHandoffState::OpeningUnknown,
        "established" => QuotaHandoffState::Established,
        "executing" => QuotaHandoffState::Executing,
        "invalidated" => QuotaHandoffState::Invalidated,
        "attention" => QuotaHandoffState::Attention,
        _ => return Err(WorkItemError::Invalid("unknown quota handoff state".into())),
    };
    let session: Option<SessionId> = row
        .session
        .map(|value| {
            value
                .parse()
                .map_err(|_| WorkItemError::Invalid("invalid handoff session".into()))
        })
        .transpose()?;
    if session.is_some_and(|value| value.as_ulid() == Default::default())
        || row.opened_seq == Some(0)
        || row.opened_seq.is_some() != session.is_some()
    {
        return Err(WorkItemError::Invalid(
            "invalid handoff opening evidence".into(),
        ));
    }
    if matches!(
        state,
        QuotaHandoffState::Reserved | QuotaHandoffState::OpeningUnknown
    ) && row.opened_seq.is_some()
    {
        return Err(WorkItemError::Invalid(
            "unconfirmed handoff carries established evidence".into(),
        ));
    }
    if matches!(
        state,
        QuotaHandoffState::Established | QuotaHandoffState::Executing
    ) && (row.opened_seq.is_none_or(|seq| seq == 0) || session.is_none())
    {
        return Err(WorkItemError::Invalid(
            "established handoff lacks opening evidence".into(),
        ));
    }
    if row.prompt_authorization_seq == Some(0)
        || (state == QuotaHandoffState::Executing) != row.prompt_authorization_seq.is_some()
    {
        return Err(WorkItemError::Invalid(
            "handoff execution state lacks prompt authorization evidence".into(),
        ));
    }
    Ok(QuotaHandoffInspection {
        body,
        state,
        opened_seq: row.opened_seq,
        internal_session: session,
        prompt_authorization_seq: row.prompt_authorization_seq,
    })
}

fn validate_handoff_links(conn: &Connection, body: &HandoffBody) -> Result<()> {
    let (operation_hash, operation_result): (String, String) = conn.query_row(
        "SELECT body_hash,result FROM work_item_operations WHERE operation=?",
        [body.operation.to_string()],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let result: OperationResult = serde_json::from_str(&operation_result)?;
    if operation_hash != body.operation_hash()?.to_string()
        || !matches!(result, OperationResult::Control { run, generation }
            if run == body.run && generation == body.target_control)
    {
        return Err(WorkItemError::Invalid(
            "handoff lacks its internal operation receipt".into(),
        ));
    }
    let (run, invocation, generation, runtime, candidate): (String,String,u64,String,String) = conn.query_row(
        "SELECT run,invocation,cycle_generation,runtime,body FROM work_item_quota_candidates WHERE receipt=?",
        [&body.reservation], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?)),
    )?;
    let candidate: RecoveryCandidate = serde_json::from_str(&candidate)?;
    let stage = read_bound_stage(conn, body.run, &body.logical_invocation)?;
    if run != body.run.to_string()
        || invocation != body.logical_invocation
        || generation != body.target_cycle
        || runtime != candidate.runtime()
        || &candidate != body.launch.candidate.candidate()
        || !stage.candidates().contains(&body.launch.candidate)
    {
        return Err(WorkItemError::Invalid(
            "handoff candidate or frozen policy differs".into(),
        ));
    }
    Ok(())
}

pub(super) fn current_handoff(
    tx: &Transaction<'_>,
    claim: &WorkItemLaunchClaim,
    handoff: &QuotaHandoffInspection,
) -> Result<()> {
    owner_fence(tx, claim)?;
    let body = &handoff.body;
    if body.run != claim.run
        || body.item != claim.binding.item
        || body.attempt_generation != claim.binding.generation
    {
        return Err(WorkItemError::Conflict("handoff assignment changed".into()));
    }
    let latest: u64 = tx.query_row(
        "SELECT MAX(cycle_generation) FROM work_item_quota_cycles WHERE run=? AND invocation=?",
        params![body.run.to_string(), body.logical_invocation],
        |row| row.get(0),
    )?;
    let cycle = read_cycle(tx, body.run, &body.logical_invocation, body.target_cycle)?;
    if latest != body.target_cycle || cycle.revision != body.target_revision {
        return Err(WorkItemError::Conflict(
            "handoff cycle is not current".into(),
        ));
    }
    if body.wake_identity.is_some() {
        let source = read_cycle(tx, body.run, &body.logical_invocation, body.source_cycle)?;
        if !source.closed
            || source.control_generation != body.source_control
            || body.source_revision.checked_add(1) != Some(source.revision)
        {
            return Err(WorkItemError::Conflict(
                "handoff source transition changed".into(),
            ));
        }
    }
    if cycle.closed
        || cycle.control_generation != body.target_control
        || cycle.selected_runtime.as_deref() != Some(body.launch.candidate.candidate().runtime())
    {
        return Err(WorkItemError::Conflict(
            "handoff candidate is no longer current".into(),
        ));
    }
    let control = control::read_control(tx, claim.run, None)?;
    let allowed = match control {
        None => body.target_control == 0 && body.wake_identity.is_none(),
        Some(control) => {
            control.generation == body.target_control
                && control.item == body.item
                && control.attempt_generation == body.attempt_generation
                && if body.wake_identity.is_some() {
                    control.operation == body.operation
                        && matches!(
                            control.state,
                            ExecutionControlState::ContinueReserved
                                | ExecutionControlState::Executing
                        )
                } else {
                    control.state == ExecutionControlState::Executing
                }
        },
    };
    if !allowed {
        return Err(WorkItemError::Conflict(
            "handoff control was superseded".into(),
        ));
    }
    Ok(())
}

impl WorkItemStore {
    /// Bind an established fallback opening to its exact `SessionOpened`
    /// journal event before granting that provider session prompt authority.
    pub fn authorize_fallback_prompt(
        &self,
        claim: &WorkItemLaunchClaim,
        operation: WorkItemOperationId,
        opened_seq: u64,
    ) -> Result<QuotaHandoffInspection> {
        if opened_seq == 0 {
            return Err(WorkItemError::Invalid(
                "SessionOpened sequence must be nonzero".into(),
            ));
        }
        let path = self
            .home
            .join("runs")
            .join(claim.run().to_string())
            .join("events.sqlite");
        let journal =
            Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let (schema, payload): (u32, Vec<u8>) = journal.query_row(
            "SELECT schema_version,payload FROM events WHERE seq=?",
            [opened_seq],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let event = surge_core::migrate_payload(schema, &payload)
            .map_err(|error| WorkItemError::Invalid(error.to_string()))?;
        let surge_core::EventPayload::SessionOpened {
            handoff: Some(event_operation),
            opened: Some(opened),
            ..
        } = event
        else {
            return Err(WorkItemError::Conflict(
                "fallback prompt authorization requires its durable SessionOpened event".into(),
            ));
        };
        if event_operation != operation {
            return Err(WorkItemError::Conflict(
                "SessionOpened belongs to a different quota handoff".into(),
            ));
        }
        let latest_seq: u64 =
            journal.query_row("SELECT COALESCE(MAX(seq),0) FROM events", [], |row| {
                row.get(0)
            })?;
        if latest_seq != opened_seq {
            return Err(WorkItemError::Conflict(
                "SessionOpened is not the current journal prefix".into(),
            ));
        }

        let mut connection = self.pool.get()?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let handoff = read_handoff(&tx, operation)?;
        current_handoff(&tx, claim, &handoff)?;
        if handoff.body.wake_identity.is_some()
            || handoff.state != QuotaHandoffState::Established
            || handoff.opened_seq != Some(opened_seq)
            || handoff.internal_session != Some(opened.session)
            || opened.descriptor.invocation() != handoff.body.launch.provider_invocation
        {
            return Err(WorkItemError::Conflict(
                "fallback opening evidence differs from its quota handoff".into(),
            ));
        }
        if let Some(control) = control::read_control(&tx, claim.run(), None)?
            && (control.state != surge_core::execution_recovery::ExecutionControlState::Executing
                || control.generation != handoff.body.target_control)
        {
            return Err(WorkItemError::Conflict(
                "fallback prompt is outside the current executing control".into(),
            ));
        }
        tx.execute(
            "UPDATE work_item_quota_handoffs SET state='executing',prompt_authorization_seq=? WHERE operation=? AND state='established'",
            params![opened_seq, operation.to_string()],
        )?;
        let authorized = read_handoff(&tx, operation)?;
        tx.commit()?;
        Ok(authorized)
    }

    /// Bind a durable `RunContinued` event to the exact reserved automatic
    /// wake before granting its provider session any prompt authority.
    /// Replays with the same journal sequence return the original handoff.
    pub fn authorize_automatic_wake_prompt(
        &self,
        claim: &WorkItemLaunchClaim,
        operation: WorkItemOperationId,
        continued_seq: u64,
    ) -> Result<QuotaHandoffInspection> {
        use surge_core::execution_recovery::ExecutionControlState as ControlState;

        if continued_seq == 0 {
            return Err(WorkItemError::Invalid(
                "RunContinued sequence must be nonzero".into(),
            ));
        }
        let path = self
            .home
            .join("runs")
            .join(claim.run().to_string())
            .join("events.sqlite");
        let history = crate::runs::inspection::read_folded_events(&path, claim.run())
            .map_err(|error| WorkItemError::Invalid(error.to_string()))?;
        let journal =
            Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let (schema, payload): (u32, Vec<u8>) = journal.query_row(
            "SELECT schema_version,payload FROM events WHERE seq=?",
            [continued_seq],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let event = surge_core::migrate_payload(schema, &payload)
            .map_err(|error| WorkItemError::Invalid(error.to_string()))?;
        let surge_core::EventPayload::RunContinued { control_generation } = event else {
            return Err(WorkItemError::Conflict(
                "prompt authorization requires a durable RunContinued event".into(),
            ));
        };
        let latest_seq: u64 =
            journal.query_row("SELECT COALESCE(MAX(seq),0) FROM events", [], |row| {
                row.get(0)
            })?;
        if latest_seq != continued_seq {
            return Err(WorkItemError::Conflict(
                "RunContinued is not the current journal prefix".into(),
            ));
        }
        if !matches!(history.state, surge_core::RunState::Pipeline { memory, .. }
            if memory.suspension.is_none() && memory.control_generation == control_generation)
        {
            return Err(WorkItemError::Conflict(
                "RunContinued projection does not authorize the current run".into(),
            ));
        }

        let mut connection = self.pool.get()?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let handoff = read_handoff(&tx, operation)?;
        current_handoff(&tx, claim, &handoff)?;
        if handoff.body.wake_identity.is_none() || handoff.body.target_control != control_generation
        {
            return Err(WorkItemError::Conflict(
                "RunContinued belongs to a different quota handoff".into(),
            ));
        }
        if handoff.state == QuotaHandoffState::Executing {
            if handoff.prompt_authorization_seq != Some(continued_seq) {
                return Err(WorkItemError::Conflict(
                    "automatic wake continuation replay changed its journal sequence".into(),
                ));
            }
            return Ok(handoff);
        }
        if handoff.state != QuotaHandoffState::Established {
            return Err(WorkItemError::Conflict(
                "automatic wake provider opening is not established".into(),
            ));
        }
        let mut control = control::read_control(&tx, claim.run(), None)?
            .ok_or_else(|| WorkItemError::Conflict("Continue control is absent".into()))?;
        // The journal subscriber can acknowledge this same Continue before
        // prompt sealing. Executing is an acknowledgment, not new authority:
        // the exact operation, generation and established opening still fence it.
        if !matches!(
            control.state,
            ControlState::ContinueReserved | ControlState::Executing
        ) || control.generation != handoff.body.target_control
            || control.operation != operation
        {
            return Err(WorkItemError::Conflict(
                "automatic wake Continue reservation was superseded".into(),
            ));
        }
        control.state = ControlState::Executing;
        control::write_control(&tx, &control)?;
        tx.execute(
            "UPDATE work_item_quota_handoffs SET state='executing',prompt_authorization_seq=? WHERE operation=? AND state='established'",
            params![continued_seq, operation.to_string()],
        )?;
        let authorized = read_handoff(&tx, operation)?;
        tx.commit()?;
        Ok(authorized)
    }

    /// Consume a due Capacity wake and reserve its Continue, next cycle,
    /// candidate, operation receipt and provider handoff in one transaction.
    /// The caller must already hold the original launch claim and provide the
    /// launch contract selected from the frozen policy. This records intent;
    /// `admit_provider_open` remains the separate one-shot side-effect gate.
    pub fn reserve_automatic_wake(
        &self,
        claim: &WorkItemLaunchClaim,
        expected: &RecoveryCycle,
        wake_identity: &str,
        now_ms: i64,
        launch: QuotaLaunchContract,
    ) -> Result<QuotaWakeReservation> {
        use surge_core::execution_recovery::{
            ExecutionControlState as ControlState, SessionOpenMode, WorkItemExecutionControl,
        };

        if now_ms < 0 || !identifier(wake_identity) || launch.mode() != SessionOpenMode::New {
            return Err(WorkItemError::Invalid(
                "invalid automatic quota wake".into(),
            ));
        }
        let mut connection = self.pool.get()?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        owner_fence(&tx, claim)?;
        let source = read_cycle(&tx, claim.run, &expected.invocation, expected.generation)?;
        if expected.run != claim.run
            || !expected
                .wake
                .as_ref()
                .is_some_and(|wake| wake.identity() == wake_identity && wake.due_at_ms() <= now_ms)
        {
            return Err(WorkItemError::Conflict(
                "quota wake identity, revision or due time changed".into(),
            ));
        }

        // Exact replay returns the original receipt for inspection. The
        // opening admission remains spent or unspent exactly as before.
        let existing: Option<String> = tx.query_row(
            "SELECT operation FROM work_item_quota_handoffs WHERE run=? AND logical_invocation=? AND source_cycle=? AND source_control=? AND wake_identity=?",
            params![claim.run.to_string(), expected.invocation, expected.generation, expected.control_generation, wake_identity],
            |row| row.get(0),
        ).optional()?;
        if let Some(operation) = existing {
            let operation: WorkItemOperationId = operation.parse().map_err(|_| {
                WorkItemError::Invalid("invalid stored quota wake operation".into())
            })?;
            let handoff = read_handoff(&tx, operation)?;
            if handoff.body.launch != launch
                || handoff.body.source_revision != expected.revision
                || handoff.body.wake_identity.as_deref() != Some(wake_identity)
                || !source.closed
                || source.revision != expected.revision.saturating_add(1)
                || source.wake != expected.wake
            {
                return Err(WorkItemError::Conflict(
                    "automatic wake replay changed its immutable launch contract".into(),
                ));
            }
            let cycle = read_cycle(
                &tx,
                claim.run,
                &expected.invocation,
                handoff.body.target_cycle,
            )?;
            let (body,): (String,) = tx.query_row(
                "SELECT body FROM work_item_quota_candidates WHERE receipt=?",
                [&handoff.body.reservation],
                |row| Ok((row.get(0)?,)),
            )?;
            let reservation = CandidateReservation {
                receipt: handoff.body.reservation.clone(),
                candidate: serde_json::from_str(&body)?,
                disposition: ReservationDisposition::Replayed,
            };
            return Ok(QuotaWakeReservation {
                cycle,
                reservation,
                handoff,
            });
        }

        if source != *expected || source.closed {
            return Err(WorkItemError::Conflict(
                "quota wake identity, revision or due time changed".into(),
            ));
        }
        let actionable_at: Option<i64> = tx.query_row(
            "SELECT wake_at_ms FROM work_item_quota_cycles WHERE run=? AND invocation=? AND cycle_generation=?",
            params![claim.run.to_string(), source.invocation, source.generation],
            |row| row.get(0),
        )?;
        if actionable_at != source.wake.as_ref().map(RecoveryWake::due_at_ms) {
            return Err(WorkItemError::Conflict(
                "quota wake schedule was cancelled or replaced".into(),
            ));
        }

        let stage = read_bound_stage(&tx, claim.run, &source.invocation)?;
        if !stage.candidates().contains(&launch.candidate) {
            return Err(WorkItemError::Conflict(
                "automatic wake launch is outside frozen quota policy".into(),
            ));
        }
        let latest = control::read_control(&tx, claim.run, None)?
            .ok_or_else(|| WorkItemError::Conflict("capacity control is absent".into()))?;
        if latest.generation != source.control_generation
            || latest.item != claim.binding.item
            || latest.run != claim.run
            || latest.attempt_generation != claim.binding.generation
            || latest.state != ControlState::Suspended
            || !capacity_fence(&tx, &latest, &source.invocation)?
        {
            return Err(WorkItemError::Conflict(
                "automatic wake requires the current confirmed Capacity fence".into(),
            ));
        }

        let target_cycle = source
            .generation
            .checked_add(1)
            .filter(|value| *value <= i64::MAX as u64)
            .ok_or_else(|| WorkItemError::Conflict("quota cycle generation exhausted".into()))?;
        let target_control = source
            .control_generation
            .checked_add(1)
            .filter(|value| *value <= i64::MAX as u64)
            .ok_or_else(|| WorkItemError::Conflict("quota control generation exhausted".into()))?;
        let source_revision = source.revision;
        let operation = WorkItemOperationId::new();
        let receipt = RunId::new().to_string();
        let recovery_candidate = launch.candidate.candidate().clone();
        let candidate_body = serde_json::to_string(&recovery_candidate)?;
        tx.execute(
            "UPDATE work_item_quota_cycles SET closed=1,wake_at_ms=NULL,revision=revision+1 WHERE run=? AND invocation=? AND cycle_generation=? AND closed=0 AND revision=?",
            params![claim.run.to_string(), source.invocation, source.generation, source.revision],
        )?;
        insert_cycle(
            &tx,
            claim,
            &source.invocation,
            target_cycle,
            target_control,
            source.probe_count,
        )?;
        let target_revision = 2;
        tx.execute(
            "INSERT INTO work_item_quota_candidates(run,invocation,cycle_generation,candidate_key,runtime,receipt,body) VALUES(?,?,?,?,?,?,?)",
            params![claim.run.to_string(), source.invocation, target_cycle, recovery_candidate.key()?, recovery_candidate.runtime(), receipt, candidate_body],
        )?;
        tx.execute(
            "UPDATE work_item_quota_cycles SET selected_runtime=?,revision=revision+1 WHERE run=? AND invocation=? AND cycle_generation=?",
            params![recovery_candidate.runtime(), claim.run.to_string(), source.invocation, target_cycle],
        )?;

        let handoff_body = HandoffBody {
            operation,
            run: claim.run,
            item: claim.binding.item,
            attempt_generation: claim.binding.generation,
            logical_invocation: source.invocation.clone(),
            source_cycle: source.generation,
            source_revision,
            source_control: source.control_generation,
            wake_identity: Some(wake_identity.to_owned()),
            target_cycle,
            target_revision,
            target_control,
            reservation: receipt.clone(),
            launch: launch.clone(),
        };
        handoff_body.validate()?;
        let operation_hash = handoff_body.operation_hash()?.to_string();
        let result = OperationResult::Control {
            run: claim.run,
            generation: target_control,
        };
        tx.execute(
            "INSERT INTO work_item_operations(operation,body_hash,result) VALUES(?,?,?)",
            params![
                operation.to_string(),
                operation_hash,
                serde_json::to_string(&result)?
            ],
        )?;
        let control_record = WorkItemExecutionControl {
            item: claim.binding.item,
            run: claim.run,
            attempt_generation: claim.binding.generation,
            generation: target_control,
            operation,
            state: ControlState::ContinueReserved,
            fence: latest.fence,
            allow_new_session: false,
            diagnostic: None,
        };
        tx.execute(
            "INSERT INTO work_item_execution_controls(run,generation,item,attempt_generation,operation,state,payload) VALUES(?,?,?,?,?,'continue_reserved',?)",
            params![claim.run.to_string(), target_control, claim.binding.item.to_string(), claim.binding.generation, operation.to_string(), serde_json::to_string(&control_record)?],
        )?;
        let body = serde_json::to_string(&handoff_body)?;
        tx.execute(
            "INSERT INTO work_item_quota_handoffs(operation,run,item,attempt_generation,logical_invocation,source_cycle,source_control,wake_identity,target_cycle,target_control,reservation,provider_invocation,launch_hash,state,body_hash,body) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,'reserved',?,?)",
            params![operation.to_string(), claim.run.to_string(), claim.binding.item.to_string(), claim.binding.generation, source.invocation, source.generation, source.control_generation, wake_identity, target_cycle, target_control, receipt, launch.provider_invocation().to_string(), launch.candidate().launch_hash().to_string(), ContentHash::compute(body.as_bytes()).to_string(), body],
        )?;
        tx.execute(
            "UPDATE work_items SET version=version+1 WHERE id=? AND active_run=? AND generation=?",
            params![
                claim.binding.item.to_string(),
                claim.run.to_string(),
                claim.binding.generation
            ],
        )?;
        let cycle = read_cycle(&tx, claim.run, &source.invocation, target_cycle)?;
        let handoff = read_handoff(&tx, operation)?;
        let reservation = CandidateReservation {
            receipt,
            candidate: recovery_candidate,
            disposition: ReservationDisposition::Reserved,
        };
        tx.commit()?;
        Ok(QuotaWakeReservation {
            cycle,
            reservation,
            handoff,
        })
    }

    /// Persist one reserved candidate as a one-shot provider opening epoch.
    /// The candidate must already have a first-dispatch reservation in the
    /// current cycle and must match the immutable policy bound to this stage.
    /// Replays are read-only and return the original operation identity.
    pub fn reserve_quota_open(
        &self,
        claim: &WorkItemLaunchClaim,
        cycle: &RecoveryCycle,
        reservation: &CandidateReservation,
        launch: QuotaLaunchContract,
    ) -> Result<QuotaHandoffInspection> {
        if cycle.run != claim.run
            || cycle.closed
            || cycle.selected_runtime.as_deref() != Some(reservation.candidate.runtime())
            || launch.candidate.candidate() != &reservation.candidate
        {
            return Err(WorkItemError::Conflict(
                "quota opening does not match current reservation".into(),
            ));
        }
        let operation = WorkItemOperationId::new();
        let handoff = HandoffBody {
            operation,
            run: claim.run,
            item: claim.binding.item,
            attempt_generation: claim.binding.generation,
            logical_invocation: cycle.invocation.clone(),
            source_cycle: cycle.generation,
            source_revision: cycle.revision.checked_sub(1).ok_or_else(|| {
                WorkItemError::Invalid("quota reservation revision missing".into())
            })?,
            source_control: cycle.control_generation,
            wake_identity: None,
            target_cycle: cycle.generation,
            target_revision: cycle.revision,
            target_control: cycle.control_generation,
            reservation: reservation.receipt.clone(),
            launch,
        };
        handoff.validate()?;
        let body = serde_json::to_string(&handoff)?;
        let hash = handoff.operation_hash()?.to_string();
        let mut connection = self.pool.get()?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        owner_fence(&tx, claim)?;
        let current = read_cycle(&tx, claim.run, &cycle.invocation, cycle.generation)?;
        check_cycle(&tx, claim, &current)?;
        let stored_candidate: Option<(String, String)> = tx.query_row(
            "SELECT invocation,body FROM work_item_quota_candidates WHERE receipt=? AND run=? AND cycle_generation=?",
            params![reservation.receipt, claim.run.to_string(), cycle.generation],
            |row| Ok((row.get(0)?, row.get(1)?)),
        ).optional()?;
        let Some((stored_invocation, stored_body)) = stored_candidate else {
            return Err(WorkItemError::Conflict(
                "quota reservation does not belong to current cycle".into(),
            ));
        };
        let stored_candidate: RecoveryCandidate = serde_json::from_str(&stored_body)?;
        if stored_invocation != cycle.invocation || stored_candidate != reservation.candidate {
            return Err(WorkItemError::Conflict(
                "quota reservation identity changed".into(),
            ));
        }
        let existing: Option<String> = tx.query_row(
            "SELECT operation FROM work_item_quota_handoffs WHERE run=? AND logical_invocation=? AND target_cycle=? AND reservation=?",
            params![claim.run.to_string(), cycle.invocation, cycle.generation, reservation.receipt],
            |row| row.get(0),
        ).optional()?;
        if let Some(existing) = existing {
            let operation: WorkItemOperationId = existing
                .parse()
                .map_err(|_| WorkItemError::Invalid("invalid stored quota operation".into()))?;
            let stored = read_handoff(&tx, operation)?;
            if stored.body.launch != handoff.launch
                || stored.body.run != handoff.run
                || stored.body.item != handoff.item
                || stored.body.attempt_generation != handoff.attempt_generation
                || stored.body.target_cycle != handoff.target_cycle
                || stored.body.target_control != handoff.target_control
                || stored.body.reservation != handoff.reservation
            {
                return Err(WorkItemError::Conflict(
                    "quota opening replay changed its immutable body".into(),
                ));
            }
            return Ok(stored);
        }
        if reservation.disposition != ReservationDisposition::Reserved {
            return Err(WorkItemError::Conflict(
                "replayed candidate cannot authorize provider opening".into(),
            ));
        }
        let bound = read_bound_stage(&tx, claim.run, &cycle.invocation)?;
        if !bound.candidates().contains(&handoff.launch.candidate) {
            return Err(WorkItemError::Conflict(
                "candidate is outside frozen stage policy".into(),
            ));
        }
        let result = OperationResult::Control {
            run: claim.run,
            generation: cycle.control_generation,
        };
        tx.execute(
            "INSERT INTO work_item_operations(operation,body_hash,result) VALUES(?,?,?)",
            params![operation.to_string(), hash, serde_json::to_string(&result)?],
        )?;
        tx.execute(
            "INSERT INTO work_item_quota_handoffs(operation,run,item,attempt_generation,logical_invocation,source_cycle,source_control,wake_identity,target_cycle,target_control,reservation,provider_invocation,launch_hash,state,body_hash,body) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,'reserved',?,?)",
            params![operation.to_string(), claim.run.to_string(), claim.binding.item.to_string(), claim.binding.generation, cycle.invocation, cycle.generation, cycle.control_generation, Option::<String>::None, cycle.generation, cycle.control_generation, reservation.receipt, handoff.launch.provider_invocation.to_string(), handoff.launch.candidate.launch_hash().to_string(), ContentHash::compute(body.as_bytes()).to_string(), body],
        )?;
        let stored = read_handoff(&tx, operation)?;
        tx.commit()?;
        Ok(stored)
    }

    /// Inspect a stored opening; this method never returns dispatch authority.
    pub fn quota_handoff(&self, operation: WorkItemOperationId) -> Result<QuotaHandoffInspection> {
        let connection = self.pool.get()?;
        read_handoff(&connection, operation)
    }
    /// Spend an epoch before the provider RPC. Replays cannot obtain another permit.
    pub fn admit_provider_open(
        &self,
        claim: &WorkItemLaunchClaim,
        operation: WorkItemOperationId,
    ) -> Result<QuotaOpenPermit> {
        let inspected = self.quota_handoff(operation)?;
        self.validate_opening_history(claim, &inspected)?;
        let mut conn = self.pool.get()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let handoff = read_handoff(&tx, operation)?;
        current_handoff(&tx, claim, &handoff)?;
        if handoff.state != QuotaHandoffState::Reserved {
            return Err(WorkItemError::Conflict(
                "provider opening admission already spent".into(),
            ));
        }
        let changed = tx.execute(
            "UPDATE work_item_quota_handoffs SET state='opening_unknown' WHERE operation=? AND state='reserved'",
            [operation.to_string()],
        )?;
        if changed != 1 {
            return Err(WorkItemError::Conflict("opening admission raced".into()));
        }
        tx.commit()?;
        Ok(QuotaOpenPermit {
            operation,
            run: claim.run,
            control_generation: handoff.body.target_control,
            launch: handoff.body.launch,
        })
    }
    /// Confirm only the exact new epoch-bearing durable provider opening.
    pub fn confirm_provider_open(
        &self,
        claim: &WorkItemLaunchClaim,
        operation: WorkItemOperationId,
        opened_seq: u64,
    ) -> Result<QuotaHandoffInspection> {
        let evidence = self.read_quota_opening(claim, operation, opened_seq)?;
        let mut conn = self.pool.get()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let handoff = read_handoff(&tx, operation)?;
        current_handoff(&tx, claim, &handoff)?;
        confirm_opening_contract(&tx, &handoff, &evidence)?;
        if matches!(
            handoff.state,
            QuotaHandoffState::Established | QuotaHandoffState::Executing
        ) {
            if handoff.opened_seq != Some(opened_seq)
                || handoff.internal_session != Some(evidence.session)
            {
                return Err(WorkItemError::Conflict(
                    "opening confirmation conflicts with original evidence".into(),
                ));
            }
            return Ok(handoff);
        }
        if handoff.state != QuotaHandoffState::OpeningUnknown {
            return Err(WorkItemError::Conflict(
                "opening has no spent admission".into(),
            ));
        }
        tx.execute("UPDATE work_item_quota_handoffs SET state='established',opened_seq=?,internal_session=? WHERE operation=? AND state='opening_unknown'",
            params![opened_seq,evidence.session.to_string(),operation.to_string()])?;
        let confirmed = read_handoff(&tx, operation)?;
        tx.commit()?;
        Ok(confirmed)
    }
    fn validate_opening_history(
        &self,
        claim: &WorkItemLaunchClaim,
        handoff: &QuotaHandoffInspection,
    ) -> Result<()> {
        let path = self
            .home
            .join("runs")
            .join(claim.run.to_string())
            .join("events.sqlite");
        let history = crate::runs::inspection::read_folded_events(&path, claim.run)
            .map_err(|error| WorkItemError::Invalid(error.to_string()))?;
        let connection = self.pool.get()?;
        let stage = read_bound_stage(&connection, claim.run, &handoff.body.logical_invocation)?;
        let surge_core::RunState::Pipeline { cursor, memory, .. } = &history.state else {
            return Err(WorkItemError::Conflict(
                "opening has no live trusted journal".into(),
            ));
        };
        let expected_generation = if handoff.body.wake_identity.is_some() {
            handoff.body.source_control
        } else {
            handoff.body.target_control
        };
        if &cursor.node != stage.node() || memory.control_generation != expected_generation {
            return Err(WorkItemError::Conflict(
                "opening journal stage or control differs".into(),
            ));
        }
        if let Some(saved) = handoff.body.launch.saved.as_ref()
            && !memory
                .provider_sessions
                .get(&saved.invocation())
                .is_some_and(|history| history.iter().any(|value| &value.descriptor == saved))
        {
            return Err(WorkItemError::Conflict(
                "restore descriptor lacks durable provenance".into(),
            ));
        }
        Ok(())
    }

    fn read_quota_opening(
        &self,
        claim: &WorkItemLaunchClaim,
        operation: WorkItemOperationId,
        sequence: u64,
    ) -> Result<surge_core::execution_recovery::OpenedSession> {
        if sequence == 0 {
            return Err(WorkItemError::Invalid("opening sequence is zero".into()));
        }
        let path = self
            .home
            .join("runs")
            .join(claim.run.to_string())
            .join("events.sqlite");
        let history = crate::runs::inspection::read_folded_events(&path, claim.run)
            .map_err(|error| WorkItemError::Invalid(error.to_string()))?;
        if !matches!(history.state, surge_core::RunState::Pipeline { .. }) {
            return Err(WorkItemError::Conflict(
                "opening has no live trusted journal".into(),
            ));
        }
        let conn = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let (schema, payload): (u32, Vec<u8>) = conn.query_row(
            "SELECT schema_version,payload FROM events WHERE seq=?",
            [sequence],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let event = surge_core::migrate_payload(schema, &payload)
            .map_err(|error| WorkItemError::Invalid(error.to_string()))?;
        let EventPayload::SessionOpened {
            node,
            handoff: Some(epoch),
            opened: Some(opened),
            ..
        } = event
        else {
            return Err(WorkItemError::Invalid(
                "opening lacks host handoff epoch".into(),
            ));
        };
        if epoch != operation {
            return Err(WorkItemError::Conflict(
                "opening belongs to another epoch".into(),
            ));
        }
        let registry = self.pool.get()?;
        let handoff = read_handoff(&registry, operation)?;
        let stage = read_bound_stage(&registry, claim.run, &handoff.body.logical_invocation)?;
        let original_seq: u64 = registry.query_row(
            "SELECT opening_seq FROM work_item_quota_stages WHERE run=? AND invocation=?",
            params![claim.run.to_string(), handoff.body.logical_invocation],
            |row| row.get(0),
        )?;
        let unique_session = matches!(&history.state, surge_core::RunState::Pipeline { memory, .. }
            if memory.provider_sessions.values().flatten().filter(|value| value.session == opened.session).count() == 1);
        if &node != stage.node() || sequence <= original_seq || !unique_session {
            return Err(WorkItemError::Conflict(
                "opening has stale stage or reused internal session".into(),
            ));
        }
        Ok(opened)
    }
}

fn confirm_opening_contract(
    conn: &Connection,
    handoff: &QuotaHandoffInspection,
    opened: &surge_core::execution_recovery::OpenedSession,
) -> Result<()> {
    let launch = &handoff.body.launch;
    let workspace = record(conn, handoff.body.item)?.workspace;
    if opened.descriptor.cwd() != workspace.path
        || opened.mode != launch.mode
        || opened.descriptor.invocation() != launch.provider_invocation
        || opened.descriptor.runtime() != launch.candidate.candidate().runtime()
        || opened.descriptor.launch_hash() != launch.candidate.launch_hash()
        || launch.saved.as_ref().is_some_and(|saved| {
            saved.provider_session_id() != opened.descriptor.provider_session_id()
                || saved.cwd() != opened.descriptor.cwd()
        })
    {
        return Err(WorkItemError::Conflict(
            "opened provider differs from immutable handoff".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
