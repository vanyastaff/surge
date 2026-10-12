//! Task-owned quota recovery: the durable cycle that moves one logical stage
//! across its frozen provider candidates when a provider reports exhaustion.
//!
//! Provider opening remains gated by persistence-issued one-shot authority.
//! Every "now" in this module comes from the [`WorkItemStore`]'s own clock, so
//! the decisions made here and the rows the store stamps share one time source
//! (and tests drive both with a single `MockClock`).
use surge_acp::bridge::error::SendMessageError;
use surge_persistence::work_items::{
    WorkItemError, WorkItemLaunchClaim, WorkItemStore,
    recovery_cycles::{
        CandidateReservation, CapacitySelection, FrozenQuotaStage, QuotaEvidence,
        QuotaLaunchContract, QuotaObservation, QuotaOpenPermit, QuotaRateLimitSource,
        RecoveryCycle, RecoveryWake, ReservationDisposition, WakeOrigin,
    },
};

use crate::engine::stage::StageError;

/// Quota recovery cannot discard malformed provider evidence or storage conflicts.
#[derive(Debug, thiserror::Error)]
pub(crate) enum QuotaRecoveryError {
    #[error(transparent)]
    Storage(#[from] WorkItemError),
    #[error("quota recovery: {0}")]
    Invalid(&'static str),
}

/// Durable position for one logical stage's ordered provider recovery cycle.
pub struct TaskQuotaCycle {
    pub(crate) cycle: RecoveryCycle,
    pub(crate) reservation: CandidateReservation,
    pub(crate) opening_seq: u64,
}

impl TaskQuotaCycle {
    pub(crate) fn from_automatic_wake(
        cycle: RecoveryCycle,
        reservation: CandidateReservation,
    ) -> Self {
        Self {
            cycle,
            reservation,
            opening_seq: 0,
        }
    }
}

/// A stage's task-owned quota authority: the registry store, the run's launch
/// claim and the stage's frozen candidate plan. Present exactly when the run
/// holds a claim and its frozen policy has a stage for the node
/// ([`super::StageOwnership`]). The agent stage asks it; it never names a
/// `WorkItemStore` method itself.
pub struct TaskQuotaOwner {
    store: WorkItemStore,
    claim: WorkItemLaunchClaim,
    policy: FrozenQuotaStage,
}

/// What follows a task-owned candidate's exhaustion.
pub(crate) enum AfterExhaustion {
    /// The next frozen candidate holds a reserved, admitted one-shot opening.
    Retry(Box<NextOpening>),
    /// Every candidate is exhausted: a wake is armed and a capacity
    /// suspension requested. The original 429 is the stage's result.
    Suspended,
}

/// The next candidate's admitted opening and the cycle it advances.
pub(crate) struct NextOpening {
    pub(crate) cycle: TaskQuotaCycle,
    pub(crate) opening: QuotaOpenPermit,
}

/// An admitted opening and the cycle it advances, prepared before dispatch.
pub(crate) type PreparedOpening = (Option<QuotaOpenPermit>, Option<TaskQuotaCycle>);

fn recovery(context: &str) -> impl Fn(WorkItemError) -> StageError + '_ {
    move |error| StageError::RecoveryRequired(format!("{context}: {error}"))
}

impl TaskQuotaOwner {
    /// The owner for `node`'s stage, when the run is task-owned there.
    #[must_use]
    pub(crate) fn for_stage(
        store: WorkItemStore,
        claim: Option<&WorkItemLaunchClaim>,
        policy: Option<&FrozenQuotaStage>,
    ) -> Option<Self> {
        Some(Self {
            store,
            claim: claim?.clone(),
            policy: policy?.clone(),
        })
    }

    /// Whether any frozen candidate carries a configured route.
    #[must_use]
    pub(crate) fn has_configured_routes(&self) -> bool {
        self.policy
            .candidates()
            .iter()
            .any(|candidate| candidate.configured_route().is_some())
    }

