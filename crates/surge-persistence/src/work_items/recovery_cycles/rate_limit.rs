//! Host-sealed provenance for an actual typed provider rate-limit response.
use super::*;
use surge_core::{SessionId, execution_recovery::OpenedSession, id::StageInvocationId};

/// Actual opening that returned the classified RPC error; never agent-supplied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawSource")]
pub struct QuotaRateLimitSource {
    opening_seq: u64,
    provider_invocation: StageInvocationId,
    internal_session: SessionId,
    retry_after_ms: Option<u64>,
    details: String,
}
#[derive(Deserialize)]
struct RawSource {
    opening_seq: u64,
    provider_invocation: StageInvocationId,
    internal_session: SessionId,
    retry_after_ms: Option<u64>,
    details: String,
}
impl TryFrom<RawSource> for QuotaRateLimitSource {
    type Error = WorkItemError;
    fn try_from(raw: RawSource) -> Result<Self> {
        Self::new(
            raw.opening_seq,
            raw.provider_invocation,
            raw.internal_session,
            raw.retry_after_ms,
            raw.details,
        )
    }
}
impl QuotaRateLimitSource {
    /// The host supplies values retained from its actual opening RPC receipt.
    pub fn new(
        opening_seq: u64,
        provider_invocation: StageInvocationId,
        internal_session: SessionId,
        retry_after_ms: Option<u64>,
        details: String,
    ) -> Result<Self> {
        if opening_seq == 0
            || provider_invocation.as_ulid() == ulid::Ulid::nil()
            || internal_session.as_ulid() == ulid::Ulid::nil()
            || details.len() > 65_536
            || retry_after_ms.is_some_and(|delay| delay > i64::MAX as u64)
        {
            return Err(WorkItemError::Invalid(
                "invalid rate-limit source opening".into(),
            ));
        }
        Ok(Self {
            opening_seq,
            provider_invocation,
            internal_session,
            retry_after_ms,
            details,
        })
    }
    /// Original transport details; subsequent probes never replace them.
    pub fn details(&self) -> &str {
        &self.details
    }
    /// Provider-supplied delay; absence is not a fabricated reset.
    pub fn retry_after_ms(&self) -> Option<u64> {
        self.retry_after_ms
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct SealedRateLimit {
    run: RunId,
    item: WorkItemId,
    attempt_generation: u64,
    logical_invocation: String,
    cycle_generation: u64,
    control_generation: u64,
    recorded_revision: u64,
    reservation: String,
    node: surge_core::NodeKey,
    source: QuotaRateLimitSource,
    opening: OpenedSession,
    candidate: FrozenQuotaCandidate,
    observation: QuotaObservation,
}

/// Immutable original transport cause. Reading this value grants no action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotaRateLimitMarker(SealedRateLimit);
impl QuotaRateLimitMarker {
    /// Exact original response metadata and opening identity, for audit only.
    pub fn source(&self) -> &QuotaRateLimitSource {
        &self.0.source
    }
    /// Original observed exhaustion, independently of subsequent capacity probes.
    pub fn observation(&self) -> &QuotaObservation {
        &self.0.observation
    }
    /// Original connection, retained when a later probe supersedes availability.
    pub fn opening(&self) -> &OpenedSession {
        &self.0.opening
    }
}

/// Fresh typed exhaustion for one exact historical reservation and launch recipe.
/// This read-only value grants no opening or dispatch authority and makes no
/// statement about account-global freshness or observations in other runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FreshTypedExhaustion {
    marker: SealedRateLimit,
    observed_at_ms: i64,
    valid_until_ms: i64,
}
impl FreshTypedExhaustion {
    /// Reservation whose original typed response was inspected.
    pub fn receipt(&self) -> &str {
        &self.marker.reservation
    }
    /// Exact frozen runtime, account, model and launch recipe.
    pub fn candidate(&self) -> &FrozenQuotaCandidate {
        &self.marker.candidate
    }
    /// Recovery-cycle revision that sealed the original typed response.
    pub fn source_revision(&self) -> u64 {
        self.marker.recorded_revision
    }
    /// Both observations have occurred at or before this time.
    pub fn observed_at_ms(&self) -> i64 {
        self.observed_at_ms
    }
    /// Exclusive upper bound from both TTLs and any provider resets.
    pub fn valid_until_ms(&self) -> i64 {
        self.valid_until_ms
    }
}

fn exhausted_window(observation: &QuotaObservation) -> Option<(i64, i64)> {
    match observation.evidence() {
        QuotaEvidence::Observed {
            available: false,
            observed_at_ms,
            expires_at_ms,
            reset_at_ms,
        } => Some((
            *observed_at_ms,
            reset_at_ms.map_or(*expires_at_ms, |reset| reset.min(*expires_at_ms)),
        )),
        _ => None,
    }
}

fn validate_marker(marker: &SealedRateLimit) -> Result<()> {
    if marker.attempt_generation == 0
        || marker.cycle_generation == 0
        || marker.recorded_revision == 0
        || !identifier(&marker.reservation)
        || invocation_key(&marker.logical_invocation)? != marker.logical_invocation
        || marker.source.provider_invocation != marker.opening.descriptor.invocation()
        || marker.source.internal_session != marker.opening.session
        || marker.candidate.candidate().runtime() != marker.opening.descriptor.runtime()
        || marker.candidate.launch_hash() != marker.opening.descriptor.launch_hash()
        || !matches!(
            marker.observation.evidence(),
            QuotaEvidence::Observed {
                available: false,
                ..
            }
        )
    {
        return Err(WorkItemError::Invalid(
            "invalid sealed rate-limit provenance".into(),
        ));
    }
    Ok(())
}

fn read_marker(conn: &Connection, receipt: &str) -> Result<Option<SealedRateLimit>> {
    let body: Option<String> = conn.query_row(
        "SELECT typed_exhaustion FROM work_item_quota_candidates WHERE receipt=?",
        [receipt],
        |row| row.get(0),
    )?;
    let Some(body) = body else {
        return Ok(None);
    };
    let marker: SealedRateLimit = serde_json::from_str(&body)?;
    validate_marker(&marker)?;
    if marker.reservation != receipt {
        return Err(WorkItemError::Invalid("rate-limit receipt differs".into()));
    }
    let (run,invocation,cycle,runtime,candidate): (String,String,u64,String,String) = conn.query_row(
        "SELECT run,invocation,cycle_generation,runtime,body FROM work_item_quota_candidates WHERE receipt=?",
        [receipt],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?)),
    )?;
    let candidate: RecoveryCandidate = serde_json::from_str(&candidate)?;
    if run != marker.run.to_string()
        || invocation != marker.logical_invocation
        || cycle != marker.cycle_generation
        || runtime != marker.candidate.candidate().runtime()
        || &candidate != marker.candidate.candidate()
    {
        return Err(WorkItemError::Invalid(
            "rate-limit identity columns differ".into(),
        ));
    }
    let stage = handoffs::read_bound_stage(conn, marker.run, &marker.logical_invocation)?;
    let QuotaEvidence::Observed {
        observed_at_ms,
        expires_at_ms,
        reset_at_ms,
        ..
    } = marker.observation.evidence()
    else {
        return Err(WorkItemError::Invalid(
            "typed rate-limit window is absent".into(),
        ));
    };
    let expected_reset = marker
        .source
        .retry_after_ms
        .map(|delay| {
            let delay = i64::try_from(delay)
                .map_err(|_| WorkItemError::Invalid("rate-limit delay overflow".into()))?;
            observed_at_ms
                .checked_add(delay)
                .ok_or_else(|| WorkItemError::Invalid("rate-limit reset overflow".into()))
        })
        .transpose()?;
    if stage.node() != &marker.node
        || !stage.candidates().contains(&marker.candidate)
        || expires_at_ms.checked_sub(*observed_at_ms) != Some(stage.observation_ttl_ms())
        || &expected_reset != reset_at_ms
    {
        return Err(WorkItemError::Invalid(
            "typed rate-limit differs from frozen policy or original delay".into(),
        ));
    }
    Ok(Some(marker))
}

