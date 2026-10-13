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
use surge_core::run_verdict::{TerminalKind, TerminalVerdict};

/// What recovery decided to do with a single run.
///
/// Returned by [`decide_action`]; carried in [`RecoveryDecision`] so the
/// dry-run inspector and the executor share one vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryAction {
    /// Run is non-terminal, its worktree is present, and it is not stuck:
    /// resume execution from the last persisted cursor.
    Resume,
    /// The event log's verdict is a single terminal event but the registry
    /// never recorded it (the daemon died between the terminal event and
    /// the status write). Reconcile the registry to exactly that kind.
    ReconcileTerminal {
        /// The log's terminal kind; see [`reconciled_status`].
        kind: TerminalKind,
    },
    /// The event log holds conflicting terminal claims (#95). Unconfirmed:
    /// never resumed, never reconciled, nothing appended; an operator is
    /// flagged. Carries the first two conflicting sequence numbers.
    FlagTerminalConflict {
        /// Sequence of the first terminal claim.
        first_seq: u64,
        /// Sequence of the second terminal claim.
        second_seq: u64,
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
    /// The run is [`RunStatus::Parked`] (Task 12, R37/R37.1) and its
    /// `wake_at` has not passed yet. Leave it parked — resuming it now
    /// would wake it before the provider's rate-limit window (or the
    /// configured blind backoff) has actually elapsed. Deliberately not
    /// subject to the idle-stuck check (5): a parked run's silence is
    /// intentional, not a sign of a wedged run.
    SkipParked {
        /// When the run is expected to wake on its own, if recorded.
        /// `None` only if the registry row is `Parked` with no `wake_at`
        /// at all — should not happen (`set_run_parked` always writes both
        /// together), but a fact worth surfacing rather than a value worth
        /// panicking over.
        wake_at_ms: Option<i64>,
    },
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
    /// The event log's terminal verdict (#95).
    pub log_verdict: TerminalVerdict,
    /// `true` if the run's worktree directory exists on disk.
    pub worktree_exists: bool,
    /// Unix epoch ms of the most recent event in the log, or `None` if
    /// the log is empty.
    pub last_event_ms: Option<i64>,
    /// `true` if the run is already active in the engine process doing
    /// the recovery scan (only possible when recovery is re-invoked
    /// while runs are live).
    pub already_active: bool,
    /// `true` when this run appears in [`registry::due_parked`]'s result at
    /// scan time (Task 12 M3 review, BLOCKING #2). Meaningful only when
    /// `registry_status` is [`RunStatus::Parked`]; always `false`
    /// otherwise. Computed by the planner from that query's own output —
    /// not from an inline `wake_at <= now` comparison here — so
    /// `due_parked` is the single place "is a parked run due" is decided,
    /// with a real caller instead of the zero it had before this fix.
    pub parked_is_due: bool,
    /// The parked run's recorded wake time, when `registry_status` is
    /// [`RunStatus::Parked`]. Carried for [`RecoveryAction::SkipParked`]'s
    /// display value; `None` only if the row is `Parked` with no
    /// `wake_at` at all (should not happen).
    pub parked_wake_at_ms: Option<i64>,
}

/// The registry status a reconciled terminal kind projects to: the one
/// mapping recovery and `surge daemon recover` share, so an aborted log
/// never becomes `Completed` (#95).
#[must_use]
pub fn reconciled_status(kind: TerminalKind) -> RunStatus {
    match kind {
        TerminalKind::Completed => RunStatus::Completed,
        TerminalKind::Failed => RunStatus::Failed,
        TerminalKind::Aborted => RunStatus::Aborted,
    }
}

