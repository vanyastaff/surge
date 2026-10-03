//! Shared predicate for a folded task's proven success.
//!
//! Live replay, historical reports and normalized SQLite views use the common
//! revision/criteria claim gate in [`crate::verification_evidence`]. Authorized
//! unbound claims remain tracked but unverified; unauthorized claims cannot
//! introduce an obligation. Host-facing queries additionally compare the sealed
//! code identity with the current workspace and check stored report bytes.
//! [`is_evidence_backed`] reads the resulting status and verification bit.
//!
//! ## A related, deliberately separate question
//!
//! [`crate::validation::ValidationErrorKind::UnverifiedSuccessPath`] (the "W4"
//! graph lint) asks a *static* question: could this **graph's shape** let some
//! run declare success without ever passing a verification-authority node?
//! That is a design-time property of a flow, checked once at graph-validation
//! time, independent of any particular run. [`is_evidence_backed`] asks a
//! *dynamic* one: did **this specific run's actual execution** get verified?
//! A well-formed graph with no `UnverifiedSuccessPath` warning can still, in
//! principle, complete a given run without its verifier node having actually
//! run (an escalation-recovered branch, a future graph shape this lint does
//! not yet cover) — the static check is a lint on the flow's authoring, not a
//! substitute for asking the run's own event log what happened. The two are
//! complementary, not duplicates of each other.

use crate::roadmap::RoadmapStatus;

/// The minimal shape a completion-with-verification-state reduces to,
/// regardless of which of the carriers named in this module's doc produced
/// it.
///
/// Deliberately not the richer `LedgerTask`/`TaskLedgerIndexRecord` types
/// themselves — a caller builds one of *those* from its own authoritative
/// source (the live fold, the registry row, a Run Report verdict) and reads
/// off just the two fields that decide evidence-backing, so this module
/// never needs to depend on `run_state` or `surge-persistence`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NodeOutcome {
    /// The roadmap status the outcome settled at.
    pub status: RoadmapStatus,
    /// Whether a verification-authority node actually confirmed it.
    pub verified: bool,
}

impl NodeOutcome {
    /// Build a [`NodeOutcome`] from its two fields.
    #[must_use]
    pub fn new(status: RoadmapStatus, verified: bool) -> Self {
        Self { status, verified }
    }
}

/// The one predicate: was this outcome actually backed by verifier evidence,
/// or merely declared?
///
/// `true` only when the outcome reached [`RoadmapStatus::Completed`] **and**
/// was verified — either half missing means "not proven," never a default.
/// Every surface listed in this module's doc calls this instead of
/// re-deriving the same two-field check.
#[must_use]
pub fn is_evidence_backed(outcome: &NodeOutcome) -> bool {
    outcome.status == RoadmapStatus::Completed && outcome.verified
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completed_and_verified_is_evidence_backed() {
        let outcome = NodeOutcome::new(RoadmapStatus::Completed, true);
        assert!(is_evidence_backed(&outcome));
    }

    #[test]
    fn completed_but_unverified_is_not_evidence_backed() {
        let outcome = NodeOutcome::new(RoadmapStatus::Completed, false);
        assert!(!is_evidence_backed(&outcome));
    }

    /// Every non-`Completed` status, read off [`RoadmapStatus::ALL`] rather
    /// than a hand-written list here — that array is exhaustive by
    /// construction (see its own doc comment), so a future ninth status
    /// reaches this test with no second edit.
    fn non_completed_statuses() -> impl Iterator<Item = RoadmapStatus> {
        RoadmapStatus::ALL
            .into_iter()
            .filter(|status| *status != RoadmapStatus::Completed)
    }

    /// `verified: true` never overrides a non-`Completed` status — a
    /// pre-`Completed` combination is not representable through any
    /// production writer today, but the predicate itself must not let a
    /// stray `verified` flag alone declare success.
    #[test]
    fn verified_flag_alone_never_backs_a_non_completed_status() {
        for status in non_completed_statuses() {
            let outcome = NodeOutcome::new(status, true);
            assert!(
                !is_evidence_backed(&outcome),
                "status {status:?} + verified=true must not be evidence-backed"
            );
        }
    }

    /// Every non-`Completed` status with `verified: false` — the ordinary
    /// "not done yet" / "failed" cases — must also read as not
    /// evidence-backed.
    #[test]
    fn unverified_non_completed_statuses_are_not_evidence_backed() {
        for status in non_completed_statuses() {
            let outcome = NodeOutcome::new(status, false);
            assert!(!is_evidence_backed(&outcome));
        }
    }
}
