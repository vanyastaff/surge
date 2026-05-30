//! Crash recovery — daemon-side orchestration that brings non-terminal
//! runs back to life after an unclean daemon exit.
//!
//! The engine already knows how to *resume* a single run
//! ([`surge_orchestrator::engine::Engine::resume_run`]): it replays the
//! persisted event log, reconstructs the cursor + memory, and continues
//! execution. What was missing — and what this module adds — is the
//! **startup scan** that decides, for every run the registry believes was
//! in flight, what should happen to it now.
//!
//! The decision logic ([`decide_action`]) is a pure function so the policy
//! is unit-testable in isolation. The planner ([`plan_recovery`]) gathers
//! the facts (registry status, worktree presence, event-log tail) and the
//! executor ([`execute_recovery`]) carries out the side effects (resume,
//! mark-failed, flag-stuck) — skipped entirely in dry-run mode.
//!
//! See the **Crash recovery** milestone in `.ai-factory/ROADMAP.md`.

use std::time::Duration;

use surge_core::RunStatus;

/// What recovery decided to do with a single run.
///
/// Returned by [`decide_action`]; carried in [`RecoveryDecision`] so the
/// dry-run inspector and the executor share one vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryAction {
    /// Run is non-terminal, its worktree is present, and it is not stuck:
    /// resume execution from the last persisted cursor.
    Resume,
    /// The event log already reached a terminal event (`RunCompleted` /
    /// `RunFailed`) but the registry never recorded it (the daemon died
    /// in the window between the terminal event and the status write).
    /// Reconcile the registry status to match the log.
    ReconcileTerminal {
        /// `true` if the terminal event was `RunFailed`, `false` for
        /// `RunCompleted`. Drives whether the registry row becomes
        /// [`RunStatus::Failed`] or [`RunStatus::Completed`].
        failed: bool,
    },
    /// The run's git worktree directory is gone, so resume is impossible.
    /// Mark the run [`RunStatus::Failed`] with a clear reason.
    MarkFailedWorktreeLost,
    /// The run has had no new events for longer than the stuck threshold.
    /// Do not auto-resume a possibly-wedged run; raise a human-attention
    /// signal instead.
    FlagStuck {
        /// Milliseconds since the run's most recent event.
        idle_ms: i64,
    },
    /// The registry already records a terminal status
    /// ([`RunStatus::Completed`] / [`RunStatus::Failed`] /
    /// [`RunStatus::Aborted`]). Nothing to do.
    SkipTerminal,
    /// The run is already active in this engine process — recovery is
    /// being re-run while the run is live. No-op (idempotency guard).
    SkipAlreadyActive,
}

/// Facts about a single run, gathered by the planner and fed to the
/// pure [`decide_action`] policy. This is a flat facts DTO; the several
/// independent boolean flags are the natural representation here.
#[derive(Debug, Clone)]
#[allow(clippy::struct_excessive_bools)]
pub struct RunRecoveryFacts {
    /// Coarse lifecycle status from the registry DB (after the
    /// list-time stale-pid detection in
    /// [`surge_persistence::runs::Storage::list_runs`] has run).
    pub registry_status: RunStatus,
    /// `true` if folding the event log shows a terminal event
    /// (`RunCompleted` / `RunFailed`).
    pub log_terminal: bool,
    /// `true` if the terminal event observed in the log was `RunFailed`.
    /// Meaningful only when `log_terminal` is `true`.
    pub log_failed: bool,
    /// `true` if the run's worktree directory exists on disk.
    pub worktree_exists: bool,
    /// Unix epoch ms of the most recent event in the log, or `None` if
    /// the log is empty.
    pub last_event_ms: Option<i64>,
    /// `true` if the run is already active in the engine process doing
    /// the recovery scan (only possible when recovery is re-invoked
    /// while runs are live).
    pub already_active: bool,
}

