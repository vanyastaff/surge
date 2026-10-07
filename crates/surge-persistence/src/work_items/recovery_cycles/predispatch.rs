//! Planned stage admission and sealed skips are distinct from provider responses.
use super::*;
use surge_core::{EventPayload, id::StageInvocationId};

/// Historical exhaustion inspected in the same snapshot that selected this cycle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapacitySkip {
    /// Exact frozen candidate inspected.
    pub candidate: FrozenQuotaCandidate,
    /// Original host-sealed typed-response receipt.
    pub source_receipt: String,
    /// Latest actual provider admission at inspection.
    pub source_epoch: u64,
    /// Host inspection timestamp.
    pub checked_at_ms: i64,
}
/// Selection describes persisted state. Only a separately spent handoff can open a provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapacitySelection {
    /// A real candidate reservation requiring a separate opening permit.
    Selected {
        /// Current exact cycle after reservation.
        cycle: RecoveryCycle,
        /// Unique candidate receipt, including replay disposition.
        reservation: CandidateReservation,
        /// Persisted skipped prefix, never attempted-provider evidence.
        skipped: Vec<CapacitySkip>,
    },
    /// Every candidate has fresh exact evidence; no provider was opened by selection.
    AllExhausted {
        /// Current exact cycle.
        cycle: RecoveryCycle,
        /// Complete persisted skip proof.
        skipped: Vec<CapacitySkip>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct PlannedExhaustionProof {
    run: RunId,
    invocation: String,
    cycle_generation: u64,
    control_generation: u64,
    plan_seq: u64,
    stage_entry_seq: u64,
    policy_hash: ContentHash,
    skips: Vec<CapacitySkip>,
}

fn journal_event(conn: &Connection, seq: u64) -> Result<EventPayload> {
    let (schema, payload): (u32, Vec<u8>) = conn.query_row(
        "SELECT schema_version,payload FROM events WHERE seq=?",
        [seq],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    surge_core::migrate_payload(schema, &payload).map_err(|e| WorkItemError::Invalid(e.to_string()))
}
impl WorkItemStore {
    /// Bind to a real host plan occurrence, without a synthetic provider opening sequence.
    pub fn bind_planned_quota_stage(
        &self,
        claim: &WorkItemLaunchClaim,
        invocation: StageInvocationId,
        plan_seq: u64,
    ) -> Result<FrozenQuotaStage> {
        let path = self
            .home
            .join("runs")
            .join(claim.run.to_string())
            .join("events.sqlite");
        let history = crate::runs::inspection::read_folded_events(&path, claim.run)
            .map_err(|e| WorkItemError::Invalid(e.to_string()))?;
        if !matches!(history.state, surge_core::RunState::Pipeline { .. }) {
            return Err(WorkItemError::Conflict(
                "planned stage requires live journal".into(),
            ));
        }
        let journal = crate::runs::connection::RetainedConnection::read_only(&path)?;
        let EventPayload::QuotaStagePlanned {
            node,
            attempt: stage_attempt,
            stage_entry_seq,
            logical_invocation,
            control_generation,
            policy_hash,
        } = journal_event(&journal, plan_seq)?
        else {
            return Err(WorkItemError::Invalid(
                "quota binding is not a host plan occurrence".into(),
            ));
        };
        if logical_invocation != invocation
            || stage_entry_seq >= plan_seq
            || !matches!(journal_event(&journal, stage_entry_seq)?, EventPayload::StageEntered { node: ref entered, attempt } if entered == &node && attempt == stage_attempt)
        {
            return Err(WorkItemError::Conflict(
                "quota plan stage anchor changed".into(),
            ));
        }
        let expected_phase = surge_core::execution_recovery::PendingStagePhase::PlannedCapacity {
            node: node.clone(),
            logical_invocation: invocation,
            stage_entry_seq,
            plan_seq,
        };
        if !matches!(&history.state,surge_core::RunState::Pipeline { cursor,memory,.. } if cursor.node==node && memory.stage_occurrences.get(&node)==Some(&stage_entry_seq) && memory.quota_plans.get(&node)==Some(&expected_phase))
        {
            return Err(WorkItemError::Conflict(
                "planned occurrence is no longer the current journal stage".into(),
            ));
        }
        let mut statement = journal
            .prepare("SELECT schema_version,payload FROM events WHERE seq>? ORDER BY seq")?;
        let rows = statement.query_map(params![stage_entry_seq], |r| {
            Ok((r.get::<_, u32>(0)?, r.get::<_, Vec<u8>>(1)?))
        })?;
        for row in rows {
            let (schema, payload) = row?;
            let event = surge_core::migrate_payload(schema, &payload)
                .map_err(|e| WorkItemError::Invalid(e.to_string()))?;
            if matches!(
                event,
                EventPayload::StageEntered { .. }
                    | EventPayload::StageCompleted { .. }
                    | EventPayload::StageFailed { .. }
                    | EventPayload::ExecutionWriterIntent { .. }
                    | EventPayload::SessionEstablishmentRequested { .. }
                    | EventPayload::SessionOpened { .. }
                    | EventPayload::RunCompleted { .. }
                    | EventPayload::RunFailed { .. }
                    | EventPayload::RunAborted { .. }
            ) {
                return Err(WorkItemError::Conflict(
                    "quota plan prefix already changed or opened".into(),
                ));
            }
        }
        let mut connection = self.pool.get()?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        fence(&tx, claim, control_generation)?;
        let accepted = attempt(&tx, claim.run)?;
        let config: serde_json::Value = serde_json::from_str(&accepted.config)?;
        let policy: FrozenQuotaPolicy = serde_json::from_value(
            config
                .get("quota_recovery")
                .cloned()
                .ok_or_else(|| WorkItemError::Invalid("host quota policy absent".into()))?,
        )?;
        if policy.content_hash()? != policy_hash || accepted.graph.find_node(&node).is_none() {
            return Err(WorkItemError::Conflict(
                "planned policy differs from accepted attempt".into(),
            ));
        }
        let stage = policy
            .stage(&node)
            .ok_or_else(|| WorkItemError::Invalid("planned node has no frozen policy".into()))?;
        let body = serde_json::to_string(stage)?;
        let existing: Option<(Option<u64>,Option<u64>,String,String)> = tx.query_row(
            "SELECT opening_seq,plan_seq,policy_hash,policy FROM work_item_quota_stages WHERE run=? AND invocation=?",params![claim.run.to_string(),invocation.to_string()],
            |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
        if let Some(existing) = existing {
            if existing != (None, Some(plan_seq), policy_hash.to_string(), body) {
                return Err(WorkItemError::Conflict(
                    "planned quota binding is immutable".into(),
                ));
            }
        } else {
            tx.execute("INSERT INTO work_item_quota_stages(run,invocation,plan_seq,node,policy_hash,policy) VALUES(?,?,?,?,?,?)",params![claim.run.to_string(),invocation.to_string(),plan_seq,node.to_string(),policy_hash.to_string(),body])?;
        }
        let result = stage.clone();
        tx.commit()?;
        Ok(result)
    }

    /// Atomically inspect current exact exhaustion and select the first unattempted candidate.
    pub fn select_planned_capacity(
        &self,
        claim: &WorkItemLaunchClaim,
        expected: &RecoveryCycle,
        now_ms: i64,
    ) -> Result<CapacitySelection> {
        let mut connection = self.pool.get()?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = check_cycle(&tx, claim, expected)?;
        let stage = handoffs::read_bound_stage(&tx, claim.run, &current.invocation)?;
        let project = record(&tx, claim.binding.item)?.project;
        let mut skipped = Vec::new();
        for target in stage.candidates() {
            let prior: Option<(String,String)> = tx.query_row("SELECT receipt,body FROM work_item_quota_candidates WHERE run=? AND invocation=? AND cycle_generation=? AND runtime=?",params![claim.run.to_string(),current.invocation,current.generation,target.candidate().runtime()],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
            if let Some((receipt, body)) = prior {
                // An actual reservation can advance only with the original strict typed response.
                let typed = rate_limit::has_typed_marker(&tx, &receipt)?;
                if typed {
                    continue;
                }
                return Ok(CapacitySelection::Selected {
                    cycle: current,
                    reservation: CandidateReservation {
                        receipt,
                        candidate: serde_json::from_str(&body)?,
                        disposition: ReservationDisposition::Replayed,
                    },
                    skipped,
                });
            }
            if let Some(fresh) = super::super::recipe_capacity::inspect_pinned_on_connection(
                &tx, &project, target, now_ms,
            )? {
                let (epoch, _): (u64, Option<String>) = tx.query_row("SELECT epoch,exhaustion_receipt FROM recipe_opening_admissions WHERE runtime=? AND launch_hash=? ORDER BY epoch DESC LIMIT 1",params![target.candidate().runtime(),target.launch_hash().to_string()],|r|Ok((r.get(0)?,r.get(1)?)))?;
                let skip = CapacitySkip {
                    candidate: target.clone(),
                    source_receipt: fresh.receipt().to_owned(),
                    source_epoch: epoch,
                    checked_at_ms: now_ms,
                };
                let original: Option<(String,u64,String)> = tx.query_row("SELECT source_receipt,source_epoch,candidate FROM work_item_capacity_skips WHERE run=? AND invocation=? AND cycle_generation=? AND runtime=?",params![claim.run.to_string(),current.invocation,current.generation,target.candidate().runtime()],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).optional()?;
                let body = serde_json::to_string(target)?;
                match original {
                    Some((receipt, stored_epoch, candidate))
                        if receipt != skip.source_receipt
                            || stored_epoch != epoch
                            || candidate != body =>
                    {
                        return Err(WorkItemError::Conflict(
                            "persisted skip source changed; explicit replan required".into(),
                        ));
                    },
                    Some(_) => {},
                    None => {
                        tx.execute("INSERT INTO work_item_capacity_skips(run,invocation,cycle_generation,runtime,source_receipt,source_epoch,checked_at_ms,candidate) VALUES(?,?,?,?,?,?,?,?)",params![claim.run.to_string(),current.invocation,current.generation,target.candidate().runtime(),skip.source_receipt,epoch,now_ms,body])?;
                    },
                }
                skipped.push(skip);
            } else {
                let old_skip: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM work_item_capacity_skips WHERE run=? AND invocation=? AND cycle_generation=? AND runtime=?)",params![claim.run.to_string(),current.invocation,current.generation,target.candidate().runtime()],|row|row.get(0))?;
                if old_skip {
                    return Err(WorkItemError::Conflict(
                        "persisted skip is no longer current; explicit replan required".into(),
                    ));
                }
                let reservation = reserve_on_transaction(&tx, claim, &current, target.candidate())?;
                let cycle = read_cycle(&tx, claim.run, &current.invocation, current.generation)?;
                tx.commit()?;
                return Ok(CapacitySelection::Selected {
                    cycle,
                    reservation,
                    skipped,
                });
            }
        }
        tx.commit()?;
        Ok(CapacitySelection::AllExhausted {
            cycle: current,
            skipped,
        })
    }
    /// Reinspect persisted skipped origins in one snapshot before any provider effect.
    pub fn revalidate_capacity_skips(
        &self,
        claim: &WorkItemLaunchClaim,
        expected: &RecoveryCycle,
        now_ms: i64,
        opening: Option<&QuotaOpenPermit>,
    ) -> Result<Vec<CapacitySkip>> {
        let mut connection = self.pool.get()?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
        if let Some(permit) = opening {
            let handoff = handoffs::read_quota_handoff(&tx, permit.operation())?;
            handoffs::check_current_quota_handoff(&tx, claim, &handoff)?;
            let current = read_cycle(&tx, claim.run, &expected.invocation, expected.generation)?;
            if current != *expected
                || handoff.state() != QuotaHandoffState::OpeningUnknown
                || permit.run() != claim.run
                || permit.control_generation() != current.control_generation
                || handoff.logical_invocation() != current.invocation
                || handoff.launch() != permit.launch()
            {
                return Err(WorkItemError::Conflict(
                    "skip inspection differs from admitted current opening".into(),
                ));
            }
        } else {
            check_cycle(&tx, claim, expected)?;
        }
        let stage = handoffs::read_bound_stage(&tx, claim.run, &expected.invocation)?;
        let project = record(&tx, claim.binding.item)?.project;
        let mut query = tx.prepare("SELECT source_receipt,source_epoch,checked_at_ms,candidate FROM work_item_capacity_skips WHERE run=? AND invocation=? AND cycle_generation=? ORDER BY checked_at_ms,runtime")?;
        let rows = query.query_map(
            params![
                claim.run.to_string(),
                expected.invocation,
                expected.generation
            ],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, u64>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, String>(3)?,
                ))
            },
        )?;
        let mut result = Vec::new();
        for row in rows {
            let (source_receipt, source_epoch, checked_at_ms, body) = row?;
            let candidate: FrozenQuotaCandidate = serde_json::from_str(&body)?;
            let latest: u64 = tx.query_row("SELECT MAX(epoch) FROM recipe_opening_admissions WHERE runtime=? AND launch_hash=?",params![candidate.candidate().runtime(),candidate.launch_hash().to_string()],|r|r.get(0))?;
            let current = super::super::recipe_capacity::inspect_pinned_on_connection(
                &tx, &project, &candidate, now_ms,
            )?;
            if !stage.candidates().contains(&candidate)
                || latest != source_epoch
                || current
                    .as_ref()
                    .is_none_or(|fresh| fresh.receipt() != source_receipt)
            {
                return Err(WorkItemError::Conflict(
                    "skipped capacity source is no longer exact and fresh".into(),
                ));
            }
            result.push(CapacitySkip {
                candidate,
                source_receipt,
                source_epoch,
                checked_at_ms,
            });
        }
        Ok(result)
    }

    /// Seal and suspend an all-skipped stage without any synthetic provider attempt.
    pub fn suspend_planned_capacity(
        &self,
        claim: &WorkItemLaunchClaim,
        expected: &RecoveryCycle,
        now_ms: i64,
    ) -> Result<CapacityControlAssociation> {
        let mut connection = self.pool.get()?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = check_cycle(&tx, claim, expected)?;
        let stage = handoffs::read_bound_stage(&tx, claim.run, &current.invocation)?;
        let (plan_seq,policy_hash): (u64,String) = tx.query_row("SELECT plan_seq,policy_hash FROM work_item_quota_stages WHERE run=? AND invocation=? AND opening_seq IS NULL",params![claim.run.to_string(),current.invocation],|row|Ok((row.get(0)?,row.get(1)?)))?;
        let attempts: u64 = tx.query_row("SELECT COUNT(*) FROM work_item_quota_candidates WHERE run=? AND invocation=? AND cycle_generation=?",params![claim.run.to_string(),current.invocation,current.generation],|row|row.get(0))?;
        if attempts != 0 || current.selected_runtime.is_some() || current.wake.is_some() {
            return Err(WorkItemError::Conflict(
                "planned exhaustion cannot replace a provider attempt or armed wake".into(),
            ));
        }
        let path = self
            .home
            .join("runs")
            .join(claim.run.to_string())
            .join("events.sqlite");
        let journal = crate::runs::connection::RetainedConnection::read_only(&path)?;
        let EventPayload::QuotaStagePlanned {
            node,
            stage_entry_seq,
            logical_invocation,
            control_generation,
            policy_hash: journal_hash,
            ..
        } = journal_event(&journal, plan_seq)?
        else {
            return Err(WorkItemError::Invalid(
                "planned capacity proof lost its host occurrence".into(),
            ));
        };
        if logical_invocation.to_string() != current.invocation
            || control_generation != current.control_generation
            || node != *stage.node()
            || journal_hash.to_string() != policy_hash
        {
            return Err(WorkItemError::Conflict(
                "planned exhaustion anchor differs".into(),
            ));
        }
        verify_unadmitted_prefix(&journal, stage_entry_seq)?;
        verify_latest_planned_origin(&journal, stage_entry_seq, plan_seq)?;
        let project = record(&tx, claim.binding.item)?.project;
        let mut skips = Vec::new();
        let mut due = i64::MAX;
        let mut origin = WakeOrigin::ObservedReset;
        for target in stage.candidates() {
            let fresh = super::super::recipe_capacity::inspect_pinned_on_connection(
                &tx, &project, target, now_ms,
            )?
            .ok_or_else(|| {
                WorkItemError::Conflict(
                    "planned exhaustion requires every exact fresh configured source".into(),
                )
            })?;
            let (source_receipt,source_epoch,checked_at_ms,body): (String,u64,i64,String) = tx.query_row("SELECT source_receipt,source_epoch,checked_at_ms,candidate FROM work_item_capacity_skips WHERE run=? AND invocation=? AND cycle_generation=? AND runtime=?",params![claim.run.to_string(),current.invocation,current.generation,target.candidate().runtime()],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?)))?;
            let latest: u64 = tx.query_row("SELECT MAX(epoch) FROM recipe_opening_admissions WHERE runtime=? AND launch_hash=?",params![target.candidate().runtime(),target.launch_hash().to_string()],|row|row.get(0))?;
            if source_receipt != fresh.receipt()
                || source_epoch != latest
                || serde_json::from_str::<FrozenQuotaCandidate>(&body)? != *target
            {
                return Err(WorkItemError::Conflict(
                    "planned skip provenance changed".into(),
                ));
            }
            let (candidate_due, candidate_origin) = match fresh.reset_at_ms() {
                Some(reset) if reset > now_ms => (reset, WakeOrigin::ObservedReset),
                _ => (
                    now_ms
                        .checked_add(stage.policy_backoff_ms())
                        .ok_or_else(|| WorkItemError::Invalid("planned wake overflow".into()))?,
                    WakeOrigin::PolicyBackoff,
                ),
            };
            if candidate_due < due {
                due = candidate_due;
                origin = candidate_origin;
            }
            skips.push(CapacitySkip {
                candidate: target.clone(),
                source_receipt,
                source_epoch,
                checked_at_ms,
            });
        }
        let proof = PlannedExhaustionProof {
            run: claim.run,
            invocation: current.invocation.clone(),
            cycle_generation: current.generation,
            control_generation: current.control_generation,
            plan_seq,
            stage_entry_seq,
            policy_hash: journal_hash,
            skips,
        };
        let receipt = RunId::new().to_string();
        tx.execute("INSERT INTO work_item_capacity_plan_proofs(receipt,run,invocation,cycle_generation,plan_seq,body) VALUES(?,?,?,?,?,?)",params![receipt,claim.run.to_string(),current.invocation,current.generation,plan_seq,serde_json::to_string(&proof)?])?;
        let wake = RecoveryWake::new(RunId::new().to_string(), origin, now_ms, due)?;
        tx.execute("UPDATE work_item_quota_cycles SET wake=?,wake_at_ms=?,probe_count=probe_count+1,revision=revision+1 WHERE run=? AND invocation=? AND cycle_generation=?",params![serde_json::to_string(&wake)?,due,claim.run.to_string(),current.invocation,current.generation])?;
        let scheduled = read_cycle(&tx, claim.run, &current.invocation, current.generation)?;
        let association = handoffs::insert_capacity_intent(
            &tx,
            claim,
            &scheduled,
            &receipt,
            handoffs::CapacityStopKind::PreDispatchExhausted,
        )?;
        tx.commit()?;
        Ok(association)
    }
}