    /// Resume an automatic quota wake: when the run continues under a
    /// reserved automatic-wake handoff, admit that exact opening.
    ///
    /// # Errors
    /// A handoff whose generation does not match the Continue control, or
    /// any store conflict.
    pub(crate) fn prepare_automatic_wake(
        &self,
        run_id: surge_core::RunId,
        continuation: Option<&surge_core::execution_recovery::WorkItemExecutionControl>,
    ) -> Result<PreparedOpening, StageError> {
        let Some(control) = continuation.filter(|control| {
            control.state == surge_core::execution_recovery::ExecutionControlState::ContinueReserved
        }) else {
            return Ok((None, None));
        };
        let handoff = match self.store.quota_handoff(control.operation) {
            Ok(handoff) if handoff.is_automatic_wake() => handoff,
            Ok(_) | Err(WorkItemError::NotFound) => return Ok((None, None)),
            Err(error) => return Err(StageError::RecoveryRequired(error.to_string())),
        };
        if handoff.control_generation() != control.generation {
            return Err(StageError::RecoveryRequired(
                "automatic quota handoff does not match Continue generation".into(),
            ));
        }
        let cycle = self
            .store
            .recovery_cycle(
                run_id,
                handoff.logical_invocation(),
                handoff.cycle_generation(),
            )
            .map_err(|error| StageError::RecoveryRequired(error.to_string()))?;
        let reservation = CandidateReservation {
            receipt: handoff.reservation_receipt().to_owned(),
            candidate: handoff.launch().candidate().candidate().clone(),
            disposition: ReservationDisposition::Reserved,
        };
        let opening = self
            .store
            .admit_provider_open(&self.claim, control.operation)
            .map_err(|error| StageError::RecoveryRequired(error.to_string()))?;
        Ok((
            Some(opening),
            Some(TaskQuotaCycle::from_automatic_wake(cycle, reservation)),
        ))
    }

    /// Bind this stage entry's durable `QuotaStagePlanned` record (writing it
    /// once) and begin the host-planned recovery cycle.
    ///
    /// # Errors
    /// A missing stage entry, a store conflict or a log write failure.
    pub(crate) async fn bind_current_plan(
        &self,
        writer: &surge_persistence::runs::run_writer::RunWriter,
        node: &surge_core::keys::NodeKey,
        attempt: u32,
        policy_hash: surge_core::ContentHash,
        control_generation: u64,
    ) -> Result<RecoveryCycle, StageError> {
        use surge_core::run_event::{EventPayload, VersionedEventPayload};
        let storage =
            |error: surge_persistence::runs::StorageError| StageError::Storage(error.to_string());
        let current = writer.current_seq().await.map_err(storage)?;
        let events = writer
            .read_events(surge_persistence::runs::EventSeq(1)..current.next())
            .await
            .map_err(storage)?;
        let entry = events
            .iter()
            .rev()
            .find(|row| {
                matches!(&row.payload.payload, EventPayload::StageEntered { node: entered, attempt: entered_attempt }
                    if entered == node && *entered_attempt == attempt)
            })
            .ok_or_else(|| StageError::RecoveryRequired("actual stage entry missing".into()))?;
        let planned = events.iter().find_map(|row| match &row.payload.payload {
            EventPayload::QuotaStagePlanned {
                stage_entry_seq,
                logical_invocation,
                policy_hash: planned_hash,
                control_generation: planned_generation,
                ..
            } if *stage_entry_seq == entry.seq.as_u64()
                && planned_hash == &policy_hash
                && *planned_generation == control_generation =>
            {
                Some((*logical_invocation, row.seq.as_u64()))
            },
            _ => None,
        });
        let (logical, plan_seq) = if let Some(planned) = planned {
            planned
        } else {
            let logical = surge_core::id::StageInvocationId::new();
            let sequence = writer
                .append_event(VersionedEventPayload::new(
                    EventPayload::QuotaStagePlanned {
                        node: node.clone(),
                        attempt,
                        stage_entry_seq: entry.seq.as_u64(),
                        logical_invocation: logical,
                        control_generation,
                        policy_hash,
                    },
                ))
                .await
                .map_err(storage)?;
            (logical, sequence.as_u64())
        };
        self.store
            .bind_planned_quota_stage(&self.claim, logical, plan_seq)
            .map_err(|error| StageError::RecoveryRequired(error.to_string()))?;
        self.store
            .begin_recovery_cycle(&self.claim, &logical.to_string(), control_generation)
            .map_err(|error| StageError::RecoveryRequired(error.to_string()))
    }

