//! `RunHandle` returned by `Engine::start_run` / `resume_run`.

use chrono::{DateTime, Utc};

use crate::engine::error::EngineError;
use surge_core::id::RunId;
use surge_core::keys::NodeKey;
use surge_core::run_event::EventPayload;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;

/// Terminal outcome of a run's execution.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RunOutcome {
    /// The run reached a `TerminalKind::Success` node.
    Completed {
        /// `NodeKey` of the terminal node that ended the run.
        terminal: NodeKey,
    },
    /// The run reached a `TerminalKind::Failure` node or encountered an
    /// unrecoverable error.
    Failed {
        /// Human-readable description of the failure.
        error: String,
    },
    /// The run was cancelled via `Engine::stop_run` or the cancellation token.
    Aborted {
        /// Reason string supplied by the caller of `stop_run`.
        reason: String,
    },
    /// The run task exited cleanly to park — a provider rate-limit window
    /// is exhausted (`CapacityPolicy::decide` returned `Decision::Park`,
    /// Task 12 R37/R37.1). Not a final outcome the way the other three
    /// are: the registry transitions to `RunStatus::Parked` (not a
    /// terminal status — see that type's doc) with `wake_at` recorded, the
    /// worktree is left in place, and the run resumes on its own once
    /// `wake_at` passes (`Engine::resume_run`, Task 12 M4's wake
    /// scheduler). Consumers that only handle genuinely-final outcomes
    /// (`#[non_exhaustive]` already forces a wildcard arm on this enum)
    /// should treat an unhandled `Parked` as "not done yet", not "failed".
    Parked {
        /// When the run is expected to resume on its own.
        wake_at: DateTime<Utc>,
    },
}

/// Engine-flavoured projection of what was just persisted.
/// Each variant corresponds 1:1 to an [`EventPayload`] that was successfully
/// written to the event log (and therefore is durable).
#[non_exhaustive]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EngineRunEvent {
    /// A new event was persisted. Carries the payload + assigned seq.
    Persisted {
        /// Monotonically-increasing sequence number assigned to this event.
        seq: u64,
        /// The persisted event payload.
        payload: Box<EventPayload>,
    },
    /// The run reached a terminal state.
    ///
    /// Wrapped in a struct variant rather than `Terminal(RunOutcome)`
    /// so the inner [`RunOutcome`]'s `#[serde(tag = "kind")]` does
    /// not collide with this enum's own `#[serde(tag = "kind")]`
    /// when serialised over the daemon IPC wire — internally-tagged
    /// tuple variants flatten the inner object's fields into the
    /// outer enum, producing two `kind` fields and tripping
    /// `serde_json` with "duplicate field 'kind'" on read.
    Terminal {
        /// Terminal outcome of the run.
        outcome: RunOutcome,
    },
}

/// Handle to an in-flight run. Created by `Engine::start_run` / `resume_run`.
pub struct RunHandle {
    /// Identifier of the run this handle tracks.
    pub run_id: RunId,
    /// Broadcast receiver for durable engine events emitted by this run.
    pub events: broadcast::Receiver<EngineRunEvent>,
    /// Join handle for the spawned run task; resolves to the terminal outcome.
    pub completion: JoinHandle<RunOutcome>,
}

impl RunHandle {
    /// Returns the `RunId` of the run this handle tracks.
    #[must_use]
    pub fn run_id(&self) -> RunId {
        self.run_id
    }

    /// Wait for the run to finish. Consumes the handle.
    pub async fn await_completion(self) -> Result<RunOutcome, EngineError> {
        self.completion
            .await
            .map_err(|e| EngineError::Internal(format!("run task join failed: {e}")))
    }
}

/// Lightweight projection of a run's state, used by
/// `EngineFacade::list_runs` and the daemon's `ListRuns` IPC reply.
#[non_exhaustive]
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct RunSummary {
    /// Identifier of the run.
    pub run_id: RunId,
    /// Current high-level status.
    pub status: RunStatus,
    /// Wall-clock time the run was registered with the engine.
    ///
    /// In M7's `LocalEngineFacade::list_runs`, this is a placeholder
    /// set to "now" at the time `list_runs` is called — the engine
    /// does not yet track per-run start time. The daemon facade
    /// (Phase 5) returns the real registration time. For queued
    /// runs synthesised by the daemon's `ListRuns` dispatch, this
    /// holds the time the run was added to the admission queue.
    /// M8+ may unify by adding a real `started_at: DateTime<Utc>`
    /// field to `Engine::ActiveRun`.
    pub started_at: chrono::DateTime<chrono::Utc>,
    /// Highest seq the engine has persisted for this run, if any.
    pub last_event_seq: Option<u64>,
}

impl RunSummary {
    /// Construct a `RunSummary` for a run that is queued (awaiting
    /// admission) inside the daemon. `last_event_seq` is always
    /// `None` because no events have been persisted yet.
    ///
    /// Exposed because `RunSummary` is `#[non_exhaustive]`, which
    /// blocks struct-literal construction outside this crate. The
    /// daemon (a downstream crate) needs to synthesise these
    /// summaries from its `pending_starts` map.
    #[must_use]
    pub fn queued(run_id: RunId, queued_at: chrono::DateTime<chrono::Utc>) -> Self {
        Self {
            run_id,
            status: RunStatus::Awaiting,
            started_at: queued_at,
            last_event_seq: None,
        }
    }
}

/// High-level run status as observed from outside (e.g., by `surge
/// engine ls --daemon`). Distinct from the engine's internal state.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    /// Run is currently executing inside the engine.
    Active,
    /// Run is queued by the daemon's `AdmissionController`, not yet started.
    Awaiting,
    /// Run reached a successful terminal node.
    Completed,
    /// Run reached a failure terminal node or an unrecoverable error.
    Failed,
    /// Run was cancelled via `stop_run`.
    Aborted,
}