pub(super) fn planned_capacity_fence(
    conn: &Connection,
    control: &surge_core::execution_recovery::WorkItemExecutionControl,
    invocation: &str,
    node: &surge_core::NodeKey,
    logical: StageInvocationId,
    stage_entry_seq: u64,
    plan_seq: u64,
) -> Result<bool> {
    if logical.to_string() != invocation || stage_entry_seq == 0 || plan_seq <= stage_entry_seq {
        return Ok(false);
    }
    let association: Option<(String,u64,u64)> = conn.query_row("SELECT planned_receipt,cycle_generation,source_control FROM work_item_capacity_controls WHERE run=? AND target_control=? AND planned_receipt IS NOT NULL AND invalidated=0",params![control.run.to_string(),control.generation],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).optional()?;
    let Some((receipt, generation, source_control)) = association else {
        return Ok(false);
    };
    let body: String = conn.query_row("SELECT body FROM work_item_capacity_plan_proofs WHERE receipt=? AND run=? AND invocation=? AND cycle_generation=? AND plan_seq=?",params![receipt,control.run.to_string(),invocation,generation,plan_seq],|row|row.get(0))?;
    let proof: PlannedExhaustionProof = serde_json::from_str(&body)?;
    let (stored_seq,hash,stage_body): (u64,String,String) = conn.query_row("SELECT plan_seq,policy_hash,policy FROM work_item_quota_stages WHERE run=? AND invocation=? AND node=?",params![control.run.to_string(),invocation,node.to_string()],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?)))?;
    let stage: FrozenQuotaStage = serde_json::from_str(&stage_body)?;
    Ok(proof.run == control.run
        && proof.invocation == invocation
        && proof.cycle_generation == generation
        && proof.control_generation == source_control
        && source_control.checked_add(1) == Some(control.generation)
        && proof.plan_seq == plan_seq
        && stored_seq == plan_seq
        && proof.stage_entry_seq == stage_entry_seq
        && proof.policy_hash.to_string() == hash
        && proof.skips.len() == stage.candidates().len()
        && proof
            .skips
            .iter()
            .zip(stage.candidates())
            .all(|(skip, target)| &skip.candidate == target))
}