    /// Select the host-planned candidate for `cycle` and admit its opening,
    /// or, when every candidate is exhausted, revalidate the skipped
    /// configured sources and request the capacity suspension.
    ///
    /// # Errors
    /// `CapacityExhausted` once the suspension is requested; a replayed or
    /// out-of-plan reservation, a changed source or a store conflict as
    /// `RecoveryRequired`.
    pub(crate) fn select_current_plan(
        &self,
        cycle: &RecoveryCycle,
        working_dir: &std::path::Path,
        run_agent_registry: Option<&surge_acp::Registry>,
    ) -> Result<PreparedOpening, StageError> {
        let now = self.store.clock().now_ms();
        match self
            .store
            .select_planned_capacity(&self.claim, cycle, now)
            .map_err(|error| StageError::RecoveryRequired(error.to_string()))?
        {
            CapacitySelection::Selected {
                cycle, reservation, ..
            } => {
                if reservation.disposition != ReservationDisposition::Reserved {
                    return Err(StageError::RecoveryRequired(
                        "planned opening replay requires reconciliation".into(),
                    ));
                }
                let opening = self.admit_opening(&cycle, &reservation)?;
                Ok((
                    Some(opening),
                    Some(TaskQuotaCycle::from_automatic_wake(cycle, reservation)),
                ))
            },
            CapacitySelection::AllExhausted { cycle, skipped } => {
                verify_skipped_snapshots(
                    &self.store,
                    working_dir,
                    run_agent_registry,
                    skipped.iter().map(|skip| &skip.candidate),
                )?;
                self.store
                    .suspend_planned_capacity(&self.claim, &cycle, self.store.clock().now_ms())
                    .map_err(|error| StageError::RecoveryRequired(error.to_string()))?;
                Err(StageError::CapacityExhausted)
            },
        }
    }

    /// Bind the frozen stage to the actual first provider opening and begin
    /// its recovery cycle (see [`initialize_task_quota_cycle`]).
    ///
    /// # Errors
    /// See [`initialize_task_quota_cycle`].
    pub(crate) fn begin(
        &self,
        invocation: surge_core::id::StageInvocationId,
        opening_seq: u64,
        control_generation: u64,
    ) -> Result<TaskQuotaCycle, WorkItemError> {
        initialize_task_quota_cycle(
            &self.store,
            &self.claim,
            &self.policy,
            invocation,
            opening_seq,
            control_generation,
        )
    }

    /// Confirm that an admitted opening reached the provider at `opened_seq`.
    ///
    /// # Errors
    /// Opening evidence that does not match the admitted operation.
    pub(crate) fn confirm_open(
        &self,
        operation: surge_core::id::WorkItemOperationId,
        opened_seq: u64,
    ) -> Result<(), WorkItemError> {
        self.store
            .confirm_provider_open(&self.claim, operation, opened_seq)
            .map(|_| ())
    }

