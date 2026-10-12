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
        CandidateReservation, FrozenQuotaStage, QuotaEvidence, QuotaObservation,
        QuotaRateLimitSource, RecoveryCycle, RecoveryWake, WakeOrigin,
    },
};

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

/// Bind the frozen quota stage to this provider opening, begin its recovery
/// cycle and reserve the primary candidate.
///
/// # Errors
/// A persisted stage that no longer matches `policy`, a policy without a
/// primary candidate, or any store conflict.
#[must_use = "a quota cycle that is not kept cannot record exhaustion"]
pub(crate) fn initialize_task_quota_cycle(
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
pub(crate) fn record_selected_rate_limit(
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
pub(crate) fn verify_skipped_snapshots<'a>(
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
pub(crate) fn plan_quota_wake(
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