/// Decide what to do with a single run, given the gathered facts and the
/// stuck threshold. Pure — no I/O, no clock access — so the policy is
/// exhaustively unit-testable.
///
/// Decision order (first match wins):
/// 1. already active in-process → [`RecoveryAction::SkipAlreadyActive`]
/// 2. registry status truly terminal → [`RecoveryAction::SkipTerminal`]
/// 3. event log verdict is a conflict → [`RecoveryAction::FlagTerminalConflict`];
///    a single terminal event → [`RecoveryAction::ReconcileTerminal`] to its
///    exact kind (Aborted stays Aborted)
/// 4. worktree missing → [`RecoveryAction::MarkFailedWorktreeLost`]
/// 5. registry status `Parked` and not yet due → [`RecoveryAction::SkipParked`]
///    (Task 12 M3 review, BLOCKING #2); due → falls through to 7 like any
///    other healthy run
/// 6. idle longer than threshold → [`RecoveryAction::FlagStuck`]
/// 7. otherwise → [`RecoveryAction::Resume`]
///
/// The log-terminal check (3) deliberately precedes the worktree check
/// (4): a run that genuinely completed has its worktree cleaned up, so an
/// absent worktree on a finished run is expected, not a failure.
///
/// The parked check (5) deliberately precedes the idle-stuck check (6): a
/// parked run's silence for however long the provider window (or the
/// configured blind backoff) takes is *intentional*, not evidence of a
/// wedged run — without this ordering, a run parked longer than
/// `stuck_threshold` would be flagged stuck instead of quietly waiting out
/// its own park (Task 12 M3 review, BLOCKING #2: "a run stuck longer than
/// `stuck_threshold` would otherwise land in `FlagStuck`"). It also
/// precedes the ordinary resume (7): a **due** parked run still resumes
/// through the normal path (`Engine::resume_run` itself clears
/// `Parked`/`wake_at` and arms the one-shot capacity-precheck bypass — see
/// that function's own doc), so falling through here rather than
/// special-casing "due" is deliberate, not an oversight.
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
    match &facts.log_verdict {
        TerminalVerdict::Conflict { claims } => {
            return RecoveryAction::FlagTerminalConflict {
                first_seq: claims.first().map_or(0, |claim| claim.seq),
                second_seq: claims.get(1).map_or(0, |claim| claim.seq),
            };
        },
        TerminalVerdict::Terminal(claim) => {
            return RecoveryAction::ReconcileTerminal {
                kind: claim.outcome.kind(),
            };
        },
        TerminalVerdict::Open => {},
    }

    // 4. No worktree → cannot resume.
    if !facts.worktree_exists {
        return RecoveryAction::MarkFailedWorktreeLost;
    }

    // 5. Parked and not yet due: leave it parked. A due parked run falls
    //    through (no special "due" action here) to the ordinary checks
    //    below, then to Resume — see this function's own doc for why.
    if facts.registry_status == RunStatus::Parked && !facts.parked_is_due {
        return RecoveryAction::SkipParked {
            wake_at_ms: facts.parked_wake_at_ms,
        };
    }

    // 6. Idle too long → do not blindly resume a possibly-wedged run.
    //    Never reached for a still-parked (not-yet-due) run — rule 5
    //    already returned above — so a long, intentional park never
    //    trips this.
    if let Some(last) = facts.last_event_ms {
        let idle_ms = now_ms.saturating_sub(last);
        let threshold_ms = i64::try_from(stuck_threshold.as_millis()).unwrap_or(i64::MAX);
        if idle_ms > threshold_ms {
            return RecoveryAction::FlagStuck { idle_ms };
        }
    }

    // 7. Healthy non-terminal run with a live worktree (or a parked run
    //    whose wake_at has already passed): resume.
    RecoveryAction::Resume
}

/// Knobs for a recovery scan. `now_ms` is injected (rather than read from
/// a clock) so [`plan_recovery`] is deterministic under test.
#[derive(Debug, Clone)]
pub struct RecoveryOptions {
    /// A run with no new events for longer than this is flagged stuck
    /// instead of auto-resumed.
    pub stuck_threshold: Duration,
    /// Legacy placement hint retained for caller compatibility. Recovery uses
    /// each run's persisted execution path; this root must not override it.
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
    let owned = storage.bootstrap_operation_store().reserved_run_ids()?;
    let runs = storage.list_runs(RunFilter::default()).await?;