    /// Authorize the first prompt of an admitted opening: an automatic wake
    /// is authorized against its durable `RunContinued` (when this attempt
    /// continued one), any other fallback against its opening sequence.
    ///
    /// # Errors
    /// The operator-facing reason, ready to prefix to cleanup evidence.
    pub(crate) fn authorize_prompt(
        &self,
        operation: surge_core::id::WorkItemOperationId,
        opened_seq: u64,
        continued_seq: Option<u64>,
    ) -> Result<(), String> {
        let handoff = self
            .store
            .quota_handoff(operation)
            .map_err(|error| format!("read quota prompt handoff: {error}"))?;
        if handoff.is_automatic_wake() {
            if let Some(continued_seq) = continued_seq {
                self.store
                    .authorize_automatic_wake_prompt(&self.claim, operation, continued_seq)
                    .map_err(|error| {
                        format!("automatic quota prompt authorization failed: {error}")
                    })?;
            }
            return Ok(());
        }
        self.store
            .authorize_fallback_prompt(&self.claim, operation, opened_seq)
            .map(|_| ())
            .map_err(|error| format!("fallback quota prompt authorization failed: {error}"))
    }

    /// Consume an admitted opening that will never execute (its configured
    /// source changed before the provider effect).
    ///
    /// # Errors
    /// A store conflict.
    pub(crate) fn invalidate_unexecuted(
        &self,
        opening: QuotaOpenPermit,
    ) -> Result<(), WorkItemError> {
        self.store
            .invalidate_unexecuted_quota_open(&self.claim, opening)
    }

    /// Persist the selected candidate's typed exhaustion. A configured pin
    /// whose source snapshot changed under this writer is invalidated first,
    /// so the exhausted source is never re-admitted on stale authority.
    ///
    /// # Errors
    /// The exhaustion could not be durably recorded.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn record_rate_limit(
        &self,
        quota: &mut TaskQuotaCycle,
        invocation: surge_core::id::StageInvocationId,
        session: surge_core::SessionId,
        error: &SendMessageError,
        working_dir: &std::path::Path,
        run_agent_registry: Option<&surge_acp::Registry>,
        execution_writer: surge_core::id::ExecutionWriterId,
    ) -> Result<(), StageError> {
        if let Some(target) = self.policy.candidates().iter().find(|target| {
            Some(target.candidate().runtime()) == quota.cycle.selected_runtime.as_deref()
        }) && target.configured_pin().is_some()
        {
            let registry =
                crate::engine::capacity::run_agent_registry_or_builtin(run_agent_registry);
            if crate::engine::capacity_routes::verify_skipped_snapshot(
                self.store.host_home(),
                working_dir,
                &registry,
                target,
            )
            .is_err()
            {
                self.store
                    .invalidate_admitted_configured_pin(execution_writer)
                    .map_err(|error| StageError::Storage(error.to_string()))?;
            }
        }
        quota.cycle = record_selected_rate_limit(
            &self.store,
            &self.claim,
            &self.policy,
            quota,
            invocation,
            session,
            error,
        )
        .map_err(|error| {
            StageError::RecoveryRequired(format!(
                "typed quota exhaustion could not be durably recorded: {error}"
            ))
        })?;
        Ok(())
    }

    /// Recheck, just before the provider effect, every configured source the
    /// cycle skipped and the selected opening's own route and pin.
    ///
    /// # Errors
    /// A changed declaration or source snapshot.
    pub(crate) fn validate_sources(
        &self,
        quota: Option<&TaskQuotaCycle>,
        opening: Option<&QuotaOpenPermit>,
        working_dir: &std::path::Path,
        run_agent_registry: Option<&surge_acp::Registry>,
        session_config: &surge_acp::bridge::SessionConfig,
    ) -> Result<(), StageError> {
        if let Some(quota) = quota {
            let skips = self
                .store
                .revalidate_capacity_skips(
                    &self.claim,
                    &quota.cycle,
                    self.store.clock().now_ms(),
                    opening,
                )
                .map_err(|error| StageError::RecoveryRequired(error.to_string()))?;
            verify_skipped_snapshots(
                &self.store,
                working_dir,
                run_agent_registry,
                skips.iter().map(|skip| &skip.candidate),
            )?;
        }
        validate_opening_route(
            opening,
            self.store.host_home(),
            working_dir,
            run_agent_registry,
            session_config,
        )
    }

