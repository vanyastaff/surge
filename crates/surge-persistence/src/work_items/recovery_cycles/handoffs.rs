//! One-shot host authority for a selected provider opening.
mod capacity_intent;
mod opening;
use super::*;
pub use capacity_intent::CapacityStopKind;
pub(crate) use capacity_intent::{insert_capacity_intent, read_stop_kind};
pub use opening::{QuotaHandoffInspection, QuotaWakeReservation};
use surge_core::{
    execution_recovery::{ProviderSessionDescriptor, SessionOpenMode},
    id::{StageInvocationId, WorkItemOperationId},
};

/// Immutable selected launch recipe; it never grants permission to dispatch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawLaunch")]
pub struct QuotaLaunchContract {
    candidate: FrozenQuotaCandidate,
    provider_invocation: StageInvocationId,
    mode: SessionOpenMode,
    saved: Option<ProviderSessionDescriptor>,
}

#[derive(Deserialize)]
struct RawLaunch {
    candidate: FrozenQuotaCandidate,
    provider_invocation: StageInvocationId,
    mode: SessionOpenMode,
    saved: Option<ProviderSessionDescriptor>,
}

impl TryFrom<RawLaunch> for QuotaLaunchContract {
    type Error = WorkItemError;
    fn try_from(raw: RawLaunch) -> Result<Self> {
        Self::new(raw.candidate, raw.provider_invocation, raw.mode, raw.saved)
    }
}

impl QuotaLaunchContract {
    /// Resume/Load require the exact compatible saved descriptor.
    pub fn new(
        candidate: FrozenQuotaCandidate,
        provider_invocation: StageInvocationId,
        mode: SessionOpenMode,
        saved: Option<ProviderSessionDescriptor>,
    ) -> Result<Self> {
        invocation_key(&provider_invocation.to_string())?;
        let valid = match mode {
            SessionOpenMode::New => saved.is_none(),
            SessionOpenMode::Resume | SessionOpenMode::Load => {
                saved.as_ref().is_some_and(|value| {
                    value.invocation() == provider_invocation
                        && value.runtime() == candidate.candidate().runtime()
                        && value.launch_hash() == candidate.launch_hash()
                        && match mode {
                            SessionOpenMode::Resume => value.capabilities().resume,
                            SessionOpenMode::Load => value.capabilities().load,
                            SessionOpenMode::New => false,
                        }
                })
            },
        };
        if !valid {
            return Err(WorkItemError::Invalid(
                "incompatible quota opening contract".into(),
            ));
        }
        Ok(Self {
            candidate,
            provider_invocation,
            mode,
            saved,
        })
    }

    /// Exact frozen candidate recipe.
    pub fn candidate(&self) -> &FrozenQuotaCandidate {
        &self.candidate
    }
    /// Provider invocation retained on legitimate Resume/Load.
    pub fn provider_invocation(&self) -> StageInvocationId {
        self.provider_invocation
    }
    /// Exact operation admitted before the provider RPC.
    pub fn mode(&self) -> SessionOpenMode {
        self.mode
    }
    /// Saved identity; never substituted across runtimes.
    pub fn saved(&self) -> Option<&ProviderSessionDescriptor> {
        self.saved.as_ref()
    }
}

/// Current persisted opening state. Inspection never grants an opening permit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaHandoffState {
    /// Reserved but not externally admitted.
    Reserved,
    /// Admission spent; opening may have reached the provider.
    OpeningUnknown,
    /// Exact epoch-bearing durable opening has been confirmed.
    Established,
    /// Current journal/control permit prompting.
    Executing,
    /// A newer manual or terminal authority won.
    Invalidated,
    /// Evidence is insufficient for automatic action.
    Attention,
}

/// Immutable source of capacity suspension evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapacityEvidenceOrigin<'a> {
    /// A real provider attempt and its reservation.
    ProviderReservation(&'a str),
    /// A plan that exhausted every candidate without provider effects.
    PlannedExhaustion(&'a str),
}

/// Durable association discoverable even before suspension confirmation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CapacityControlAssociation {
    sequence: u64,
    run: RunId,
    invocation: String,
    cycle_generation: u64,
    source_revision: u64,
    source_control: u64,
    target_control: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    reservation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    planned_receipt: Option<String>,
    wake_identity: Option<String>,
}
type StoredCapacityAssociation = (
    u64,
    String,
    u64,
    u64,
    u64,
    Option<String>,
    Option<String>,
    Option<String>,
    String,
);

