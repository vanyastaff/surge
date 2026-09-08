//! `WakeScheduler` — periodically resumes parked runs once their `wake_at`
//! has elapsed (Task 12 M4, R37/R37.1).
//!
//! `M3` taught the engine to park a run that cannot finish inside its
//! provider's remaining rate-limit window (`RunStatus::Parked`, `wake_at`
//! recorded). A parked run comes back today only when the daemon restarts
//! and [`crate::recovery::recover_on_startup`] scans for due rows — there
//! is no periodic wake. This module adds that: a tick loop that polls
//! [`surge_persistence::runs::Storage::due_parked`] (the single source of
//! "is a parked run due", already documented as this scheduler's intended
//! caller) and resumes what it finds.
//!
//! **Same tick-loop shape as [`crate::inbox::snooze_scheduler::SnoozeScheduler`],
//! not extended or abstracted from it.** The two operate on unrelated
//! domains — `ticket_index` / `TicketState::Snoozed` / `IntakeRepo` for the
//! snoozer, `runs` / `RunStatus::Parked` / the run registry for this one —
//! and the only thing they share is the ~15-line "poll on an interval, act
//! on what's due" shape. Rule of three: two occurrences do not earn an
//! abstraction; merge them if a third scheduler with the same shape shows
//! up. The clock, though, is **not** copied from the snoozer — that one
//! reads `Utc::now()` inline (`snooze_scheduler.rs`), which would make this
//! scheduler's tests depend on wall-clock sleeps. The precedent this module
//! follows instead is [`crate::recovery::RecoveryOptions::now_ms`] (an
//! injected value, not a live clock read) and
//! [`surge_persistence::runs::Clock`] (the trait itself) — an
//! `Arc<dyn Clock>` field, swapped for a `MockClock` under test.
//!
//! **Resume goes through exactly one seam:** [`crate::server::resume_run_tracked`]
//! — the same function [`crate::recovery::DaemonRecoveryEffects::resume`]
//! calls for a startup-recovered run. Admission, the broadcast registry,
//! and the frozen-budget re-arm (`Engine::resume_run`, mechanism from
//! commit `1ed5caa`) all live behind that one seam; a second resume path
//! here would silently lose all three.
//!
//! **Worktree probe on the wake path**, unlike crash recovery's own
//! `<worktrees_root>/<run_id>` reconstruction (documented as a known
//! limitation on `plan_recovery` — a run launched with a custom
//! `--worktree` is not resumable through it): `RunParked` carries the
//! run's *actual* worktree path, recorded at the moment it parked
//! (zero extra cost — the engine already holds it). This scheduler reads
//! that recorded path (via [`surge_persistence::runs::current_status`]'s
//! `parked_worktree` field) and probes *it*, not a guess. A missing
//! worktree fails the run honestly through
//! [`crate::recovery::fail_run_in_log_and_registry`] — reused, not
//! duplicated — rather than attempting (and silently losing) a resume into
//! a directory that is no longer there.
//!
//! **Blind-park-limit escalation** (`CapacityConfig::blind_park_limit`):
//! the resume-time capacity-precheck bypass (`Engine::resume_run`) means a
//! genuinely down runtime still burns exactly one real attempt per wake,
//! then re-parks — bounded, self-correcting, not a livelock (see the
//! `surge-capacity-row-never-expires` project note). But a long streak of
//! such blind re-parks with no successful dispatch between them is a signal
//! worth a human's attention, which nothing today raises. The same
//! [`surge_persistence::runs::current_status`] read this scheduler already
//! pays for per due run carries `consecutive_blind_parks` /
//! `blind_park_limit_escalated` (Task 12 M4) — crossing the limit raises
//! `EventPayload::EscalationRequested` (the same durable, Telegram-cockpit-
//! surfaced mechanism `engine/stage/agent.rs`'s loop guard and
//! `engine/bootstrap.rs`'s edit-loop cap already use) and a desktop
//! notification (the exact pattern [`crate::recovery::DaemonRecoveryEffects::flag_stuck`]
//! uses for a stuck run) — once per streak, not once per tick, so the
//! run keeps parking/waking (unchanged) while a human has been told.

use std::sync::Arc;
use std::time::Duration;