    /// After the selected candidate's exhaustion is durable: reserve and
    /// admit the next frozen candidate, or arm the recovery wake and request
    /// the capacity suspension.
    ///
    /// # Errors
    /// A consumed or out-of-plan reservation, a changed skipped source, a
    /// missing typed exhaustion or a store conflict.
    pub(crate) fn after_exhaustion(
        &self,
        quota: &TaskQuotaCycle,
        working_dir: &std::path::Path,
        run_agent_registry: Option<&surge_acp::Registry>,
    ) -> Result<AfterExhaustion, StageError> {
        let configured = self.has_configured_routes();
        let next = if configured {
            match self
                .store
                .select_planned_capacity(&self.claim, &quota.cycle, self.store.clock().now_ms())
                .map_err(recovery("select next configured quota candidate"))?
            {
                CapacitySelection::Selected { reservation, .. } => Some(reservation),
                CapacitySelection::AllExhausted { .. } => None,
            }
        } else {
            self.store
                .reserve_next_candidate_after_exhaustion(&self.claim, &quota.cycle, &self.policy)
                .map_err(recovery("reserve next quota candidate"))?
        };
        if let Some(reservation) = next {
            if reservation.disposition != ReservationDisposition::Reserved {
                return Err(StageError::RecoveryRequired(
                    "next quota candidate reservation was already consumed".into(),
                ));
            }
            let selected = self
                .store
                .recovery_cycle(
                    self.claim.run(),
                    &quota.cycle.invocation,
                    quota.cycle.generation,
                )
                .map_err(recovery("read selected quota cycle"))?;
            let opening = self.admit_opening(&selected, &reservation)?;
            let cycle = self
                .store
                .recovery_cycle(
                    self.claim.run(),
                    &quota.cycle.invocation,
                    quota.cycle.generation,
                )
                .map_err(recovery("read fallback quota cycle"))?;
            return Ok(AfterExhaustion::Retry(Box::new(NextOpening {
                cycle: TaskQuotaCycle {
                    cycle,
                    reservation,
                    opening_seq: quota.opening_seq,
                },
                opening,
            })));
        }

        let now = self.store.clock().now_ms();
        let marker = self
            .store
            .typed_rate_limit(&quota.reservation.receipt)
            .map_err(recovery("read typed quota exhaustion"))?
            .ok_or_else(|| {
                StageError::RecoveryRequired("exhausted quota candidate has no typed origin".into())
            })?;
        let wake = plan_quota_wake(
            surge_core::RunId::new().to_string(),
            &self.policy,
            marker.observation(),
            now,
        )
        .map_err(|error| StageError::RecoveryRequired(format!("plan quota wake: {error}")))?;
        let scheduled = self
            .store
            .arm_recovery_wake(&self.claim, &quota.cycle, &quota.reservation.receipt, &wake)
            .map_err(recovery("arm quota wake"))?;
        if configured {
            let skips = self
                .store
                .revalidate_capacity_skips(&self.claim, &scheduled, now, None)
                .map_err(recovery("revalidate exhausted configured skips"))?;
            verify_skipped_snapshots(
                &self.store,
                working_dir,
                run_agent_registry,
                skips.iter().map(|skip| &skip.candidate),
            )?;
            self.store.request_capacity_suspend_with_skips(
                &self.claim,
                &scheduled,
                &quota.reservation.receipt,
                now,
            )
        } else {
            self.store.request_capacity_suspend(
                &self.claim,
                &scheduled,
                &quota.reservation.receipt,
                now,
            )
        }
        .map_err(recovery("reserve quota suspension"))?;
        Ok(AfterExhaustion::Suspended)
    }