/// Bounded raw-history page; the cursor advances across inactive associations too.
#[derive(Debug)]
pub struct CapacityReconciliationPage {
    /// Currently owned associations requiring reconciliation; never dispatch authority.
    pub entries: Vec<CapacityControlAssociation>,
    /// Next raw sequence, within the caller's immutable high-water bound.
    pub next_cursor: Option<u64>,
}

impl CapacityControlAssociation {
    /// Monotonic bounded-page cursor.
    pub fn sequence(&self) -> u64 {
        self.sequence
    }
    /// Owning run.
    pub fn run(&self) -> RunId {
        self.run
    }
    /// Logical stage invocation.
    pub fn invocation(&self) -> &str {
        &self.invocation
    }
    /// Exact cycle preserved across the suspension.
    pub fn cycle_generation(&self) -> u64 {
        self.cycle_generation
    }
    /// Revision of the cycle from which this stop was reserved.
    pub fn source_revision(&self) -> u64 {
        self.source_revision
    }
    /// Typed immutable proof associated with the stop.
    pub fn evidence_origin(&self) -> Result<CapacityEvidenceOrigin<'_>> {
        match (&self.reservation, &self.planned_receipt) {
            (Some(receipt), None) => Ok(CapacityEvidenceOrigin::ProviderReservation(receipt)),
            (None, Some(receipt)) => Ok(CapacityEvidenceOrigin::PlannedExhaustion(receipt)),
            _ => Err(WorkItemError::Invalid(
                "capacity evidence must have exactly one origin".into(),
            )),
        }
    }
    /// Actual provider reservation; planned proofs cannot satisfy this boundary.
    pub fn provider_reservation(&self) -> Option<&str> {
        self.reservation.as_deref()
    }
    /// Newly reserved suspension generation.
    pub fn target_control(&self) -> u64 {
        self.target_control
    }
}

pub(crate) fn read_quota_handoff(
    conn: &Connection,
    operation: WorkItemOperationId,
) -> Result<QuotaHandoffInspection> {
    opening::read_handoff(conn, operation)
}

pub(crate) fn check_current_quota_handoff(
    tx: &Transaction<'_>,
    claim: &WorkItemLaunchClaim,
    handoff: &QuotaHandoffInspection,
) -> Result<()> {
    opening::current_handoff(tx, claim, handoff)
}

/// A transaction-issued one-shot opening capability; not cloneable or deserializable.
#[derive(Debug)]
pub struct QuotaOpenPermit {
    operation: WorkItemOperationId,
    run: RunId,
    control_generation: u64,
    launch: QuotaLaunchContract,
}

impl QuotaOpenPermit {
    /// Exact opening epoch included in SessionOpened evidence.
    pub fn operation(&self) -> WorkItemOperationId {
        self.operation
    }
    /// Owning run.
    pub fn run(&self) -> RunId {
        self.run
    }
    /// Current dispatch-control generation.
    pub fn control_generation(&self) -> u64 {
        self.control_generation
    }
    /// Immutable launch recipe authorized by this opening epoch.
    pub fn launch(&self) -> &QuotaLaunchContract {
        &self.launch
    }
    /// Consume this permit before issuing one provider opening RPC.
    pub fn into_opening(self) -> (WorkItemOperationId, RunId, u64, QuotaLaunchContract) {
        (
            self.operation,
            self.run,
            self.control_generation,
            self.launch,
        )
    }
}

