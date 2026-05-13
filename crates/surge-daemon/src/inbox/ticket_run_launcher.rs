//! Shared launcher for tracker-sourced runs.
//!
//! Centralizes the worktree-provision → graph-build → engine-start →
//! ticket-index-transition pipeline that turns an approved inbox card
//! into a running engine. This is the single spot that knows how to
//! seed `project_context` for tracker-sourced runs; the L0 / L1 / L2 /
//! L3 tiers landing in subsequent tasks all reuse it.
//!
//! ### Scope boundary vs. CLI bootstrap
//!
//! `surge bootstrap` (free-form CLI prompt) intentionally uses its own
//! driver (`run_bootstrap_in_worktree` + `start_followup_run`) because
//! it streams approvals from stdin and is not tracker-sourced. Both
//! paths apply `with_project_context_seed` at engine start, so the
//! semantic contract is consistent even though they don't share this
//! launcher.
//!
//! ### Concurrent-action handling
//!
//! [`TicketRunLauncher::launch`] returns [`LaunchOutcome::StateRejected`]
//! when the FSM transition is refused — for example, another receiver
//! resolved the card first. The caller is expected to treat this as a
//! benign no-op (log + advance).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use surge_core::SurgeConfig;
use surge_core::graph::Graph;
use surge_core::id::RunId;
use surge_intake::TaskSource;
use surge_intake::types::{TaskDetails, TaskId};
use surge_orchestrator::archetype_registry::ArchetypeRegistry;
use surge_orchestrator::bootstrap::{BootstrapGraphBuilder, BootstrapPrompt};
use surge_orchestrator::engine::config::EngineRunConfig;
use surge_orchestrator::engine::facade::EngineFacade;
use surge_orchestrator::engine::handle::RunHandle;
use surge_persistence::intake::{IntakeError, IntakeRepo, IntakeRow, TicketState};
use surge_persistence::runs::storage::Storage;
use tracing::{info, warn};

/// Bundle returned by [`TicketRunLauncher::fetch_ticket_for_start`] —
/// everything the launch step needs about the ticket.
#[derive(Clone)]
pub struct TicketStart {
    /// The current ticket-index row (state checked by the caller).
    pub ticket_row: IntakeRow,
    /// Source registered for `ticket_row.source_id`.
    pub source: Arc<dyn TaskSource>,
    /// Parsed task id.
    pub task_id: TaskId,
    /// Fully fetched task details (title / description / labels / url).
    pub details: TaskDetails,
}

/// Bundle returned on a successful launch — the caller spawns the
/// [`crate::inbox::state_sync::TicketStateSync`] follower with these.
pub struct LaunchedRun {
    /// Newly minted run id.
    pub run_id: RunId,
    /// Task id forwarded into the state-sync follower.
    pub task_id: TaskId,
    /// Source needed by the state-sync follower for tracker comments.
    pub source: Arc<dyn TaskSource>,
    /// Live run handle (events + completion).
    pub handle: RunHandle,
}

/// Outcome of a launch attempt.
pub enum LaunchOutcome {
    /// Run successfully started.
    Launched(LaunchedRun),
    /// State transition refused — concurrent action elsewhere. The
    /// caller logs and treats as benign.
    StateRejected {
        /// Task id whose transition was refused.
        task_id: String,
        /// Source state.
        from: TicketState,
        /// Target state that the launcher attempted.
        to: TicketState,
    },
}

/// Shared launcher state — handle to storage, engine, bootstrap builder,
/// archetype registry, and the daemon-level paths/config. Cloned cheaply
/// via `Arc` fields.
#[derive(Clone)]
pub struct TicketRunLauncher {
    storage: Arc<Storage>,
    engine: Arc<dyn EngineFacade>,
    bootstrap: Arc<dyn BootstrapGraphBuilder>,
    archetypes: Arc<ArchetypeRegistry>,
    worktrees_root: PathBuf,
    project_root: PathBuf,
    config: SurgeConfig,
}