    // Task 12 M3 review, BLOCKING #2: the single source of "is a parked
    // run due" — `registry::due_parked`'s own query (`status = 'parked'
    // AND wake_at <= now`) — computed once per scan, not re-derived
    // per-run from an inline timestamp comparison. `due_parked` had zero
    // real callers before this; this is its first.
    let due_parked_ids: std::collections::HashSet<surge_core::id::RunId> = storage
        .due_parked(opts.now_ms)
        .await?
        .into_iter()
        .map(|s| s.id)
        .collect();

    let mut decisions = Vec::new();
    for summary in runs {
        // Persistent tasks use their claim-fenced recovery owner, never generic resume.
        if storage.work_items().for_run(summary.id)?.is_some() {
            continue;
        }
        if owned.contains(&summary.id) {
            continue;
        }
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
        // Engine admission persists the actual execution directory as
        // project_path. Reconstructing a convention-based path loses custom
        // --worktree runs and can resume a different checkout.
        let worktree_path = summary.project_path.clone();
        let worktree_exists = worktree_path.is_dir();

        // Read the event-log tail to learn whether the log already reached
        // a terminal event and when the last event landed. A missing or
        // unreadable per-run DB is tolerated — treat it as an empty log so
        // one corrupt run cannot abort the whole scan.
        let (log_verdict, last_event_ms, active_node) = match storage.open_run_reader(run_id).await
        {
            Ok(reader) => match current_status(&reader, run_id).await {
                Ok(snap) => (snap.verdict, snap.last_event_at_ms, snap.active_node),
                Err(e) => {
                    tracing::warn!(
                        target: "surge.recovery",
                        run_id = %run_id,
                        error = %e,
                        "current_status read failed; treating log as empty"
                    );
                    (TerminalVerdict::Open, None, None)
                },
            },
            Err(e) => {
                tracing::debug!(
                    target: "surge.recovery",
                    run_id = %run_id,
                    error = %e,
                    "open_run_reader failed; treating log as empty"
                );
                (TerminalVerdict::Open, None, None)
            },
        };

        let facts = RunRecoveryFacts {
            registry_status: summary.status,
            log_verdict,
            worktree_exists,
            last_event_ms,
            already_active: active_run_ids.contains(&run_id),
            parked_is_due: summary.status == RunStatus::Parked && due_parked_ids.contains(&run_id),
            parked_wake_at_ms: summary.wake_at_ms,
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

    /// Reconcile a run whose log reached one terminal event to the matching
    /// registry status ([`reconciled_status`]).
    async fn reconcile_terminal(
        &self,
        run_id: &surge_core::id::RunId,
        kind: TerminalKind,
    ) -> Result<(), String>;

    /// Raise a human-attention signal for a run whose log holds conflicting
    /// terminal claims. Must not append to the log or change the registry.
    async fn flag_terminal_conflict(
        &self,
        run_id: &surge_core::id::RunId,
        first_seq: u64,
        second_seq: u64,
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
    /// Runs flagged for conflicting terminal history.
    pub flagged_conflict: usize,
    /// Decisions that required no action (already terminal / already active).
    pub skipped: usize,
    /// Runs left parked because their `wake_at` has not passed yet (Task
    /// 12 M3 review, BLOCKING #2) — distinct from `skipped` so an operator
    /// (or a test) can tell "nothing to do" apart from "intentionally
    /// still waiting".
    pub parked: usize,
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
            RecoveryAction::FlagTerminalConflict {
                first_seq,
                second_seq,
            } => {
                tracing::warn!(target: "surge.recovery", run_id = %d.run_id, first_seq,
                    second_seq, "conflicting terminal history; left untouched for an operator");
                if dry_run {
                    out.flagged_conflict += 1;
                } else {
                    match effects
                        .flag_terminal_conflict(&d.run_id, first_seq, second_seq)
                        .await
                    {
                        Ok(()) => out.flagged_conflict += 1,
                        Err(e) => {
                            out.errors += 1;
                            tracing::warn!(target: "surge.recovery", run_id = %d.run_id,
                                error = %e, "flag-terminal-conflict notify failed");
                        },
                    }
                }
            },
            RecoveryAction::ReconcileTerminal { kind } => {
                if dry_run {
                    out.reconciled += 1;
                } else {
                    match effects.reconcile_terminal(&d.run_id, kind).await {
                        Ok(()) => {
                            out.reconciled += 1;
                            tracing::info!(
                                target: "surge.recovery",
                                run_id = %d.run_id,
                                ?kind,
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
            RecoveryAction::SkipParked { wake_at_ms } => {
                out.parked += 1;
                tracing::info!(
                    target: "surge.recovery",
                    run_id = %d.run_id,
                    ?wake_at_ms,
                    "left parked; not yet due"
                );
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
        flagged_conflict = out.flagged_conflict,
        skipped = out.skipped,
        parked = out.parked,
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
    /// Persisted event source shared with the IPC server.
    pub tracking: crate::tracked_run::TrackingContext,
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
            &self.tracking,
            &self.admission,
            &self.broadcast,
        )
        .await
    }

    async fn mark_failed(
        &self,
        run_id: &surge_core::id::RunId,
        reason: &str,
    ) -> Result<(), String> {
        fail_run_in_log_and_registry(&self.storage, *run_id, reason, self.now_ms).await
    }

    async fn reconcile_terminal(
        &self,
        run_id: &surge_core::id::RunId,
        kind: TerminalKind,
    ) -> Result<(), String> {
        self.storage
            .set_run_status(run_id, reconciled_status(kind), Some(self.now_ms))
            .await
            .map_err(|e| e.to_string())
    }

    async fn flag_terminal_conflict(
        &self,
        run_id: &surge_core::id::RunId,
        first_seq: u64,
        second_seq: u64,
    ) -> Result<(), String> {
        self.notify_operator(
            run_id,
            format!("Run {run_id} has conflicting terminal history"),
            format!(
                "Its event log claims termination more than once (seq {first_seq} and \
                 {second_seq}), so its outcome is unconfirmed. Recovery left it untouched; \
                 inspect with `surge engine replay {run_id}`."
            ),
        )
        .await
    }

    async fn flag_stuck(&self, run_id: &surge_core::id::RunId, idle_ms: i64) -> Result<(), String> {
        let idle_hours = idle_ms / 3_600_000;
        self.notify_operator(
            run_id,
            format!("Run {run_id} may be stuck"),
            format!(
                "No new events for ~{idle_hours}h after a daemon restart. \
                 Recovery left it untouched; inspect with `surge engine watch {run_id}` \
                 or abort with `surge run abort {run_id}`."
            ),
        )
        .await
    }
}

impl DaemonRecoveryEffects {
    /// Desktop human-attention notice for a run recovery left untouched.
    /// No desktop channel configured is tolerated: the executor already
    /// logged the decision at WARN.
    async fn notify_operator(
        &self,
        run_id: &surge_core::id::RunId,
        title: String,
        body: String,
    ) -> Result<(), String> {
        let node = surge_core::keys::NodeKey::try_new("recovery")
            .map_err(|e| format!("bad recovery node key: {e}"))?;
        let rendered = surge_notify::RenderedNotification {
            severity: surge_core::notify_config::NotifySeverity::Warn,
            title,
            body,
            artifact_paths: vec![],
        };
        let ctx = surge_notify::NotifyDeliveryContext {
            run_id: *run_id,
            node: &node,
        };
        match self
            .notifier
            .deliver(
                &ctx,
                &surge_core::notify_config::NotifyChannel::Desktop,
                &rendered,
            )
            .await
        {
            Ok(()) | Err(surge_notify::NotifyError::ChannelNotConfigured) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}

/// Default threshold after which a run with no new events is considered
/// stuck rather than resumable: 24 hours (per the roadmap's stuck-run
/// detection bullet).
pub const DEFAULT_STUCK_THRESHOLD: Duration = Duration::from_hours(24);

/// Mark a run failed in BOTH the event log and the registry.
///
/// Appends a terminal `RunFailed` event to the per-run log, then sets the
/// registry status to `Failed`. The event log is the source of truth, so
/// updating only the registry would leave `replay` / `current_status`
/// folding the run as still in-flight while `list`/recovery report it
/// failed. The per-run event DB lives under `~/.surge/runs/<run_id>/`,
/// independent of the (possibly lost) worktree, so the append succeeds even
/// for the worktree-lost case.
///
/// Best-effort on the log append: if the writer cannot be opened, the
/// registry is still updated and the discrepancy is logged.
pub async fn fail_run_in_log_and_registry(
    storage: &std::sync::Arc<surge_persistence::runs::Storage>,
    run_id: surge_core::id::RunId,
    reason: &str,
    now_ms: i64,
) -> Result<(), String> {
    match storage.open_run_writer(run_id).await {
        Ok(writer) => {
            let ev = surge_core::run_event::VersionedEventPayload::new(
                surge_core::run_event::EventPayload::RunFailed {
                    error: reason.to_string(),
                },
            );
            if let Err(e) = writer.append_event(ev).await {
                tracing::warn!(
                    target: "surge.recovery",
                    run_id = %run_id,
                    error = %e,
                    "could not append RunFailed event; registry will still be updated"
                );
            }
            // Flush + release the writer lock before the registry write.
            let _ = writer.close().await;
        },
        Err(e) => {
            tracing::warn!(
                target: "surge.recovery",
                run_id = %run_id,
                error = %e,
                "could not open writer to append RunFailed; updating registry only"
            );
        },
    }

    storage
        .set_run_status(&run_id, RunStatus::Failed, Some(now_ms))
        .await
        .map_err(|e| e.to_string())
}

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
    tracking: &crate::tracked_run::TrackingContext,
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
        tracking: tracking.clone(),
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
        async fn reconcile_terminal(&self, _r: &RunId, _kind: TerminalKind) -> Result<(), String> {
            self.reconcile_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        async fn flag_terminal_conflict(
            &self,
            _r: &RunId,
            _first_seq: u64,
            _second_seq: u64,
        ) -> Result<(), String> {
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
                decision(RecoveryAction::ReconcileTerminal {
                    kind: TerminalKind::Failed,
                }),
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
                flagged_conflict: 0,
                skipped: 2,
                parked: 0,
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

    const NOW: i64 = 1_700_000_000_000;

    fn opts(worktrees_root: std::path::PathBuf) -> RecoveryOptions {
        RecoveryOptions {
            stuck_threshold: Duration::from_hours(24),
            worktrees_root,
            now_ms: NOW,
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn plans_resume_failed_worktree_and_skips_terminal() {
        let tmp = crate::runtime_home_fixture::FixtureHome::new().unwrap();
        {
            let storage = Storage::open(tmp.path()).await.unwrap();
            let wt_root = tmp.path().join("worktrees");

            // Run A — candidate with a present worktree → Resume.
            let run_a = RunId::new();
            let custom_path = tmp.path().join("custom-checkout");
            let present_writer = storage.create_run(run_a, &custom_path, None).await.unwrap();
            std::fs::create_dir_all(&custom_path).unwrap();

            // Run B — candidate with an absent worktree → MarkFailedWorktreeLost.
            let run_b = RunId::new();
            let lost_writer = storage.create_run(run_b, "/proj", None).await.unwrap();

            // Run C — terminal (Completed) → not a candidate, no decision.
            let run_c = RunId::new();
            let terminal_writer = storage.create_run(run_c, "/proj", None).await.unwrap();
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
            assert_eq!(a.worktree_path, custom_path);
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
            present_writer.close().await.unwrap();
            lost_writer.close().await.unwrap();
            terminal_writer.close().await.unwrap();
        }
        tmp.close().unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn active_run_is_reported_as_skip_already_active() {
        let tmp = crate::runtime_home_fixture::FixtureHome::new().unwrap();
        {
            let storage = Storage::open(tmp.path()).await.unwrap();
            let wt_root = tmp.path().join("worktrees");

            let run = RunId::new();
            let writer = storage
                .create_run(run, wt_root.join(run.to_string()), None)
                .await
                .unwrap();
            std::fs::create_dir_all(wt_root.join(run.to_string())).unwrap();

            let mut active = HashSet::new();
            active.insert(run);

            let report = plan_recovery(&storage, &opts(wt_root), &active)
                .await
                .unwrap();
            assert_eq!(report.decisions.len(), 1);
            assert_eq!(
                report.decisions[0].action,
                RecoveryAction::SkipAlreadyActive
            );
            writer.close().await.unwrap();
        }
        tmp.close().unwrap();
    }

    /// Task 12 M3 review, BLOCKING #2, end-to-end through `plan_recovery`
    /// (not just `decide_action`'s pure logic in isolation): a parked run
    /// not yet due must come back `SkipParked`, and a parked run past its
    /// `wake_at` must come back `Resume` — proving `Storage::due_parked`
    /// is actually consulted by the planner, not just present on `Storage`
    /// with nothing calling it.
    #[tokio::test(flavor = "multi_thread")]
    async fn plan_recovery_leaves_not_yet_due_parked_runs_parked_and_resumes_due_ones() {
        let tmp = crate::runtime_home_fixture::FixtureHome::new().unwrap();
        {
            let storage = Storage::open(tmp.path()).await.unwrap();
            let wt_root = tmp.path().join("worktrees");

            // Not yet due: wake_at in the future.
            let run_waiting = RunId::new();
            let waiting_writer = storage
                .create_run(run_waiting, wt_root.join(run_waiting.to_string()), None)
                .await
                .unwrap();
            std::fs::create_dir_all(wt_root.join(run_waiting.to_string())).unwrap();
            storage
                .set_run_parked(&run_waiting, NOW + 3_600_000)
                .await
                .unwrap();

            // Due: wake_at already passed.
            let run_due = RunId::new();
            let due_writer = storage
                .create_run(run_due, wt_root.join(run_due.to_string()), None)
                .await
                .unwrap();
            std::fs::create_dir_all(wt_root.join(run_due.to_string())).unwrap();
            storage
                .set_run_parked(&run_due, NOW - 3_600_000)
                .await
                .unwrap();

            let report = plan_recovery(&storage, &opts(wt_root), &HashSet::new())
                .await
                .unwrap();

            let waiting = report
                .decisions
                .iter()
                .find(|d| d.run_id == run_waiting)
                .expect("waiting run present");
            assert_eq!(
                waiting.action,
                RecoveryAction::SkipParked {
                    wake_at_ms: Some(NOW + 3_600_000)
                }
            );

            let due = report
                .decisions
                .iter()
                .find(|d| d.run_id == run_due)
                .expect("due run present");
            assert_eq!(due.action, RecoveryAction::Resume);
            waiting_writer.close().await.unwrap();
            due_writer.close().await.unwrap();
        }
        tmp.close().unwrap();
    }
}

#[cfg(test)]
mod decide_action_tests {
    use super::*;

    const HOUR_MS: i64 = 3_600_000;
    const NOW: i64 = 1_700_000_000_000;
    const STUCK: Duration = Duration::from_hours(24);

    /// Baseline: a Crashed run, worktree present, fresh event, not active.
    fn healthy_crashed() -> RunRecoveryFacts {
        RunRecoveryFacts {
            registry_status: RunStatus::Crashed,
            log_verdict: TerminalVerdict::Open,
            worktree_exists: true,
            last_event_ms: Some(NOW - HOUR_MS),
            already_active: false,
            parked_is_due: false,
            parked_wake_at_ms: None,
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

    fn terminal(kind: TerminalKind) -> TerminalVerdict {
        use surge_core::run_verdict::{TerminalClaim, TerminalOutcome};
        let outcome = match kind {
            TerminalKind::Completed => TerminalOutcome::Completed {
                terminal_node: "end".try_into().unwrap(),
            },
            TerminalKind::Failed => TerminalOutcome::Failed {
                error: "boom".into(),
            },
            TerminalKind::Aborted => TerminalOutcome::Aborted {
                reason: "stopped".into(),
            },
        };
        TerminalVerdict::Terminal(TerminalClaim { seq: 9, outcome })
    }

    #[test]
    fn a_terminal_log_reconciles_to_its_exact_kind() {
        for kind in [
            TerminalKind::Completed,
            TerminalKind::Failed,
            TerminalKind::Aborted,
        ] {
            let facts = RunRecoveryFacts {
                log_verdict: terminal(kind),
                // A finished run's worktree may be cleaned up: never read as
                // worktree-lost.
                worktree_exists: false,
                ..healthy_crashed()
            };
            assert_eq!(
                decide_action(&facts, NOW, STUCK),
                RecoveryAction::ReconcileTerminal { kind }
            );
        }
    }

    #[test]
    fn an_aborted_log_reconciles_to_aborted_not_completed() {
        assert_eq!(reconciled_status(TerminalKind::Aborted), RunStatus::Aborted);
        assert_eq!(
            reconciled_status(TerminalKind::Completed),
            RunStatus::Completed
        );
        assert_eq!(reconciled_status(TerminalKind::Failed), RunStatus::Failed);
    }

    #[test]
    fn a_conflicting_log_is_flagged_before_worktree_and_stuck_checks() {
        use surge_core::run_verdict::{TerminalClaim, TerminalOutcome};
        let claim = |seq| TerminalClaim {
            seq,
            outcome: TerminalOutcome::Aborted { reason: "x".into() },
        };
        let facts = RunRecoveryFacts {
            log_verdict: TerminalVerdict::Conflict {
                claims: vec![claim(4), claim(7)],
            },
            worktree_exists: false,
            last_event_ms: Some(0),
            ..healthy_crashed()
        };
        assert_eq!(
            decide_action(&facts, NOW, STUCK),
            RecoveryAction::FlagTerminalConflict {
                first_seq: 4,
                second_seq: 7
            }
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
    fn parked_and_not_yet_due_is_skipped_not_resumed() {
        // Task 12 M3 review, BLOCKING #2: before this fix, a `Parked` run
        // fell all the way through to `Resume` (nothing in `decide_action`
        // recognized the status at all) — every daemon restart would wake
        // every parked run immediately, regardless of `wake_at`.
        let facts = RunRecoveryFacts {
            registry_status: RunStatus::Parked,
            parked_is_due: false,
            parked_wake_at_ms: Some(NOW + HOUR_MS),
            ..healthy_crashed()
        };
        assert_eq!(
            decide_action(&facts, NOW, STUCK),
            RecoveryAction::SkipParked {
                wake_at_ms: Some(NOW + HOUR_MS)
            }
        );
    }

    #[test]
    fn parked_and_due_falls_through_to_resume() {
        // The other half: a parked run whose `wake_at` *has* passed
        // resumes through the ordinary path — `Engine::resume_run` itself
        // clears `Parked`/`wake_at` and arms the capacity-precheck bypass
        // (see that function's own doc); `decide_action` does not need a
        // separate "due" action.
        let facts = RunRecoveryFacts {
            registry_status: RunStatus::Parked,
            parked_is_due: true,
            parked_wake_at_ms: Some(NOW - HOUR_MS),
            ..healthy_crashed()
        };
        assert_eq!(decide_action(&facts, NOW, STUCK), RecoveryAction::Resume);
    }

    /// Mutation test: swapping the priority of rules 5 (parked) and 6
    /// (stuck) must fail. A run parked far longer than `stuck_threshold`
    /// (its `wake_at` deliberately still in the future) must be skipped as
    /// parked, never flagged stuck — if rule 6 were checked first, this
    /// exact fixture would come back `FlagStuck`, not `SkipParked`.
    #[test]
    fn parked_run_idle_longer_than_stuck_threshold_is_still_skipped_not_flagged_stuck() {
        let facts = RunRecoveryFacts {
            registry_status: RunStatus::Parked,
            parked_is_due: false,
            parked_wake_at_ms: Some(NOW + HOUR_MS),
            last_event_ms: Some(NOW - 100 * HOUR_MS), // far beyond STUCK (24h)
            ..healthy_crashed()
        };
        let action = decide_action(&facts, NOW, STUCK);
        assert_eq!(
            action,
            RecoveryAction::SkipParked {
                wake_at_ms: Some(NOW + HOUR_MS)
            },
            "a parked run's intentional silence must never be read as stuck, got {action:?}"
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
        assert_eq!(decide_action(&facts, NOW, STUCK), RecoveryAction::Resume);
    }
}