fn trusted_stage_opening(
    store: &WorkItemStore,
    claim: &WorkItemLaunchClaim,
    logical_invocation: StageInvocationId,
    opening_seq: u64,
) -> Result<(
    surge_core::NodeKey,
    surge_core::execution_recovery::OpenedSession,
)> {
    let path = store
        .home
        .join("runs")
        .join(claim.run.to_string())
        .join("events.sqlite");
    let history = crate::runs::inspection::read_folded_events(&path, claim.run)
        .map_err(|error| WorkItemError::Invalid(error.to_string()))?;
    if !matches!(history.state, surge_core::RunState::Pipeline { .. }) {
        return Err(WorkItemError::Conflict(
            "quota stage has no live trusted journal".into(),
        ));
    }
    let journal = crate::runs::connection::RetainedConnection::read_only(&path)?;
    let (schema, payload): (u32, Vec<u8>) = journal.query_row(
        "SELECT schema_version,payload FROM events WHERE seq=?",
        [opening_seq],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let event = surge_core::migrate_payload(schema, &payload)
        .map_err(|error| WorkItemError::Invalid(error.to_string()))?;
    let surge_core::EventPayload::SessionOpened {
        node,
        opened: Some(opened),
        ..
    } = event
    else {
        return Err(WorkItemError::Invalid(
            "quota stage requires provider opening evidence".into(),
        ));
    };
    if opened.descriptor.invocation() != logical_invocation {
        return Err(WorkItemError::Conflict(
            "logical invocation differs from original opening".into(),
        ));
    }
    Ok((node, opened))
}

impl WorkItemStore {
    /// Read the immutable quota Capacity association for a requested control.
    /// This is descriptive only; it cannot confirm suspension or authorize wake.
    pub fn capacity_control_association(
        &self,
        run: RunId,
        target_control: u64,
    ) -> Result<Option<CapacityControlAssociation>> {
        let connection = self.pool.get()?;
        let row: Option<StoredCapacityAssociation> =
            connection
                .query_row(
                    "SELECT sequence,invocation,cycle_generation,source_revision,source_control,reservation,planned_receipt,wake_identity,body FROM work_item_capacity_controls WHERE run=? AND target_control=? AND invalidated=0",
                    params![run.to_string(), target_control],
                    |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                            row.get(5)?,
                            row.get(6)?,
                            row.get(7)?,
                            row.get(8)?,
                        ))
                    },
                )
                .optional()?;
        let Some((
            sequence,
            invocation,
            cycle_generation,
            source_revision,
            source_control,
            reservation,
            planned_receipt,
            wake_identity,
            body,
        )) = row
        else {
            return Ok(None);
        };
        let association = CapacityControlAssociation {
            sequence,
            run,
            invocation,
            cycle_generation,
            source_revision,
            source_control,
            target_control,
            reservation,
            planned_receipt,
            wake_identity,
        };
        let mut original = association.clone();
        original.sequence = 0;
        if serde_json::to_string(&original)? != body {
            return Err(WorkItemError::Invalid(
                "capacity association differs from immutable body".into(),
            ));
        }
        Ok(Some(association))
    }

    /// Stable upper cursor for a bounded cold-reconciliation sweep.
    pub fn capacity_reconciliation_high_water(&self) -> Result<u64> {
        Ok(self.pool.get()?.query_row(
            "SELECT COALESCE(MAX(sequence),0) FROM work_item_capacity_controls",
            [],
            |row| row.get(0),
        )?)
    }

    /// Inspect pending owned Capacity associations independently of due-wake visibility.
    /// These rows grant no control, provider-opening or cleanup authority.
    pub fn capacity_reconciliation_page(
        &self,
        after: u64,
        through: u64,
        limit: u32,
    ) -> Result<CapacityReconciliationPage> {
        if limit == 0 || limit > 100 || after > through || through > i64::MAX as u64 {
            return Err(WorkItemError::Invalid(
                "invalid capacity reconciliation page".into(),
            ));
        }
        let conn = self.pool.get()?;
        let mut statement = conn.prepare(
            "SELECT c.sequence,c.run,c.invocation,c.cycle_generation,c.source_revision,c.source_control,c.target_control,c.reservation,c.planned_receipt,c.wake_identity,c.body,EXISTS(SELECT 1 FROM work_items i JOIN work_item_attempts a ON a.run=c.run WHERE i.id=c.item AND c.reconciled=0 AND c.invalidated=0 AND i.active_run=c.run AND i.generation=c.attempt_generation AND a.generation=c.attempt_generation AND i.archived_at_ms IS NULL AND a.state IN ('reserved','launched','attention','suspended')) FROM work_item_capacity_controls c WHERE c.sequence>? AND c.sequence<=? ORDER BY c.sequence LIMIT ?",
        )?;
        let rows = statement.query_map(params![after, through, limit], |row| {
            let association = CapacityControlAssociation {
                sequence: row.get(0)?,
                run: row.get::<_, String>(1)?.parse::<RunId>().map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        1,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })?,
                invocation: row.get(2)?,
                cycle_generation: row.get(3)?,
                source_revision: row.get(4)?,
                source_control: row.get(5)?,
                target_control: row.get(6)?,
                reservation: row.get(7)?,
                planned_receipt: row.get(8)?,
                wake_identity: row.get(9)?,
            };
            Ok((
                association,
                row.get::<_, String>(10)?,
                row.get::<_, bool>(11)?,
            ))
        })?;
        let mut result = Vec::new();
        let mut count = 0;
        let mut last = None;
        for row in rows {
            let (association, body, eligible) = row?;
            count += 1;
            last = Some(association.sequence);
            if !eligible {
                continue;
            }
            if invocation_key(&association.invocation)? != association.invocation
                || association.target_control
                    != association
                        .source_control
                        .checked_add(1)
                        .ok_or_else(|| WorkItemError::Invalid("capacity control overflow".into()))?
            {
                return Err(WorkItemError::Invalid(
                    "invalid stored capacity association".into(),
                ));
            }
            let mut original = association.clone();
            original.sequence = 0;
            if serde_json::to_string(&original)? != body {
                return Err(WorkItemError::Invalid(
                    "capacity association columns differ from body".into(),
                ));
            }
            result.push(association);
        }
        Ok(CapacityReconciliationPage {
            entries: result,
            next_cursor: last.filter(|cursor| count == limit && *cursor < through),
        })
    }

    /// Reserve a provider-origin suspension after validating immutable fresh skipped routes.
    pub fn request_capacity_suspend_with_skips(
        &self,
        claim: &WorkItemLaunchClaim,
        expected: &RecoveryCycle,
        reservation: &str,
        now_ms: i64,
    ) -> Result<CapacityControlAssociation> {
        self.reserve_provider_capacity_stop(claim, expected, reservation, now_ms, true)
    }

    /// Atomically reserve a Capacity suspension and its discoverable cycle join.
    pub fn request_capacity_suspend(
        &self,
        claim: &WorkItemLaunchClaim,
        expected: &RecoveryCycle,
        reservation: &str,
        now_ms: i64,
    ) -> Result<CapacityControlAssociation> {
        self.reserve_provider_capacity_stop(claim, expected, reservation, now_ms, false)
    }

    fn reserve_provider_capacity_stop(
        &self,
        claim: &WorkItemLaunchClaim,
        expected: &RecoveryCycle,
        reservation: &str,
        now_ms: i64,
        allow_skips: bool,
    ) -> Result<CapacityControlAssociation> {
        use surge_core::execution_recovery::{ExecutionControlState, WorkItemExecutionControl};
        if now_ms < 0 {
            return Err(WorkItemError::Invalid(
                "invalid capacity observation time".into(),
            ));
        }
        let mut connection = self.pool.get()?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        owner_fence(&tx, claim)?;
        if expected.run != claim.run || invocation_key(&expected.invocation)? != expected.invocation
        {
            return Err(WorkItemError::Conflict(
                "capacity cycle identity differs".into(),
            ));
        }
        if let Some(original) = read_capacity_association(&tx, expected)? {
            let latest = control::read_control(&tx, claim.run, None)?
                .ok_or_else(|| WorkItemError::Conflict("capacity control is absent".into()))?;
            if original.source_revision != expected.revision
                || original.provider_reservation() != Some(reservation)
                || original.wake_identity
                    != expected
                        .wake
                        .as_ref()
                        .map(|wake| wake.identity().to_owned())
                || latest.generation != original.target_control
                || latest.run != claim.run
                || latest.item != claim.binding.item
                || latest.attempt_generation != claim.binding.generation
                || !matches!(
                    latest.state,
                    ExecutionControlState::SuspendRequested
                        | ExecutionControlState::Suspended
                        | ExecutionControlState::Attention
                )
            {
                return Err(WorkItemError::Conflict(
                    "capacity replay is stale or conflicts with immutable intent".into(),
                ));
            }
            return Ok(original);
        }
        let current = check_cycle(&tx, claim, expected)?;
        let stage = read_bound_stage(&tx, claim.run, &current.invocation)?;
        let project = allow_skips
            .then(|| record(&tx, claim.binding.item).map(|item| item.project))
            .transpose()?;
        ensure_candidates_unavailable(&tx, &current, &stage, now_ms, project.as_ref())?;
        let selected: String = tx.query_row(
            "SELECT runtime FROM work_item_quota_candidates WHERE run=? AND invocation=? AND cycle_generation=? AND receipt=?",
            params![claim.run.to_string(),current.invocation,current.generation,reservation],
            |row|row.get(0),
        )?;
        if Some(selected) != current.selected_runtime {
            return Err(WorkItemError::Conflict(
                "capacity reservation is not the selected candidate".into(),
            ));
        }
        let generation = current
            .control_generation
            .checked_add(1)
            .filter(|value| *value <= i64::MAX as u64)
            .ok_or_else(|| {
                WorkItemError::Conflict("capacity control generation exhausted".into())
            })?;
        let operation = WorkItemOperationId::new();
        let mut association = CapacityControlAssociation {
            sequence: 0,
            run: claim.run,
            invocation: current.invocation.clone(),
            cycle_generation: current.generation,
            source_revision: current.revision,
            source_control: current.control_generation,
            target_control: generation,
            reservation: Some(reservation.to_owned()),
            planned_receipt: None,
            wake_identity: current.wake.as_ref().map(|wake| wake.identity().to_owned()),
        };
        let body = serde_json::to_string(&association)?;
        let internal_body = serde_json::to_vec(
            &serde_json::json!({"internal":"quota_capacity_suspend_v1","association":association}),
        )?;
        tx.execute(
            "INSERT INTO work_item_operations(operation,body_hash,result) VALUES(?,?,?)",
            params![
                operation.to_string(),
                ContentHash::compute(&internal_body).to_string(),
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
            diagnostic: None,
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
        tx.commit()?;
        Ok(association)
    }

    /// Bind a logical stage to its actual original opening and frozen host policy.
    /// This inspection grants neither a candidate reservation nor opening authority.
    pub fn bind_quota_stage(
        &self,
        claim: &WorkItemLaunchClaim,
        logical_invocation: StageInvocationId,
        opening_seq: u64,
    ) -> Result<FrozenQuotaStage> {
        let invocation = invocation_key(&logical_invocation.to_string())?;
        let (node, opened) = trusted_stage_opening(self, claim, logical_invocation, opening_seq)?;
        let mut connection = self.pool.get()?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        owner_fence(&tx, claim)?;
        let attempt = attempt(&tx, claim.run)?;
        if attempt.graph.find_node(&node).is_none() {
            return Err(WorkItemError::Invalid(
                "quota node absent from accepted graph".into(),
            ));
        }
        let config: serde_json::Value = serde_json::from_str(&attempt.config)?;
        let policy: FrozenQuotaPolicy = serde_json::from_value(
            config
                .get("quota_recovery")
                .filter(|value| !value.is_null())
                .cloned()
                .ok_or_else(|| WorkItemError::Invalid("frozen quota policy is absent".into()))?,
        )?;
        let stage = policy
            .stage(&node)
            .ok_or_else(|| WorkItemError::Invalid("frozen quota stage is absent".into()))?;
        let primary = stage
            .candidates()
            .first()
            .ok_or_else(|| WorkItemError::Invalid("frozen primary candidate is absent".into()))?;
        if primary.candidate().runtime() != opened.descriptor.runtime()
            || primary.launch_hash() != opened.descriptor.launch_hash()
        {
            return Err(WorkItemError::Conflict(
                "original opening differs from frozen primary recipe".into(),
            ));
        }
        let body = serde_json::to_string(stage)?;
        let hash = policy.content_hash()?.to_string();
        let existing: Option<(u64, String, String)> = tx.query_row(
            "SELECT opening_seq,policy_hash,policy FROM work_item_quota_stages WHERE run=? AND invocation=?",
            params![claim.run.to_string(), invocation],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        ).optional()?;
        if let Some(existing) = existing {
            if existing != (opening_seq, hash, body) {
                return Err(WorkItemError::Conflict(
                    "logical quota binding is immutable".into(),
                ));
            }
        } else {
            tx.execute(
                "INSERT INTO work_item_quota_stages(run,invocation,opening_seq,node,policy_hash,policy) VALUES(?,?,?,?,?,?)",
                params![claim.run.to_string(), invocation, opening_seq, node.to_string(), hash, body],
            )?;
        }
        let result = stage.clone();
        tx.commit()?;
        Ok(result)
    }
}

type CapacityAssociationRow = (
    u64,
    u64,
    u64,
    Option<String>,
    Option<String>,
    Option<String>,
    String,
);

pub(super) fn read_capacity_association(
    conn: &Connection,
    expected: &RecoveryCycle,
) -> Result<Option<CapacityControlAssociation>> {
    let row: Option<CapacityAssociationRow> = conn.query_row(
        "SELECT sequence,source_revision,target_control,reservation,planned_receipt,wake_identity,body FROM work_item_capacity_controls WHERE run=? AND invocation=? AND cycle_generation=? AND source_control=? AND invalidated=0",
        params![expected.run.to_string(),expected.invocation,expected.generation,expected.control_generation],
        |row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?,row.get(6)?)),
    ).optional()?;
    let Some((
        sequence,
        source_revision,
        target_control,
        reservation,
        planned_receipt,
        wake_identity,
        body,
    )) = row
    else {
        return Ok(None);
    };
    let result = CapacityControlAssociation {
        sequence,
        run: expected.run,
        invocation: expected.invocation.clone(),
        cycle_generation: expected.generation,
        source_revision,
        source_control: expected.control_generation,
        target_control,
        reservation,
        planned_receipt,
        wake_identity,
    };
    let mut original = result.clone();
    original.sequence = 0;
    if serde_json::to_string(&original)? != body {
        return Err(WorkItemError::Invalid(
            "capacity association differs from immutable body".into(),
        ));
    }
    Ok(Some(result))
}