/// Decide what to do with a single run, given the gathered facts and the
/// stuck threshold. Pure — no I/O, no clock access — so the policy is
/// exhaustively unit-testable.
///
/// Decision order (first match wins):
/// 1. already active in-process → [`RecoveryAction::SkipAlreadyActive`]
/// 2. registry status truly terminal → [`RecoveryAction::SkipTerminal`]
/// 3. event log reached terminal → [`RecoveryAction::ReconcileTerminal`]
/// 4. worktree missing → [`RecoveryAction::MarkFailedWorktreeLost`]
/// 5. idle longer than threshold → [`RecoveryAction::FlagStuck`]
/// 6. otherwise → [`RecoveryAction::Resume`]
///
/// The log-terminal check (3) deliberately precedes the worktree check
/// (4): a run that genuinely completed has its worktree cleaned up, so an
/// absent worktree on a finished run is expected, not a failure.
#[must_use]
pub fn decide_action(
    facts: &RunRecoveryFacts,
    now_ms: i64,
    stuck_threshold: Duration,
) -> RecoveryAction {
    // 1. Idempotency: never touch a run that is already live in-process.
    if facts.already_active {
        return RecoveryAction::SkipAlreadyActive;
    }

    // 2. Registry already records a genuinely-terminal status. Note that
    //    `RunStatus::Crashed` is `is_terminal()` per the core enum but is
    //    NOT terminal for recovery — it is the prime candidate — so we
    //    match the three real terminal states explicitly rather than
    //    calling `is_terminal()`.
    if matches!(
        facts.registry_status,
        RunStatus::Completed | RunStatus::Failed | RunStatus::Aborted
    ) {
        return RecoveryAction::SkipTerminal;
    }

    // 3. The event log reached a terminal event but the registry never
    //    recorded it. Reconcile. Checked BEFORE the worktree probe: a
    //    finished run legitimately has its worktree cleaned up, so an
    //    absent worktree here is expected, not a failure.
    if facts.log_terminal {
        return RecoveryAction::ReconcileTerminal {
            failed: facts.log_failed,
        };
    }

    // 4. No worktree → cannot resume.
    if !facts.worktree_exists {
        return RecoveryAction::MarkFailedWorktreeLost;
    }

    // 5. Idle too long → do not blindly resume a possibly-wedged run.
    if let Some(last) = facts.last_event_ms {
        let idle_ms = now_ms.saturating_sub(last);
        let threshold_ms = i64::try_from(stuck_threshold.as_millis()).unwrap_or(i64::MAX);
        if idle_ms > threshold_ms {
            return RecoveryAction::FlagStuck { idle_ms };
        }
    }

    // 6. Healthy non-terminal run with a live worktree: resume.
    RecoveryAction::Resume
}

/// Knobs for a recovery scan. `now_ms` is injected (rather than read from
/// a clock) so [`plan_recovery`] is deterministic under test.
#[derive(Debug, Clone)]
pub struct RecoveryOptions {
    /// A run with no new events for longer than this is flagged stuck
    /// instead of auto-resumed.
    pub stuck_threshold: Duration,
    /// Root under which daemon-launched runs place their worktree at
    /// `<worktrees_root>/<run_id>` (see the inbox ticket-run launcher).
    pub worktrees_root: std::path::PathBuf,
    /// Wall-clock "now" in Unix epoch ms, used for stuck detection.
    pub now_ms: i64,
}

/// One run's recovery decision, suitable for the dry-run inspector and
/// the executor alike.
#[derive(Debug, Clone)]
pub struct RecoveryDecision {
    /// The run this decision concerns.
    pub run_id: surge_core::id::RunId,
    /// Registry status observed at scan time (post stale-pid detection).
    pub prior_status: RunStatus,
    /// Most-recent `StageEntered` node in the run's log, if any. Surfaced
    /// for per-stage recovery telemetry ("which stage were crashed runs
    /// at?").
    pub active_node: Option<String>,
    /// Worktree path the run would resume into.
    pub worktree_path: std::path::PathBuf,
    /// What recovery decided to do.
    pub action: RecoveryAction,
}

/// Result of a recovery scan: one [`RecoveryDecision`] per non-terminal
/// candidate run. Truly-terminal runs (`Completed` / `Failed` /
/// `Aborted`) are not represented — they need no recovery.
#[derive(Debug, Clone, Default)]
pub struct RecoveryReport {
    /// Per-run decisions, in registry-listing order.
    pub decisions: Vec<RecoveryDecision>,
}

impl RecoveryReport {
    /// Count decisions whose action matches `pred`.
    #[must_use]
    pub fn count<F: Fn(&RecoveryAction) -> bool>(&self, pred: F) -> usize {
        self.decisions.iter().filter(|d| pred(&d.action)).count()
    }

    /// Number of runs that will be resumed.
    #[must_use]
    pub fn resume_count(&self) -> usize {
        self.count(|a| matches!(a, RecoveryAction::Resume))
    }
}

