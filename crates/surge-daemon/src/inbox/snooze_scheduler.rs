//! `SnoozeScheduler` — periodically re-emits snoozed inbox cards once their
//! `snooze_until` has elapsed.

use crate::inbox::enqueue_inbox_card;
use chrono::Utc;
use std::sync::Arc;
use std::time::Duration;
use surge_intake::types::{Priority, TaskId};
use surge_persistence::intake::{IntakeRepo, TicketState};
use surge_persistence::runs::storage::Storage;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

/// Tokio loop polling `ticket_index` for due-snoozed rows and re-emitting them.
pub struct SnoozeScheduler {
    /// Storage handle.
    pub storage: Arc<Storage>,
    /// How often to poll. Spec default: 5 minutes.
    pub poll_interval: Duration,
}

impl SnoozeScheduler {
    /// Drive the loop until cancellation.
    pub async fn run(self, shutdown: CancellationToken) {
        let mut interval = tokio::time::interval(self.poll_interval);
        loop {
            tokio::select! {
                () = shutdown.cancelled() => return,
                _ = interval.tick() => {}
            }
            if let Err(e) = self.tick().await {
                warn!(error = %e, "snooze scheduler tick failed");
            }
        }
    }

    async fn tick(&self) -> Result<(), String> {
        let now = Utc::now();
        let due = {
            let conn = self
                .storage
                .acquire_registry_conn()
                .map_err(|e| e.to_string())?;
            IntakeRepo::new(&conn)
                .fetch_due_snoozed(now)
                .map_err(|e| e.to_string())?
        };
        for row in due {
            // Defensive state check (the DB query already filters, but
            // a concurrent action could have changed state in between).
            if row.state != TicketState::Snoozed {
                continue;
            }
            let new_token = ulid::Ulid::new().to_string();
            {
                let conn = self
                    .storage
                    .acquire_registry_conn()
                    .map_err(|e| e.to_string())?;
                let repo = IntakeRepo::new(&conn);
                if let Err(e) =
                    repo.update_state_validated(&row.task_id, TicketState::InboxNotified)
                {
                    warn!(error = %e, task_id = %row.task_id, "snooze re-emit transition failed");
                    continue;
                }
                if let Err(e) = repo.set_callback_token(&row.task_id, &new_token) {
                    warn!(error = %e, task_id = %row.task_id, "snooze re-emit set_callback_token failed");
                    continue;
                }
                if let Err(e) = repo.clear_snooze_until(&row.task_id) {
                    warn!(error = %e, task_id = %row.task_id, "snooze re-emit clear_snooze_until failed");
                    continue;
                }
            }

            // Build a fresh InboxCardPayload from the row data.
            let Ok(task_id) = TaskId::try_new(row.task_id.clone()) else {
                continue;
            };
            let priority = row
                .priority
                .as_deref()
                .and_then(crate::inbox::consumer_helpers::parse_priority_str)
                .unwrap_or(Priority::Medium);
            // The snooze re-emission deliberately reuses the stored title
            // surrogate `task_id` because we don't keep the original ticket
            // title on `ticket_index`. The user knows which ticket this is
            // by its task_id; the integration test in Phase 10 verifies the
            // payload reaches the queue.
            let payload = surge_notify::messages::InboxCardPayload {
                task_id,
                source_id: row.source_id.clone(),
                provider: row.provider.clone(),
                title: format!("(snoozed re-emission) {}", row.task_id),
                summary: String::new(),
                priority,
                task_url: String::new(),
                callback_token: new_token,
            };
            if let Err(e) = enqueue_inbox_card(&self.storage, &payload).await {
                warn!(error = %e, task_id = %row.task_id, "snooze re-emit enqueue failed");
            } else {
                info!(task_id = %row.task_id, "snoozed card re-emitted");
            }
        }
        Ok(())
    }
}