pub(super) fn read_bound_stage(
    conn: &Connection,
    run: RunId,
    invocation: &str,
) -> Result<FrozenQuotaStage> {
    let (hash, body): (String, String) = conn.query_row(
        "SELECT policy_hash,policy FROM work_item_quota_stages WHERE run=? AND invocation=?",
        params![run.to_string(), invocation],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let stage: FrozenQuotaStage = serde_json::from_str(&body)?;
    let config: serde_json::Value = serde_json::from_str(&attempt(conn, run)?.config)?;
    let policy: FrozenQuotaPolicy = serde_json::from_value(
        config
            .get("quota_recovery")
            .cloned()
            .ok_or_else(|| WorkItemError::Invalid("frozen quota policy is absent".into()))?,
    )?;
    if hash != policy.content_hash()?.to_string() || policy.stage(stage.node()) != Some(&stage) {
        return Err(WorkItemError::Conflict(
            "quota binding differs from immutable host policy".into(),
        ));
    }
    Ok(stage)
}

fn ensure_candidates_unavailable(
    conn: &Connection,
    cycle: &RecoveryCycle,
    stage: &FrozenQuotaStage,
    now_ms: i64,
    project: Option<&surge_core::id::WorkItemProjectId>,
) -> Result<()> {
    for candidate in stage.candidates() {
        if let Some(project) = project
            && super::predispatch::validated_current_skip(conn, cycle, project, candidate, now_ms)?
        {
            continue;
        }
        let evidence: Option<String> = conn.query_row(
            "SELECT observation FROM work_item_quota_candidates WHERE run=? AND invocation=? AND cycle_generation=? AND runtime=?",
            params![cycle.run.to_string(),cycle.invocation,cycle.generation,candidate.candidate().runtime()],
            |row|row.get(0),
        ).optional()?.flatten();
        let evidence = evidence
            .map(|body| serde_json::from_str::<QuotaObservation>(&body))
            .transpose()?;
        if matches!(evidence.as_ref().map(QuotaObservation::evidence),Some(QuotaEvidence::Observed {observed_at_ms,..}) if *observed_at_ms>now_ms)
        {
            return Err(WorkItemError::Conflict(
                "quota observation is from the future".into(),
            ));
        }
        if !evidence.as_ref().is_some_and(|value| {
            !matches!(
                value.evidence(),
                QuotaEvidence::Observed {
                    available: true,
                    expires_at_ms,
                    ..
                } if now_ms < *expires_at_ms
            )
        }) {
            return Err(WorkItemError::Conflict(
                "eligible candidate remains unattempted or available".into(),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate() -> FrozenQuotaCandidate {
        FrozenQuotaCandidate::new(
            RecoveryCandidate::new("provider-a".into(), AccountEvidence::Unknown).unwrap(),
            None,
            ContentHash::compute(b"actual-launch-recipe"),
        )
        .unwrap()
    }

    #[test]
    fn deserialized_restore_cannot_infer_missing_saved_identity() {
        let valid = QuotaLaunchContract::new(
            candidate(),
            StageInvocationId::new(),
            SessionOpenMode::New,
            None,
        )
        .unwrap();
        let mut raw = serde_json::to_value(&valid).unwrap();
        raw["mode"] = serde_json::json!("resume");
        assert!(serde_json::from_value::<QuotaLaunchContract>(raw.clone()).is_err());
        raw["mode"] = serde_json::json!("load");
        assert!(serde_json::from_value::<QuotaLaunchContract>(raw).is_err());
    }
}
