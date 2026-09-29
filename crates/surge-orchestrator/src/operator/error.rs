//! Typed failures of the operator services.
//!
//! Display strings are the operator-facing messages the `surge` CLI and the
//! MCP server have always shown; source chains keep the storage fault
//! reachable for callers that print `{:#}`.

use surge_core::run_state::FoldError;
use surge_core::{Attention, RunId};
use surge_persistence::runs::{OpenError, StorageError};

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
/// other [`OperatorError`] variants, so a caller can tell a bad id from a
/// fault by matching [`OperatorError::RunId`].
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

/// Failure of an operator service call.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum OperatorError {
    /// The run id did not resolve to exactly one run.
    #[error(transparent)]
    RunId(#[from] RunIdError),
    /// The run registry could not be listed.
    #[error("list runs")]
    ListRuns(#[source] StorageError),
    /// The run registry could not be listed while matching a run-id suffix.
    #[error("list runs for id match")]
    ListRunsForIdMatch(#[source] StorageError),
    /// A run's event log could not be opened.
    #[error("open run {run_id}")]
    OpenRun {
        /// The run whose log failed to open.
        run_id: RunId,
        /// Underlying storage fault.
        #[source]
        source: OpenError,
    },
    /// A run's event log could not be opened for a report or trace.
    #[error("open event log for run {run_id}")]
    OpenEventLog {
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
    #[error("fold run {run_id}: {cause}")]
    Fold {
        /// The run whose log failed to fold.
        run_id: RunId,
        /// Why the fold rejected the log.
        cause: FoldError,
    },
    /// The run is not blocked on pipeline human input.
    #[error(
        "run {run_id} is not waiting for human input (attention: {attention:?}). \
         Bootstrap approvals are answered via `surge bootstrap` or Telegram."
    )]
    NotAwaitingInput {
        /// The inspected run.
        run_id: RunId,
        /// What the run is actually doing.
        attention: Attention,
    },
    /// The operator's raw JSON answer did not parse.
    #[error("parse --json")]
    ParseAnswerJson(#[source] serde_json::Error),
    /// A tool-driven request needs a `text` or `json` answer and got neither.
    #[error("this run awaits a free-form tool response; pass --text or --json")]
    MissingToolAnswer,
    /// A `HumanGate` needs an outcome key and got none.
    #[error("this HumanGate needs `--outcome <key>`; run `surge resolve <run>` for options")]
    MissingOutcome,
    /// The outcome is not one the gate declares.
    #[error("outcome {outcome:?} is not valid for this gate; valid: {valid}")]
    InvalidOutcome {
        /// The rejected outcome key.
        outcome: String,
        /// The gate's declared keys, comma-separated.
        valid: String,
    },
    /// The daemon hosting the run declined or could not take the answer.
    #[error(
        "resolve failed: {cause}. The run must be active in a running daemon \
         (started with `surge engine run --daemon`)."
    )]
    DeliveryFailed {
        /// The daemon-side failure.
        cause: EngineError,
    },
    /// The OTLP trace could not be rendered as JSON.
    #[error("render trace as JSON")]
    RenderTrace(#[source] serde_json::Error),
}