/// Reinspect one immutable skipped origin within the caller's registry snapshot.
pub(super) fn validated_current_skip(
    conn: &Connection,
    cycle: &RecoveryCycle,
    project: &surge_core::id::WorkItemProjectId,
    target: &FrozenQuotaCandidate,
    now_ms: i64,
) -> Result<bool> {
    let saved:Option<(String,u64,String)>=conn.query_row("SELECT source_receipt,source_epoch,candidate FROM work_item_capacity_skips WHERE run=? AND invocation=? AND cycle_generation=? AND runtime=?",params![cycle.run.to_string(),cycle.invocation,cycle.generation,target.candidate().runtime()],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).optional()?;
    let Some((receipt, epoch, body)) = saved else {
        return Ok(false);
    };
    let latest: Option<u64> = conn.query_row(
        "SELECT MAX(epoch) FROM recipe_opening_admissions WHERE runtime=? AND launch_hash=?",
        params![
            target.candidate().runtime(),
            target.launch_hash().to_string()
        ],
        |row| row.get(0),
    )?;
    let current =
        super::super::recipe_capacity::inspect_pinned_on_connection(conn, project, target, now_ms)?;
    if latest != Some(epoch)
        || serde_json::from_str::<FrozenQuotaCandidate>(&body)? != *target
        || current
            .as_ref()
            .is_none_or(|fresh| fresh.receipt() != receipt)
    {
        return Err(WorkItemError::Conflict(
            "skipped provider origin is no longer exact and fresh".into(),
        ));
    }
    Ok(true)
}

