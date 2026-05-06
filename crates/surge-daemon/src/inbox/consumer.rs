//! `InboxActionConsumer` — polls `inbox_action_queue` and dispatches
//! Start/Snooze/Skip handlers.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use surge_intake::TaskSource;
use surge_orchestrator::bootstrap::BootstrapGraphBuilder;
use surge_orchestrator::engine::facade::EngineFacade;
use surge_persistence::inbox_queue::{self, InboxActionKind, InboxActionRow};
use surge_persistence::runs::storage::Storage;
use tokio_util::sync::CancellationToken;
use tracing::warn;

/// Polls `inbox_action_queue` and dispatches handlers.
pub struct InboxActionConsumer {
    /// Storage handle for queue + ticket_index access.
    pub storage: Arc<Storage>,
    /// Trait object for converting prompts → graphs.
    pub bootstrap: Arc<dyn BootstrapGraphBuilder>,
    /// Engine facade used to actually start runs.
    pub engine: Arc<dyn EngineFacade>,
    /// Source registry for tracker comments / labels.
    pub sources: Arc<HashMap<String, Arc<dyn TaskSource>>>,
    /// Root directory under which per-run worktrees are created.
    pub worktrees_root: PathBuf,
    /// How often the queue is polled.
    pub poll_interval: Duration,
}

impl InboxActionConsumer {
    /// Drive the polling loop until cancellation.
    pub async fn run(self, shutdown: CancellationToken) {
        let mut interval = tokio::time::interval(self.poll_interval);
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => return,
                _ = interval.tick() => {}
            }
            if let Err(e) = self.tick().await {
                warn!(error = %e, "InboxActionConsumer tick failed");
            }
        }
    }

    async fn tick(&self) -> Result<(), String> {
        let pending = {
            let conn = self
                .storage
                .acquire_registry_conn()
                .map_err(|e| e.to_string())?;
            inbox_queue::list_pending_actions(&conn).map_err(|e| e.to_string())?
        };
        for row in pending {
            let result = match row.kind {
                InboxActionKind::Start => self.handle_start(&row).await,
                InboxActionKind::Snooze => self.handle_snooze(&row).await,
                InboxActionKind::Skip => self.handle_skip(&row).await,
            };
            if let Err(e) = result {
                warn!(
                    error = %e,
                    seq = row.seq,
                    kind = row.kind.as_str(),
                    task_id = %row.task_id,
                    "inbox action handler error"
                );
            }
            // Advance cursor regardless: failed actions are surfaced via logs;
            // cursor never blocks on transient errors.
            let conn = self
                .storage
                .acquire_registry_conn()
                .map_err(|e| e.to_string())?;
            inbox_queue::mark_action_processed(&conn, row.seq).map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    // Tasks 6.2-6.3 fill these in.
    async fn handle_start(&self, _row: &InboxActionRow) -> Result<(), String> {
        Err("not implemented (Task 6.2)".into())
    }
    async fn handle_snooze(&self, _row: &InboxActionRow) -> Result<(), String> {
        Err("not implemented (Task 6.3)".into())
    }
    async fn handle_skip(&self, _row: &InboxActionRow) -> Result<(), String> {
        Err("not implemented (Task 6.3)".into())
    }
}
