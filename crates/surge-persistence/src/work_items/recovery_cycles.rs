//! Durable quota routing metadata. Reads never authorize provider dispatch.
use super::*;
mod handoffs;
mod policy;
mod predispatch;
mod rate_limit;
pub use handoffs::{
    CapacityControlAssociation, CapacityEvidenceOrigin, CapacityReconciliationPage,
    CapacityStopKind, QuotaHandoffInspection, QuotaHandoffState, QuotaLaunchContract,
    QuotaOpenPermit, QuotaWakeReservation,
};
pub use policy::{FrozenQuotaCandidate, FrozenQuotaPolicy, FrozenQuotaStage, QuotaRoutingMode};
pub use predispatch::{CapacitySelection, CapacitySkip, PlannedResumeInspection};
pub(super) use rate_limit::inspect_current_receipt;
pub use rate_limit::{FreshTypedExhaustion, QuotaRateLimitMarker, QuotaRateLimitSource};

const MAX_CANDIDATES: u64 = 32;
const MAX_BACKOFF_MS: i64 = 86_400_000;

/// Revoke automatic wake eligibility in the caller's manual-control transaction.
/// Historical candidates and observations remain available for inspection.
pub(super) fn invalidate_recovery_wakes(
    tx: &Transaction<'_>,
    item: WorkItemId,
    run: RunId,
    attempt_generation: u64,
) -> Result<()> {
    tx.execute(
        "UPDATE work_item_quota_cycles SET wake_at_ms=NULL,revision=revision+1 WHERE item=? AND run=? AND attempt_generation=? AND closed=0",
        params![item.to_string(), run.to_string(), attempt_generation],
    )?;
    tx.execute(
        "UPDATE work_item_quota_handoffs SET state='invalidated',diagnostic='superseded by manual control' WHERE item=? AND run=? AND attempt_generation=? AND state IN ('reserved','opening_unknown','established','executing')",
        params![item.to_string(), run.to_string(), attempt_generation],
    )?;
    tx.execute(
        "UPDATE work_item_capacity_controls SET invalidated=1 WHERE item=? AND run=? AND attempt_generation=? AND reconciled=0",
        params![item.to_string(),run.to_string(),attempt_generation],
    )?;
    Ok(())
}

/// Account identity is known only when the provider actually exposes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AccountEvidence {
    /// Opaque provider/account identity; never credentials.
    Known {
        /// Canonical provider identity.
        provider: String,
        /// Opaque observed account identity.
        account_key: String,
    },
    /// No account identity was observed.
    Unknown,
}

/// Caller resolves aliases through the orchestrator's canonical runtime resolver.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawCandidate")]
pub struct RecoveryCandidate {
    runtime: String,
    account: AccountEvidence,
}
#[derive(Deserialize)]
struct RawCandidate {
    runtime: String,
    account: AccountEvidence,
}
impl TryFrom<RawCandidate> for RecoveryCandidate {
    type Error = WorkItemError;
    fn try_from(value: RawCandidate) -> Result<Self> {
        Self::new(value.runtime, value.account)
    }
}
fn identifier(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}
impl RecoveryCandidate {
    /// Validate bounded identifiers; canonical resolution remains the caller's responsibility.
    pub fn new(runtime: String, account: AccountEvidence) -> Result<Self> {
        if !identifier(&runtime)
            || matches!(&account, AccountEvidence::Known { provider, account_key } if !identifier(provider) || !identifier(account_key))
        {
            return Err(WorkItemError::Invalid(
                "invalid recovery candidate identity".into(),
            ));
        }
        Ok(Self { runtime, account })
    }
    /// Canonical runtime selected for this invocation only.
    pub fn runtime(&self) -> &str {
        &self.runtime
    }
    /// Observed account identity, if any.
    pub fn account(&self) -> &AccountEvidence {
        &self.account
    }
    fn key(&self) -> Result<String> {
        match &self.account {
            AccountEvidence::Known {
                provider,
                account_key,
            } => Ok(serde_json::to_string(&("known", provider, account_key))?),
            AccountEvidence::Unknown => Ok(serde_json::to_string(&("unknown", &self.runtime))?),
        }
    }
}

/// Probe evidence never converts unknown or unsupported into availability.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum QuotaEvidence {
    /// Actual provider evidence with a bounded validity interval.
    Observed {
        /// Provider evidence observation time.
        observed_at_ms: i64,
        /// Evidence validity ends here.
        expires_at_ms: i64,
        /// Whether actual evidence reports available capacity.
        available: bool,
        /// Provider-reported reset, if known.
        reset_at_ms: Option<i64>,
    },
    /// Provider exposes no supported quota probe.
    Unsupported,
    /// Probe failed or evidence could not be interpreted.
    Unknown,
}
/// Validated observation, including when decoded from storage or external JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawObservation")]
pub struct QuotaObservation {
    evidence: QuotaEvidence,
}
#[derive(Deserialize)]
struct RawObservation {
    evidence: QuotaEvidence,
}
impl TryFrom<RawObservation> for QuotaObservation {
    type Error = WorkItemError;
    fn try_from(value: RawObservation) -> Result<Self> {
        Self::new(value.evidence)
    }
}
impl QuotaObservation {
    /// Validate actual observation intervals without guessing unknown evidence.
    pub fn new(evidence: QuotaEvidence) -> Result<Self> {
        if matches!(&evidence, QuotaEvidence::Observed { observed_at_ms, expires_at_ms, reset_at_ms, .. } if *observed_at_ms < 0 || expires_at_ms <= observed_at_ms || reset_at_ms.is_some_and(|reset| reset < *observed_at_ms))
        {
            return Err(WorkItemError::Invalid(
                "invalid quota observation interval".into(),
            ));
        }
        Ok(Self { evidence })
    }
    /// Unknown evidence cannot authorize a reset wake.
    pub fn unknown() -> Self {
        Self {
            evidence: QuotaEvidence::Unknown,
        }
    }
    /// Unsupported probes leave availability unknown.
    pub fn unsupported() -> Self {
        Self {
            evidence: QuotaEvidence::Unsupported,
        }
    }
    /// Actual evidence or explicit absence of a supported observation.
    pub fn evidence(&self) -> &QuotaEvidence {
        &self.evidence
    }
}
/// Why a retry is due; policy retry does not claim provider availability.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WakeOrigin {
    /// Provider reported the reset time.
    ObservedReset,
    /// Bounded retry policy; availability remains unknown.
    PolicyBackoff,
}
/// Validated immutable wake identity and bounded timing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawWake")]
pub struct RecoveryWake {
    identity: String,
    origin: WakeOrigin,
    armed_at_ms: i64,
    due_at_ms: i64,
}
#[derive(Deserialize)]
struct RawWake {
    identity: String,
    origin: WakeOrigin,
    armed_at_ms: i64,
    due_at_ms: i64,
}
impl TryFrom<RawWake> for RecoveryWake {
    type Error = WorkItemError;
    fn try_from(value: RawWake) -> Result<Self> {
        Self::new(
            value.identity,
            value.origin,
            value.armed_at_ms,
            value.due_at_ms,
        )
    }
}
impl RecoveryWake {
    /// Policy delays are positive and at most one day.
    pub fn new(
        identity: String,
        origin: WakeOrigin,
        armed_at_ms: i64,
        due_at_ms: i64,
    ) -> Result<Self> {
        let delay = due_at_ms.checked_sub(armed_at_ms);
        if !identifier(&identity)
            || armed_at_ms < 0
            || !delay.is_some_and(|delay| {
                delay > 0 && (origin == WakeOrigin::ObservedReset || delay <= MAX_BACKOFF_MS)
            })
        {
            return Err(WorkItemError::Invalid("invalid recovery wake".into()));
        }
        Ok(Self {
            identity,
            origin,
            armed_at_ms,
            due_at_ms,
        })
    }
    /// Immutable wake identity used by consumption CAS.
    pub fn identity(&self) -> &str {
        &self.identity
    }
    /// Earliest retry time.
    pub fn due_at_ms(&self) -> i64 {
        self.due_at_ms
    }
    /// Distinguish observed resets from policy retries.
    pub fn origin(&self) -> &WakeOrigin {
        &self.origin
    }
}
/// Read snapshot, not execution authority. All mutations recheck it in SQLite.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryCycle {
    /// Immutable attempt run.
    pub run: RunId,
    /// Immutable stage invocation.
    pub invocation: String,
    /// Retry cycle ordinal, scoped to invocation.
    pub generation: u64,
    /// Control authorization generation.
    pub control_generation: u64,
    /// Optimistic metadata version.
    pub revision: u64,
    /// Historical cycle no longer accepts new dispatches.
    pub closed: bool,
    /// Selected invocation runtime override.
    pub selected_runtime: Option<String>,
    /// Checked retry audit count retained across cycle rollovers.
    pub probe_count: u64,
    /// Scheduled retry; never proves available quota.
    pub wake: Option<RecoveryWake>,
}
/// Durable reservation receipt; replay must not dispatch again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateReservation {
    /// Durable immutable reservation identity.
    pub receipt: String,
    /// Original immutable candidate body.
    pub candidate: RecoveryCandidate,
    /// First reservation versus read-only replay.
    pub disposition: ReservationDisposition,
}
/// Only `Reserved` grants a first dispatch opportunity under the retained claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReservationDisposition {
    /// First reservation; dispatch under the retained claim is permitted.
    Reserved,
    /// Existing reservation; provider dispatch is forbidden.
    Replayed,
}