/// Read-only cold reconstruction result; it grants no provider effect capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlannedResumeInspection {
    /// No prior entry exists for this current node.
    NoCurrentOccurrence,
    /// A prior entry has no spent candidate or provider effect; retain its identity.
    UnadmittedOccurrence {
        /// Actual durable StageEntered sequence.
        stage_entry_seq: u64,
    },
    /// An authenticated outcome and route consumed the previous occurrence.
    CompletedOccurrence,
}

impl WorkItemStore {
    /// Inspect the original current planned occurrence before cold reconstruction can
    /// append another entry. Existing reservations are uncertainty, never retry authority.
    pub fn inspect_planned_stage_resume(
        &self,
        claim: &WorkItemLaunchClaim,
        expected_cursor: &surge_core::run_state::Cursor,
        expected_policy: &ContentHash,
        expected_control: u64,
    ) -> Result<PlannedResumeInspection> {
        let path = self
            .home
            .join("runs")
            .join(claim.run.to_string())
            .join("events.sqlite");
        let history = crate::runs::inspection::read_folded_events(&path, claim.run)
            .map_err(|error| WorkItemError::Invalid(error.to_string()))?;
        let surge_core::RunState::Pipeline { cursor, memory, .. } = &history.state else {
            return Err(WorkItemError::Conflict(
                "cold quota inspection requires live original journal".into(),
            ));
        };
        if cursor != expected_cursor {
            return Err(WorkItemError::Conflict("cold quota cursor changed".into()));
        }
        let mut connection = self.pool.get()?;
        let tx = connection.transaction()?;
        fence(&tx, claim, expected_control)?;
        let orphaned:u64=tx.query_row("SELECT COUNT(*) FROM work_item_quota_candidates c LEFT JOIN work_item_quota_stages s ON s.run=c.run AND s.invocation=c.invocation WHERE c.run=? AND s.invocation IS NULL",[claim.run.to_string()],|row|row.get(0))?;
        if orphaned != 0 {
            return Err(WorkItemError::Conflict(
                "cold quota candidate lost its binding provenance".into(),
            ));
        }
        let accepted = attempt(&tx, claim.run)?;
        let config: serde_json::Value = serde_json::from_str(&accepted.config)?;
        let policy: FrozenQuotaPolicy = serde_json::from_value(config["quota_recovery"].clone())?;
        if policy.content_hash()? != *expected_policy || policy.stage(&cursor.node).is_none() {
            return Err(WorkItemError::Conflict("cold quota policy changed".into()));
        }
        let journal = crate::runs::connection::RetainedConnection::read_only(&path)?;
        let entry = memory.stage_occurrences.get(&cursor.node).copied();
        let mut bound:Option<(String,Option<u64>,Option<u64>)>=tx.query_row("SELECT invocation,plan_seq,opening_seq FROM work_item_quota_stages WHERE run=? AND node=? ORDER BY COALESCE(plan_seq,opening_seq) DESC LIMIT 1",params![claim.run.to_string(),cursor.node.to_string()],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).optional()?;
        let Some(entry) = entry else {
            if bound.is_some() {
                return Err(WorkItemError::Conflict(
                    "cold quota binding lost its original entry".into(),
                ));
            }
            return Ok(PlannedResumeInspection::NoCurrentOccurrence);
        };
        if !matches!(journal_event(&journal,entry)?,EventPayload::StageEntered {node,attempt} if node==cursor.node && attempt==cursor.attempt)
        {
            return Err(WorkItemError::Conflict(
                "cold quota original entry changed".into(),
            ));
        }
        let plan = memory.quota_plans.get(&cursor.node);
        validate_current_cold_plan(
            &journal,
            plan,
            cursor,
            entry,
            expected_policy,
            expected_control,
        )?;
        if let Some((logical, plan_seq, opening_seq)) = &bound {
            let bound_entry = bound_occurrence_entry(&journal, *plan_seq, *opening_seq)?;
            let provider_invocations=tx.prepare("SELECT provider_invocation FROM work_item_quota_handoffs WHERE run=? AND logical_invocation=?")?.query_map(params![claim.run.to_string(),logical],|row|row.get::<_,String>(0))?.collect::<std::result::Result<Vec<_>,_>>()?;
            let routed = provider_invocations.iter().find_map(|provider| {
                memory
                    .committed_stage_outcomes
                    .iter()
                    .find_map(|(invocation, outcome)| {
                        (invocation.to_string() == *provider
                            && outcome.commit.context().node == cursor.node
                            && !outcome.conflicting
                            && outcome.committed_seq > bound_entry)
                            .then_some(outcome.routed_seq)
                            .flatten()
                    })
            });
            if entry == bound_entry {
                if plan_seq.is_some()
                    && !matches!(plan,Some(surge_core::execution_recovery::PendingStagePhase::PlannedCapacity {logical_invocation,plan_seq:actual,..}) if logical==&logical_invocation.to_string() && Some(*actual)==*plan_seq)
                {
                    return Err(WorkItemError::Conflict(
                        "current completed or unfinished quota binding changed its plan".into(),
                    ));
                }
                if routed.is_some() {
                    return Ok(PlannedResumeInspection::CompletedOccurrence);
                }
            } else if routed.is_some_and(|routed| bound_entry < entry && routed < entry) {
                // A newer unadmitted occurrence retains its own entry and plan;
                // authenticated older history grants no fresh occurrence identity.
                bound = None;
            } else {
                return Err(WorkItemError::Conflict(
                    "unfinished quota binding superseded its original occurrence".into(),
                ));
            }
        }
        if let Some((logical, plan_seq, _)) = &bound
            && !matches!(plan,Some(surge_core::execution_recovery::PendingStagePhase::PlannedCapacity {logical_invocation,plan_seq:actual,..}) if logical==&logical_invocation.to_string() && Some(*actual)==*plan_seq)
        {
            return Err(WorkItemError::Conflict(
                "cold quota binding lacks original planned provenance".into(),
            ));
        }
        if let Some((logical, _, _)) = bound {
            let reservations: u64 = tx.query_row(
                "SELECT COUNT(*) FROM work_item_quota_candidates WHERE run=? AND invocation=?",
                params![claim.run.to_string(), logical],
                |row| row.get(0),
            )?;
            if reservations != 0 {
                return Err(WorkItemError::Conflict(
                    "unfinished original quota reservation requires containment".into(),
                ));
            }
        }
        verify_unadmitted_prefix(&journal, entry)?;
        Ok(PlannedResumeInspection::UnadmittedOccurrence {
            stage_entry_seq: entry,
        })
    }
}

