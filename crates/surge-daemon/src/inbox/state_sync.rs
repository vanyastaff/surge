//! `TicketStateSync` — drives `ticket_index` FSM from engine `RunHandle`
//! events and posts tracker comments.

use std::sync::Arc;
use surge_intake::TaskSource;
use surge_intake::types::TaskId;
use surge_orchestrator::engine::handle::{EngineRunEvent, RunHandle, RunOutcome};
use surge_persistence::intake::{IntakeRepo, TicketState};
use surge_persistence::runs::storage::Storage;
use tracing::{info, warn};

/// Per-run subscriber that mirrors engine state into `ticket_index` and
/// posts tracker comments on terminal events.
pub struct TicketStateSync {
    task_id: TaskId,
    storage: Arc<Storage>,
    source: Arc<dyn TaskSource>,
}

impl TicketStateSync {
    /// Construct with the originating ticket and the registered `TaskSource`.
    #[must_use]
    pub fn new(task_id: TaskId, storage: Arc<Storage>, source: Arc<dyn TaskSource>) -> Self {
        Self {
            task_id,
            storage,
            source,
        }
    }

    /// Drive the loop: consume events from `handle.events`, apply FSM
    /// transitions, and post tracker comments. Returns when the run
    /// reaches a terminal state or the broadcast sender is dropped.
    pub async fn run(self, mut handle: RunHandle) {
        info!(task_id = %self.task_id, run_id = %handle.run_id, "TicketStateSync started");
        let mut went_active = false;
        loop {
            match handle.events.recv().await {
                Ok(EngineRunEvent::Persisted { .. }) if !went_active => {
                    if let Err(e) = self.set_state(TicketState::Active).await {
                        warn!(error = %e, "transition to Active failed");
                    }
                    went_active = true;
                },
                Ok(EngineRunEvent::Terminal(outcome)) => {
                    self.on_terminal(&outcome).await;
                    return;
                },
                Ok(_) => {
                    // Unknown future variant — ignore and keep looping.
                },
                Err(_) => {
                    // Sender dropped — engine has exited. We exit silently
                    // (the engine will have written its own RunCompleted /
                    // RunFailed event already if it terminated normally).
                    return;
                },
            }
        }
    }

    #[allow(clippy::unused_async)]
    async fn set_state(&self, to: TicketState) -> Result<(), String> {
        let conn = self
            .storage
            .acquire_registry_conn()
            .map_err(|e| e.to_string())?;
        IntakeRepo::new(&conn)
            .update_state_validated(self.task_id.as_str(), to)
            .map_err(|e| e.to_string())
    }

    async fn on_terminal(&self, outcome: &RunOutcome) {
        let (state, comment) = match outcome {
            RunOutcome::Completed { .. } => {
                (TicketState::Completed, "✅ Surge run complete.".to_string())
            },
            RunOutcome::Failed { error } => {
                (TicketState::Failed, format!("❌ Surge run failed: {error}"))
            },
            RunOutcome::Aborted { reason } => {
                (TicketState::Aborted, format!("Surge run aborted: {reason}"))
            },
            _ => {
                warn!(task_id = %self.task_id, "on_terminal: unknown RunOutcome variant; defaulting to Failed");
                (
                    TicketState::Failed,
                    "Surge run ended with unknown outcome.".to_string(),
                )
            },
        };
        if let Err(e) = self.set_state(state).await {
            warn!(error = %e, ?state, "transition to terminal state failed");
        }
        if let Err(e) = self.source.post_comment(&self.task_id, &comment).await {
            warn!(error = %e, task_id = %self.task_id, "tracker comment on terminal failed");
        }
        info!(task_id = %self.task_id, ?state, "TicketStateSync done");
    }
}
