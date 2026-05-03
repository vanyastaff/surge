//! `RunHandle` returned by `Engine::start_run` / `resume_run`.

use crate::engine::error::EngineError;
use surge_core::id::RunId;
use surge_core::keys::NodeKey;
use surge_core::run_event::EventPayload;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;

#[derive(Debug, Clone, PartialEq)]
pub enum RunOutcome {
    Completed { terminal: NodeKey },
    Failed { error: String },
    Aborted { reason: String },
}

/// Engine-flavoured projection of what was just persisted.
/// Each variant corresponds 1:1 to an EventPayload that was successfully
/// written to the event log (and therefore is durable).
#[derive(Debug, Clone)]
pub enum EngineRunEvent {
    /// A new event was persisted. Carries the payload + assigned seq.
    Persisted { seq: u64, payload: EventPayload },
    /// The run reached a terminal state.
    Terminal(RunOutcome),
}

pub struct RunHandle {
    pub run_id: RunId,
    pub events: broadcast::Receiver<EngineRunEvent>,
    pub completion: JoinHandle<RunOutcome>,
}

impl RunHandle {
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