/// The exact immutable row's fields were validated by the caller. A later
/// plan, even with identical logical identity, cannot reuse that row's authority.
fn verify_latest_planned_origin(journal: &Connection, entry: u64, expected: u64) -> Result<()> {
    let mut statement = journal
        .prepare("SELECT seq,schema_version,payload FROM events WHERE seq>? ORDER BY seq DESC")?;
    let rows = statement.query_map([entry], |row| {
        Ok((
            row.get::<_, u64>(0)?,
            row.get::<_, u32>(1)?,
            row.get::<_, Vec<u8>>(2)?,
        ))
    })?;
    for row in rows {
        let (seq, schema, payload) = row?;
        let event = surge_core::migrate_payload(schema, &payload)
            .map_err(|error| WorkItemError::Invalid(error.to_string()))?;
        if matches!(event, EventPayload::QuotaStagePlanned { .. }) {
            return if seq == expected {
                Ok(())
            } else {
                Err(WorkItemError::Conflict(
                    "planned exhaustion origin was superseded".into(),
                ))
            };
        }
    }
    Err(WorkItemError::Conflict(
        "planned exhaustion origin is absent".into(),
    ))
}

fn verify_unadmitted_prefix(journal: &Connection, entry: u64) -> Result<()> {
    let mut statement =
        journal.prepare("SELECT schema_version,payload FROM events WHERE seq>? ORDER BY seq")?;
    let rows = statement.query_map([entry], |row| {
        Ok((row.get::<_, u32>(0)?, row.get::<_, Vec<u8>>(1)?))
    })?;
    for row in rows {
        let (schema, payload) = row?;
        let event = surge_core::migrate_payload(schema, &payload)
            .map_err(|error| WorkItemError::Invalid(error.to_string()))?;
        if matches!(
            event,
            EventPayload::StageEntered { .. }
                | EventPayload::StageCompleted { .. }
                | EventPayload::StageFailed { .. }
                | EventPayload::ExecutionWriterIntent { .. }
                | EventPayload::SessionEstablishmentRequested { .. }
                | EventPayload::SessionOpened { .. }
                | EventPayload::RunCompleted { .. }
                | EventPayload::RunFailed { .. }
                | EventPayload::RunAborted { .. }
        ) {
            return Err(WorkItemError::Conflict(
                "planned occurrence has provider or advanced journal uncertainty".into(),
            ));
        }
    }
    Ok(())
}