impl TicketRunLauncher {
    /// Construct a launcher with the daemon-level dependencies.
    #[must_use]
    pub fn new(
        storage: Arc<Storage>,
        engine: Arc<dyn EngineFacade>,
        bootstrap: Arc<dyn BootstrapGraphBuilder>,
        archetypes: Arc<ArchetypeRegistry>,
        worktrees_root: PathBuf,
        project_root: PathBuf,
        config: SurgeConfig,
    ) -> Self {
        Self {
            storage,
            engine,
            bootstrap,
            archetypes,
            worktrees_root,
            project_root,
            config,
        }
    }

    /// Resolve a ticket row + its source + full task details from a
    /// callback token.
    ///
    /// Returns `Ok(None)` when the token is not found (stale tap on a
    /// resolved card). The caller is responsible for the state-eligibility
    /// check (`InboxNotified | Snoozed`) against the returned
    /// `ticket_row.state`.
    ///
    /// # Errors
    /// Returns the user-facing error string for `SQLite`, source, or
    /// `task_id` parse failures.
    pub async fn fetch_ticket_for_start(
        &self,
        sources: &HashMap<String, Arc<dyn TaskSource>>,
        callback_token: &str,
    ) -> Result<Option<TicketStart>, String> {
        let ticket_row = {
            let conn = self
                .storage
                .acquire_registry_conn()
                .map_err(|e| e.to_string())?;
            IntakeRepo::new(&conn)
                .fetch_by_callback_token(callback_token)
                .map_err(|e| e.to_string())?
        };
        let Some(ticket_row) = ticket_row else {
            return Ok(None);
        };

        let source = sources
            .get(&ticket_row.source_id)
            .cloned()
            .ok_or_else(|| format!("source {} not registered", ticket_row.source_id))?;
        let task_id =
            TaskId::try_new(ticket_row.task_id.clone()).map_err(|e| format!("task_id: {e}"))?;
        let details = source
            .fetch_task(&task_id)
            .await
            .map_err(|e| format!("fetch_task: {e}"))?;

        Ok(Some(TicketStart {
            ticket_row,
            source,
            task_id,
            details,
        }))
    }

    /// Provision a worktree, build the run graph (bootstrap or
    /// archetype-template depending on `policy_hint`), start the run,
    /// transition `ticket_index` to `RunStarted`, and post the tracker
    /// "started" comment.
    ///
    /// `decided_via` is the wire-level channel string from
    /// `InboxActionRow.decided_via` (`"telegram"` / `"desktop"` /
    /// `"auto"`); the human-readable label rendering happens inside.
    ///
    /// `policy_hint` carries the L2 template name from
    /// `InboxActionRow.policy_hint`. When `Some(name)` the launcher
    /// resolves the name against the `ArchetypeRegistry` and uses the
    /// template's graph as-is. When `None` (L1 / L3 path) it falls
    /// through to the configured `BootstrapGraphBuilder::build`.
    /// Unknown template names degrade to bootstrap with a WARN log.
    ///
    /// # Errors
    /// Returns a user-facing error string when worktree provisioning,
    /// graph build, engine start, or post-start persistence fails. State
    /// transition rejection is **not** an error — it surfaces as
    /// [`LaunchOutcome::StateRejected`].
    pub async fn launch(
        &self,
        start: TicketStart,
        decided_via: &str,
        policy_hint: Option<&str>,
    ) -> Result<LaunchOutcome, String> {
        let TicketStart {
            ticket_row,
            source,
            task_id,
            details,
        } = start;

        // Provision worktree.
        let run_id = RunId::new();
        let worktree = self.worktrees_root.join(run_id.to_string());
        std::fs::create_dir_all(&worktree).map_err(|e| format!("worktree mkdir: {e}"))?;

        // Build the run graph — L2 template path takes priority; everything
        // else falls back to the configured bootstrap builder.
        let graph = self
            .resolve_graph(&ticket_row, &task_id, &details, run_id, &worktree, policy_hint)
            .await?;

        // Start the run with project-context seed applied.
        let run_config = surge_orchestrator::project_context::with_project_context_seed(
            EngineRunConfig::default(),
            &self.project_root,
            &self.config,
        );
        let handle = self
            .engine
            .start_run(run_id, graph, worktree, run_config)
            .await
            .map_err(|e| format!("engine.start_run: {e}"))?;

        // Update ticket_index: set run_id, transition to RunStarted, clear callback_token.
        let transition_result = {
            let conn = self
                .storage
                .acquire_registry_conn()
                .map_err(|e| e.to_string())?;
            let repo = IntakeRepo::new(&conn);
            repo.set_run_id(&ticket_row.task_id, run_id.to_string())
                .map_err(|e| e.to_string())?;
            match repo.update_state_validated(&ticket_row.task_id, TicketState::RunStarted) {
                Ok(()) => {
                    repo.clear_callback_token(&ticket_row.task_id)
                        .map_err(|e| e.to_string())?;
                    Ok(())
                },
                Err(IntakeError::InvalidTransition { from, to }) => Err((from, to)),
                Err(e) => return Err(e.to_string()),
            }
        };
        if let Err((from, to)) = transition_result {
            return Ok(LaunchOutcome::StateRejected {
                task_id: ticket_row.task_id,
                from,
                to,
            });
        }

        // Post tracker comment.
        let via_label = render_via_label(decided_via);
        let comment = format!(
            "Surge run #{} started — see {} for progress.",
            run_id.short(),
            via_label,
        );
        if let Err(e) = source.post_comment(&task_id, &comment).await {
            warn!(error = %e, task_id = %task_id, "tracker comment on Start failed");
        }

        info!(
            target: "intake::launcher",
            task_id = %task_id,
            run_id = %run_id,
            via = decided_via,
            "ticket run launched"
        );

        Ok(LaunchOutcome::Launched(LaunchedRun {
            run_id,
            task_id,
            source,
            handle,
        }))
    }