fn owner_fence(tx: &Transaction<'_>, claim: &WorkItemLaunchClaim) -> Result<()> {
    claim.verify_lock()?;
    let valid: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM work_item_attempts a JOIN work_items i ON i.id=a.item WHERE a.run=? AND a.generation=? AND a.claim_token=? AND a.item=? AND i.active_run=a.run AND i.generation=a.generation AND i.archived_at_ms IS NULL AND a.state IN ('reserved','launched','suspended'))", params![claim.run.to_string(), claim.binding.generation, claim.token,claim.binding.item.to_string()], |row|row.get(0))?;
    if !valid {
        return Err(WorkItemError::Conflict("obsolete recovery owner".into()));
    }
    Ok(())
}
fn fence(tx: &Transaction<'_>, claim: &WorkItemLaunchClaim, control: u64) -> Result<()> {
    owner_fence(tx, claim)?;
    let latest = control::read_control(tx, claim.run, None)?;
    if latest.as_ref().map_or(0, |value| value.generation) != control
        || latest.is_some_and(|value| {
            value.run != claim.run
                || value.item != claim.binding.item
                || value.attempt_generation != claim.binding.generation
                || !matches!(
                    value.state,
                    surge_core::execution_recovery::ExecutionControlState::Executing
                )
        })
    {
        return Err(WorkItemError::Conflict(
            "obsolete recovery ownership/control".into(),
        ));
    }
    Ok(())
}
pub(super) fn read_cycle(
    conn: &Connection,
    run: RunId,
    invocation: &str,
    generation: u64,
) -> Result<RecoveryCycle> {
    let invocation = invocation_key(invocation)?;
    let (control_generation, revision, closed, selected_runtime, probe_count, wake): (u64,u64,bool,Option<String>,u64,Option<String>) = conn.query_row("SELECT control_generation,revision,closed,selected_runtime,probe_count,wake FROM work_item_quota_cycles WHERE run=? AND invocation=? AND cycle_generation=?", params![run.to_string(),invocation,generation], |row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?)))?;
    Ok(RecoveryCycle {
        run,
        invocation,
        generation,
        control_generation,
        revision,
        closed,
        selected_runtime,
        probe_count,
        wake: wake.map(|value| serde_json::from_str(&value)).transpose()?,
    })
}
/// Shared read-only predicates: refusal does not manufacture new quota lineage.
pub(super) fn validate_capacity_transfer(
    conn: &Connection,
    binding_run: RunId,
    binding: &WorkItemBinding,
    expected: &RecoveryCycle,
    new_control: u64,
) -> Result<bool> {
    let current = read_cycle(conn, binding_run, &expected.invocation, expected.generation)?;
    if expected.run != binding_run
        || current.invocation != expected.invocation
        || current.closed
        || current.revision != expected.revision
        || current.control_generation != expected.control_generation
        || expected.control_generation.checked_add(1) != Some(new_control)
    {
        return Ok(false);
    }
    let latest = control::read_control(conn, binding_run, None)?.ok_or(WorkItemError::NotFound)?;
    if latest.generation != new_control
        || latest.run != binding_run
        || latest.attempt_generation != binding.generation
        || latest.item != binding.item
        || !matches!(
            latest.state,
            surge_core::execution_recovery::ExecutionControlState::Suspended
                | surge_core::execution_recovery::ExecutionControlState::Executing
        )
        || !capacity_fence(conn, &latest, &current.invocation)?
    {
        return Ok(false);
    }
    let raw:Option<String>=conn.query_row("SELECT observation FROM work_item_quota_candidates WHERE run=? AND invocation=? AND cycle_generation=? AND runtime=? AND observation IS NOT NULL",params![binding_run.to_string(),current.invocation,current.generation,current.selected_runtime],|row|row.get(0)).optional()?;
    let evidence = raw
        .map(|value| serde_json::from_str::<QuotaObservation>(&value))
        .transpose()?
        .is_some_and(|value| {
            !matches!(
                value.evidence(),
                QuotaEvidence::Observed {
                    available: true,
                    ..
                }
            )
        });
    let planned = handoffs::read_capacity_association(conn, &current)?
        .map(|association| {
            association
                .evidence_origin()
                .map(|origin| matches!(origin, CapacityEvidenceOrigin::PlannedExhaustion(_)))
        })
        .transpose()?
        .unwrap_or(false);
    if !evidence && !(planned && current.selected_runtime.is_none()) {
        return Ok(false);
    }
    Ok(true)
}

fn check_cycle(
    tx: &Transaction<'_>,
    claim: &WorkItemLaunchClaim,
    expected: &RecoveryCycle,
) -> Result<RecoveryCycle> {
    if expected.run != claim.run {
        return Err(WorkItemError::Conflict("wrong recovery run".into()));
    }
    fence(tx, claim, expected.control_generation)?;
    let current = read_cycle(tx, expected.run, &expected.invocation, expected.generation)?;
    if current.closed
        || current.invocation != expected.invocation
        || current.revision != expected.revision
        || current.control_generation != expected.control_generation
    {
        return Err(WorkItemError::Conflict(
            "obsolete recovery cycle revision".into(),
        ));
    }
    Ok(current)
}

fn apply_quota_observation(
    tx: &Transaction<'_>,
    current: &RecoveryCycle,
    receipt: &str,
    observation: &QuotaObservation,
) -> Result<RecoveryCycle> {
    let (prior_exhaustion, prior_observed): (Option<String>, Option<String>) = tx.query_row(
        "SELECT exhaustion,observed FROM work_item_quota_candidates WHERE run=? AND invocation=? AND cycle_generation=? AND receipt=?",
        params![current.run.to_string(),current.invocation,current.generation,receipt],
        |row| Ok((row.get(0)?,row.get(1)?)),
    )?;
    let mut exhaustion = prior_exhaustion
        .map(|value| serde_json::from_str::<QuotaObservation>(&value))
        .transpose()?;
    let mut known = prior_observed
        .map(|value| serde_json::from_str::<QuotaObservation>(&value))
        .transpose()?;
    if let QuotaEvidence::Observed {
        observed_at_ms,
        available,
        ..
    } = observation.evidence()
    {
        if matches!(known.as_ref().map(QuotaObservation::evidence),Some(QuotaEvidence::Observed {observed_at_ms:prior,..}) if prior>=observed_at_ms)
            && known.as_ref() != Some(observation)
        {
            return Err(WorkItemError::Conflict("outdated quota observation".into()));
        }
        exhaustion = if *available {
            None
        } else {
            Some(observation.clone())
        };
        known = Some(observation.clone());
    }
    tx.execute(
        "UPDATE work_item_quota_candidates SET observation=?,exhaustion=?,observed=? WHERE run=? AND invocation=? AND cycle_generation=? AND receipt=?",
        params![serde_json::to_string(observation)?,exhaustion.map(|value|serde_json::to_string(&value)).transpose()?,known.map(|value|serde_json::to_string(&value)).transpose()?,current.run.to_string(),current.invocation,current.generation,receipt],
    )?;
    bump(tx, current)?;
    read_cycle(tx, current.run, &current.invocation, current.generation)
}

