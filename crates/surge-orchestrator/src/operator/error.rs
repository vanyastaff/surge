//! Typed failures of the operator services.
//!
//! Display strings are the operator-facing messages the `surge` CLI and the
//! MCP server have always shown; source chains keep the storage fault
//! reachable for callers that print `{:#}`.

use surge_core::run_state::FoldError;
use surge_core::{Attention, RunId};
use surge_persistence::runs::{OpenError, StorageError};

use surge_persistence::PersistenceError;

use crate::engine::EngineError;

/// Minimum length of a run-id suffix accepted by
/// [`resolve_run_id`](crate::operator::resolve_run_id). Short/empty suffixes
/// (`""` matches every run via `ends_with`) are rejected outright.
pub(crate) const MIN_SUFFIX_LEN: usize = 6;

/// Upper bound on runs scanned when matching a suffix. Chosen well above any
/// realistic active-run count; if a scan hits it, the match is reported as
/// possibly-truncated rather than silently wrong.
pub(crate) const SUFFIX_SCAN_LIMIT: usize = 5000;

/// Why a run id given by an operator (CLI argument or MCP client) did not
/// resolve to exactly one run. Storage failures are *not* this type: they are
/// other [`OperatorError`] variants; [`OperatorError::kind`] tells a bad id
/// from a fault.
#[derive(Debug, thiserror::Error)]
pub enum RunIdError {
    /// Not a full ULID and too short to match by suffix.
    #[error(
        "run id {value:?} is not a full ULID and is too short to match by suffix \
         (need ≥{MIN_SUFFIX_LEN} chars, or pass the full id)"
    )]
    TooShort {
        /// The id as the operator gave it.
        value: String,
    },
    /// Matched one run, but the scan hit its ceiling so the match may not be unique.
    #[error(
        "matched run {value:?}, but there are ≥{SUFFIX_SCAN_LIMIT} runs so the match \
         may be ambiguous; pass the full run id"
    )]
    PossiblyAmbiguous {
        /// The id as the operator gave it.
        value: String,
    },
    /// No run ends with the given suffix.
    #[error("no run matching {value:?}")]
    NotFound {
        /// The id as the operator gave it.
        value: String,
    },
    /// More than one run ends with the given suffix.
    #[error("{count} runs match {value:?}; use the full run id")]
    Ambiguous {
        /// The id as the operator gave it.
        value: String,
        /// How many runs matched.
        count: usize,
    },
}

/// Coarse classification of an [`OperatorError`], for adapters that map
/// failures onto their own error codes without matching every variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperatorErrorKind {
    /// The operator's input is unusable (malformed, ambiguous or mismatched).
    InvalidInput,
    /// The named run does not exist.
    NotFound,
    /// The run is not blocked on human input.
    NotAwaitingInput,
    /// A storage, daemon or internal fault.
    Fault,
}