    /// Reserve and admit the one-shot opening for a reserved frozen candidate.
    fn admit_opening(
        &self,
        cycle: &RecoveryCycle,
        reservation: &CandidateReservation,
    ) -> Result<QuotaOpenPermit, StageError> {
        let target = self
            .policy
            .candidates()
            .iter()
            .find(|target| target.candidate() == &reservation.candidate)
            .cloned()
            .ok_or_else(|| {
                StageError::RecoveryRequired(
                    "reserved quota candidate is absent from frozen launch policy".into(),
                )
            })?;
        let launch = QuotaLaunchContract::new(
            target,
            surge_core::id::StageInvocationId::new(),
            surge_core::execution_recovery::SessionOpenMode::New,
            None,
        )
        .map_err(recovery("build quota launch contract"))?;
        let handoff = self
            .store
            .reserve_quota_open(&self.claim, cycle, reservation, launch)
            .map_err(recovery("reserve quota opening"))?;
        self.store
            .admit_provider_open(&self.claim, handoff.operation())
            .map_err(recovery("admit quota opening"))
    }
}

/// The selected opening's configured route and pin must still be what the
/// host admitted, immediately before the provider effect.
fn validate_opening_route(
    opening: Option<&QuotaOpenPermit>,
    host_home: &std::path::Path,
    working_dir: &std::path::Path,
    run_agent_registry: Option<&surge_acp::Registry>,
    session_config: &surge_acp::bridge::SessionConfig,
) -> Result<(), StageError> {
    let Some(permit) = opening else {
        return Ok(());
    };
    let Some(route) = permit.launch().candidate().configured_route() else {
        return Ok(());
    };
    let registry = crate::engine::capacity::run_agent_registry_or_builtin(run_agent_registry);
    if registry
        .find_normalized(&session_config.runtime)
        .and_then(|entry| entry.capacity_route.as_ref())
        != Some(route)
    {
        return Err(StageError::RecoveryRequired(
            "selected configured route declaration changed before provider effect".into(),
        ));
    }
    let actual = crate::engine::capacity_routes::configured_pin(
        host_home,
        &session_config.runtime,
        &session_config.agent_kind,
        route,
        &session_config.env,
        working_dir,
    );
    if permit.launch().candidate().configured_pin().is_some()
        && actual.as_ref() != permit.launch().candidate().configured_pin()
    {
        return Err(StageError::RecoveryRequired(
            "selected configured source changed before provider effect".into(),
        ));
    }
    Ok(())
}

/// Bind the frozen quota stage to this provider opening, begin its recovery
/// cycle and reserve the primary candidate.
///
/// # Errors
/// A persisted stage that no longer matches `policy`, a policy without a
/// primary candidate, or any store conflict.
#[must_use = "a quota cycle that is not kept cannot record exhaustion"]
fn initialize_task_quota_cycle(
    store: &WorkItemStore,
    claim: &WorkItemLaunchClaim,
    policy: &FrozenQuotaStage,
    invocation: surge_core::id::StageInvocationId,
    opening_seq: u64,
    control_generation: u64,
) -> Result<TaskQuotaCycle, WorkItemError> {
    let bound = store.bind_quota_stage(claim, invocation, opening_seq)?;
    if &bound != policy {
        return Err(WorkItemError::Conflict(
            "persisted quota stage changed after opening".into(),
        ));
    }
    let cycle = store.begin_recovery_cycle(claim, &invocation.to_string(), control_generation)?;
    let primary = policy
        .candidates()
        .first()
        .ok_or_else(|| WorkItemError::Invalid("frozen quota primary is absent".into()))?;
    let reservation = store.reserve_recovery_candidate(claim, &cycle, primary.candidate())?;
    let cycle = store.recovery_cycle(claim.run(), &invocation.to_string(), cycle.generation)?;
    Ok(TaskQuotaCycle {
        cycle,
        reservation,
        opening_seq,
    })
}

