//! `RunHandle` returned by `Engine::start_run` / `resume_run`.

use crate::engine::error::EngineError;
use surge_core::id::RunId;
use surge_core::keys::NodeKey;
use surge_core::run_event::EventPayload;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;

/// Terminal outcome of a run's execution.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
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
}

/// Engine-flavoured projection of what was just persisted.
/// Each variant corresponds 1:1 to an [`EventPayload`] that was successfully
/// written to the event log (and therefore is durable).
#[non_exhaustive]
#[derive(Debug, Clone)]
pub enum EngineRunEvent {
    /// A new event was persisted. Carries the payload + assigned seq.
    Persisted {
        /// Monotonically-increasing sequence number assigned to this event.
        seq: u64,
        /// The persisted event payload.
        payload: EventPayload,
    },
    /// The run reached a terminal state.
    Terminal(RunOutcome),
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
