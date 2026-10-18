//! A run's terminal verdict: the one rule for "did this run end, and how?"
//! (#95).
//!
//! The per-run event log is the only source of truth for termination. Every
//! surface that needs the answer — resume, the CLI disk check, the replay
//! view, the registry projection, daemon recovery, the status snapshot, the
//! run report and the trace exporter — calls [`TerminalVerdict::of`] and keeps
//! only its own *policy* for each of the three answers. None re-derives the
//! answer.
//!
//! # Definitive terminal events
//! Exactly `RunCompleted`, `RunFailed` and `RunAborted`. `RunParked`,
//! `RunWokeFromPark`, `RunSuspended`, `RunRecoveryRequired` and `ForkCreated`
//! are not terminal and never affect the verdict. The classification below is
//! an exhaustive match with no wildcard arm, so a new event variant forces a
//! decision here (the ADR-0017 precedent).
//!
//! # Conflict
//! Any second definitive terminal event is a conflict, including an identical
//! duplicate. The log is append-only and exactly one owner may end a run: a
//! second terminal append proves two writers each believed they owned
//! termination, whatever the payload says. Conflict always means
//! *unconfirmed*: no surface may resolve it by picking one claim.
//!
//! # Invariants
//! - Pure and infallible: no I/O, no clock, no panic.
//! - Monotone over prefixes: `Open` may become `Terminal` or `Conflict`,
//!   `Terminal` may become `Conflict`, nothing goes back.
//! - Independent of non-terminal events.
//!
//! Execution activity after the first terminal event (a stage entered, an
//! outcome reported, …) is evidence of a writer bug, not a terminal claim: it
//! is reported separately by [`TerminalHistory::post_terminal_activity`] and
//! never changes the verdict.
use serde::{Deserialize, Serialize};

use crate::keys::NodeKey;
use crate::run_event::{EventPayload, RunEvent};

/// How a run ended, as one definitive terminal event states it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TerminalOutcome {
    /// `RunCompleted` at a terminal node.
    Completed { terminal_node: NodeKey },
    /// `RunFailed`.
    Failed { error: String },
    /// `RunAborted`.
    Aborted { reason: String },
}

impl TerminalOutcome {
    /// The outcome's kind, without its payload.
    #[must_use]
    pub fn kind(&self) -> TerminalKind {
        match self {
            Self::Completed { .. } => TerminalKind::Completed,
            Self::Failed { .. } => TerminalKind::Failed,
            Self::Aborted { .. } => TerminalKind::Aborted,
        }
    }
}

/// The kind of a definitive terminal event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalKind {
    Completed,
    Failed,
    Aborted,
}

/// One definitive terminal event and where it sits in the log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalClaim {
    /// The event's sequence number.
    pub seq: u64,
    /// What the event claims.
    pub outcome: TerminalOutcome,
}

/// Whether a run's log says it ended, and how.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "verdict", rename_all = "snake_case")]
pub enum TerminalVerdict {
    /// No definitive terminal event.
    #[default]
    Open,
    /// Exactly one definitive terminal event.
    Terminal(TerminalClaim),
    /// Two or more definitive terminal events, in sequence order (never
    /// fewer than two). Unconfirmed: no claim may be picked as the answer.
    Conflict { claims: Vec<TerminalClaim> },
}

impl TerminalVerdict {
    /// The verdict over an ordered sequence of `(seq, payload)` pairs.
    #[must_use]
    pub fn of<'e>(events: impl IntoIterator<Item = (u64, &'e EventPayload)>) -> Self {
        TerminalHistory::of(events).verdict
    }

    /// The verdict over core run events.
    #[must_use]
    pub fn of_run_events(events: &[RunEvent]) -> Self {
        Self::of(events.iter().map(|event| (event.seq, &event.payload)))
    }

    /// The single terminal claim, when the verdict is [`Self::Terminal`].
    #[must_use]
    pub fn terminal(&self) -> Option<&TerminalClaim> {
        match self {
            Self::Terminal(claim) => Some(claim),
            Self::Open | Self::Conflict { .. } => None,
        }
    }

    /// Sequence numbers of the conflicting claims; empty unless
    /// [`Self::Conflict`].
    #[must_use]
    pub fn conflicting_seqs(&self) -> Vec<u64> {
        match self {
            Self::Conflict { claims } => claims.iter().map(|claim| claim.seq).collect(),
            Self::Open | Self::Terminal(_) => Vec::new(),
        }
    }
}