use surge_core::id::RunId;
use surge_core::keys::NodeKey;
use surge_core::notify_config::{NotifyChannel, NotifySeverity};
use surge_core::run_event::{EscalationCause, EventPayload, VersionedEventPayload};
use surge_notify::{NotifyDeliverer, NotifyDeliveryContext, RenderedNotification};
use surge_orchestrator::engine::facade::EngineFacade;
use surge_persistence::runs::{Clock, RunStatusSnapshot, Storage};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::admission::AdmissionController;
use crate::broadcast::BroadcastRegistry;
use crate::recovery::fail_run_in_log_and_registry;

/// Tokio loop polling `runs.wake_at` for due-parked rows and resuming them.
///
/// See the module doc for why this is a sibling of
/// [`crate::recovery`]/[`crate::admission`] (not folded into
/// `inbox::snooze_scheduler`) and why the clock is injected.
pub struct WakeScheduler {
    /// Run registry + per-run event logs.
    pub storage: Arc<Storage>,
    /// Engine facade used to resume due runs — the daemon's in-process
    /// `LocalEngineFacade` in production.
    pub facade: Arc<dyn EngineFacade>,
    /// Shared admission controller (same instance the IPC server and crash
    /// recovery use, so slot accounting is consistent across all three).
    pub admission: Arc<AdmissionController>,
    /// Shared broadcast registry (same instance the IPC server and crash
    /// recovery use).
    pub broadcast: Arc<BroadcastRegistry>,
    /// Wall clock, injected rather than read inline so this scheduler is
    /// deterministic under test — see the module doc.
    pub clock: Arc<dyn Clock>,
    /// Notifier for the blind-park-limit escalation's desktop channel (same
    /// instance `recovery.rs`'s stuck-run notifier uses).
    pub notifier: Arc<dyn NotifyDeliverer>,
    /// Consecutive blind-park cap before escalating
    /// (`SurgeConfig.capacity.blind_park_limit` — `CapacityConfig`'s own
    /// field, sourced here rather than added to `CapacityPolicy`: that
    /// struct's doc already notes this cap is consumed by the caller that
    /// counts parks across the event log, not by the stateless `decide`
    /// call).
    pub blind_park_limit: u32,
    /// How often to poll `due_parked`.
    pub poll_interval: Duration,
}

/// Default poll interval: frequent enough that a run parked on the
/// shortest realistic blind-backoff policy (`capacity_config::
/// DEFAULT_BLIND_BACKOFF`'s 5 minutes) wakes promptly after `wake_at`
/// passes, infrequent enough not to hammer the registry DB with polling.
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(30);

impl WakeScheduler {
    /// Drive the loop until `shutdown` is cancelled.
    pub async fn run(self, shutdown: CancellationToken) {
        let mut interval = tokio::time::interval(self.poll_interval);
        loop {
            tokio::select! {
                () = shutdown.cancelled() => return,
                _ = interval.tick() => {}
            }
            self.tick().await;
        }
    }

    /// One poll: resume every run [`surge_persistence::runs::Storage::due_parked`]
    /// reports as due at this instant. A `due_parked` query failure is
    /// logged and skipped — one bad poll must not crash the scheduler task;
    /// the next tick tries again.
    async fn tick(&self) {
        let now_ms = self.clock.now_ms();
        let due = match self.storage.due_parked(now_ms).await {
            Ok(rows) => rows,
            Err(error) => {
                warn!(
                    target: "surge.wake_scheduler",
                    %error,
                    "due_parked query failed; skipping this tick"
                );
                return;
            },
        };
        for run in due {
            self.wake_one(run.id, now_ms).await;
        }
    }