#[derive(Clone, Copy)]
enum RateLimitReadPurpose {
    NewOrigin,
    CleanupStop,
}
struct RateLimitOpeningEvidence {
    node: surge_core::NodeKey,
    opening: OpenedSession,
    epoch: Option<surge_core::id::WorkItemOperationId>,
    prefix: u64,
}

impl WorkItemStore {
    /// Inspect one reservation in a consistent read snapshot. Candidate mismatch,
    /// cancelled exhaustion, future observations and elapsed validity return None.
    /// Invalid stored provenance is an error. This never authorizes dispatch.
    pub fn inspect_fresh_typed_exhaustion(
        &self,
        receipt: &str,
        expected: &FrozenQuotaCandidate,
        now_ms: i64,
    ) -> Result<Option<FreshTypedExhaustion>> {
        let mut connection = self.pool.get()?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
        let Some(marker) = read_marker(&tx, receipt)? else {
            return Ok(None);
        };
        if &marker.candidate != expected {
            return Ok(None);
        }
        let (observed, exhaustion): (Option<String>, Option<String>) = tx.query_row(
            "SELECT observed,exhaustion FROM work_item_quota_candidates WHERE receipt=?",
            [receipt],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let observed = observed
            .map(|body| serde_json::from_str::<QuotaObservation>(&body))
            .transpose()?;
        let exhaustion = exhaustion
            .map(|body| serde_json::from_str::<QuotaObservation>(&body))
            .transpose()?;
        let Some(observed) = observed else {
            return Err(WorkItemError::Invalid(
                "typed rate-limit observed evidence is absent".into(),
            ));
        };
        match observed.evidence() {
            QuotaEvidence::Observed {
                available: true, ..
            } if exhaustion.is_none() => return Ok(None),
            QuotaEvidence::Observed {
                available: false, ..
            } if exhaustion.as_ref() == Some(&observed) => {},
            _ => {
                return Err(WorkItemError::Invalid(
                    "typed rate-limit latest observed/exhaustion disagree".into(),
                ));
            },
        }
        let Some((original_at, original_until)) = exhausted_window(&marker.observation) else {
            return Ok(None);
        };
        let Some((latest_at, latest_until)) = exhausted_window(&observed) else {
            return Ok(None);
        };
        if latest_at < original_at {
            return Err(WorkItemError::Invalid(
                "typed rate-limit latest observation precedes original response".into(),
            ));
        }
        let observed_at_ms = latest_at;
        let valid_until_ms = original_until.min(latest_until);
        if now_ms < observed_at_ms || now_ms >= valid_until_ms {
            return Ok(None);
        }
        Ok(Some(FreshTypedExhaustion {
            marker,
            observed_at_ms,
            valid_until_ms,
        }))
    }
    /// Historical typed origin is independent of the latest quota probe.
    pub fn typed_rate_limit(&self, receipt: &str) -> Result<Option<QuotaRateLimitMarker>> {
        let connection = self.pool.get()?;
        Ok(read_marker(&connection, receipt)?.map(QuotaRateLimitMarker))
    }
    /// Host recorder called only after an actual SendMessageError::RateLimited.
    /// A generic observed-false probe cannot set this immutable provenance.
    pub fn record_selected_rate_limit(
        &self,
        claim: &WorkItemLaunchClaim,
        expected: &RecoveryCycle,
        receipt: &str,
        source: &QuotaRateLimitSource,
        observation: &QuotaObservation,
    ) -> Result<RecoveryCycle> {
        if !matches!(
            observation.evidence(),
            QuotaEvidence::Observed {
                available: false,
                ..
            }
        ) {
            return Err(WorkItemError::Invalid(
                "typed rate-limit must observe exhaustion".into(),
            ));
        }
        let evidence = self.trusted_rate_limit_opening(
            claim,
            expected,
            source,
            RateLimitReadPurpose::NewOrigin,
        )?;
        let RateLimitOpeningEvidence {
            node,
            opening,
            epoch,
            prefix,
        } = evidence;
        let mut connection = self.pool.get()?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = check_cycle(&tx, claim, expected)?;
        let latest: u64 = tx.query_row(
            "SELECT MAX(cycle_generation) FROM work_item_quota_cycles WHERE run=? AND invocation=?",
            params![current.run.to_string(), current.invocation],
            |row| row.get(0),
        )?;
        if latest != current.generation {
            return Err(WorkItemError::Conflict(
                "rate-limit cycle is not current".into(),
            ));
        }
        self.check_rate_limit_prefix(claim, prefix)?;
        let stage = handoffs::read_bound_stage(&tx, claim.run, &current.invocation)?;
        let QuotaEvidence::Observed {
            observed_at_ms,
            expires_at_ms,
            reset_at_ms,
            ..
        } = observation.evidence()
        else {
            return Err(WorkItemError::Invalid(
                "rate-limit observation missing".into(),
            ));
        };
        let reset = source
            .retry_after_ms
            .map(|delay| {
                let delay = i64::try_from(delay)
                    .map_err(|_| WorkItemError::Invalid("rate-limit delay overflow".into()))?;
                observed_at_ms
                    .checked_add(delay)
                    .ok_or_else(|| WorkItemError::Invalid("rate-limit reset overflow".into()))
            })
            .transpose()?;
        if expires_at_ms.checked_sub(*observed_at_ms) != Some(stage.observation_ttl_ms())
            || &reset != reset_at_ms
        {
            return Err(WorkItemError::Invalid(
                "rate-limit window differs from frozen policy or original delay".into(),
            ));
        }
        let (runtime,body): (String,String) = tx.query_row(
            "SELECT runtime,body FROM work_item_quota_candidates WHERE run=? AND invocation=? AND cycle_generation=? AND receipt=?",
            params![claim.run.to_string(),current.invocation,current.generation,receipt],|row|Ok((row.get(0)?,row.get(1)?)),
        )?;
        let candidate: RecoveryCandidate = serde_json::from_str(&body)?;
        let frozen = stage
            .candidates()
            .iter()
            .find(|frozen| frozen.candidate() == &candidate)
            .ok_or_else(|| {
                WorkItemError::Conflict("rate-limit candidate is outside frozen policy".into())
            })?;
        if opening.descriptor.cwd() != record(&tx, claim.binding.item)?.workspace.path
            || stage.node() != &node
            || Some(&runtime) != current.selected_runtime.as_ref()
            || opening.descriptor.runtime() != runtime
            || opening.descriptor.launch_hash() != frozen.launch_hash()
        {
            return Err(WorkItemError::Conflict(
                "rate-limit belongs to a former candidate".into(),
            ));
        }
        let marker = SealedRateLimit {
            run: claim.run,
            item: claim.binding.item,
            attempt_generation: claim.binding.generation,
            logical_invocation: current.invocation.clone(),
            cycle_generation: current.generation,
            control_generation: current.control_generation,
            recorded_revision: current
                .revision
                .checked_add(1)
                .ok_or_else(|| WorkItemError::Conflict("rate-limit revision exhausted".into()))?,
            reservation: receipt.into(),
            node,
            source: source.clone(),
            opening,
            candidate: frozen.clone(),
            observation: observation.clone(),
        };
        validate_marker(&marker)?;
        if let Some(original) = read_marker(&tx, receipt)? {
            let mut replay = marker;
            replay.recorded_revision = original.recorded_revision;
            if replay != original {
                return Err(WorkItemError::Conflict(
                    "typed rate-limit immutable body differs".into(),
                ));
            }
            return Ok(current);
        }
        // A repeated immutable origin is an audit replay, not a new admission.
        // Recording the first origin advances the cycle revision, so only a
        // new origin must satisfy the opening's pre-error revision fence.
        validate_rate_limit_epoch(&tx, claim, &current, source, epoch)?;
        let next = apply_quota_observation(&tx, &current, receipt, observation)?;
        tx.execute("UPDATE work_item_quota_candidates SET typed_exhaustion=? WHERE receipt=? AND typed_exhaustion IS NULL",
            params![serde_json::to_string(&marker)?,receipt])?;
        tx.commit()?;
        Ok(next)
    }

    fn trusted_rate_limit_opening(
        &self,
        claim: &WorkItemLaunchClaim,
        expected: &RecoveryCycle,
        source: &QuotaRateLimitSource,
        purpose: RateLimitReadPurpose,
    ) -> Result<RateLimitOpeningEvidence> {
        let path = self
            .home
            .join("runs")
            .join(claim.run.to_string())
            .join("events.sqlite");
        let history = crate::runs::inspection::read_folded_events(&path, claim.run)
            .map_err(|error| WorkItemError::Invalid(error.to_string()))?;
        let conn = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let (version, bytes): (u32, Vec<u8>) = conn.query_row(
            "SELECT schema_version,payload FROM events WHERE seq=?",
            [source.opening_seq],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let event = surge_core::migrate_payload(version, &bytes)
            .map_err(|error| WorkItemError::Invalid(error.to_string()))?;
        let surge_core::EventPayload::SessionOpened {
            node,
            opened: Some(opening),
            handoff,
            ..
        } = event
        else {
            return Err(WorkItemError::Invalid(
                "rate-limit source has no actual opening".into(),
            ));
        };
        if source.provider_invocation != opening.descriptor.invocation()
            || source.internal_session != opening.session
            || !matches!(&history.state,surge_core::RunState::Pipeline {cursor,memory,..}
                if cursor.node==node && memory.control_generation==expected.control_generation
                && memory.provider_sessions.get(&source.provider_invocation).and_then(|values|values.last())==Some(&opening))
        {
            return Err(WorkItemError::Conflict(
                "rate-limit source is stale or external".into(),
            ));
        }
        let latest: u64 = conn.query_row("SELECT COALESCE(MAX(seq),0) FROM events", [], |row| {
            row.get(0)
        })?;
        if latest != history.event_count {
            return Err(WorkItemError::Conflict(
                "rate-limit journal prefix changed".into(),
            ));
        }
        if matches!(purpose, RateLimitReadPurpose::NewOrigin)
            && source_closed_in_prefix(&conn, source, history.event_count)?
        {
            return Err(WorkItemError::Conflict(
                "rate-limit source session was stopped".into(),
            ));
        }
        Ok(RateLimitOpeningEvidence {
            node,
            opening,
            epoch: handoff,
            prefix: history.event_count,
        })
    }
    fn check_rate_limit_prefix(&self, claim: &WorkItemLaunchClaim, prefix: u64) -> Result<()> {
        let path = self
            .home
            .join("runs")
            .join(claim.run.to_string())
            .join("events.sqlite");
        let conn = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let actual: u64 = conn.query_row("SELECT COALESCE(MAX(seq),0) FROM events", [], |row| {
            row.get(0)
        })?;
        if actual != prefix {
            return Err(WorkItemError::Conflict(
                "rate-limit journal prefix changed".into(),
            ));
        }
        Ok(())
    }
}

fn validate_rate_limit_epoch(
    tx: &Transaction<'_>,
    claim: &WorkItemLaunchClaim,
    current: &RecoveryCycle,
    source: &QuotaRateLimitSource,
    epoch: Option<surge_core::id::WorkItemOperationId>,
) -> Result<()> {
    if let Some(epoch) = epoch {
        let handoff = handoffs::read_quota_handoff(tx, epoch)?;
        handoffs::check_current_quota_handoff(tx, claim, &handoff)?;
        if handoff.state() != QuotaHandoffState::Executing
            || handoff.internal_session() != Some(source.internal_session)
            || handoff.launch().provider_invocation() != source.provider_invocation
        {
            return Err(WorkItemError::Conflict(
                "rate-limit handoff is not current Executing".into(),
            ));
        }
    } else {
        let original: u64 = tx.query_row(
            "SELECT opening_seq FROM work_item_quota_stages WHERE run=? AND invocation=?",
            params![claim.run.to_string(), current.invocation],
            |row| row.get(0),
        )?;
        if original != source.opening_seq
            || source.provider_invocation.to_string() != current.invocation
        {
            return Err(WorkItemError::Conflict(
                "rate-limit primary opening was not sealed".into(),
            ));
        }
    }
    Ok(())
}

impl WorkItemStore {
    /// Reserve a non-wake stop for the selected failed provider's unknown cleanup.
    /// Unattempted alternatives remain unknown; this is not total quota exhaustion.
    pub fn request_quota_cleanup_stop(
        &self,
        claim: &WorkItemLaunchClaim,
        expected: &RecoveryCycle,
        reservation: &str,
    ) -> Result<CapacityControlAssociation> {
        let original = self.typed_rate_limit(reservation)?.ok_or_else(|| {
            WorkItemError::Conflict("cleanup stop has no typed provider rejection".into())
        })?;
        let mut connection = self.pool.get()?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        owner_fence(&tx, claim)?;
        let marker = read_marker(&tx, reservation)?.ok_or(WorkItemError::NotFound)?;
        if marker != original.0
            || marker.run != claim.run
            || marker.item != claim.binding.item
            || marker.attempt_generation != claim.binding.generation
            || marker.logical_invocation != expected.invocation
            || marker.cycle_generation != expected.generation
            || marker.control_generation != expected.control_generation
        {
            return Err(WorkItemError::Conflict(
                "cleanup stop source assignment differs".into(),
            ));
        }
        if let Some(association) = handoffs::read_capacity_association(&tx, expected)? {
            let target =
                control::read_control(&tx, claim.run, None)?.ok_or(WorkItemError::NotFound)?;
            if association.source_revision() != expected.revision
                || association.reservation() != reservation
                || target.generation != association.target_control()
                || !matches!(
                    target.state,
                    surge_core::execution_recovery::ExecutionControlState::SuspendRequested
                        | surge_core::execution_recovery::ExecutionControlState::Attention
                )
                || handoffs::read_stop_kind(&tx, &association)?
                    != handoffs::CapacityStopKind::CleanupUnknown
            {
                return Err(WorkItemError::Conflict(
                    "cleanup stop replay is stale or conflicting".into(),
                ));
            }
            return Ok(association);
        }
        let current = check_cycle(&tx, claim, expected)?;
        validate_current_marker(&tx, &current, &marker)?;
        // This phase permits the already stopped source because its typed
        // provenance was recorded before cleanup; it issues no opening permit.
        self.trusted_rate_limit_opening(
            claim,
            &current,
            &marker.source,
            RateLimitReadPurpose::CleanupStop,
        )?;
        let association = handoffs::insert_capacity_intent(
            &tx,
            claim,
            &current,
            reservation,
            handoffs::CapacityStopKind::CleanupUnknown,
        )?;
        tx.commit()?;
        Ok(association)
    }
}

fn validate_current_marker(
    conn: &Connection,
    current: &RecoveryCycle,
    marker: &SealedRateLimit,
) -> Result<()> {
    let latest: u64 = conn.query_row(
        "SELECT MAX(cycle_generation) FROM work_item_quota_cycles WHERE run=? AND invocation=?",
        params![current.run.to_string(), current.invocation],
        |row| row.get(0),
    )?;
    let known: Option<String> = conn.query_row(
        "SELECT observed FROM work_item_quota_candidates WHERE receipt=?",
        [&marker.reservation],
        |row| row.get(0),
    )?;
    let known = known
        .map(|body| serde_json::from_str::<QuotaObservation>(&body))
        .transpose()?;
    if latest != current.generation
        || current.revision < marker.recorded_revision
        || current.selected_runtime.as_deref() != Some(marker.candidate.candidate().runtime())
        || matches!(
            known.as_ref().map(QuotaObservation::evidence),
            Some(QuotaEvidence::Observed {
                available: true,
                ..
            })
        )
    {
        return Err(WorkItemError::Conflict(
            "typed quota rejection is no longer current".into(),
        ));
    }
    Ok(())
}

fn source_closed_in_prefix(
    conn: &Connection,
    source: &QuotaRateLimitSource,
    through: u64,
) -> Result<bool> {
    let mut cursor = source.opening_seq;
    let mut query = conn.prepare("SELECT seq,schema_version,payload FROM events WHERE seq>? AND seq<=? ORDER BY seq LIMIT 128")?;
    loop {
        let rows = query
            .query_map(params![cursor, through], |row| {
                Ok((
                    row.get::<_, u64>(0)?,
                    row.get::<_, u32>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        if rows.is_empty() {
            return Ok(false);
        }
        for (seq, version, bytes) in rows {
            let event = surge_core::migrate_payload(version, &bytes)
                .map_err(|error| WorkItemError::Invalid(error.to_string()))?;
            if matches!(event,surge_core::EventPayload::SessionClosed {session,..} if session==source.internal_session)
            {
                return Ok(true);
            }
            cursor = seq;
        }
    }
}
