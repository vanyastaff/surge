//! Task-owned quota evidence and bounded retry planning.
//! Provider opening remains gated by persistence-issued one-shot authority.
use surge_acp::{
    bridge::error::SendMessageError,
    quota_observation::{SubscriptionAvailability, SubscriptionQuotaProbe, observe_rate_limit},
};
use surge_persistence::{
    runs::Clock,
    work_items::{
        WorkItemError, WorkItemLaunchClaim, WorkItemStore,
        recovery_cycles::{
            CandidateReservation, FrozenQuotaStage, QuotaEvidence, QuotaObservation, RecoveryCycle,
            RecoveryWake, WakeOrigin,
        },
    },
};

/// Quota recovery cannot discard malformed provider evidence or storage conflicts.
#[derive(Debug, thiserror::Error)]
pub(crate) enum QuotaRecoveryError {
    #[error(transparent)]
    Storage(#[from] WorkItemError),
    #[error(transparent)]
    Observation(#[from] surge_acp::quota_observation::QuotaObservationError),
    #[error("quota recovery: {0}")]
    Invalid(&'static str),
}

/// Persist original typed exhaustion before cleanup can obscure the stage error.
/// Non-quota errors produce no write and cannot clear prior exhaustion.
pub(crate) fn record_typed_rate_limit(
    store: &WorkItemStore,
    claim: &WorkItemLaunchClaim,
    cycle: &RecoveryCycle,
    reservation: &CandidateReservation,
    policy: &FrozenQuotaStage,
    error: &SendMessageError,
    clock: &dyn Clock,
) -> Result<Option<RecoveryCycle>, QuotaRecoveryError> {
    let Some(observed) = observe_rate_limit(error, clock.now_ms(), policy.observation_ttl_ms())?
    else {
        return Ok(None);
    };
    if cycle.selected_runtime.as_deref() != Some(reservation.candidate.runtime()) {
        return Err(QuotaRecoveryError::Invalid(
            "quota error belongs to an old candidate",
        ));
    }
    let evidence = QuotaObservation::new(QuotaEvidence::Observed {
        observed_at_ms: observed.observed_at_ms(),
        expires_at_ms: observed.expires_at_ms(),
        available: false,
        reset_at_ms: observed.reset_at_ms(),
    })?;
    Ok(Some(store.record_recovery_observation(
        claim,
        cycle,
        &reservation.receipt,
        &evidence,
    )?))
}

/// Probe availability remains independent of runtime binary readiness.
/// Expired provider evidence becomes Unknown, preserving stored exhaustion.
pub(crate) fn probe_evidence(
    probe: &SubscriptionQuotaProbe,
    clock: &dyn Clock,
) -> Result<QuotaObservation, QuotaRecoveryError> {
    match probe {
        SubscriptionQuotaProbe::Unsupported => Ok(QuotaObservation::unsupported()),
        SubscriptionQuotaProbe::Unknown => Ok(QuotaObservation::unknown()),
        SubscriptionQuotaProbe::Observed(observed) if !observed.is_fresh(clock.now_ms()) => {
            Ok(QuotaObservation::unknown())
        },
        SubscriptionQuotaProbe::Observed(observed) => {
            Ok(QuotaObservation::new(QuotaEvidence::Observed {
                observed_at_ms: observed.observed_at_ms(),
                expires_at_ms: observed.expires_at_ms(),
                available: observed.availability() == SubscriptionAvailability::Available,
                reset_at_ms: observed.reset_at_ms(),
            })?)
        },
    }
}

/// Choose an actual future reset or an explicit bounded policy retry.
/// Neither wake origin asserts that a provider has regained capacity.
pub(crate) fn plan_quota_wake(
    identity: String,
    policy: &FrozenQuotaStage,
    evidence: &QuotaObservation,
    clock: &dyn Clock,
) -> Result<RecoveryWake, QuotaRecoveryError> {
    let now = clock.now_ms();
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
    use surge_persistence::{
        runs::MockClock,
        work_items::recovery_cycles::{
            AccountEvidence, FrozenQuotaCandidate, QuotaRoutingMode, RecoveryCandidate,
        },
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

    #[test]
    fn unsupported_and_unknown_probe_plan_bounded_retry_without_availability() {
        let clock = MockClock::new(100);
        for probe in [
            SubscriptionQuotaProbe::Unsupported,
            SubscriptionQuotaProbe::Unknown,
        ] {
            let evidence = probe_evidence(&probe, &clock).unwrap();
            assert!(!matches!(
                evidence.evidence(),
                QuotaEvidence::Observed {
                    available: true,
                    ..
                }
            ));
            let wake = plan_quota_wake("wake".into(), &policy(), &evidence, &clock).unwrap();
            assert_eq!(wake.origin(), &WakeOrigin::PolicyBackoff);
            assert_eq!(wake.due_at_ms(), 1100);
        }
    }
}