/// Persist the selected candidate's typed exhaustion before cleanup can
/// obscure the stage error. A non-quota error writes nothing and returns the
/// cycle unchanged, so it can never clear prior exhaustion.
///
/// # Errors
/// Delay or expiry arithmetic that overflows, or a store conflict.
#[must_use = "the returned cycle carries the recorded exhaustion"]
fn record_selected_rate_limit(
    store: &WorkItemStore,
    claim: &WorkItemLaunchClaim,
    policy: &FrozenQuotaStage,
    quota: &TaskQuotaCycle,
    invocation: surge_core::id::StageInvocationId,
    session: surge_core::SessionId,
    error: &SendMessageError,
) -> Result<RecoveryCycle, WorkItemError> {
    let SendMessageError::RateLimited {
        retry_after,
        details,
    } = error
    else {
        return Ok(quota.cycle.clone());
    };
    let now = store.clock().now_ms();
    let delay_ms = retry_after
        .map(|delay| {
            i64::try_from(delay.as_millis())
                .map_err(|_| WorkItemError::Invalid("quota retry delay overflow".into()))
        })
        .transpose()?;
    let expires_at_ms = now
        .checked_add(policy.observation_ttl_ms())
        .ok_or_else(|| WorkItemError::Invalid("quota observation expiry overflow".into()))?;
    let reset_at_ms = delay_ms
        .map(|delay| {
            now.checked_add(delay)
                .ok_or_else(|| WorkItemError::Invalid("quota reset overflow".into()))
        })
        .transpose()?;
    let retry_after_ms = delay_ms
        .map(u64::try_from)
        .transpose()
        .map_err(|_| WorkItemError::Invalid("quota retry delay is negative".into()))?;
    let source = QuotaRateLimitSource::new(
        quota.opening_seq,
        invocation,
        session,
        retry_after_ms,
        details.clone(),
    )?;
    let observation = QuotaObservation::new(QuotaEvidence::Observed {
        observed_at_ms: now,
        expires_at_ms,
        available: false,
        reset_at_ms,
    })?;
    store.record_selected_rate_limit(
        claim,
        &quota.cycle,
        &quota.reservation.receipt,
        &source,
        &observation,
    )
}

/// Recheck every skipped configured candidate against today's declared source
/// snapshot (no provider RPCs), under the run's own agent registry. The one
/// loop every skip-revalidation path shares.
///
/// # Errors
/// The first candidate whose route declaration or source snapshot changed.
fn verify_skipped_snapshots<'a>(
    store: &WorkItemStore,
    working_dir: &std::path::Path,
    run_agent_registry: Option<&surge_acp::Registry>,
    skipped: impl IntoIterator<
        Item = &'a surge_persistence::work_items::recovery_cycles::FrozenQuotaCandidate,
    >,
) -> Result<(), crate::engine::stage::StageError> {
    let registry = crate::engine::capacity::run_agent_registry_or_builtin(run_agent_registry);
    for candidate in skipped {
        crate::engine::capacity_routes::verify_skipped_snapshot(
            store.host_home(),
            working_dir,
            &registry,
            candidate,
        )?;
    }
    Ok(())
}