/// Scan the registry for non-terminal runs and decide what to do with
/// each. Read-only: gathers facts (registry status, worktree presence,
/// event-log tail) and applies [`decide_action`]. No side effects — the
/// returned report drives [`execute_recovery`] (or the dry-run printer).
///
/// `active_run_ids` are runs already live in the engine process running
/// the scan; they are reported as [`RecoveryAction::SkipAlreadyActive`]
/// so re-invoking recovery while runs are in flight is a no-op.
///
/// # Errors
///
/// Returns [`surge_persistence::runs::StorageError`] if the registry
/// listing fails. Per-run event-log read failures are tolerated (treated
/// as an empty log) so one corrupt run cannot abort the whole scan.
#[allow(clippy::implicit_hasher)]
pub async fn plan_recovery(
    storage: &std::sync::Arc<surge_persistence::runs::Storage>,
    opts: &RecoveryOptions,
    active_run_ids: &std::collections::HashSet<surge_core::id::RunId>,
) -> Result<RecoveryReport, surge_persistence::runs::StorageError> {
    use surge_persistence::runs::{RunFilter, current_status};

    // `list_runs` runs stale-pid detection first: any `Running` /
    // `Bootstrapping` row whose recorded daemon pid is no longer alive is
    // flipped to `Crashed` in the registry. After a daemon crash that is
    // exactly the population we want to recover.
    let runs = storage.list_runs(RunFilter::default()).await?;

    let mut decisions = Vec::new();
    for summary in runs {
        // Candidates are everything NOT genuinely terminal. `Crashed` is a
        // candidate (the prime one), so we exclude only the three real
        // terminal states rather than using `RunStatus::is_terminal`.
        if matches!(
            summary.status,
            RunStatus::Completed | RunStatus::Failed | RunStatus::Aborted
        ) {
            continue;
        }

        let run_id = summary.id;
        let worktree_path = opts.worktrees_root.join(run_id.to_string());
        let worktree_exists = worktree_path.exists();

        // Read the event-log tail to learn whether the log already reached
        // a terminal event and when the last event landed. A missing or
        // unreadable per-run DB is tolerated — treat it as an empty log so
        // one corrupt run cannot abort the whole scan.
        let (log_terminal, log_failed, last_event_ms, active_node) =
            match storage.open_run_reader(run_id).await {
                Ok(reader) => match current_status(&reader, run_id).await {
                    Ok(snap) => (
                        snap.terminal,
                        snap.failed,
                        snap.last_event_at_ms,
                        snap.active_node,
                    ),
                    Err(e) => {
                        tracing::warn!(
                            target: "surge.recovery",
                            run_id = %run_id,
                            error = %e,
                            "current_status read failed; treating log as empty"
                        );
                        (false, false, None, None)
                    },
                },
                Err(e) => {
                    tracing::debug!(
                        target: "surge.recovery",
                        run_id = %run_id,
                        error = %e,
                        "open_run_reader failed; treating log as empty"
                    );
                    (false, false, None, None)
                },
            };

        let facts = RunRecoveryFacts {
            registry_status: summary.status,
            log_terminal,
            log_failed,
            worktree_exists,
            last_event_ms,
            already_active: active_run_ids.contains(&run_id),
        };
        let action = decide_action(&facts, opts.now_ms, opts.stuck_threshold);

        decisions.push(RecoveryDecision {
            run_id,
            prior_status: summary.status,
            active_node,
            worktree_path,
            action,
        });
    }

    Ok(RecoveryReport { decisions })
}

/// Side effects recovery performs. Abstracted behind a trait so the
/// executor's control flow ([`execute_recovery`]) is unit-testable with a
/// counting mock, while the daemon supplies a concrete implementation that
/// wires resume into admission + the broadcast registry and registry
/// updates into the persistence layer.
///
/// All methods return `Result<(), String>`: a per-run failure is recorded
/// and tallied, never fatal — one un-resumable run must not abort recovery
/// of the rest.
#[async_trait::async_trait]
pub trait RecoveryEffects: Send + Sync {
    /// Resume `run_id`, executing into `worktree_path`. The daemon impl
    /// admits the run, registers its broadcast channel, and spawns the
    /// forwarder so `RunFinished` is published globally (keeping tracker
    /// completion + L3 merge gates working for recovered runs).
    async fn resume(
        &self,
        run_id: &surge_core::id::RunId,
        worktree_path: &std::path::Path,
    ) -> Result<(), String>;

    /// Mark `run_id` failed in the registry (worktree lost).
    async fn mark_failed(&self, run_id: &surge_core::id::RunId, reason: &str)
    -> Result<(), String>;