fn reserve_on_transaction(
    tx: &Transaction<'_>,
    claim: &WorkItemLaunchClaim,
    expected: &RecoveryCycle,
    candidate: &RecoveryCandidate,
) -> Result<CandidateReservation> {
    fence(tx, claim, expected.control_generation)?;
    if expected.run != claim.run {
        return Err(WorkItemError::Conflict("wrong recovery run".into()));
    }
    let key = candidate.key()?;
    let body = serde_json::to_string(candidate)?;
    let prior:Option<(String,String)>=tx.query_row("SELECT receipt,body FROM work_item_quota_candidates WHERE run=? AND invocation=? AND cycle_generation=? AND (candidate_key=? OR runtime=?)",params![expected.run.to_string(),expected.invocation,expected.generation,key,candidate.runtime],|row|Ok((row.get(0)?,row.get(1)?))).optional()?;
    if let Some((receipt, original)) = prior {
        if original != body {
            return Err(WorkItemError::Conflict(
                "candidate key has a different immutable body".into(),
            ));
        }
        return Ok(CandidateReservation {
            receipt,
            candidate: serde_json::from_str(&original)?,
            disposition: ReservationDisposition::Replayed,
        });
    }
    check_cycle(tx, claim, expected)?;
    let count:u64=tx.query_row("SELECT COUNT(*) FROM work_item_quota_candidates WHERE run=? AND invocation=? AND cycle_generation=?",params![expected.run.to_string(),expected.invocation,expected.generation],|row|row.get(0))?;
    if count >= MAX_CANDIDATES {
        return Err(WorkItemError::Conflict(
            "recovery candidate budget exhausted".into(),
        ));
    }
    let receipt = RunId::new().to_string();
    tx.execute("INSERT INTO work_item_quota_candidates(run,invocation,cycle_generation,candidate_key,runtime,receipt,body) VALUES(?,?,?,?,?,?,?)",params![expected.run.to_string(),expected.invocation,expected.generation,key,candidate.runtime,receipt,body])?;
    tx.execute("UPDATE work_item_quota_cycles SET selected_runtime=?,revision=revision+1 WHERE run=? AND invocation=? AND cycle_generation=?",params![candidate.runtime,expected.run.to_string(),expected.invocation,expected.generation])?;
    Ok(CandidateReservation {
        receipt,
        candidate: candidate.clone(),
        disposition: ReservationDisposition::Reserved,
    })
}