/// Choose an actual future reset or an explicit bounded policy retry.
///
/// A reset is trusted only while its observation is still fresh; otherwise
/// the frozen policy backoff applies. Neither wake origin asserts that a
/// provider has regained capacity.
///
/// # Errors
/// Fresh *available* evidence (it cannot justify an exhausted wake), or
/// arithmetic overflow.
#[must_use = "a planned wake does nothing until it is armed"]
fn plan_quota_wake(
    identity: String,
    policy: &FrozenQuotaStage,
    evidence: &QuotaObservation,
    now: i64,
) -> Result<RecoveryWake, QuotaRecoveryError> {
    if let QuotaEvidence::Observed {
        available: true,
        observed_at_ms,
        expires_at_ms,
        ..
    } = evidence.evidence()
        && *observed_at_ms <= now
        && now < *expires_at_ms
    {
        return Err(QuotaRecoveryError::Invalid(
            "fresh available quota cannot arm an exhausted wake",
        ));
    }
    let (origin, due) = match evidence.evidence() {
        QuotaEvidence::Observed {
            available: false,
            observed_at_ms,
            expires_at_ms,
            reset_at_ms: Some(reset),
        } if *observed_at_ms <= now && now < *expires_at_ms && *reset > now => {
            (WakeOrigin::ObservedReset, *reset)
        },
        _ => (
            WakeOrigin::PolicyBackoff,
            now.checked_add(policy.policy_backoff_ms())
                .ok_or(QuotaRecoveryError::Invalid("quota wake overflow"))?,
        ),
    };
    Ok(RecoveryWake::new(identity, origin, now, due)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use surge_core::{ContentHash, NodeKey};
    use surge_persistence::work_items::recovery_cycles::{
        AccountEvidence, FrozenQuotaCandidate, QuotaRoutingMode, RecoveryCandidate,
    };

    fn policy() -> FrozenQuotaStage {
        FrozenQuotaStage::new(
            NodeKey::try_from("impl").unwrap(),
            QuotaRoutingMode::Disabled,
            vec![
                FrozenQuotaCandidate::new(
                    RecoveryCandidate::new("a".into(), AccountEvidence::Unknown).unwrap(),
                    None,
                    ContentHash::compute(b"a"),
                )
                .unwrap(),
            ],
            1000,
            1000,
        )
        .unwrap()
    }

    fn exhausted(observed: i64, expires: i64, reset: Option<i64>) -> QuotaObservation {
        QuotaObservation::new(QuotaEvidence::Observed {
            observed_at_ms: observed,
            expires_at_ms: expires,
            available: false,
            reset_at_ms: reset,
        })
        .unwrap()
    }

    #[test]
    fn unsupported_and_unknown_evidence_plan_bounded_retry() {
        for evidence in [QuotaObservation::unsupported(), QuotaObservation::unknown()] {
            let wake = plan_quota_wake("wake".into(), &policy(), &evidence, 100).unwrap();
            assert_eq!(wake.origin(), &WakeOrigin::PolicyBackoff);
            assert_eq!(wake.due_at_ms(), 1100);
        }
    }

    #[test]
    fn fresh_future_reset_wakes_at_the_reset() {
        let wake = plan_quota_wake(
            "wake".into(),
            &policy(),
            &exhausted(100, 1100, Some(500)),
            100,
        )
        .unwrap();
        assert_eq!(wake.origin(), &WakeOrigin::ObservedReset);
        assert_eq!(wake.due_at_ms(), 500);
    }

    #[test]
    fn past_or_missing_reset_falls_back_to_policy_backoff() {
        for reset in [None, Some(50)] {
            let wake = plan_quota_wake("wake".into(), &policy(), &exhausted(10, 1100, reset), 100)
                .unwrap();
            assert_eq!(wake.origin(), &WakeOrigin::PolicyBackoff);
            assert_eq!(wake.due_at_ms(), 1100);
        }
    }

    #[test]
    fn expired_observation_does_not_trust_its_reset() {
        let wake = plan_quota_wake(
            "wake".into(),
            &policy(),
            &exhausted(10, 90, Some(5_000)),
            100,
        )
        .unwrap();
        assert_eq!(wake.origin(), &WakeOrigin::PolicyBackoff);
    }

    #[test]
    fn fresh_available_evidence_cannot_arm_a_wake() {
        let available = QuotaObservation::new(QuotaEvidence::Observed {
            observed_at_ms: 100,
            expires_at_ms: 1100,
            available: true,
            reset_at_ms: None,
        })
        .unwrap();
        assert!(plan_quota_wake("wake".into(), &policy(), &available, 100).is_err());
    }
}