/// The verdict plus the post-terminal activity diagnostic, accumulated one
/// event at a time.
#[derive(Debug, Clone, Default)]
pub struct TerminalHistory {
    claims: Vec<TerminalClaim>,
    post_terminal_activity: Vec<u64>,
    verdict: TerminalVerdict,
}

impl TerminalHistory {
    /// Accumulate a whole ordered sequence of `(seq, payload)` pairs.
    #[must_use]
    pub fn of<'e>(events: impl IntoIterator<Item = (u64, &'e EventPayload)>) -> Self {
        let mut history = Self::default();
        for (seq, payload) in events {
            history.observe(seq, payload);
        }
        history
    }

    /// Observe the next event of the log, in sequence order.
    pub fn observe(&mut self, seq: u64, payload: &EventPayload) {
        match classify(payload) {
            EventClass::Terminal(outcome) => {
                self.claims.push(TerminalClaim { seq, outcome });
                self.verdict = match self.claims.as_slice() {
                    [only] => TerminalVerdict::Terminal(only.clone()),
                    claims => TerminalVerdict::Conflict {
                        claims: claims.to_vec(),
                    },
                };
            },
            EventClass::Activity if !self.claims.is_empty() => {
                self.post_terminal_activity.push(seq);
            },
            EventClass::Activity | EventClass::Bookkeeping => {},
        }
    }

    /// The verdict so far.
    #[must_use]
    pub fn verdict(&self) -> &TerminalVerdict {
        &self.verdict
    }

    /// Consume the accumulator, returning the verdict.
    #[must_use]
    pub fn into_verdict(self) -> TerminalVerdict {
        self.verdict
    }

    /// Sequence numbers of execution activity observed after the first
    /// terminal event: evidence of a writer bug, never a terminal claim.
    #[must_use]
    pub fn post_terminal_activity(&self) -> &[u64] {
        &self.post_terminal_activity
    }
}

/// What an event means for the verdict.
enum EventClass {
    /// A definitive terminal claim.
    Terminal(TerminalOutcome),
    /// Execution progress: suspicious after a terminal event.
    Activity,
    /// Lineage, observation, usage and lifecycle bookkeeping: legal after a
    /// terminal event (a fork's `ForkCreated`, park/wake, hook records, …).
    Bookkeeping,
}