    /// Resume a single due run: read its status snapshot (worktree +
    /// blind-park streak, one read), escalate if the streak just crossed
    /// [`Self::blind_park_limit`], then probe the worktree and either
    /// resume through [`crate::server::resume_run_tracked`] or fail the run
    /// honestly (reusing [`fail_run_in_log_and_registry`]) when the
    /// worktree — or the recorded path itself — is missing. Never a silent
    /// resume attempt into a path that does not exist.
    async fn wake_one(&self, run_id: RunId, now_ms: i64) {
        let snapshot = match self.read_snapshot(run_id).await {
            Ok(snapshot) => snapshot,
            Err(error) => {
                warn!(
                    target: "surge.wake_scheduler",
                    %run_id,
                    %error,
                    "failed to read the run's own log; leaving it parked for the next tick"
                );
                return;
            },
        };

        if snapshot.consecutive_blind_parks >= self.blind_park_limit
            && !snapshot.blind_park_limit_escalated
        {
            self.escalate_blind_park_limit(run_id, snapshot.consecutive_blind_parks)
                .await;
        }

        let Some(worktree_path) = snapshot.parked_worktree else {
            warn!(
                target: "surge.wake_scheduler",
                %run_id,
                "due parked run has no recorded worktree in its own log; failing honestly \
                 instead of guessing a path"
            );
            self.fail_honestly(
                run_id,
                "parked run's worktree is unrecorded; cannot resume",
                now_ms,
            )
            .await;
            return;
        };

        if !worktree_path.exists() {
            warn!(
                target: "surge.wake_scheduler",
                %run_id,
                worktree = %worktree_path.display(),
                "recorded worktree is gone; marking failed instead of a silent resume attempt"
            );
            self.fail_honestly(run_id, "worktree lost; cannot resume", now_ms)
                .await;
            return;
        }

        match crate::server::resume_run_tracked(
            run_id,
            worktree_path,
            self.facade.as_ref(),
            &self.admission,
            &self.broadcast,
        )
        .await
        {
            Ok(()) => info!(target: "surge.wake_scheduler", %run_id, "woke parked run"),
            Err(error) => warn!(
                target: "surge.wake_scheduler",
                %run_id,
                %error,
                "wake resume failed; the run stays Parked for a later tick to retry"
            ),
        }
    }

    /// Fold the run's own event log into a [`RunStatusSnapshot`] — one read
    /// serving both the recorded worktree
    /// ([`RunStatusSnapshot::parked_worktree`], the actual path, not a
    /// `<worktrees_root>/<run_id>` reconstruction) and the blind-park
    /// streak ([`RunStatusSnapshot::consecutive_blind_parks`]).
    async fn read_snapshot(&self, run_id: RunId) -> Result<RunStatusSnapshot, String> {
        let reader = self
            .storage
            .open_run_reader(run_id)
            .await
            .map_err(|e| e.to_string())?;
        surge_persistence::runs::current_status(&reader, run_id)
            .await
            .map_err(|e| e.to_string())
    }

    /// Raise the blind-park-limit escalation exactly once per streak:
    /// appends `EventPayload::EscalationRequested { cause:
    /// CapacityBlindParkLimitExceeded }` to the run's own log (the durable,
    /// Telegram-cockpit-surfaced escalation trace every other escalation
    /// path in this codebase already uses — `escalations.rs`,
    /// `cockpit/dispatch.rs`), then delivers a desktop notification via the
    /// exact pattern `recovery.rs::flag_stuck` uses. Both are best-effort:
    /// a delivery failure must not stop the run from waking normally.
    async fn escalate_blind_park_limit(&self, run_id: RunId, consecutive: u32) {
        let reason = format!(
            "runtime exhausted {consecutive} consecutive time(s) with no learned reset time \
             and no successful dispatch between wakes (blind_park_limit = {}); this run may be \
             gated on a genuinely down or misconfigured runtime and needs a human look",
            self.blind_park_limit
        );

        match self.storage.open_run_writer(run_id).await {
            Ok(writer) => {
                let ev = VersionedEventPayload::new(EventPayload::EscalationRequested {
                    stage: None,
                    reason: reason.clone(),
                    cause: EscalationCause::CapacityBlindParkLimitExceeded,
                });
                if let Err(error) = writer.append_event(ev).await {
                    warn!(target: "surge.wake_scheduler", %run_id, %error, "failed to append blind-park-limit EscalationRequested");
                }
                let _ = writer.close().await;
            },
            Err(error) => {
                warn!(target: "surge.wake_scheduler", %run_id, %error, "failed to open run writer for blind-park-limit escalation");
            },
        }

        let Ok(node) = NodeKey::try_new("wake_scheduler") else {
            return;
        };
        let rendered = RenderedNotification {
            severity: NotifySeverity::Warn,
            title: format!("Run {run_id} stuck in a blind-park loop"),
            body: reason,
            artifact_paths: vec![],
        };
        let ctx = NotifyDeliveryContext {
            run_id,
            node: &node,
        };
        match self
            .notifier
            .deliver(&ctx, &NotifyChannel::Desktop, &rendered)
            .await
        {
            Ok(()) | Err(surge_notify::NotifyError::ChannelNotConfigured) => {},
            Err(error) => {
                warn!(target: "surge.wake_scheduler", %run_id, %error, "blind-park-limit notify delivery failed");
            },
        }
    }