    /// Reconcile a run whose log reached a terminal event to the matching
    /// registry status (`Completed` or `Failed`).
    async fn reconcile_terminal(
        &self,
        run_id: &surge_core::id::RunId,
        failed: bool,
    ) -> Result<(), String>;

    /// Raise a human-attention signal for a stuck run.
    async fn flag_stuck(&self, run_id: &surge_core::id::RunId, idle_ms: i64) -> Result<(), String>;
}

/// Tally of what [`execute_recovery`] did (or would do, in dry-run).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RecoveryOutcome {
    /// Runs resumed.
    pub resumed: usize,
    /// Runs whose registry status was reconciled to a terminal value.
    pub reconciled: usize,
    /// Runs marked failed because their worktree was gone.
    pub failed_worktree: usize,
    /// Runs flagged for human attention as stuck.
    pub flagged_stuck: usize,
    /// Decisions that required no action (already terminal / already active).
    pub skipped: usize,
    /// Per-run effect failures (logged; never fatal).
    pub errors: usize,
}

/// Carry out the actions in `report`. In `dry_run` mode the tallies are
/// computed but no effect is invoked. Per-run effect errors are counted in
/// [`RecoveryOutcome::errors`] and logged, never propagated, so one bad run
/// cannot abort recovery of the others.
///
/// Emits a `surge.recovery` tracing summary so operators see how many runs
/// were recovered, reconciled, failed, and flagged.
#[allow(clippy::too_many_lines)]
pub async fn execute_recovery(
    report: &RecoveryReport,
    effects: &dyn RecoveryEffects,
    dry_run: bool,
) -> RecoveryOutcome {
    let mut out = RecoveryOutcome::default();

    for d in &report.decisions {
        match d.action {
            RecoveryAction::Resume => {
                if dry_run {
                    out.resumed += 1;
                } else {
                    match effects.resume(&d.run_id, &d.worktree_path).await {
                        Ok(()) => {
                            out.resumed += 1;
                            tracing::info!(
                                target: "surge.recovery",
                                run_id = %d.run_id,
                                prior = %d.prior_status,
                                stage = d.active_node.as_deref().unwrap_or("?"),
                                "resumed crashed run"
                            );
                        },
                        Err(e) => {
                            out.errors += 1;
                            tracing::warn!(
                                target: "surge.recovery",
                                run_id = %d.run_id,
                                error = %e,
                                "resume failed"
                            );
                        },
                    }
                }
            },
            RecoveryAction::ReconcileTerminal { failed } => {
                if dry_run {
                    out.reconciled += 1;
                } else {
                    match effects.reconcile_terminal(&d.run_id, failed).await {
                        Ok(()) => {
                            out.reconciled += 1;
                            tracing::info!(
                                target: "surge.recovery",
                                run_id = %d.run_id,
                                failed,
                                "reconciled registry to terminal log state"
                            );
                        },
                        Err(e) => {
                            out.errors += 1;
                            tracing::warn!(
                                target: "surge.recovery",
                                run_id = %d.run_id,
                                error = %e,
                                "reconcile failed"
                            );
                        },
                    }
                }
            },
            RecoveryAction::MarkFailedWorktreeLost => {
                if dry_run {
                    out.failed_worktree += 1;
                } else {
                    match effects
                        .mark_failed(&d.run_id, "worktree lost; cannot resume")
                        .await
                    {
                        Ok(()) => {
                            out.failed_worktree += 1;
                            tracing::warn!(
                                target: "surge.recovery",
                                run_id = %d.run_id,
                                worktree = %d.worktree_path.display(),
                                "marked failed: worktree lost"
                            );
                        },
                        Err(e) => {
                            out.errors += 1;
                            tracing::warn!(
                                target: "surge.recovery",
                                run_id = %d.run_id,
                                error = %e,
                                "mark-failed failed"
                            );
                        },
                    }
                }
            },
            RecoveryAction::FlagStuck { idle_ms } => {
                if dry_run {
                    out.flagged_stuck += 1;
                } else {
                    match effects.flag_stuck(&d.run_id, idle_ms).await {
                        Ok(()) => {
                            out.flagged_stuck += 1;
                            tracing::warn!(
                                target: "surge.recovery",
                                run_id = %d.run_id,
                                idle_ms,
                                "flagged stuck run for human attention"
                            );
                        },
                        Err(e) => {
                            out.errors += 1;
                            tracing::warn!(
                                target: "surge.recovery",
                                run_id = %d.run_id,
                                error = %e,
                                "flag-stuck notify failed"
                            );
                        },
                    }
                }
            },
            RecoveryAction::SkipTerminal | RecoveryAction::SkipAlreadyActive => {
                out.skipped += 1;
            },
        }
    }

    tracing::info!(
        target: "surge.recovery",
        dry_run,
        scanned = report.decisions.len(),
        resumed = out.resumed,
        reconciled = out.reconciled,
        failed_worktree = out.failed_worktree,
        flagged_stuck = out.flagged_stuck,
        skipped = out.skipped,
        errors = out.errors,
        "recovery pass complete"
    );

    out
}