fn validate_current_cold_plan(
    journal: &Connection,
    phase: Option<&surge_core::execution_recovery::PendingStagePhase>,
    cursor: &surge_core::run_state::Cursor,
    entry: u64,
    policy: &ContentHash,
    control: u64,
) -> Result<()> {
    if let Some(surge_core::execution_recovery::PendingStagePhase::PlannedCapacity {
        logical_invocation,
        stage_entry_seq,
        plan_seq,
        node,
    }) = phase
        && (*stage_entry_seq != entry
            || node != &cursor.node
            || !matches!(journal_event(journal,*plan_seq)?,EventPayload::QuotaStagePlanned {node,attempt,stage_entry_seq,logical_invocation:logical,control_generation,policy_hash} if node==cursor.node && attempt==cursor.attempt && stage_entry_seq==entry && logical==*logical_invocation && control_generation==control && policy_hash==*policy))
    {
        return Err(WorkItemError::Conflict(
            "cold quota planned identity changed".into(),
        ));
    }
    Ok(())
}

fn bound_occurrence_entry(
    journal: &Connection,
    plan: Option<u64>,
    opening: Option<u64>,
) -> Result<u64> {
    if let Some(plan) = plan {
        let EventPayload::QuotaStagePlanned {
            stage_entry_seq, ..
        } = journal_event(journal, plan)?
        else {
            return Err(WorkItemError::Conflict(
                "bound quota plan is not a real occurrence".into(),
            ));
        };
        return Ok(stage_entry_seq);
    }
    let opening = opening
        .ok_or_else(|| WorkItemError::Conflict("bound quota occurrence has no origin".into()))?;
    let mut statement = journal
        .prepare("SELECT seq,schema_version,payload FROM events WHERE seq<? ORDER BY seq DESC")?;
    let rows = statement.query_map([opening], |row| {
        Ok((
            row.get::<_, u64>(0)?,
            row.get::<_, u32>(1)?,
            row.get::<_, Vec<u8>>(2)?,
        ))
    })?;
    for row in rows {
        let (seq, schema, payload) = row?;
        if matches!(
            surge_core::migrate_payload(schema, &payload)
                .map_err(|error| WorkItemError::Invalid(error.to_string()))?,
            EventPayload::StageEntered { .. }
        ) {
            return Ok(seq);
        }
    }
    Err(WorkItemError::Conflict(
        "bound original opening lost its stage entry".into(),
    ))
}