/// Failure of an operator service call.
///
/// Display strings are neutral (no CLI flags); adapters add their own hints.
/// Underlying faults are reachable through [`std::error::Error::source`], so
/// print the whole chain (`{:#}` on an `anyhow::Error`) to show the cause.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum OperatorError {
    /// The run id did not resolve to exactly one run.
    #[error(transparent)]
    RunId(#[from] RunIdError),
    /// The run registry could not be listed.
    #[error("list runs")]
    ListRuns(#[source] StorageError),
    /// A run's event log could not be opened.
    #[error("open run {run_id}")]
    OpenRun {
        /// The run whose log failed to open.
        run_id: RunId,
        /// Underlying storage fault.
        #[source]
        source: OpenError,
    },
    /// A run's event log could not be read.
    #[error("read events for {run_id}")]
    ReadEvents {
        /// The run whose log failed to read.
        run_id: RunId,
        /// Underlying storage fault.
        #[source]
        source: StorageError,
    },
    /// A run's event log did not fold into a run state.
    #[error("fold run {run_id}")]
    Fold {
        /// The run whose log failed to fold.
        run_id: RunId,
        /// Why the fold rejected the log.
        #[source]
        cause: FoldError,
    },
    /// The run is not blocked on pipeline human input.
    #[error("run {run_id} is not waiting for human input (attention: {attention:?})")]
    NotAwaitingInput {
        /// The inspected run.
        run_id: RunId,
        /// What the run is actually doing.
        attention: Attention,
    },
    /// A tool-driven request needs a text or JSON answer and got neither.
    #[error("this request awaits a free-form answer (text or JSON)")]
    MissingToolAnswer,
    /// A `HumanGate` needs an outcome key and got none.
    #[error("this gate needs an outcome")]
    MissingOutcome,
    /// The outcome is not one the gate declares.
    #[error("outcome {outcome:?} is not valid for this gate; valid: {valid}")]
    InvalidOutcome {
        /// The rejected outcome key.
        outcome: String,
        /// The gate's declared keys, comma-separated.
        valid: String,
    },
    /// The pending request is a bootstrap approval, which only a human may give.
    #[error(
        "run is blocked at a bootstrap approval (@{node}); description, roadmap and flow \
         approvals must be given by a human"
    )]
    HumanOnlyGate {
        /// The bootstrap gate node.
        node: String,
    },
    /// The daemon hosting the run declined or could not take the answer.
    #[error("resolve failed")]
    DeliveryFailed {
        /// The daemon-side failure.
        #[source]
        cause: EngineError,
    },
    /// The task-ledger index query failed.
    #[error(transparent)]
    TaskLedger(StorageError),
    /// The steer message is blank.
    #[error("steer message must not be blank")]
    BlankSteer,
    /// The daemon hosting the run declined or could not take the steer request.
    #[error("steer failed")]
    SteerFailed {
        /// The daemon-side failure.
        #[source]
        cause: EngineError,
    },
    /// The project-memory store could not be opened or searched.
    #[error(transparent)]
    MemoryStore(PersistenceError),
    /// The OTLP trace could not be rendered as JSON.
    #[error("render trace as JSON")]
    RenderTrace(#[source] serde_json::Error),
}

impl OperatorError {
    /// Coarse classification for adapters.
    pub fn kind(&self) -> OperatorErrorKind {
        match self {
            Self::RunId(RunIdError::NotFound { .. }) => OperatorErrorKind::NotFound,
            Self::RunId(_)
            | Self::MissingToolAnswer
            | Self::MissingOutcome
            | Self::InvalidOutcome { .. }
            | Self::HumanOnlyGate { .. }
            | Self::BlankSteer => OperatorErrorKind::InvalidInput,
            Self::NotAwaitingInput { .. } => OperatorErrorKind::NotAwaitingInput,
            Self::ListRuns(_)
            | Self::OpenRun { .. }
            | Self::ReadEvents { .. }
            | Self::Fold { .. }
            | Self::DeliveryFailed { .. }
            | Self::TaskLedger(_)
            | Self::SteerFailed { .. }
            | Self::MemoryStore(_)
            | Self::RenderTrace(_) => OperatorErrorKind::Fault,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_separates_a_bad_id_from_a_missing_run_and_a_fault() {
        let not_found = OperatorError::from(RunIdError::NotFound { value: "x".into() });
        assert_eq!(not_found.kind(), OperatorErrorKind::NotFound);
        let too_short = OperatorError::from(RunIdError::TooShort { value: "x".into() });
        assert_eq!(too_short.kind(), OperatorErrorKind::InvalidInput);
        assert_eq!(
            OperatorError::BlankSteer.kind(),
            OperatorErrorKind::InvalidInput
        );
        assert_eq!(
            OperatorError::ListRuns(StorageError::Io(std::io::Error::other("disk"))).kind(),
            OperatorErrorKind::Fault
        );
    }
}