/// Concrete [`RecoveryEffects`] backed by the daemon's live machinery:
/// resume goes through admission + the broadcast registry (so recovered
/// runs publish `RunFinished` globally), registry mutations go through
/// [`surge_persistence::runs::Storage::set_run_status`], and stuck-run
/// signals go out as desktop notifications.
pub struct DaemonRecoveryEffects {
    /// Run registry + per-run event logs.
    pub storage: std::sync::Arc<surge_persistence::runs::Storage>,
    /// Engine facade used to resume runs.
    pub facade: std::sync::Arc<dyn surge_orchestrator::engine::facade::EngineFacade>,
    /// Shared admission controller (same instance the IPC server uses).
    pub admission: std::sync::Arc<crate::admission::AdmissionController>,
    /// Shared broadcast registry (same instance the IPC server uses).
    pub broadcast: std::sync::Arc<crate::broadcast::BroadcastRegistry>,
    /// Notifier for stuck-run human-attention cards.
    pub notifier: std::sync::Arc<dyn surge_notify::NotifyDeliverer>,
    /// Wall-clock "now" in ms, stamped as `ended_at` on terminal updates.
    pub now_ms: i64,
}

#[async_trait::async_trait]
impl RecoveryEffects for DaemonRecoveryEffects {
    async fn resume(
        &self,
        run_id: &surge_core::id::RunId,
        worktree_path: &std::path::Path,
    ) -> Result<(), String> {
        crate::server::resume_run_tracked(
            *run_id,
            worktree_path.to_path_buf(),
            self.facade.as_ref(),
            &self.admission,
            &self.broadcast,
        )
        .await
    }

    async fn mark_failed(
        &self,
        run_id: &surge_core::id::RunId,
        _reason: &str,
    ) -> Result<(), String> {
        self.storage
            .set_run_status(run_id, RunStatus::Failed, Some(self.now_ms))
            .await
            .map_err(|e| e.to_string())
    }

    async fn reconcile_terminal(
        &self,
        run_id: &surge_core::id::RunId,
        failed: bool,
    ) -> Result<(), String> {
        let status = if failed {
            RunStatus::Failed
        } else {
            RunStatus::Completed
        };
        self.storage
            .set_run_status(run_id, status, Some(self.now_ms))
            .await
            .map_err(|e| e.to_string())
    }

    async fn flag_stuck(
        &self,
        run_id: &surge_core::id::RunId,
        idle_ms: i64,
    ) -> Result<(), String> {
        let node = surge_core::keys::NodeKey::try_new("recovery")
            .map_err(|e| format!("bad recovery node key: {e}"))?;
        let idle_hours = idle_ms / 3_600_000;
        let rendered = surge_notify::RenderedNotification {
            severity: surge_core::notify_config::NotifySeverity::Warn,
            title: format!("Run {run_id} may be stuck"),
            body: format!(
                "No new events for ~{idle_hours}h after a daemon restart. \
                 Recovery left it untouched; inspect with `surge engine watch {run_id}` \
                 or abort with `surge run abort {run_id}`."
            ),
            artifact_paths: vec![],
        };
        let ctx = surge_notify::NotifyDeliveryContext {
            run_id: *run_id,
            node: &node,
        };
        match self
            .notifier
            .deliver(&ctx, &surge_core::notify_config::NotifyChannel::Desktop, &rendered)
            .await
        {
            // Success, or no desktop channel configured (the stuck run is
            // still logged at WARN by the executor) — both are tolerated.
            Ok(()) | Err(surge_notify::NotifyError::ChannelNotConfigured) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}

/// Default threshold after which a run with no new events is considered
/// stuck rather than resumable: 24 hours (per the roadmap's stuck-run
/// detection bullet).
pub const DEFAULT_STUCK_THRESHOLD: Duration = Duration::from_secs(24 * 3600);

/// Run a full crash-recovery pass at daemon startup: scan the registry,
/// decide per run, and carry out the actions against the live daemon
/// machinery. Nothing is active at startup, so `active_run_ids` is empty.
///
/// Errors from the registry scan are logged and swallowed (returning an
/// empty outcome) so a recovery hiccup never blocks the daemon from
/// coming up to serve the operator.
#[allow(clippy::too_many_arguments)]
pub async fn recover_on_startup(
    storage: &std::sync::Arc<surge_persistence::runs::Storage>,
    facade: &std::sync::Arc<dyn surge_orchestrator::engine::facade::EngineFacade>,
    admission: &std::sync::Arc<crate::admission::AdmissionController>,
    broadcast: &std::sync::Arc<crate::broadcast::BroadcastRegistry>,
    notifier: &std::sync::Arc<dyn surge_notify::NotifyDeliverer>,
    worktrees_root: std::path::PathBuf,
    now_ms: i64,
) -> RecoveryOutcome {
    let opts = RecoveryOptions {
        stuck_threshold: DEFAULT_STUCK_THRESHOLD,
        worktrees_root,
        now_ms,
    };
    let active = std::collections::HashSet::new();

    let report = match plan_recovery(storage, &opts, &active).await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(
                target: "surge.recovery",
                error = %e,
                "recovery scan failed; daemon starting without recovery"
            );
            return RecoveryOutcome::default();
        },
    };

