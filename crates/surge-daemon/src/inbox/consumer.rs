//! `InboxActionConsumer` — polls `inbox_action_queue` and dispatches
//! Start/Snooze/Skip handlers.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use surge_core::SurgeConfig;
use surge_intake::TaskSource;
use surge_intake::types::TaskId;
use surge_orchestrator::archetype_registry::ArchetypeRegistry;
use surge_orchestrator::bootstrap::BootstrapGraphBuilder;
use surge_orchestrator::engine::facade::EngineFacade;
use surge_persistence::inbox_queue::{self, InboxActionKind, InboxActionRow};
use surge_persistence::intake::{IntakeError, IntakeRepo, TicketState};
use surge_persistence::runs::storage::Storage;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::inbox::ticket_run_launcher::{LaunchOutcome, TicketRunLauncher};

/// Polls `inbox_action_queue` and dispatches handlers.
pub struct InboxActionConsumer {
    /// Storage handle for queue + `ticket_index` access.
    pub storage: Arc<Storage>,
    /// Trait object for converting prompts → graphs.
    pub bootstrap: Arc<dyn BootstrapGraphBuilder>,
    /// Engine facade used to actually start runs.
    pub engine: Arc<dyn EngineFacade>,
    /// Archetype-template registry used by the L2 path
    /// (`surge:template/<name>`). Looked up from
    /// [`surge_persistence::inbox_queue::InboxActionRow::policy_hint`].
    pub archetypes: Arc<ArchetypeRegistry>,
    /// Source registry for tracker comments / labels.
    pub sources: Arc<HashMap<String, Arc<dyn TaskSource>>>,
    /// Root directory under which per-run worktrees are created.
    pub worktrees_root: PathBuf,
    /// Project root used for config and project-context seeding.
    pub project_root: PathBuf,
    /// Config captured from the daemon's project root.
    pub config: SurgeConfig,
    /// How often the queue is polled.
    pub poll_interval: Duration,
}