    /// Fail `run_id` in both the event log and the registry, reusing
    /// [`fail_run_in_log_and_registry`] — the exact mechanism crash
    /// recovery's `MarkFailedWorktreeLost` uses — rather than a second
    /// mark-failed implementation.
    async fn fail_honestly(&self, run_id: RunId, reason: &str, now_ms: i64) {
        if let Err(error) =
            fail_run_in_log_and_registry(&self.storage, run_id, reason, now_ms).await
        {
            warn!(
                target: "surge.wake_scheduler",
                %run_id,
                %error,
                "mark-failed also failed; the run stays Parked with a wake_at that will never \
                 succeed on its own"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Mutex;
    use surge_core::capacity::WakeBasis;
    use surge_core::run_event::{EventPayload, VersionedEventPayload};
    use surge_orchestrator::engine::EngineRunConfig;
    use surge_orchestrator::engine::error::EngineError;
    use surge_orchestrator::engine::handle::{EngineRunEvent, RunHandle, RunOutcome, RunSummary};
    use surge_persistence::runs::MockClock;
    use tempfile::tempdir;

    /// Records every `resume_run` call (id + the worktree path it was
    /// given) and returns a handle that completes immediately — enough to
    /// let `resume_run_tracked`'s forward task wind down cleanly, and
    /// enough to prove exactly which worktree `WakeScheduler` resolved.
    #[derive(Default)]
    struct StubFacade {
        resume_calls: Mutex<Vec<(RunId, PathBuf)>>,
    }

    #[async_trait::async_trait]
    impl EngineFacade for StubFacade {
        async fn start_run(
            &self,
            _run_id: RunId,
            _graph: surge_core::graph::Graph,
            _worktree_path: PathBuf,
            _run_config: EngineRunConfig,
        ) -> Result<RunHandle, EngineError> {
            Err(EngineError::Internal("stub: start_run not used".into()))
        }

        async fn resume_run(
            &self,
            run_id: RunId,
            worktree_path: PathBuf,
        ) -> Result<RunHandle, EngineError> {
            self.resume_calls
                .lock()
                .unwrap()
                .push((run_id, worktree_path));
            let (tx, rx) = tokio::sync::broadcast::channel(8);
            let _ = tx.send(EngineRunEvent::Terminal {
                outcome: RunOutcome::Completed {
                    terminal: NodeKey::try_from("end").unwrap(),
                },
            });
            drop(tx);
            let completion = tokio::spawn(async move {
                RunOutcome::Completed {
                    terminal: NodeKey::try_from("end").unwrap(),
                }
            });
            Ok(RunHandle {
                run_id,
                events: rx,
                completion,
            })
        }

        async fn stop_run(&self, _run_id: RunId, _reason: String) -> Result<(), EngineError> {
            Ok(())
        }

        async fn resolve_human_input(
            &self,
            _run_id: RunId,
            _call_id: Option<String>,
            _response: serde_json::Value,
        ) -> Result<(), EngineError> {
            Ok(())
        }

        async fn list_runs(&self) -> Result<Vec<RunSummary>, EngineError> {
            Ok(vec![])
        }
    }

    /// Default `blind_park_limit` for tests that aren't specifically about
    /// the escalation threshold — high enough that none of them cross it
    /// incidentally (each parks a run at most a handful of times).
    const TEST_BLIND_PARK_LIMIT: u32 = 100;

    fn scheduler(
        storage: Arc<Storage>,
        facade: Arc<dyn EngineFacade>,
        clock: Arc<MockClock>,
    ) -> WakeScheduler {
        WakeScheduler {
            storage,
            facade,
            admission: Arc::new(AdmissionController::new(8, 16)),
            broadcast: Arc::new(BroadcastRegistry::new()),
            clock,
            // No channels configured: `deliver` always maps to
            // `ChannelNotConfigured`, tolerated as `Ok` — same "notifier
            // present but inert" fixture `daemon_recovery.rs`'s tests use
            // for `flag_stuck`.
            notifier: Arc::new(surge_notify::MultiplexingNotifier::new()),
            blind_park_limit: TEST_BLIND_PARK_LIMIT,
            poll_interval: Duration::from_millis(10),
        }
    }

    /// Write a `RunParked` event to `run_id`'s own log and, mirroring the
    /// real `run_task.rs::parked` writer, set the matching registry status
    /// and `wake_at` — the two writes `due_parked` and `parked_worktree`
    /// each depend on.
    async fn park(
        storage: &Arc<Storage>,
        run_id: RunId,
        worktree: &std::path::Path,
        wake_at_ms: i64,
    ) {
        let writer = storage.open_run_writer(run_id).await.unwrap();
        writer
            .append_event(VersionedEventPayload::new(EventPayload::RunParked {
                wake_at: chrono::DateTime::from_timestamp_millis(wake_at_ms).unwrap(),
                runtime: Some("claude-acp".into()),
                worktree: worktree.to_path_buf(),
                basis: WakeBasis::PolicyBackoff,
                reason: "exhausted".into(),
            }))
            .await
            .unwrap();
        writer.flush().await.unwrap();
        writer.close().await.unwrap();
        storage.set_run_parked(&run_id, wake_at_ms).await.unwrap();
    }

    const NOW: i64 = 1_700_000_000_000;

    #[tokio::test(flavor = "multi_thread")]
    async fn tick_resumes_a_due_run_using_its_recorded_worktree() {
        let tmp = tempdir().unwrap();
        let storage = Storage::open(tmp.path()).await.unwrap();
        let run_id = RunId::new();
        storage.create_run(run_id, "/proj", None).await.unwrap();
        let real_worktree = tmp.path().join("actual-worktree");
        std::fs::create_dir_all(&real_worktree).unwrap();
        park(&storage, run_id, &real_worktree, NOW - 1_000).await;

        let stub = Arc::new(StubFacade::default());
        let facade: Arc<dyn EngineFacade> = stub.clone();
        let clock = Arc::new(MockClock::new(NOW));
        let sched = scheduler(storage.clone(), facade, clock);

        sched.tick().await;

        let calls = stub.resume_calls.lock().unwrap();
        assert_eq!(calls.len(), 1, "the due run must resume exactly once");
        assert_eq!(
            calls[0],
            (run_id, real_worktree),
            "must resume into the RECORDED worktree, not a reconstructed guess"
        );
    }

    /// Mutation-provable: neutralizing `due_parked`'s `wake_at <= now`
    /// filter (or otherwise resuming regardless of due-ness) would make
    /// this assertion fail — a not-yet-due parked run must never resume.
    #[tokio::test(flavor = "multi_thread")]
    async fn tick_does_not_resume_a_not_yet_due_run() {
        let tmp = tempdir().unwrap();
        let storage = Storage::open(tmp.path()).await.unwrap();
        let run_id = RunId::new();
        storage.create_run(run_id, "/proj", None).await.unwrap();
        let real_worktree = tmp.path().join("actual-worktree");
        std::fs::create_dir_all(&real_worktree).unwrap();
        park(&storage, run_id, &real_worktree, NOW + 3_600_000).await;

        let stub = Arc::new(StubFacade::default());
        let facade: Arc<dyn EngineFacade> = stub.clone();
        let clock = Arc::new(MockClock::new(NOW));
        let sched = scheduler(storage.clone(), facade, clock);

        sched.tick().await;

        assert!(
            stub.resume_calls.lock().unwrap().is_empty(),
            "a run whose wake_at has not passed must not resume"
        );
        let summary = storage.get_run(&run_id).await.unwrap().unwrap();
        assert_eq!(summary.status, surge_core::RunStatus::Parked);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn tick_fails_honestly_when_the_recorded_worktree_is_gone() {
        let tmp = tempdir().unwrap();
        let storage = Storage::open(tmp.path()).await.unwrap();
        let run_id = RunId::new();
        storage.create_run(run_id, "/proj", None).await.unwrap();
        let gone_worktree = tmp.path().join("never-created");
        park(&storage, run_id, &gone_worktree, NOW - 1_000).await;

        let stub = Arc::new(StubFacade::default());
        let facade: Arc<dyn EngineFacade> = stub.clone();
        let clock = Arc::new(MockClock::new(NOW));
        let sched = scheduler(storage.clone(), facade, clock);

        sched.tick().await;

        assert!(
            stub.resume_calls.lock().unwrap().is_empty(),
            "must never attempt a resume into a worktree that does not exist"
        );
        let summary = storage.get_run(&run_id).await.unwrap().unwrap();
        assert_eq!(
            summary.status,
            surge_core::RunStatus::Failed,
            "a missing worktree must fail the run honestly, not leave it silently Parked"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn tick_fails_honestly_when_no_worktree_was_ever_recorded() {
        let tmp = tempdir().unwrap();
        let storage = Storage::open(tmp.path()).await.unwrap();
        let run_id = RunId::new();
        storage.create_run(run_id, "/proj", None).await.unwrap();
        // Registry says Parked and due, but the run's own log never wrote a
        // RunParked event at all (e.g. a corrupted/truncated log) — the
        // legacy-path equivalent of "we have nowhere to resume into".
        storage.set_run_parked(&run_id, NOW - 1_000).await.unwrap();

        let stub = Arc::new(StubFacade::default());
        let facade: Arc<dyn EngineFacade> = stub.clone();
        let clock = Arc::new(MockClock::new(NOW));
        let sched = scheduler(storage.clone(), facade, clock);

        sched.tick().await;

        assert!(stub.resume_calls.lock().unwrap().is_empty());
        let summary = storage.get_run(&run_id).await.unwrap().unwrap();
        assert_eq!(summary.status, surge_core::RunStatus::Failed);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn tick_resumes_every_due_run_and_skips_every_not_yet_due_one() {
        let tmp = tempdir().unwrap();
        let storage = Storage::open(tmp.path()).await.unwrap();

        let mut due_ids = Vec::new();
        for i in 0..3 {
            let run_id = RunId::new();
            storage.create_run(run_id, "/proj", None).await.unwrap();
            let wt = tmp.path().join(format!("due-{i}"));
            std::fs::create_dir_all(&wt).unwrap();
            park(&storage, run_id, &wt, NOW - 1_000).await;
            due_ids.push(run_id);
        }
        let not_due_run = RunId::new();
        storage
            .create_run(not_due_run, "/proj", None)
            .await
            .unwrap();
        let not_due_wt = tmp.path().join("not-due");
        std::fs::create_dir_all(&not_due_wt).unwrap();
        park(&storage, not_due_run, &not_due_wt, NOW + 60_000).await;

        let stub = Arc::new(StubFacade::default());
        let facade: Arc<dyn EngineFacade> = stub.clone();
        let clock = Arc::new(MockClock::new(NOW));
        let sched = scheduler(storage.clone(), facade, clock);

        sched.tick().await;

        let resumed: std::collections::HashSet<RunId> = stub
            .resume_calls
            .lock()
            .unwrap()
            .iter()
            .map(|(id, _)| *id)
            .collect();
        assert_eq!(resumed, due_ids.into_iter().collect());
    }

    async fn escalation_count(storage: &Arc<Storage>, run_id: RunId) -> usize {
        let reader = storage.open_run_reader(run_id).await.unwrap();
        let last = reader.current_seq().await.unwrap();
        reader
            .read_events(
                surge_persistence::runs::EventSeq(0)..surge_persistence::runs::EventSeq(last.0 + 1),
            )
            .await
            .unwrap()
            .iter()
            .filter(|ev| {
                matches!(
                    &ev.payload.payload,
                    EventPayload::EscalationRequested {
                        cause:
                            surge_core::run_event::EscalationCause::CapacityBlindParkLimitExceeded,
                        ..
                    }
                )
            })
            .count()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn tick_escalates_once_when_the_blind_park_limit_is_crossed() {
        let tmp = tempdir().unwrap();
        let storage = Storage::open(tmp.path()).await.unwrap();
        let run_id = RunId::new();
        storage.create_run(run_id, "/proj", None).await.unwrap();
        let wt = tmp.path().join("actual-worktree");
        std::fs::create_dir_all(&wt).unwrap();

        // Two consecutive blind parks recorded in the run's own log —
        // `park` appends one `RunParked{PolicyBackoff}` and sets the
        // registry row each call.
        park(&storage, run_id, &wt, NOW - 2_000).await;
        park(&storage, run_id, &wt, NOW - 1_000).await;

        let stub = Arc::new(StubFacade::default());
        let facade: Arc<dyn EngineFacade> = stub.clone();
        let clock = Arc::new(MockClock::new(NOW));
        let mut sched = scheduler(storage.clone(), facade, clock);
        sched.blind_park_limit = 2;

        sched.tick().await;

        assert_eq!(
            escalation_count(&storage, run_id).await,
            1,
            "2 consecutive blind parks against a limit of 2 must raise exactly one escalation"
        );
        assert_eq!(
            stub.resume_calls.lock().unwrap().len(),
            1,
            "escalating must not stop the run from still getting its normal wake attempt"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn tick_does_not_escalate_below_the_blind_park_limit() {
        let tmp = tempdir().unwrap();
        let storage = Storage::open(tmp.path()).await.unwrap();
        let run_id = RunId::new();
        storage.create_run(run_id, "/proj", None).await.unwrap();
        let wt = tmp.path().join("actual-worktree");
        std::fs::create_dir_all(&wt).unwrap();
        park(&storage, run_id, &wt, NOW - 1_000).await;

        let stub = Arc::new(StubFacade::default());
        let facade: Arc<dyn EngineFacade> = stub.clone();
        let clock = Arc::new(MockClock::new(NOW));
        let mut sched = scheduler(storage.clone(), facade, clock);
        sched.blind_park_limit = 2;

        sched.tick().await;

        assert_eq!(
            escalation_count(&storage, run_id).await,
            0,
            "1 consecutive blind park against a limit of 2 must not escalate yet"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn tick_does_not_re_escalate_the_same_streak_on_a_later_tick() {
        let tmp = tempdir().unwrap();
        let storage = Storage::open(tmp.path()).await.unwrap();
        let run_id = RunId::new();
        storage.create_run(run_id, "/proj", None).await.unwrap();
        let wt = tmp.path().join("actual-worktree");
        std::fs::create_dir_all(&wt).unwrap();
        park(&storage, run_id, &wt, NOW - 2_000).await;
        park(&storage, run_id, &wt, NOW - 1_000).await;

        let stub = Arc::new(StubFacade::default());
        let facade: Arc<dyn EngineFacade> = stub.clone();
        let clock = Arc::new(MockClock::new(NOW));
        let mut sched = scheduler(storage.clone(), facade, clock);
        sched.blind_park_limit = 2;

        // The stub facade never clears the registry's Parked status (unlike
        // the real Engine::resume_run), so the same run is still `due` on
        // a second tick with the same unresolved streak.
        sched.tick().await;
        sched.tick().await;

        assert_eq!(
            escalation_count(&storage, run_id).await,
            1,
            "a second tick against the same unresolved streak must not raise a second \
             escalation — once per streak, not once per tick"
        );
    }

    // `run()` itself — `tokio::select!` around `interval.tick()` calling
    // `self.tick()`, then a cancellation check — is not given its own
    // loop-level test here. `tokio::time::{pause,advance}` (the only way
    // to drive an interval without a real sleep — required by this
    // milestone's "no sleep in tests" bar) needs the `current_thread`
    // runtime flavor, which `Storage::open` itself refuses
    // (`OpenError::SingleThreadedRuntime` — it needs real thread
    // concurrency for its background writer). `tick()` above already
    // covers every real decision this scheduler makes; `run()`'s own
    // remaining surface (interval + cancellation) is generic tokio
    // plumbing with the same shape as `inbox::snooze_scheduler::
    // SnoozeScheduler::run`, which carries no dedicated test of its own
    // either.
}