impl WorkItemStore {
    /// Begin one cycle without altering the frozen graph, configuration or sessions.
    pub fn begin_recovery_cycle(
        &self,
        claim: &WorkItemLaunchClaim,
        invocation: &str,
        control_generation: u64,
    ) -> Result<RecoveryCycle> {
        let canonical = invocation_key(invocation)?;
        let invocation = canonical.as_str();
        let mut conn = self.pool.get()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        fence(&tx, claim, control_generation)?;
        let existing: Option<u64> = tx.query_row("SELECT cycle_generation FROM work_item_quota_cycles WHERE run=? AND invocation=? AND closed=0",params![claim.run.to_string(),invocation],|row|row.get(0)).optional()?;
        if let Some(generation) = existing {
            let cycle = read_cycle(&tx, claim.run, invocation, generation)?;
            if cycle.control_generation != control_generation {
                return Err(WorkItemError::Conflict("cycle control changed".into()));
            }
            return Ok(cycle);
        }
        let previous:u64=tx.query_row("SELECT COALESCE(MAX(cycle_generation),0) FROM work_item_quota_cycles WHERE run=? AND invocation=?",params![claim.run.to_string(),invocation],|row|row.get(0))?;
        if previous != 0 {
            return Err(WorkItemError::Conflict(
                "closed cycle requires explicit wake rollover".into(),
            ));
        }
        insert_cycle(&tx, claim, invocation, 1, control_generation, 0)?;
        let cycle = read_cycle(&tx, claim.run, invocation, 1)?;
        tx.commit()?;
        Ok(cycle)
    }
    /// Persist before dispatch. Exact replay returns no additional dispatch grant.
    pub fn reserve_recovery_candidate(
        &self,
        claim: &WorkItemLaunchClaim,
        expected: &RecoveryCycle,
        candidate: &RecoveryCandidate,
    ) -> Result<CandidateReservation> {
        let mut conn = self.pool.get()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let result = reserve_on_transaction(&tx, claim, expected, candidate)?;
        tx.commit()?;
        Ok(result)
    }
    /// Reserve the next frozen candidate only after every prior reservation
    /// has a host-sealed typed exhaustion marker. Replays never grant a second
    /// dispatch and an opening whose result is uncertain blocks rotation.
    pub fn reserve_next_candidate_after_exhaustion(
        &self,
        claim: &WorkItemLaunchClaim,
        expected: &RecoveryCycle,
        policy: &FrozenQuotaStage,
    ) -> Result<Option<CandidateReservation>> {
        if policy.mode() == QuotaRoutingMode::Disabled {
            return Ok(None);
        }
        if expected.run != claim.run {
            return Err(WorkItemError::Conflict("wrong recovery run".into()));
        }
        let conn = self.pool.get()?;
        let bound = handoffs::read_bound_stage(&conn, claim.run, &expected.invocation)?;
        if &bound != policy {
            return Err(WorkItemError::Conflict(
                "candidate selection differs from the bound frozen stage".into(),
            ));
        }
        let current = read_cycle(
            &conn,
            expected.run,
            &expected.invocation,
            expected.generation,
        )?;
        if current != *expected || current.closed {
            return Err(WorkItemError::Conflict(
                "quota cycle is stale or closed".into(),
            ));
        }
        drop(conn);
        let reservations =
            self.recovery_reservations(expected.run, &expected.invocation, expected.generation)?;
        if reservations.is_empty()
            || !reservations
                .iter()
                .any(|value| current.selected_runtime.as_deref() == Some(value.candidate.runtime()))
        {
            return Err(WorkItemError::Conflict(
                "quota cycle has no exact selected reservation".into(),
            ));
        }
        for reservation in &reservations {
            if self.typed_rate_limit(&reservation.receipt)?.is_none() {
                return Err(WorkItemError::Conflict(
                    "candidate exhaustion is not durably established".into(),
                ));
            }
        }
        let attempted = reservations
            .iter()
            .map(|reservation| reservation.candidate.runtime())
            .collect::<std::collections::BTreeSet<_>>();
        let Some(next) = policy
            .candidates()
            .iter()
            .find(|candidate| !attempted.contains(candidate.candidate().runtime()))
        else {
            return Ok(None);
        };
        let reservation = self.reserve_recovery_candidate(claim, &current, next.candidate())?;
        Ok((reservation.disposition == ReservationDisposition::Reserved).then_some(reservation))
    }
    /// Exact historical cycle metadata; never a dispatch authorization.
    pub fn recovery_cycle(
        &self,
        run: RunId,
        invocation: &str,
        generation: u64,
    ) -> Result<RecoveryCycle> {
        read_cycle(&*self.pool.get()?, run, invocation, generation)
    }
    /// Frozen task-owned quota policy bound to a logical stage invocation.
    /// Inspection grants no candidate reservation or provider-open authority.
    pub fn bound_quota_stage(&self, run: RunId, invocation: &str) -> Result<FrozenQuotaStage> {
        let connection = self.pool.get()?;
        handoffs::read_bound_stage(&connection, run, invocation)
    }
    /// Bounded historical receipts for restart selection. Every result is a replay.
    pub fn recovery_reservations(
        &self,
        run: RunId,
        invocation: &str,
        generation: u64,
    ) -> Result<Vec<CandidateReservation>> {
        let invocation = invocation_key(invocation)?;
        let conn = self.pool.get()?;
        let mut statement=conn.prepare("SELECT receipt,body FROM work_item_quota_candidates WHERE run=? AND invocation=? AND cycle_generation=? ORDER BY receipt LIMIT 33")?;
        let rows = statement
            .query_map(params![run.to_string(), invocation, generation], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        if rows.len() > MAX_CANDIDATES as usize {
            return Err(WorkItemError::Invalid(
                "stored recovery candidate budget exceeded".into(),
            ));
        }
        rows.into_iter()
            .map(|(receipt, body)| {
                Ok(CandidateReservation {
                    receipt,
                    candidate: serde_json::from_str(&body)?,
                    disposition: ReservationDisposition::Replayed,
                })
            })
            .collect()
    }
    /// Record an exact reservation's probe, fenced against late delivery.
    pub fn record_recovery_observation(
        &self,
        claim: &WorkItemLaunchClaim,
        expected: &RecoveryCycle,
        receipt: &str,
        observation: &QuotaObservation,
    ) -> Result<RecoveryCycle> {
        let mut conn = self.pool.get()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = check_cycle(&tx, claim, expected)?;
        let current = apply_quota_observation(&tx, &current, receipt, observation)?;
        tx.commit()?;
        Ok(current)
    }
    /// Retain unknown probes separately from the last observed exhausted evidence.
    pub fn recovery_observation(
        &self,
        receipt: &str,
    ) -> Result<(Option<QuotaObservation>, Option<QuotaObservation>)> {
        let conn = self.pool.get()?;
        let (latest, exhaustion): (Option<String>, Option<String>) = conn.query_row(
            "SELECT observation,exhaustion FROM work_item_quota_candidates WHERE receipt=?",
            [receipt],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        Ok((
            latest
                .map(|value| serde_json::from_str(&value))
                .transpose()?,
            exhaustion
                .map(|value| serde_json::from_str(&value))
                .transpose()?,
        ))
    }
    /// Arm a bounded policy wake or a reset supported by unexpired actual evidence.
    pub fn arm_recovery_wake(
        &self,
        claim: &WorkItemLaunchClaim,
        expected: &RecoveryCycle,
        receipt: &str,
        wake: &RecoveryWake,
    ) -> Result<RecoveryCycle> {
        let mut conn = self.pool.get()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = check_cycle(&tx, claim, expected)?;
        if current.probe_count >= i64::MAX as u64 || current.wake.is_some() {
            return Err(WorkItemError::Conflict(
                "wake already armed or probe counter overflow".into(),
            ));
        }
        let (observation,exhaustion):(Option<String>,Option<String>)=tx.query_row("SELECT observation,exhaustion FROM work_item_quota_candidates WHERE run=? AND invocation=? AND cycle_generation=? AND receipt=?",params![expected.run.to_string(),expected.invocation,expected.generation,receipt],|row|Ok((row.get(0)?,row.get(1)?)))?;
        validate_wake_evidence(wake, observation.as_deref(), exhaustion.as_deref())?;
        tx.execute("UPDATE work_item_quota_cycles SET wake=?,wake_at_ms=?,probe_count=probe_count+1,revision=revision+1 WHERE run=? AND invocation=? AND cycle_generation=?",params![serde_json::to_string(wake)?,wake.due_at_ms,expected.run.to_string(),expected.invocation,expected.generation])?;
        let current = read_cycle(&tx, expected.run, &expected.invocation, expected.generation)?;
        tx.commit()?;
        Ok(current)
    }
    /// Atomically consume one due wake and preserve the prior cycle as history.
    pub fn consume_recovery_wake(
        &self,
        claim: &WorkItemLaunchClaim,
        expected: &RecoveryCycle,
        wake_identity: &str,
        now_ms: i64,
    ) -> Result<RecoveryCycle> {
        let mut conn = self.pool.get()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = check_cycle(&tx, claim, expected)?;
        let actionable_at: Option<i64> = tx.query_row(
            "SELECT wake_at_ms FROM work_item_quota_cycles WHERE run=? AND invocation=? AND cycle_generation=?",
            params![current.run.to_string(),current.invocation,current.generation],
            |row|row.get(0),
        )?;
        if !current.wake.as_ref().is_some_and(|wake| {
            wake.identity == wake_identity
                && wake.due_at_ms <= now_ms
                && actionable_at == Some(wake.due_at_ms)
        }) {
            return Err(WorkItemError::Conflict(
                "wake identity changed or is not due".into(),
            ));
        }
        let next = current
            .generation
            .checked_add(1)
            .filter(|value| *value <= i64::MAX as u64)
            .ok_or_else(|| WorkItemError::Invalid("cycle generation overflow".into()))?;
        tx.execute("UPDATE work_item_quota_cycles SET closed=1,wake_at_ms=NULL,revision=revision+1 WHERE run=? AND invocation=? AND cycle_generation=?",params![expected.run.to_string(),expected.invocation,expected.generation])?;
        insert_cycle(
            &tx,
            claim,
            &expected.invocation,
            next,
            current.control_generation,
            current.probe_count,
        )?;
        let next = read_cycle(&tx, expected.run, &expected.invocation, next)?;
        tx.commit()?;
        Ok(next)
    }
    /// Close a cycle while keeping its candidate receipts and probe evidence.
    pub fn finish_recovery_cycle(
        &self,
        claim: &WorkItemLaunchClaim,
        expected: &RecoveryCycle,
    ) -> Result<()> {
        let mut conn = self.pool.get()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        check_cycle(&tx, claim, expected)?;
        tx.execute("UPDATE work_item_quota_cycles SET closed=1,wake_at_ms=NULL,revision=revision+1 WHERE run=? AND invocation=? AND cycle_generation=?",params![expected.run.to_string(),expected.invocation,expected.generation])?;
        tx.commit()?;
        Ok(())
    }
    /// Advance exactly one automatic capacity control; manual controls cannot revive wakes.
    pub fn rebind_recovery_control(
        &self,
        claim: &WorkItemLaunchClaim,
        expected: &RecoveryCycle,
        new_control: u64,
    ) -> Result<RecoveryCycle> {
        let mut conn = self.pool.get()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        owner_fence(&tx, claim)?;
        if !validate_capacity_transfer(&tx, claim.run, &claim.binding, expected, new_control)? {
            return Err(WorkItemError::Conflict(
                "obsolete capacity control transfer".into(),
            ));
        }
        let current = read_cycle(&tx, claim.run, &expected.invocation, expected.generation)?;
        tx.execute("UPDATE work_item_quota_cycles SET control_generation=?,revision=revision+1 WHERE run=? AND invocation=? AND cycle_generation=?",params![new_control,current.run.to_string(),current.invocation,current.generation])?;
        let current = read_cycle(&tx, current.run, &current.invocation, current.generation)?;
        tx.commit()?;
        Ok(current)
    }
    /// Bounded indexed discovery. A returned row grants no wake or dispatch authority.
    pub fn due_recovery_wakes(&self, now_ms: i64, limit: u32) -> Result<Vec<RecoveryCycle>> {
        let connection = self.pool.get()?;
        due_recovery_wakes(&connection, None, now_ms, limit)
    }
    /// Bounded wake discovery scoped to one run, for schedulers already polling
    /// the parked-run index. Returned rows grant no wake or dispatch authority.
    pub fn due_recovery_wakes_for_run(
        &self,
        run: RunId,
        now_ms: i64,
        limit: u32,
    ) -> Result<Vec<RecoveryCycle>> {
        let connection = self.pool.get()?;
        due_recovery_wakes(&connection, Some(run), now_ms, limit)
    }
}
fn due_recovery_wakes(
    conn: &Connection,
    run_filter: Option<RunId>,
    now_ms: i64,
    limit: u32,
) -> Result<Vec<RecoveryCycle>> {
    if now_ms < 0 || limit == 0 || limit > 100 {
        return Err(WorkItemError::Invalid(
            "invalid due-wake query bound".into(),
        ));
    }
    let sql = if run_filter.is_some() {
        "SELECT q.run,q.invocation,q.cycle_generation FROM work_item_quota_cycles q JOIN work_item_attempts a ON a.run=q.run JOIN work_items i ON i.id=q.item WHERE q.closed=0 AND q.wake_at_ms<=? AND q.run=? AND a.generation=q.attempt_generation AND i.active_run=q.run AND i.generation=q.attempt_generation AND i.archived_at_ms IS NULL AND a.state IN ('reserved','launched','suspended') ORDER BY q.wake_at_ms,q.run,q.invocation LIMIT ?"
    } else {
        "SELECT q.run,q.invocation,q.cycle_generation FROM work_item_quota_cycles q JOIN work_item_attempts a ON a.run=q.run JOIN work_items i ON i.id=q.item WHERE q.closed=0 AND q.wake_at_ms<=? AND a.generation=q.attempt_generation AND i.active_run=q.run AND i.generation=q.attempt_generation AND i.archived_at_ms IS NULL AND a.state IN ('reserved','launched','suspended') ORDER BY q.wake_at_ms,q.run,q.invocation LIMIT ?"
    };
    let keys = if let Some(run) = run_filter {
        conn.prepare(sql)?
            .query_map(params![now_ms, run.to_string(), limit], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, u64>(2)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?
    } else {
        conn.prepare(sql)?
            .query_map(params![now_ms, limit], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, u64>(2)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?
    };
    let mut result = Vec::new();
    for (run, invocation, generation) in keys {
        let run = run
            .parse::<RunId>()
            .map_err(|error| WorkItemError::Invalid(error.to_string()))?;
        let cycle = read_cycle(conn, run, &invocation, generation)?;
        let control = control::read_control(conn, run, None)?;
        let eligible = match control {
            None => cycle.control_generation == 0,
            Some(value)
                if value.state
                    == surge_core::execution_recovery::ExecutionControlState::Executing =>
            {
                value.generation == cycle.control_generation
            },
            Some(value)
                if value.state
                    == surge_core::execution_recovery::ExecutionControlState::Suspended =>
            {
                capacity_fence(conn, &value, &invocation)?
                    && (value.generation == cycle.control_generation
                        || cycle.control_generation.checked_add(1) == Some(value.generation))
            },
            Some(_) => false,
        };
        if eligible {
            result.push(cycle);
        }
    }
    Ok(result)
}
fn invocation_key(value: &str) -> Result<String> {
    let parsed = value
        .parse::<surge_core::id::StageInvocationId>()
        .map_err(|error| WorkItemError::Invalid(error.to_string()))?;
    if parsed.as_ulid().to_bytes() == [0; 16] {
        return Err(WorkItemError::Invalid("nil stage invocation".into()));
    }
    Ok(parsed.to_string())
}
fn capacity_fence(
    conn: &Connection,
    control: &surge_core::execution_recovery::WorkItemExecutionControl,
    invocation: &str,
) -> Result<bool> {
    use surge_core::execution_recovery::{PendingStagePhase, SuspensionReason};
    let Some(fence) = control.fence.as_ref().filter(|fence| {
        fence.cleanup_confirmed && matches!(fence.reason, SuspensionReason::Capacity { .. })
    }) else {
        return Ok(false);
    };
    if let PendingStagePhase::PlannedCapacity {
        node,
        logical_invocation,
        stage_entry_seq,
        plan_seq,
    } = &fence.pending_stage
    {
        return predispatch::planned_capacity_fence(
            conn,
            control,
            invocation,
            node,
            *logical_invocation,
            *stage_entry_seq,
            *plan_seq,
        );
    }
    let PendingStagePhase::Interrupted {
        invocation: pending,
        node,
    } = &fence.pending_stage
    else {
        return Ok(false);
    };
    if pending.to_string() == invocation {
        return Ok(true);
    }
    let authorized: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM work_item_quota_handoffs h JOIN work_item_quota_stages s ON s.run=h.run AND s.invocation=h.logical_invocation JOIN work_item_quota_candidates c ON c.receipt=h.reservation WHERE h.run=? AND h.item=? AND h.attempt_generation=? AND h.logical_invocation=? AND h.provider_invocation=? AND s.node=? AND h.state IN ('established','executing') AND c.typed_exhaustion IS NOT NULL)",
        params![
            control.run.to_string(),
            control.item.to_string(),
            control.attempt_generation,
            invocation,
            pending.to_string(),
            node.to_string(),
        ],
        |row| row.get(0),
    )?;
    Ok(authorized)
}
fn bump(tx: &Transaction<'_>, cycle: &RecoveryCycle) -> Result<()> {
    tx.execute("UPDATE work_item_quota_cycles SET revision=revision+1 WHERE run=? AND invocation=? AND cycle_generation=?",params![cycle.run.to_string(),cycle.invocation,cycle.generation])?;
    Ok(())
}
fn validate_wake_evidence(
    wake: &RecoveryWake,
    observation: Option<&str>,
    exhaustion: Option<&str>,
) -> Result<()> {
    let latest = observation
        .map(serde_json::from_str::<QuotaObservation>)
        .transpose()?;
    let exhausted = exhaustion
        .map(serde_json::from_str::<QuotaObservation>)
        .transpose()?;
    let valid = match wake.origin {
        WakeOrigin::ObservedReset => {
            matches!(exhausted.as_ref().map(QuotaObservation::evidence),Some(QuotaEvidence::Observed {available:false,observed_at_ms,expires_at_ms,reset_at_ms:Some(reset)}) if *observed_at_ms<=wake.armed_at_ms && *expires_at_ms>wake.armed_at_ms && *reset==wake.due_at_ms)
        },
        WakeOrigin::PolicyBackoff => latest.as_ref().is_some_and(|value| {
            !matches!(
                value.evidence(),
                QuotaEvidence::Observed {
                    available: true,
                    ..
                }
            )
        }),
    };
    if !valid {
        return Err(WorkItemError::Conflict(
            "wake lacks matching quota/policy evidence".into(),
        ));
    }
    Ok(())
}
fn insert_cycle(
    tx: &Transaction<'_>,
    claim: &WorkItemLaunchClaim,
    invocation: &str,
    generation: u64,
    control: u64,
    probes: u64,
) -> Result<()> {
    tx.execute("INSERT INTO work_item_quota_cycles(run,invocation,cycle_generation,item,attempt_generation,control_generation,revision,probe_count) VALUES(?,?,?,?,?,?,1,?)",params![claim.run.to_string(),invocation,generation,claim.binding.item.to_string(),claim.binding.generation,control,probes])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime_home_fixture::FixtureHome;
    use surge_core::id::WorkItemOperationId;

    async fn fixture() -> (FixtureHome, (WorkItemStore, WorkItemLaunchClaim)) {
        let home = FixtureHome::new().unwrap();
        let storage = crate::runs::Storage::open(home.path()).await.unwrap();
        let store = storage.work_items();
        let intent = WorkItemWorkspace {
            repository: home.path().join("repo/.git"),
            checkout: home.path().join("repo"),
            path: home.path().join("retained"),
            ownership: RunId::new().to_string(),
            branch: "retained".into(),
            base_commit: "a".repeat(40),
        };
        let create = WorkItemCommand::Create {
            operation_id: WorkItemOperationId::new(),
            project: intent.checkout.clone(),
            title: "Quota".into(),
            requirements: WorkItemRequirements::new(
                "Original".into(),
                vec!["Keep attempts".into()],
            )
            .unwrap(),
        };
        let WorkItemResult::Detail(detail) = store
            .mutate(&create, Some(&intent), None, "human", 1)
            .unwrap()
        else {
            panic!("detail")
        };
        let start = WorkItemCommand::Start {
            operation_id: WorkItemOperationId::new(),
            item: detail.item.id,
            expected_version: detail.item.version,
            graph: Box::new(
                toml::from_str(include_str!("../../../../examples/flow_terminal_only.toml"))
                    .unwrap(),
            ),
            quota_recovery: None,
        };
        let WorkItemResult::Attempt(attempt) =
            store.mutate(&start, None, Some("{}"), "host", 2).unwrap()
        else {
            panic!("attempt")
        };
        let claim = store.claim(attempt.run).unwrap();
        (home, (store, claim))
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn reservation_and_unknown_evidence_survive_reopen_without_redispatch() {
        let (home, owners) = fixture().await;
        {
            let (store, claim) = owners;
            let invocation = "invocation-01ARZ3NDEKTSV4RRFFQ69G5FAV";
            let first = RecoveryCandidate::new("claude".into(), AccountEvidence::Unknown).unwrap();
            let cycle = store.begin_recovery_cycle(&claim, invocation, 0).unwrap();
            let receipt = store
                .reserve_recovery_candidate(&claim, &cycle, &first)
                .unwrap();
            assert_eq!(receipt.disposition, ReservationDisposition::Reserved);
            let cycle = store.recovery_cycle(claim.run(), invocation, 1).unwrap();
            store
                .record_recovery_observation(
                    &claim,
                    &cycle,
                    &receipt.receipt,
                    &QuotaObservation::unknown(),
                )
                .unwrap();
            let reopened = crate::runs::Storage::open(home.path())
                .await
                .unwrap()
                .work_items();
            let cycle = reopened.recovery_cycle(claim.run(), invocation, 1).unwrap();
            assert_eq!(
                reopened.recovery_observation(&receipt.receipt).unwrap(),
                (Some(QuotaObservation::unknown()), None)
            );
            let replay = reopened
                .reserve_recovery_candidate(&claim, &cycle, &first)
                .unwrap();
            assert_eq!(replay.receipt, receipt.receipt);
            assert_eq!(replay.disposition, ReservationDisposition::Replayed);
            assert_eq!(
                reopened
                    .recovery_reservations(claim.run(), invocation, 1)
                    .unwrap(),
                vec![replay.clone()]
            );
            assert_eq!(
                reopened.recovery_cycle(claim.run(), invocation, 1).unwrap(),
                cycle
            );
            let second = RecoveryCandidate::new("codex".into(), AccountEvidence::Unknown).unwrap();
            assert_eq!(
                reopened
                    .reserve_recovery_candidate(&claim, &cycle, &second)
                    .unwrap()
                    .disposition,
                ReservationDisposition::Reserved
            );
        }
        home.close().expect("close runtime home");
    }
    const INVOCATION: &str = "invocation-01ARZ3NDEKTSV4RRFFQ69G5FAV";
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn bare_and_prefixed_invocation_share_one_cycle_namespace() {
        let (home, owners) = fixture().await;
        {
            let (store, claim) = owners;
            let prefixed = store.begin_recovery_cycle(&claim, INVOCATION, 0).unwrap();
            let bare = INVOCATION.strip_prefix("invocation-").unwrap();
            assert_eq!(
                store.begin_recovery_cycle(&claim, bare, 0).unwrap(),
                prefixed
            );
            assert_eq!(
                store.recovery_cycle(claim.run(), bare, 1).unwrap(),
                prefixed
            );
            let mut forged = prefixed.clone();
            forged.invocation = bare.into();
            let result = store.finish_recovery_cycle(&claim, &forged);
            let after = store.recovery_cycle(claim.run(), INVOCATION, 1).unwrap();
            match result {
                Ok(()) => assert!(
                    after.closed,
                    "successful finish must durably close the canonical cycle"
                ),
                Err(WorkItemError::Conflict(_)) => assert_eq!(after, prefixed),
                Err(error) => panic!("unexpected finish failure: {error}"),
            }
            assert!(
                store
                    .reserve_recovery_candidate(&claim, &forged, &candidate("claude"))
                    .is_err()
            );
            assert_eq!(
                store.recovery_cycle(claim.run(), INVOCATION, 1).unwrap(),
                prefixed
            );
            assert!(
                store
                    .begin_recovery_cycle(
                        &claim,
                        &surge_core::id::StageInvocationId::nil().to_string(),
                        0
                    )
                    .is_err()
            );
        }
        home.close().expect("close runtime home");
    }
    fn candidate(runtime: &str) -> RecoveryCandidate {
        RecoveryCandidate::new(runtime.into(), AccountEvidence::Unknown).unwrap()
    }
    fn observed(at: i64, available: bool) -> QuotaObservation {
        QuotaObservation::new(QuotaEvidence::Observed {
            observed_at_ms: at,
            expires_at_ms: at + 10000,
            available,
            reset_at_ms: Some(at + 1000),
        })
        .unwrap()
    }
    fn pending(
        store: &WorkItemStore,
        claim: &WorkItemLaunchClaim,
    ) -> (RecoveryCycle, CandidateReservation) {
        let cycle = store.begin_recovery_cycle(claim, INVOCATION, 0).unwrap();
        let reservation = store
            .reserve_recovery_candidate(claim, &cycle, &candidate("claude"))
            .unwrap();
        (
            store.recovery_cycle(claim.run(), INVOCATION, 1).unwrap(),
            reservation,
        )
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn newer_available_and_unknown_preserve_observation_order() {
        let (home, owners) = fixture().await;
        {
            let (store, claim) = owners;
            let (cycle, reservation) = pending(&store, &claim);
            let cycle = store
                .record_recovery_observation(
                    &claim,
                    &cycle,
                    &reservation.receipt,
                    &observed(200, true),
                )
                .unwrap();
            assert!(
                store
                    .record_recovery_observation(
                        &claim,
                        &cycle,
                        &reservation.receipt,
                        &observed(100, false)
                    )
                    .is_err()
            );
            assert_eq!(
                store.recovery_observation(&reservation.receipt).unwrap(),
                (Some(observed(200, true)), None)
            );
            let cycle = store
                .record_recovery_observation(
                    &claim,
                    &cycle,
                    &reservation.receipt,
                    &observed(300, false),
                )
                .unwrap();
            let cycle = store
                .record_recovery_observation(
                    &claim,
                    &cycle,
                    &reservation.receipt,
                    &QuotaObservation::unknown(),
                )
                .unwrap();
            assert!(
                store
                    .record_recovery_observation(
                        &claim,
                        &cycle,
                        &reservation.receipt,
                        &observed(250, false)
                    )
                    .is_err()
            );
            assert_eq!(
                store.recovery_observation(&reservation.receipt).unwrap(),
                (
                    Some(QuotaObservation::unknown()),
                    Some(observed(300, false))
                )
            );
        }
        home.close().expect("close runtime home");
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn unknown_policy_wake_rolls_once_and_retains_history_across_reopen() {
        let (home, owners) = fixture().await;
        {
            let (store, claim) = owners;
            let (cycle, reservation) = pending(&store, &claim);
            let cycle = store
                .record_recovery_observation(
                    &claim,
                    &cycle,
                    &reservation.receipt,
                    &QuotaObservation::unsupported(),
                )
                .unwrap();
            let wake =
                RecoveryWake::new("wake-1".into(), WakeOrigin::PolicyBackoff, 100, 1100).unwrap();
            let cycle = store
                .arm_recovery_wake(&claim, &cycle, &reservation.receipt, &wake)
                .unwrap();
            assert!(
                store
                    .consume_recovery_wake(&claim, &cycle, wake.identity(), 1099)
                    .is_err()
            );
            let reopened = crate::runs::Storage::open(home.path())
                .await
                .unwrap()
                .work_items();
            assert_eq!(
                reopened.due_recovery_wakes(1100, 1).unwrap(),
                vec![cycle.clone()]
            );
            let next = reopened
                .consume_recovery_wake(&claim, &cycle, wake.identity(), 1100)
                .unwrap();
            assert_eq!((next.generation, next.probe_count), (2, 1));
            assert!(
                store
                    .consume_recovery_wake(&claim, &cycle, wake.identity(), 1100)
                    .is_err()
            );
            assert!(
                store
                    .recovery_cycle(claim.run(), INVOCATION, 1)
                    .unwrap()
                    .closed
            );
            assert_eq!(
                store.recovery_observation(&reservation.receipt).unwrap(),
                (Some(QuotaObservation::unsupported()), None)
            );
            assert_eq!(
                store
                    .reserve_recovery_candidate(&claim, &next, &candidate("claude"))
                    .unwrap()
                    .disposition,
                ReservationDisposition::Reserved
            );
        }
        home.close().expect("close runtime home");
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn concurrent_wake_consumption_creates_one_next_cycle() {
        let (home, owners) = fixture().await;
        {
            let (store, claim) = owners;
            let (cycle, reservation) = pending(&store, &claim);
            let cycle = store
                .record_recovery_observation(
                    &claim,
                    &cycle,
                    &reservation.receipt,
                    &QuotaObservation::unknown(),
                )
                .unwrap();
            let wake = RecoveryWake::new("concurrent".into(), WakeOrigin::PolicyBackoff, 100, 1100)
                .unwrap();
            let cycle = store
                .arm_recovery_wake(&claim, &cycle, &reservation.receipt, &wake)
                .unwrap();
            let results = std::thread::scope(|scope| {
                let first =
                    scope.spawn(|| store.consume_recovery_wake(&claim, &cycle, "concurrent", 1100));
                let second =
                    scope.spawn(|| store.consume_recovery_wake(&claim, &cycle, "concurrent", 1100));
                [first.join().unwrap(), second.join().unwrap()]
            });
            assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
            assert_eq!(
                store
                    .recovery_cycle(claim.run(), INVOCATION, 2)
                    .unwrap()
                    .probe_count,
                1
            );
            assert!(store.recovery_cycle(claim.run(), INVOCATION, 3).is_err());
        }
        home.close().expect("close runtime home");
    }

    /// Isolates schedule revocation; full Manual-control generation semantics
    /// are covered separately by the real daemon control tests.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancelled_schedule_cannot_consume_retained_wake_audit() {
        let (home, owners) = fixture().await;
        {
            let (store, claim) = owners;
            let invocation = "invocation-01ARZ3NDEKTSV4RRFFQ69G5FAV";
            let cycle = store.begin_recovery_cycle(&claim, invocation, 0).unwrap();
            let reservation = store
                .reserve_recovery_candidate(&claim, &cycle, &candidate("a"))
                .unwrap();
            let cycle = store.recovery_cycle(claim.run(), invocation, 1).unwrap();
            let cycle = store
                .record_recovery_observation(
                    &claim,
                    &cycle,
                    &reservation.receipt,
                    &QuotaObservation::unknown(),
                )
                .unwrap();
            let wake = RecoveryWake::new("cancelled".into(), WakeOrigin::PolicyBackoff, 100, 1100)
                .unwrap();
            store
                .arm_recovery_wake(&claim, &cycle, &reservation.receipt, &wake)
                .unwrap();
            let mut conn = store.pool.get().unwrap();
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .unwrap();
            owner_fence(&tx, &claim).unwrap();
            invalidate_recovery_wakes(
                &tx,
                claim.binding.item,
                claim.run(),
                claim.binding.generation,
            )
            .unwrap();
            tx.commit().unwrap();
            drop(conn);
            let refreshed = store.recovery_cycle(claim.run(), invocation, 1).unwrap();
            assert_eq!(
                refreshed.wake,
                Some(wake),
                "cancelled timing remains immutable audit"
            );
            assert!(store.due_recovery_wakes(1100, 10).unwrap().is_empty());
            let result = store.consume_recovery_wake(&claim, &refreshed, "cancelled", 1100);
            assert!(
                result.is_err(),
                "retained audit JSON must not grant wake consumption: {result:?}"
            );
            assert_eq!(
                store.recovery_cycle(claim.run(), invocation, 1).unwrap(),
                refreshed
            );
            assert!(
                store.recovery_cycle(claim.run(), invocation, 2).is_err(),
                "cancelled schedule must not allocate a fresh cycle"
            );
        }
        home.close().expect("close runtime home");
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn confirmed_capacity_rebind_preserves_receipts_but_manual_fence_rejects() {
        use surge_core::execution_recovery::{
            ExecutionControlState, PendingStagePhase, SuspensionFence, SuspensionReason,
        };
        let (home, owners) = fixture().await;
        {
            let (store, claim) = owners;
            let (cycle, reservation) = pending(&store, &claim);
            let cycle = store
                .record_recovery_observation(
                    &claim,
                    &cycle,
                    &reservation.receipt,
                    &QuotaObservation::unknown(),
                )
                .unwrap();
            let wake =
                RecoveryWake::new("capacity".into(), WakeOrigin::PolicyBackoff, 100, 1100).unwrap();
            let cycle = store
                .arm_recovery_wake(&claim, &cycle, &reservation.receipt, &wake)
                .unwrap();
            let item = store.show(claim.binding.item).unwrap().item;
            store
                .mutate(
                    &WorkItemCommand::Suspend {
                        operation_id: WorkItemOperationId::new(),
                        item: item.id,
                        expected_version: item.version,
                    },
                    None,
                    None,
                    "host",
                    100,
                )
                .unwrap();
            assert!(store.due_recovery_wakes(1100, 10).unwrap().is_empty());
            assert!(
                store
                    .consume_recovery_wake(&claim, &cycle, "capacity", 1100)
                    .is_err()
            );
            assert!(store.rebind_recovery_control(&claim, &cycle, 1).is_err());
            let cancelled = store.recovery_cycle(claim.run(), INVOCATION, 1).unwrap();
            assert!(
                store
                    .rebind_recovery_control(&claim, &cancelled, 1)
                    .is_err()
            );
            assert!(store.due_recovery_wakes(1100, 10).unwrap().is_empty());

            // Independent registry fixture models an already journal-confirmed host
            // Capacity fence. It never converts a cancelled manual control into
            // capacity authority; this checks storage, not engine/containment proof.
            let (capacity_home, owners) = fixture().await;
            {
                let (store, claim) = owners;
                let (cycle, reservation) = pending(&store, &claim);
                let cycle = store
                    .record_recovery_observation(
                        &claim,
                        &cycle,
                        &reservation.receipt,
                        &QuotaObservation::unknown(),
                    )
                    .unwrap();
                let cycle = store
                    .arm_recovery_wake(&claim, &cycle, &reservation.receipt, &wake)
                    .unwrap();
                let conn = store.pool.get().unwrap();
                let operation = WorkItemOperationId::new();
                conn.execute(
                    "INSERT INTO work_item_operations(operation,body_hash,result) VALUES(?,?,?)",
                    params![
                        operation.to_string(),
                        ContentHash::compute(b"capacity-store-fixture").to_string(),
                        "{}"
                    ],
                )
                .unwrap();
                let mut control = surge_core::execution_recovery::WorkItemExecutionControl {
                    item: claim.binding.item,
                    run: claim.run,
                    attempt_generation: claim.binding.generation,
                    generation: 1,
                    operation,
                    state: ExecutionControlState::Suspended,
                    fence: None,
                    allow_new_session: false,
                    diagnostic: None,
                };
                conn.execute(
                    "INSERT INTO work_item_execution_controls(run,generation,item,attempt_generation,operation,state,payload) VALUES(?,?,?,?,?,'suspended',?)",
                    params![claim.run.to_string(), 1, claim.binding.item.to_string(), claim.binding.generation, operation.to_string(), serde_json::to_string(&control).unwrap()],
                ).unwrap();
                control.state = ExecutionControlState::Suspended;
                control.fence = Some(SuspensionFence {
                    control_generation: 1,
                    snapshot_seq: 10,
                    pending_stage: PendingStagePhase::Interrupted {
                        node: surge_core::NodeKey::try_from("agent").unwrap(),
                        invocation: INVOCATION.parse().unwrap(),
                    },
                    cleanup_confirmed: true,
                    reason: SuspensionReason::Manual,
                });
                control::write_control(&conn, &control).unwrap();
                assert!(store.rebind_recovery_control(&claim, &cycle, 1).is_err());
                control.fence.as_mut().unwrap().reason = SuspensionReason::Capacity {
                    wake_at_ms: Some(1100),
                };
                control::write_control(&conn, &control).unwrap();
                conn.execute(
                    "UPDATE work_item_quota_candidates SET observation='{}' WHERE receipt=?",
                    [&reservation.receipt],
                )
                .unwrap();
                assert!(store.rebind_recovery_control(&claim, &cycle, 1).is_err());
                conn.execute(
                    "UPDATE work_item_quota_candidates SET observation=? WHERE receipt=?",
                    params![
                        serde_json::to_string(&QuotaObservation::unknown()).unwrap(),
                        reservation.receipt
                    ],
                )
                .unwrap();
                drop(conn);
                let current = store.rebind_recovery_control(&claim, &cycle, 1).unwrap();
                assert_eq!(current.wake, cycle.wake);
                assert_eq!(
                    store.capacity_control_association(claim.run(), 1).unwrap(),
                    None,
                    "a capacity-looking fence without a journal association is not quota authority"
                );
                assert_eq!(
                    store.due_recovery_wakes(1100, 10).unwrap(),
                    vec![current.clone()]
                );
                assert!(
                    store
                        .consume_recovery_wake(&claim, &current, "capacity", 1100)
                        .is_err()
                );
                assert_eq!(
                    store.recovery_observation(&reservation.receipt).unwrap(),
                    (Some(QuotaObservation::unknown()), None)
                );
            }
            capacity_home.close().expect("close runtime home");
        }
        home.close().expect("close runtime home");
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stale_revision_claim_and_manual_control_reject_mutations() {
        let (home, owners) = fixture().await;
        {
            let (store, claim) = owners;
            let (cycle, reservation) = pending(&store, &claim);
            assert!(matches!(store.claim(claim.run()), Err(WorkItemError::Busy)));
            let current = store
                .record_recovery_observation(
                    &claim,
                    &cycle,
                    &reservation.receipt,
                    &QuotaObservation::unknown(),
                )
                .unwrap();
            assert!(
                store
                    .reserve_recovery_candidate(&claim, &cycle, &candidate("codex"))
                    .is_err()
            );
            let item = store.show(claim.binding.item).unwrap().item;
            store
                .mutate(
                    &WorkItemCommand::Suspend {
                        operation_id: WorkItemOperationId::new(),
                        item: item.id,
                        expected_version: item.version,
                    },
                    None,
                    None,
                    "human",
                    100,
                )
                .unwrap();
            assert!(
                store
                    .reserve_recovery_candidate(&claim, &current, &candidate("codex"))
                    .is_err()
            );
            assert!(store.rebind_recovery_control(&claim, &current, 1).is_err());
            store
                .pool
                .get()
                .unwrap()
                .execute(
                    "UPDATE work_item_attempts SET claim_token='replacement' WHERE run=?",
                    [claim.run.to_string()],
                )
                .unwrap();
            assert!(
                store
                    .reserve_recovery_candidate(&claim, &current, &candidate("claude"))
                    .is_err()
            );
        }
        home.close().expect("close runtime home");
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn known_accounts_deduplicate_and_unknown_accounts_do_not_guess_merges() {
        let (home, owners) = fixture().await;
        {
            let (store, claim) = owners;
            let cycle = store.begin_recovery_cycle(&claim, INVOCATION, 0).unwrap();
            let account = AccountEvidence::Known {
                provider: "provider".into(),
                account_key: "opaque".into(),
            };
            let first = RecoveryCandidate::new("runtime-a".into(), account.clone()).unwrap();
            let reservation = store
                .reserve_recovery_candidate(&claim, &cycle, &first)
                .unwrap();
            let replay = store
                .reserve_recovery_candidate(&claim, &cycle, &first)
                .unwrap();
            assert_eq!(replay.receipt, reservation.receipt);
            assert_eq!(replay.disposition, ReservationDisposition::Replayed);
            let cycle = store.recovery_cycle(claim.run(), INVOCATION, 1).unwrap();
            assert!(
                store
                    .reserve_recovery_candidate(
                        &claim,
                        &cycle,
                        &RecoveryCandidate::new("runtime-b".into(), account).unwrap()
                    )
                    .is_err()
            );
            let cycle = store.recovery_cycle(claim.run(), INVOCATION, 1).unwrap();
            store
                .reserve_recovery_candidate(&claim, &cycle, &candidate("runtime-b"))
                .unwrap();
            let cycle = store.recovery_cycle(claim.run(), INVOCATION, 1).unwrap();
            assert!(
                store
                    .reserve_recovery_candidate(
                        &claim,
                        &cycle,
                        &RecoveryCandidate::new(
                            "runtime-b".into(),
                            AccountEvidence::Known {
                                provider: "provider".into(),
                                account_key: "now-observed".into()
                            }
                        )
                        .unwrap()
                    )
                    .is_err()
            );
            store
                .reserve_recovery_candidate(&claim, &cycle, &candidate("runtime-c"))
                .unwrap();
        }
        home.close().expect("close runtime home");
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn unknown_or_expired_evidence_cannot_arm_observed_reset() {
        let (home, owners) = fixture().await;
        {
            let (store, claim) = owners;
            let (cycle, reservation) = pending(&store, &claim);
            let cycle = store
                .record_recovery_observation(
                    &claim,
                    &cycle,
                    &reservation.receipt,
                    &QuotaObservation::unknown(),
                )
                .unwrap();
            let reset =
                RecoveryWake::new("reset".into(), WakeOrigin::ObservedReset, 100, 1100).unwrap();
            assert!(
                store
                    .arm_recovery_wake(&claim, &cycle, &reservation.receipt, &reset)
                    .is_err()
            );
            let expired = QuotaObservation::new(QuotaEvidence::Observed {
                observed_at_ms: 1,
                expires_at_ms: 10,
                available: false,
                reset_at_ms: Some(1100),
            })
            .unwrap();
            let cycle = store
                .record_recovery_observation(&claim, &cycle, &reservation.receipt, &expired)
                .unwrap();
            assert!(
                store
                    .arm_recovery_wake(&claim, &cycle, &reservation.receipt, &reset)
                    .is_err()
            );
            let cycle = store
                .record_recovery_observation(
                    &claim,
                    &cycle,
                    &reservation.receipt,
                    &observed(100, false),
                )
                .unwrap();
            let armed = store
                .arm_recovery_wake(&claim, &cycle, &reservation.receipt, &reset)
                .unwrap();
            assert_eq!(armed.wake, Some(reset));
        }
        home.close().expect("close runtime home");
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn bounded_policy_checks_continue_after_sixteen_cycles() {
        let (home, owners) = fixture().await;
        {
            let (store, claim) = owners;
            let mut cycle = store.begin_recovery_cycle(&claim, INVOCATION, 0).unwrap();
            for ordinal in 1..=17 {
                let receipt = store
                    .reserve_recovery_candidate(&claim, &cycle, &candidate("claude"))
                    .unwrap();
                cycle = store
                    .recovery_cycle(claim.run(), INVOCATION, cycle.generation)
                    .unwrap();
                cycle = store
                    .record_recovery_observation(
                        &claim,
                        &cycle,
                        &receipt.receipt,
                        &QuotaObservation::unknown(),
                    )
                    .unwrap();
                let at = ordinal * 1000;
                let wake = RecoveryWake::new(
                    format!("wake-{ordinal}"),
                    WakeOrigin::PolicyBackoff,
                    at,
                    at + 1000,
                )
                .unwrap();
                cycle = store
                    .arm_recovery_wake(&claim, &cycle, &receipt.receipt, &wake)
                    .unwrap();
                cycle = store
                    .consume_recovery_wake(&claim, &cycle, wake.identity(), at + 1000)
                    .unwrap();
            }
            assert_eq!((cycle.generation, cycle.probe_count), (18, 17));
        }
        home.close().expect("close runtime home");
    }
    #[test]
    fn deserialization_enforces_private_constructor_invariants() {
        assert!(
            serde_json::from_str::<RecoveryCandidate>(r#"{"runtime":"","account":"Unknown"}"#)
                .is_err()
        );
        assert!(serde_json::from_str::<QuotaObservation>(r#"{"evidence":{"Observed":{"observed_at_ms":20,"expires_at_ms":10,"available":false,"reset_at_ms":null}}}"#).is_err());
        assert!(
            RecoveryWake::new(
                "reset".into(),
                WakeOrigin::ObservedReset,
                1,
                MAX_BACKOFF_MS + 2
            )
            .is_ok()
        );
        assert!(
            RecoveryWake::new(
                "policy".into(),
                WakeOrigin::PolicyBackoff,
                1,
                MAX_BACKOFF_MS + 2
            )
            .is_err()
        );
    }
    #[test]
    fn placeholder_upgrade_preserves_nonactionable_audit_and_reapply() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE work_items(id TEXT PRIMARY KEY); CREATE TABLE work_item_attempts(run TEXT PRIMARY KEY); CREATE TABLE work_item_recovery_cycles(run TEXT,invocation TEXT,control_generation INTEGER,cycle_generation INTEGER,attempted_candidates TEXT,selected_runtime TEXT,wake_at_ms INTEGER); INSERT INTO work_item_recovery_cycles VALUES('run-old','invocation-old',7,3,'[\"old\"]','old',1);").unwrap();
        let migrations: &[(&str, &str)] = &[(
            "registry-0024-recovery-cycles",
            include_str!("../runs/migrations/registry/0024_recovery_cycles.sql"),
        )];
        let clock = crate::runs::clock::MockClock::new(1);
        crate::runs::migrations::apply(&mut conn, migrations, &clock).unwrap();
        crate::runs::migrations::apply(&mut conn, migrations, &clock).unwrap();
        let original:(String,String,i64)=conn.query_row("SELECT attempted_candidates,selected_runtime,wake_at_ms FROM work_item_recovery_cycles_legacy",[],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).unwrap();
        assert_eq!(original, ("[\"old\"]".into(), "old".into(), 1));
        let count: u64 = conn
            .query_row("SELECT COUNT(*) FROM work_item_quota_cycles", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 0);
    }
}