impl InboxActionConsumer {
    /// Drive the polling loop until cancellation.
    pub async fn run(self, shutdown: CancellationToken) {
        let mut interval = tokio::time::interval(self.poll_interval);
        loop {
            tokio::select! {
                () = shutdown.cancelled() => return,
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

    /// Build a [`TicketRunLauncher`] from this consumer's daemon-level state.
    ///
    /// Cheap: every field clone is either an `Arc` bump or a `PathBuf` /
    /// small-config clone. Done per `handle_start` invocation rather than
    /// stored to avoid touching upstream construction sites.
    fn launcher(&self) -> TicketRunLauncher {
        TicketRunLauncher::new(
            Arc::clone(&self.storage),
            Arc::clone(&self.engine),
            Arc::clone(&self.bootstrap),
            Arc::clone(&self.archetypes),
            self.worktrees_root.clone(),
            self.project_root.clone(),
            self.config.clone(),
        )
    }

    async fn handle_start(&self, row: &InboxActionRow) -> Result<(), String> {
        let launcher = self.launcher();
        let Some(start) = launcher
            .fetch_ticket_for_start(&self.sources, &row.callback_token)
            .await?
        else {
            info!(token = %row.callback_token, "Start: callback token not found; ignoring");
            return Ok(());
        };

        // Idempotency: state must still be awaiting decision.
        if !matches!(
            start.ticket_row.state,
            TicketState::InboxNotified | TicketState::Snoozed
        ) {
            info!(
                state = ?start.ticket_row.state,
                task_id = %start.ticket_row.task_id,
                "Start: ticket no longer awaiting decision; ignoring"
            );
            return Ok(());
        }

        match launcher
            .launch(start, &row.decided_via, row.policy_hint.as_deref())
            .await?
        {
            LaunchOutcome::Launched(run) => {
                let sync = crate::inbox::state_sync::TicketStateSync::new(
                    run.task_id.clone(),
                    Arc::clone(&self.storage),
                    run.source,
                );
                tokio::spawn(sync.run(run.handle));
                info!(
                    task_id = %run.task_id,
                    run_id = %run.run_id,
                    "inbox Start dispatched"
                );
            },
            LaunchOutcome::StateRejected { task_id, from, to } => {
                warn!(
                    ?from,
                    ?to,
                    task_id,
                    "Start: state transition rejected; assuming concurrent action"
                );
            },
        }

        Ok(())
    }

    #[allow(clippy::unused_async)]
    async fn handle_snooze(&self, row: &InboxActionRow) -> Result<(), String> {
        let ticket_row = {
            let conn = self
                .storage
                .acquire_registry_conn()
                .map_err(|e| e.to_string())?;
            IntakeRepo::new(&conn)
                .fetch_by_callback_token(&row.callback_token)
                .map_err(|e| e.to_string())?
        };
        let Some(ticket_row) = ticket_row else {
            return Ok(());
        };
        if !matches!(ticket_row.state, TicketState::InboxNotified) {
            return Ok(());
        }
        let Some(until) = row.snooze_until else {
            return Err("snooze action without snooze_until".into());
        };
        let conn = self
            .storage
            .acquire_registry_conn()
            .map_err(|e| e.to_string())?;
        let repo = IntakeRepo::new(&conn);
        match repo.update_state_validated(&ticket_row.task_id, TicketState::Snoozed) {
            Ok(()) => {},
            Err(IntakeError::InvalidTransition { .. }) => return Ok(()),
            Err(e) => return Err(e.to_string()),
        }
        repo.set_snooze_until(&ticket_row.task_id, until)
            .map_err(|e| e.to_string())?;
        info!(task_id = %ticket_row.task_id, ?until, "inbox Snooze applied");
        Ok(())
    }

    async fn handle_skip(&self, row: &InboxActionRow) -> Result<(), String> {
        let ticket_row = {
            let conn = self
                .storage
                .acquire_registry_conn()
                .map_err(|e| e.to_string())?;
            IntakeRepo::new(&conn)
                .fetch_by_callback_token(&row.callback_token)
                .map_err(|e| e.to_string())?
        };
        let Some(ticket_row) = ticket_row else {
            return Ok(());
        };
        if !matches!(
            ticket_row.state,
            TicketState::InboxNotified | TicketState::Snoozed
        ) {
            return Ok(());
        }
        let source = self
            .sources
            .get(&ticket_row.source_id)
            .ok_or_else(|| format!("source {} not registered", ticket_row.source_id))?;

        {
            let conn = self
                .storage
                .acquire_registry_conn()
                .map_err(|e| e.to_string())?;
            let repo = IntakeRepo::new(&conn);
            match repo.update_state_validated(&ticket_row.task_id, TicketState::Skipped) {
                Ok(()) => {},
                Err(IntakeError::InvalidTransition { .. }) => return Ok(()),
                Err(e) => return Err(e.to_string()),
            }
            // Clear the callback token so a stale tap can't resolve to this
            // row anymore. `tg_chat_id` and `tg_message_id` are intentionally
            // kept so a future `editMessageReplyMarkup` polish can strip the
            // keyboard from the original Telegram card.
            repo.clear_callback_token(&ticket_row.task_id)
                .map_err(|e| e.to_string())?;
        }

        let task_id =
            TaskId::try_new(ticket_row.task_id.clone()).map_err(|e| format!("task_id: {e}"))?;
        if let Err(e) = source.set_label(&task_id, "surge:skipped", true).await {
            warn!(error = %e, task_id = %task_id, "set_label surge:skipped failed");
        }
        if let Err(e) = source
            .post_comment(&task_id, "Surge: ticket skipped by user.")
            .await
        {
            warn!(error = %e, task_id = %task_id, "tracker comment on Skip failed");
        }
        info!(task_id = %task_id, "inbox Skip applied");
        Ok(())
    }
}