    /// Resolve the run graph from `policy_hint` (L2 template) or fall
    /// back to the bootstrap builder. Unknown template names degrade to
    /// bootstrap with a WARN log so an operator misconfiguration does
    /// not block the run.
    async fn resolve_graph(
        &self,
        ticket_row: &IntakeRow,
        task_id: &TaskId,
        details: &TaskDetails,
        run_id: RunId,
        worktree: &std::path::Path,
        policy_hint: Option<&str>,
    ) -> Result<Graph, String> {
        if let Some(name) = policy_hint {
            match self.archetypes.resolve(name) {
                Ok(resolved) => {
                    info!(
                        target: "intake::launcher",
                        task_id = %task_id,
                        run_id = %run_id,
                        template = %name,
                        "L2: resolved template archetype"
                    );
                    return Ok(resolved.graph);
                },
                Err(e) => {
                    warn!(
                        target: "intake::launcher",
                        task_id = %task_id,
                        run_id = %run_id,
                        template = %name,
                        error = %e,
                        "L2: template not found; degrading to L1 bootstrap"
                    );
                },
            }
        }

        let prompt = BootstrapPrompt {
            title: details.title.clone(),
            description: details.description.clone(),
            tracker_url: Some(details.url.clone()),
            priority: ticket_row
                .priority
                .as_deref()
                .and_then(crate::inbox::consumer_helpers::parse_priority_str),
            labels: details.labels.clone(),
        };
        self.bootstrap
            .build(run_id, prompt, worktree.to_path_buf())
            .await
            .map_err(|e| format!("bootstrap.build: {e}"))
    }
}

/// Map the wire-level `decided_via` string to a human-readable label.
///
/// Exposed for tests; production callers pass through [`TicketRunLauncher::launch`].
#[must_use]
pub fn render_via_label(decided_via: &str) -> &str {
    match decided_via {
        "telegram" => "Telegram",
        "desktop" => "the desktop notification",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn via_label_known_channels() {
        assert_eq!(render_via_label("telegram"), "Telegram");
        assert_eq!(render_via_label("desktop"), "the desktop notification");
    }

    #[test]
    fn via_label_unknown_channel_passes_through() {
        assert_eq!(render_via_label("webhook"), "webhook");
        assert_eq!(render_via_label(""), "");
    }
}