    if report.decisions.is_empty() {
        tracing::info!(target: "surge.recovery", "no non-terminal runs to recover");
        return RecoveryOutcome::default();
    }

    let effects = DaemonRecoveryEffects {
        storage: storage.clone(),
        facade: facade.clone(),
        admission: admission.clone(),
        broadcast: broadcast.clone(),
        notifier: notifier.clone(),
        now_ms,
    };
    execute_recovery(&report, &effects, false).await
}

#[cfg(test)]
mod execute_recovery_tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use surge_core::id::RunId;

    /// Counting mock. `resume_should_fail` lets a test simulate an
    /// un-resumable run.
    #[derive(Default)]
    struct MockEffects {
        resume_calls: AtomicUsize,
        mark_failed_calls: AtomicUsize,
        reconcile_calls: AtomicUsize,
        flag_stuck_calls: AtomicUsize,
        resume_should_fail: bool,
    }

    #[async_trait::async_trait]
    impl RecoveryEffects for MockEffects {
        async fn resume(&self, _r: &RunId, _w: &Path) -> Result<(), String> {
            self.resume_calls.fetch_add(1, Ordering::SeqCst);
            if self.resume_should_fail {
                Err("simulated resume failure".into())
            } else {
                Ok(())
            }
        }
        async fn mark_failed(&self, _r: &RunId, _reason: &str) -> Result<(), String> {
            self.mark_failed_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        async fn reconcile_terminal(&self, _r: &RunId, _failed: bool) -> Result<(), String> {
            self.reconcile_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        async fn flag_stuck(&self, _r: &RunId, _idle_ms: i64) -> Result<(), String> {
            self.flag_stuck_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    fn decision(action: RecoveryAction) -> RecoveryDecision {
        RecoveryDecision {
            run_id: RunId::new(),
            prior_status: RunStatus::Crashed,
            active_node: Some("implement".into()),
            worktree_path: PathBuf::from("/wt"),
            action,
        }
    }

    fn full_report() -> RecoveryReport {
        RecoveryReport {
            decisions: vec![
                decision(RecoveryAction::Resume),
                decision(RecoveryAction::ReconcileTerminal { failed: true }),
                decision(RecoveryAction::MarkFailedWorktreeLost),
                decision(RecoveryAction::FlagStuck { idle_ms: 99 }),
                decision(RecoveryAction::SkipTerminal),
                decision(RecoveryAction::SkipAlreadyActive),
            ],
        }
    }

    #[tokio::test]
    async fn each_action_invokes_its_effect_and_is_tallied() {
        let effects = MockEffects::default();
        let outcome = execute_recovery(&full_report(), &effects, false).await;

        assert_eq!(effects.resume_calls.load(Ordering::SeqCst), 1);
        assert_eq!(effects.reconcile_calls.load(Ordering::SeqCst), 1);
        assert_eq!(effects.mark_failed_calls.load(Ordering::SeqCst), 1);
        assert_eq!(effects.flag_stuck_calls.load(Ordering::SeqCst), 1);

        assert_eq!(
            outcome,
            RecoveryOutcome {
                resumed: 1,
                reconciled: 1,
                failed_worktree: 1,
                flagged_stuck: 1,
                skipped: 2,
                errors: 0,
            }
        );
    }

    #[tokio::test]
    async fn dry_run_performs_no_effects_but_still_tallies() {
        let effects = MockEffects::default();
        let outcome = execute_recovery(&full_report(), &effects, true).await;

        assert_eq!(effects.resume_calls.load(Ordering::SeqCst), 0);
        assert_eq!(effects.mark_failed_calls.load(Ordering::SeqCst), 0);
        assert_eq!(effects.reconcile_calls.load(Ordering::SeqCst), 0);
        assert_eq!(effects.flag_stuck_calls.load(Ordering::SeqCst), 0);

        // Tallies still reflect intended actions so the dry-run inspector
        // can report what would happen.
        assert_eq!(outcome.resumed, 1);
        assert_eq!(outcome.reconciled, 1);
        assert_eq!(outcome.failed_worktree, 1);
        assert_eq!(outcome.flagged_stuck, 1);
        assert_eq!(outcome.skipped, 2);
        assert_eq!(outcome.errors, 0);
    }

    #[tokio::test]
    async fn resume_failure_is_counted_not_fatal() {
        let effects = MockEffects {
            resume_should_fail: true,
            ..Default::default()
        };
        let report = RecoveryReport {
            decisions: vec![
                decision(RecoveryAction::Resume),
                decision(RecoveryAction::MarkFailedWorktreeLost),
            ],
        };
        let outcome = execute_recovery(&report, &effects, false).await;

        // The mark-failed action after the failing resume still ran.
        assert_eq!(effects.mark_failed_calls.load(Ordering::SeqCst), 1);
        assert_eq!(outcome.errors, 1);
        assert_eq!(outcome.resumed, 0);
        assert_eq!(outcome.failed_worktree, 1);
    }
}

#[cfg(test)]
mod plan_recovery_tests {
    use super::*;
    use rusqlite::params;
    use std::collections::HashSet;
    use surge_core::id::RunId;
    use surge_persistence::runs::Storage;
    use tempfile::tempdir;

    const NOW: i64 = 1_700_000_000_000;

    fn opts(worktrees_root: std::path::PathBuf) -> RecoveryOptions {
        RecoveryOptions {
            stuck_threshold: Duration::from_secs(24 * 3600),
            worktrees_root,
            now_ms: NOW,
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn plans_resume_failed_worktree_and_skips_terminal() {
        let tmp = tempdir().unwrap();
        let storage = Storage::open(tmp.path()).await.unwrap();
        let wt_root = tmp.path().join("worktrees");

        // Run A — candidate with a present worktree → Resume.
        let run_a = RunId::new();
        let _wa = storage.create_run(run_a.clone(), "/proj", None).await.unwrap();
        std::fs::create_dir_all(wt_root.join(run_a.to_string())).unwrap();

        // Run B — candidate with an absent worktree → MarkFailedWorktreeLost.
        let run_b = RunId::new();
        let _wb = storage.create_run(run_b.clone(), "/proj", None).await.unwrap();

        // Run C — terminal (Completed) → not a candidate, no decision.
        let run_c = RunId::new();
        let _wc = storage.create_run(run_c.clone(), "/proj", None).await.unwrap();
        {
            let conn = storage.acquire_registry_conn().unwrap();
            conn.execute(
                "UPDATE runs SET status = 'completed', ended_at = ?1 WHERE id = ?2",
                params![NOW, run_c.to_string()],
            )
            .unwrap();
        }

        let report = plan_recovery(&storage, &opts(wt_root), &HashSet::new())
            .await
            .unwrap();

        assert_eq!(report.decisions.len(), 2, "only A and B are candidates");
        let a = report
            .decisions
            .iter()
            .find(|d| d.run_id == run_a)
            .expect("A present");
        assert_eq!(a.action, RecoveryAction::Resume);
        let b = report
            .decisions
            .iter()
            .find(|d| d.run_id == run_b)
            .expect("B present");
        assert_eq!(b.action, RecoveryAction::MarkFailedWorktreeLost);
        assert!(
            report.decisions.iter().all(|d| d.run_id != run_c),
            "terminal run C must not appear"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn active_run_is_reported_as_skip_already_active() {
        let tmp = tempdir().unwrap();
        let storage = Storage::open(tmp.path()).await.unwrap();
        let wt_root = tmp.path().join("worktrees");

        let run = RunId::new();
        let _w = storage.create_run(run.clone(), "/proj", None).await.unwrap();
        std::fs::create_dir_all(wt_root.join(run.to_string())).unwrap();

        let mut active = HashSet::new();
        active.insert(run.clone());

        let report = plan_recovery(&storage, &opts(wt_root), &active)
            .await
            .unwrap();
        assert_eq!(report.decisions.len(), 1);
        assert_eq!(report.decisions[0].action, RecoveryAction::SkipAlreadyActive);
    }
}

#[cfg(test)]
mod decide_action_tests {
    use super::*;

    const HOUR_MS: i64 = 3_600_000;
    const NOW: i64 = 1_700_000_000_000;
    const STUCK: Duration = Duration::from_secs(24 * 3600);

    /// Baseline: a Crashed run, worktree present, fresh event, not active.
    fn healthy_crashed() -> RunRecoveryFacts {
        RunRecoveryFacts {
            registry_status: RunStatus::Crashed,
            log_terminal: false,
            log_failed: false,
            worktree_exists: true,
            last_event_ms: Some(NOW - HOUR_MS),
            already_active: false,
        }
    }

    #[test]
    fn already_active_run_is_skipped() {
        let facts = RunRecoveryFacts {
            already_active: true,
            ..healthy_crashed()
        };
        assert_eq!(
            decide_action(&facts, NOW, STUCK),
            RecoveryAction::SkipAlreadyActive
        );
    }

    #[test]
    fn registry_terminal_run_is_skipped() {
        for status in [RunStatus::Completed, RunStatus::Failed, RunStatus::Aborted] {
            let facts = RunRecoveryFacts {
                registry_status: status,
                ..healthy_crashed()
            };
            assert_eq!(
                decide_action(&facts, NOW, STUCK),
                RecoveryAction::SkipTerminal,
                "status {status:?} should skip"
            );
        }
    }

    #[test]
    fn log_terminal_completed_reconciles_without_failure() {
        let facts = RunRecoveryFacts {
            log_terminal: true,
            log_failed: false,
            // Worktree already cleaned up after completion — must NOT be
            // misread as worktree-lost.
            worktree_exists: false,
            ..healthy_crashed()
        };
        assert_eq!(
            decide_action(&facts, NOW, STUCK),
            RecoveryAction::ReconcileTerminal { failed: false }
        );
    }

    #[test]
    fn log_terminal_failed_reconciles_with_failure() {
        let facts = RunRecoveryFacts {
            log_terminal: true,
            log_failed: true,
            ..healthy_crashed()
        };
        assert_eq!(
            decide_action(&facts, NOW, STUCK),
            RecoveryAction::ReconcileTerminal { failed: true }
        );
    }

    #[test]
    fn missing_worktree_marks_failed() {
        let facts = RunRecoveryFacts {
            worktree_exists: false,
            ..healthy_crashed()
        };
        assert_eq!(
            decide_action(&facts, NOW, STUCK),
            RecoveryAction::MarkFailedWorktreeLost
        );
    }

    #[test]
    fn idle_beyond_threshold_flags_stuck() {
        let facts = RunRecoveryFacts {
            last_event_ms: Some(NOW - 25 * HOUR_MS),
            ..healthy_crashed()
        };
        assert_eq!(
            decide_action(&facts, NOW, STUCK),
            RecoveryAction::FlagStuck {
                idle_ms: 25 * HOUR_MS
            }
        );
    }

    #[test]
    fn healthy_non_terminal_run_resumes() {
        assert_eq!(
            decide_action(&healthy_crashed(), NOW, STUCK),
            RecoveryAction::Resume
        );
    }

    #[test]
    fn running_and_bootstrapping_are_recovery_candidates() {
        for status in [RunStatus::Running, RunStatus::Bootstrapping] {
            let facts = RunRecoveryFacts {
                registry_status: status,
                ..healthy_crashed()
            };
            assert_eq!(
                decide_action(&facts, NOW, STUCK),
                RecoveryAction::Resume,
                "status {status:?} should resume"
            );
        }
    }

    #[test]
    fn empty_log_with_present_worktree_resumes() {
        // No events yet (last_event_ms None) is not "stuck" — it just
        // never produced an event before the crash. Resume.
        let facts = RunRecoveryFacts {
            last_event_ms: None,
            ..healthy_crashed()
        };
        assert_eq!(
            decide_action(&facts, NOW, STUCK),
            RecoveryAction::Resume
        );
    }
}