/// The one classification of every event variant. Exhaustive on purpose.
#[allow(clippy::too_many_lines)]
fn classify(payload: &EventPayload) -> EventClass {
    match payload {
        EventPayload::RunCompleted { terminal_node } => {
            EventClass::Terminal(TerminalOutcome::Completed {
                terminal_node: terminal_node.clone(),
            })
        },
        EventPayload::RunFailed { error } => EventClass::Terminal(TerminalOutcome::Failed {
            error: error.clone(),
        }),
        EventPayload::RunAborted { reason } => EventClass::Terminal(TerminalOutcome::Aborted {
            reason: reason.clone(),
        }),

        EventPayload::RunStarted { .. }
        | EventPayload::RunContinued { .. }
        | EventPayload::RunSuspended { .. }
        | EventPayload::RunRecoveryRequired { .. }
        | EventPayload::PipelineMaterialized { .. }
        | EventPayload::GraphRevisionAccepted { .. }
        | EventPayload::BootstrapStageStarted { .. }
        | EventPayload::BootstrapArtifactProduced { .. }
        | EventPayload::BootstrapApprovalRequested { .. }
        | EventPayload::BootstrapApprovalDecided { .. }
        | EventPayload::BootstrapEditRequested { .. }
        | EventPayload::RoadmapPatchApplied { .. }
        | EventPayload::RoadmapUpdated { .. }
        | EventPayload::StageEntered { .. }
        | EventPayload::QuotaStagePlanned { .. }
        | EventPayload::StageInputsResolved { .. }
        | EventPayload::SessionEstablishmentRequested { .. }
        | EventPayload::SessionOpened { .. }
        | EventPayload::ToolCalled { .. }
        | EventPayload::ToolResultReceived { .. }
        | EventPayload::ArtifactProduced { .. }
        | EventPayload::OutcomeReported { .. }
        | EventPayload::StageOutcomeCommitted { .. }
        | EventPayload::StageRouteCommitted { .. }
        | EventPayload::GateStageOutcomeCommitted { .. }
        | EventPayload::GateStageRouteCommitted { .. }
        | EventPayload::StageCompleted { .. }
        | EventPayload::StageFailed { .. }
        | EventPayload::EdgeTraversed { .. }
        | EventPayload::LoopIterationStarted { .. }
        | EventPayload::LoopIterationCompleted { .. }
        | EventPayload::LoopCompleted { .. }
        | EventPayload::SubgraphEntered { .. }
        | EventPayload::SubgraphExited { .. }
        | EventPayload::TaskSplit { .. }
        | EventPayload::TaskAcceptedByHuman { .. }
        | EventPayload::TaskStatusChanged { .. }
        | EventPayload::TaskDiscovered { .. }
        | EventPayload::TaskVerified { .. }
        | EventPayload::StageRuntimeRotated { .. }
        | EventPayload::PullRequestOpened { .. }
        | EventPayload::RequirementRevised { .. }
        | EventPayload::SteerDelivered { .. }
        | EventPayload::HumanInputRequested { .. }
        | EventPayload::HumanInputResolved { .. }
        | EventPayload::HumanInputTimedOut { .. }
        | EventPayload::ApprovalRequested { .. }
        | EventPayload::ApprovalDecided { .. }
        | EventPayload::SandboxElevationRequested { .. }
        | EventPayload::SandboxElevationDecided { .. }
        | EventPayload::SandboxElevationTimedOut { .. }
        | EventPayload::OutcomeRejectedByHook { .. }
        | EventPayload::SkillBound { .. }
        | EventPayload::StageToolReceipt { .. }
        | EventPayload::WorkItemAttemptBound { .. }
        | EventPayload::OwnedFlowInputsBound { .. } => EventClass::Activity,

        EventPayload::RunParked { .. }
        | EventPayload::RunWokeFromPark { .. }
        | EventPayload::ForkCreated { .. }
        | EventPayload::HookExecuted { .. }
        | EventPayload::OwnedFlowWakeRefused { .. }
        | EventPayload::VerificationSubjectObserved { .. }
        | EventPayload::VerificationCriteriaAccepted { .. }
        | EventPayload::BootstrapTelemetry { .. }
        | EventPayload::RoadmapPatchDrafted { .. }
        | EventPayload::RoadmapPatchApprovalRequested { .. }
        | EventPayload::RoadmapPatchApprovalDecided { .. }
        | EventPayload::ExecutionWriterIntent { .. }
        | EventPayload::ExecutionWriterEstablished { .. }
        | EventPayload::ExecutionWriterClosed { .. }
        | EventPayload::ExecutionWriterGroupStopped { .. }
        | EventPayload::SessionClosed { .. }
        | EventPayload::RuntimeVersionWarning { .. }
        | EventPayload::ProviderUsageObserved { .. }
        | EventPayload::ContextUsageObserved { .. }
        | EventPayload::TokensConsumed { .. }
        | EventPayload::BudgetWarningRaised { .. }
        | EventPayload::BudgetExceeded { .. }
        | EventPayload::NotifyDelivered { .. }
        | EventPayload::EscalationRequested { .. }
        | EventPayload::ContinuationSeedBound { .. } => EventClass::Bookkeeping,
    }
}

#[cfg(test)]
mod tests;
