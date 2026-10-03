//! Shared operator-facing status, separate from execution and decision ownership.

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::RunStatus;
use crate::capacity::WakeBasis;
use crate::run_state::{Attention, RunState, TerminalReason};

/// Why progress is waiting. Recovery always requires inspection by an operator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WaitingReason {
    /// A durable human-input request or bootstrap approval is unresolved.
    HumanInput,
    /// A durable capacity pause has a recorded wake time.
    Capacity {
        /// Recorded wake time, not a promise of successful dispatch.
        until: DateTime<Utc>,
        /// Observed reset or configured backoff.
        basis: WakeBasis,
        /// Runtime correlated with this run's parked event.
        runtime: Option<String>,
    },
    /// The registry confirms loss of the hosting daemon without durable completion.
    RecoveryRequired,
}

/// A display fact. Unknown evidence must never be presented as active work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", content = "reason", rename_all = "snake_case")]
pub enum RunDisplayState {
    /// Trusted history shows active execution.
    Working,
    /// Trusted history shows completion.
    Done(TerminalReason),
    /// Progress is blocked for a known reason.
    Waiting(WaitingReason),
    /// Missing, unreadable or discontinuous evidence.
    Unknown,
}

impl RunDisplayState {
    /// Project authoritative history and independent registry crash evidence.
    /// A durable terminal event wins over a stale crash label.
    #[must_use]
    pub fn from_state(state: Option<&RunState>, registry: Option<RunStatus>) -> Self {
        if let Some(RunState::Terminal { kind, .. }) = state {
            return Self::Done(*kind);
        }
        if registry == Some(RunStatus::Crashed) {
            return Self::Waiting(WaitingReason::RecoveryRequired);
        }
        if matches!(state, None | Some(RunState::NotStarted)) {
            return Self::Unknown;
        }
        match state.map(RunState::attention) {
            Some(Attention::NeedsInput) => Self::Waiting(WaitingReason::HumanInput),
            Some(Attention::Waiting {
                until,
                basis,
                runtime,
            }) => Self::Waiting(WaitingReason::Capacity {
                until,
                basis,
                runtime,
            }),
            Some(Attention::Working) => Self::Working,
            Some(Attention::Done(kind)) => Self::Done(kind),
            None => Self::Unknown,
        }
    }

    /// Shared reason text across CLI, Telegram and desktop.
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::Working => "Running",
            Self::Done(TerminalReason::Completed) => "Completed",
            Self::Done(TerminalReason::Failed) => "Failed",
            Self::Done(TerminalReason::Aborted) => "Aborted",
            Self::Waiting(WaitingReason::HumanInput) => "Waiting for human input",
            Self::Waiting(WaitingReason::Capacity { .. }) => "Waiting for capacity",
            Self::Waiting(WaitingReason::RecoveryRequired) => "Recovery requires an operator",
            Self::Unknown => "Run state is unconfirmed",
        }
    }

    /// Shared next action; display text never initiates a recovery or answers a gate.
    #[must_use]
    pub const fn next_action(&self) -> Option<&'static str> {
        match self {
            Self::Waiting(WaitingReason::HumanInput) => {
                Some("Answer the pending request in the Inbox.")
            },
            Self::Waiting(WaitingReason::Capacity { .. }) => Some(
                "Wait for the recorded wake time; successful dispatch depends on available capacity.",
            ),
            Self::Waiting(WaitingReason::RecoveryRequired) => {
                Some("Inspect the run and choose whether to resume or abort.")
            },
            Self::Unknown => Some("Inspect the run journal before taking action."),
            Self::Working | Self::Done(_) => None,
        }
    }

    /// Typed wake evidence, available only for a capacity pause.
    #[must_use]
    pub const fn wake(&self) -> Option<(DateTime<Utc>, WakeBasis)> {
        match self {
            Self::Waiting(WaitingReason::Capacity { until, basis, .. }) => Some((*until, *basis)),
            _ => None,
        }
    }
}
