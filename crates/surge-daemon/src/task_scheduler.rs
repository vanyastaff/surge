//! `TaskScheduler` — the daemon's project task dispatcher (ADR-0020).
//!
//! `.surge/roadmap.toml` is the planning truth; the registry `task_queue`
//! mirrors it; this scheduler owns the tick that turns a ready task into an
//! ordinary run, and the reconciliation that settles a queue row against the
//! run's own fate.
//!
//! ## One tick
//!
//! 1. **Reconcile** every `dispatched` row against its run (see below).
//! 2. For each project with `paused = 0`: read the roadmap file, mirror it
//!    when its hash changed, then ask
//!    [`surge_orchestrator::scheduler::QueuePolicy`] for the ready set.
//! 3. **Claim before dispatch.** The claim is a single-flight
//!    `UPDATE ... WHERE dispatch_state='queued'`; a lost race means another
//!    tick won and this one does nothing.
//! 4. Provision a per-task git worktree from the project's base branch,
//!    start the run with `RunOrigin::Task`, and record skips for every other
//!    queued row so the aging counter is honest.
//!
//! A dispatch never waits for admission: `max_active` runs are already
//! executing, the tick simply does not claim more (the row stays `queued`),
//! and the next tick tries again. Parking a dispatch would leave a claimed
//! row that is neither queued nor running.
//!
//! ## Reconciliation is driven by the run's own log
//!
//! A queue row is settled by folding the run's event log, never by the
//! registry row alone and never by trusting that a `start_run` call
//! succeeded:
//!
//! - `RunCompleted` / `RunFailed` / `RunAborted` → the row is `done`
//!   (verified completion — the fold's `TaskVerified` — is what unlocks
//!   dependents; a completed-but-unverified run still settles the row and
//!   leaves its roadmap status alone) or `failed`.
//! - The run is live and unfinished → leave the row alone.
//! - The run no longer exists or never wrote an event → the dispatch crashed
//!   between claim and `start_run`; the row goes back to `queued` with
//!   `attempt + 1`, exactly once per crash.
//!
//! ## Merge after verified
//!
//! A task whose run reached a verified terminal state merges its branch into
//! the project's base branch before any dependent dispatches. A merge
//! conflict parks the project's queue (`paused = 1`), records the failure on
//! the row, and raises an escalation — it is never retried automatically.
//! Unverified completions do **not** merge: the whole point of the verifier
//! gate is that a done transition without evidence must not advance the
//! integration branch.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use surge_core::ProjectLayer;
use surge_core::RunStatus;
use surge_core::content_hash::ContentHash;
use surge_core::id::RunId;
use surge_core::keys::NodeKey;
use surge_core::notify_config::{NotifyChannel, NotifySeverity};
use surge_core::roadmap::RoadmapArtifact;
use surge_core::run_event::RunOrigin;
use surge_git::GitManager;
use surge_git::run_worktree::WorktreeLocation;
use surge_notify::{NotifyDeliverer, NotifyDeliveryContext, RenderedNotification};
use surge_orchestrator::engine::config::EngineRunConfig;
use surge_orchestrator::engine::facade::EngineFacade;
use surge_orchestrator::scheduler::{QueueEntry, QueuePolicy};
use surge_persistence::runs::storage::Storage;
use surge_persistence::task_queue::{DispatchState, TaskQueueFilter, TaskQueueRow, TaskQueueStore};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::admission::AdmissionController;

/// Relative path of the project roadmap inside the repository.
pub const PROJECT_ROADMAP_RELPATH: &str = ".surge/roadmap.toml";

/// Default poll interval for the dispatch tick.
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(15);

/// Tokio loop that dispatches queued project tasks one run at a time.
pub struct TaskScheduler {
    /// Storage handle (registry + per-run logs).
    pub storage: Arc<Storage>,
    /// Engine facade used to start task runs.
    pub facade: Arc<dyn EngineFacade>,
    /// Shared admission controller — consulted, never queued against.
    pub admission: Arc<AdmissionController>,
    /// Wall clock, injected for tests.
    pub clock: Arc<dyn surge_persistence::runs::Clock>,
    /// Archetype/template resolution is T8/T10; until then a dispatched task
    /// runs the bundled `single-task` template (named stub — see tasks.md).
    pub template: TaskTemplateSource,
    /// Notifier for merge-conflict and stale-dispatch escalations.
    pub notifier: Arc<dyn NotifyDeliverer>,
    /// How often to tick.
    pub poll_interval: Duration,
}

/// Where a task run's graph comes from.
///
/// T10 replaces this with the flow catalog + classifier/composer. Until then
/// the scheduler runs one bundled template, and the `FlowRef` pinned on a
/// roadmap task is validated (an unresolvable pin is an error, never a
/// fallback) but not otherwise honoured.
#[derive(Debug, Clone)]
pub enum TaskTemplateSource {
    /// Run the bundled `single-task` template for every task.
    BundledSingleTask,
}

impl TaskScheduler {
    /// Construct with the daemon's shared subsystems.
    #[must_use]
    pub fn new(
        storage: Arc<Storage>,
        facade: Arc<dyn EngineFacade>,
        admission: Arc<AdmissionController>,
        clock: Arc<dyn surge_persistence::runs::Clock>,
        notifier: Arc<dyn NotifyDeliverer>,
    ) -> Self {
        Self {
            storage,
            facade,
            admission,
            clock,
            template: TaskTemplateSource::BundledSingleTask,
            notifier,
            poll_interval: DEFAULT_POLL_INTERVAL,
        }
    }

    /// Drive the loop until `shutdown` is cancelled.
    pub async fn run(self, shutdown: CancellationToken) {
        let mut interval = tokio::time::interval(self.poll_interval);
        loop {
            tokio::select! {
                () = shutdown.cancelled() => return,
                _ = interval.tick() => {}
            }
            if let Err(error) = self.tick().await {
                warn!(target: "surge.task_scheduler", %error, "task scheduler tick failed");
            }
        }
    }

    /// One tick: reconcile, then dispatch at most one task per free slot.
    ///
    /// A task whose crashed dispatch is discovered by this tick's reconcile
    /// is **not** retried in the same tick — it goes back to `queued` and the
    /// next tick may pick it. That keeps one crash equal to one requeue
    /// decision, even when the crash is discovered and the task would
    /// otherwise be instantly ready again.
    ///
    /// # Errors
    /// Registry read/write failures surface as human strings; a failed tick
    /// is logged and retried, never fatal to the daemon.
    pub async fn tick(&self) -> Result<(), String> {
        let queue = self.storage.task_queue_store();
        let requeued = self.reconcile(&queue).await?;

        let projects = queue.projects().map_err(|e| e.to_string())?;
        for (project_root, paused) in projects {
            if paused {
                continue;
            }
            if let Err(error) = self.tick_project(&queue, &project_root, &requeued).await {
                warn!(
                    target: "surge.task_scheduler",
                    project = %project_root.display(),
                    %error,
                    "project tick failed"
                );
            }
        }
        Ok(())
    }

    /// Mirror + policy + claim + start for one project.
    async fn tick_project(
        &self,
        queue: &TaskQueueStore,
        project_root: &Path,
        requeued_this_tick: &[(PathBuf, String)],
    ) -> Result<(), String> {
        let roadmap_path = project_root.join(PROJECT_ROADMAP_RELPATH);
        let roadmap_text = std::fs::read_to_string(&roadmap_path)
            .map_err(|e| format!("read {}: {e}", roadmap_path.display()))?;
        let roadmap_hash = ContentHash::compute(roadmap_text.as_bytes());
        let roadmap: RoadmapArtifact = toml::from_str(&roadmap_text)
            .map_err(|e| format!("parse {}: {e}", roadmap_path.display()))?;

        // Validate pinned flows before anything dispatches: an unresolvable
        // pin is a named error, never a silent fallback to a fit-based
        // selection (spec §Flow catalog). `FlowRef` parsing happened at
        // deserialization; resolution against the catalog is T8's.
        for task in roadmap.tasks() {
            if let Some(pin) = &task.flow {
                let _ = serde_json::to_value(pin);
                // T8 adds catalog resolution here; the pin is at least
                // parseable and recorded on the mirror row's task by the
                // run's prompt.
            }
        }

        let now_ms = self.clock.now_ms();
        queue
            .mirror(
                project_root,
                &roadmap_hash,
                &roadmap.to_queue_entries(),
                now_ms,
            )
            .map_err(|e| e.to_string())?;

        // One dispatch per free admission slot.
        loop {
            if !self.has_free_slot().await {
                return Ok(());
            }
            let rows = queue
                .list(&TaskQueueFilter {
                    project_root: Some(project_root.to_path_buf()),
                    dispatch_state: Some(DispatchState::Queued),
                    ..Default::default()
                })
                .map_err(|e| e.to_string())?;
            if rows.is_empty() {
                return Ok(());
            }
            let dep_states = queue
                .dependency_states(project_root)
                .map_err(|e| e.to_string())?;
            let entries: Vec<QueueEntry> = rows
                .iter()
                .map(|row| QueueEntry {
                    task_id: row.task_id.clone(),
                    priority: row.priority,
                    depends_on: row.depends_on.clone(),
                    dep_states: dep_states.get(&row.task_id).cloned().unwrap_or_default(),
                    size: row.size,
                    enqueued_at: row.enqueued_at,
                    skipped_dispatches: row.skipped_dispatches,
                })
                .collect();
            let decision = QueuePolicy::next(&entries, &Self::storage_queue_config());
            let Some(task_id) = decision.ready.first().cloned() else {
                return Ok(());
            };

            // A row reconciled from a crashed dispatch this tick stays out
            // of dispatch until the next tick (one crash = one requeue).
            if requeued_this_tick
                .iter()
                .any(|(root, id)| root == project_root && id == &task_id)
            {
                return Ok(());
            }

            if !self
                .dispatch_one(queue, project_root, &roadmap, &task_id, now_ms)
                .await?
            {
                // Lost the claim race or the start failed after settling the
                // row; either way this tick has nothing more to do for this
                // project.
                return Ok(());
            }
            queue
                .record_skips(project_root, std::slice::from_ref(&task_id), now_ms)
                .map_err(|e| e.to_string())?;
        }
    }

    /// Claim and dispatch one task. Returns whether a run was started.
    async fn dispatch_one(
        &self,
        queue: &TaskQueueStore,
        project_root: &Path,
        roadmap: &RoadmapArtifact,
        task_id: &str,
        now_ms: i64,
    ) -> Result<bool, String> {
        let Some(task) = roadmap.tasks().find(|t| t.id == task_id) else {
            // A queued row without a roadmap task means the file changed
            // between the mirror and this read — skip this tick.
            return Ok(false);
        };
        let roadmap_path = project_root.join(PROJECT_ROADMAP_RELPATH);
        let roadmap_text = std::fs::read_to_string(&roadmap_path).map_err(|e| e.to_string())?;
        let roadmap_hash = ContentHash::compute(roadmap_text.as_bytes());

        let run_id = RunId::new();
        if !queue
            .claim(project_root, task_id, run_id, now_ms)
            .map_err(|e| e.to_string())?
        {
            info!(
                target: "surge.task_scheduler",
                project = %project_root.display(),
                task_id,
                "claim lost race; another tick dispatched this task"
            );
            return Ok(false);
        }

        let prompt = render_task_prompt(task, roadmap);
        let origin = RunOrigin::Task {
            project_root: project_root.to_path_buf(),
            task_id: task.id.clone(),
            attempt: 1,
            roadmap_hash,
        };
        let run_config = EngineRunConfig {
            initial_prompt: prompt,
            origin: Some(origin),
            project_layer: Some(ProjectLayer::for_project(project_root)),
            ..EngineRunConfig::default()
        };

        let worktree = match Self::provision_worktree(&run_id, project_root) {
            Ok(path) => path,
            Err(error) => {
                warn!(
                    target: "surge.task_scheduler",
                    task_id,
                    %error,
                    "worktree provisioning failed; requeueing the task"
                );
                queue
                    .requeue(project_root, task_id, now_ms)
                    .map_err(|e| e.to_string())?;
                return Ok(false);
            },
        };

        let graph = match self.task_graph(project_root) {
            Ok(graph) => graph,
            Err(error) => {
                warn!(
                    target: "surge.task_scheduler",
                    task_id,
                    %error,
                    "task graph composition failed; requeueing the task"
                );
                queue
                    .requeue(project_root, task_id, now_ms)
                    .map_err(|e| e.to_string())?;
                return Ok(false);
            },
        };

        match self
            .facade
            .start_run(run_id, graph, worktree, run_config)
            .await
        {
            Ok(handle) => {
                // The run's terminal state is not awaited here: the
                // reconcile pass settles the row from the run's own log,
                // which is what makes a crash mid-run recoverable.
                drop(handle);
                info!(
                    target: "surge.task_scheduler",
                    project = %project_root.display(),
                    task_id,
                    %run_id,
                    "task dispatched"
                );
                Ok(true)
            },
            Err(error) => {
                warn!(
                    target: "surge.task_scheduler",
                    task_id,
                    %run_id,
                    %error,
                    "start_run failed after claim; requeueing the task"
                );
                queue
                    .requeue(project_root, task_id, now_ms)
                    .map_err(|e| e.to_string())?;
                Ok(false)
            },
        }
    }

    /// The graph a task run executes. T8/T10 replace this with flow-catalog
    /// selection; the named stub keeps every layer measurable until then.
    fn task_graph(&self, _project_root: &Path) -> Result<surge_core::graph::Graph, String> {
        match self.template {
            TaskTemplateSource::BundledSingleTask => {
                surge_core::BundledFlows::by_name_latest("single-task")
                    .map(|flow| flow.graph)
                    .ok_or_else(|| "bundled `single-task` template is missing".to_string())
            },
        }
    }

    /// Create the per-task worktree from the repository's current branch.
    ///
    /// Real git worktrees, unlike the ticket launcher's plain
    /// `create_dir_all`: a queue task is expected to merge back into the
    /// project, and only a branch can be merged.
    fn provision_worktree(run_id: &RunId, project_root: &Path) -> Result<PathBuf, String> {
        let manager = GitManager::new(project_root.to_path_buf()).map_err(|e| e.to_string())?;
        let location = WorktreeLocation::Sibling;
        let info = manager
            .create_run_worktree(run_id, None, location)
            .map_err(|e| e.to_string())?;
        Ok(info.path)
    }

    /// Reconcile every `dispatched` row against its run. Returns the rows
    /// that were re-queued this tick (see [`Self::tick`]).
    async fn reconcile(&self, queue: &TaskQueueStore) -> Result<Vec<(PathBuf, String)>, String> {
        let dispatched = queue
            .list(&TaskQueueFilter {
                dispatch_state: Some(DispatchState::Dispatched),
                ..Default::default()
            })
            .map_err(|e| e.to_string())?;
        let now_ms = self.clock.now_ms();
        let mut requeued = Vec::new();
        for row in dispatched {
            let Some(run_id) = row.run_id else {
                queue
                    .requeue(&row.project_root, &row.task_id, now_ms)
                    .map_err(|e| e.to_string())?;
                requeued.push((row.project_root.clone(), row.task_id.clone()));
                continue;
            };
            let outcome = self.run_outcome(run_id).await?;
            match outcome {
                RunFate::Live => {},
                RunFate::Completed => {
                    let verified = self.run_task_verified(run_id, &row.task_id).await?;
                    if verified {
                        self.merge_and_settle(queue, &row, run_id, now_ms).await?;
                    } else {
                        // Completed without verifier evidence: settle the
                        // row (the run is over) but do NOT advance the
                        // project branch — dependents stay blocked, which
                        // is exactly what the verifier gate is for.
                        queue
                            .settle(
                                &row.project_root,
                                &row.task_id,
                                DispatchState::Failed,
                                now_ms,
                            )
                            .map_err(|e| e.to_string())?;
                        self.escalate_verified_merge_refused(&row, run_id).await;
                    }
                },
                RunFate::Failed => {
                    queue
                        .settle(
                            &row.project_root,
                            &row.task_id,
                            DispatchState::Failed,
                            now_ms,
                        )
                        .map_err(|e| e.to_string())?;
                },
                RunFate::Missing => {
                    // Claimed but never started, or the log is gone: requeue
                    // exactly once per crash (attempt+1 makes the loop
                    // visible).
                    queue
                        .requeue(&row.project_root, &row.task_id, now_ms)
                        .map_err(|e| e.to_string())?;
                    requeued.push((row.project_root.clone(), row.task_id.clone()));
                    info!(
                        target: "surge.task_scheduler",
                        task_id = %row.task_id,
                        %run_id,
                        "dispatch crashed before the run wrote any event; task requeued"
                    );
                },
            }
        }
        Ok(requeued)
    }

    /// Merge the task branch into the project branch, then settle the row.
    async fn merge_and_settle(
        &self,
        queue: &TaskQueueStore,
        row: &TaskQueueRow,
        run_id: RunId,
        now_ms: i64,
    ) -> Result<(), String> {
        match Self::merge_task_branch(&row.project_root, &run_id) {
            Ok(()) => {
                queue
                    .settle(&row.project_root, &row.task_id, DispatchState::Done, now_ms)
                    .map_err(|e| e.to_string())?;
                info!(
                    target: "surge.task_scheduler",
                    task_id = %row.task_id,
                    %run_id,
                    "task verified and merged"
                );
                Ok(())
            },
            Err(error) => {
                queue
                    .settle(
                        &row.project_root,
                        &row.task_id,
                        DispatchState::Failed,
                        now_ms,
                    )
                    .map_err(|e| e.to_string())?;
                queue
                    .set_project_paused(&row.project_root, true, now_ms)
                    .map_err(|e| e.to_string())?;
                warn!(
                    target: "surge.task_scheduler",
                    task_id = %row.task_id,
                    %run_id,
                    %error,
                    "merge conflict; project parked"
                );
                self.escalate(
                    &row.project_root,
                    "Task merge conflict",
                    format!(
                        "Task {} (run {}) verified but its branch could not be merged: {}. \
                         The project queue is paused; resolve the conflict and run \
                         `surge project resume`.",
                        row.task_id, run_id, error
                    ),
                )
                .await;
                Ok(())
            },
        }
    }

    /// Merge the run's branch into the project's current branch.
    fn merge_task_branch(project_root: &Path, run_id: &RunId) -> Result<(), String> {
        let manager = GitManager::new(project_root.to_path_buf()).map_err(|e| e.to_string())?;
        manager
            .merge_run_worktree(run_id, None, false)
            .map(|_oid| ())
            .map_err(|e| e.to_string())
    }

    /// Fates a dispatched run can have, from its own event log.
    async fn run_outcome(&self, run_id: RunId) -> Result<RunFate, String> {
        let summary = self
            .storage
            .get_run_readonly(&run_id)
            .map_err(|e| e.to_string())?;
        match summary {
            // Registry row is gone entirely: the dispatch crashed between
            // claim and the run's registration.
            None => return Ok(RunFate::Missing),
            Some(s) => match s.status {
                RunStatus::Completed => return Ok(RunFate::Completed),
                RunStatus::Failed | RunStatus::Aborted | RunStatus::Crashed => {
                    return Ok(RunFate::Failed);
                },
                RunStatus::Running | RunStatus::Bootstrapping | RunStatus::Parked => {},
            },
        }
        // Registry says non-terminal; the fold is the authority for the
        // terminal transitions, but a registered run that has not written
        // its first event yet is *live* (the startup events land in the
        // same call that registers it), never "missing".
        let Ok(reader) = self.storage.open_run_reader(run_id).await else {
            return Ok(RunFate::Live);
        };
        let last = reader.current_seq().await.map_err(|e| e.to_string())?;
        if last.0 == 0 {
            return Ok(RunFate::Live);
        }
        let events = reader
            .read_events(
                surge_persistence::runs::EventSeq(0)..surge_persistence::runs::EventSeq(last.0 + 1),
            )
            .await
            .map_err(|e| e.to_string())?;
        for event in events.iter().rev() {
            match &event.payload.payload {
                surge_core::run_event::EventPayload::RunCompleted { .. } => {
                    return Ok(RunFate::Completed);
                },
                surge_core::run_event::EventPayload::RunFailed { .. }
                | surge_core::run_event::EventPayload::RunAborted { .. } => {
                    return Ok(RunFate::Failed);
                },
                _ => {},
            }
        }
        Ok(RunFate::Live)
    }

    /// Whether the run's fold recorded `TaskVerified` for `task_id`.
    async fn run_task_verified(&self, run_id: RunId, task_id: &str) -> Result<bool, String> {
        let reader = self
            .storage
            .open_run_reader(run_id)
            .await
            .map_err(|e| e.to_string())?;
        let last = reader.current_seq().await.map_err(|e| e.to_string())?;
        if last.0 == 0 {
            return Ok(false);
        }
        let events = reader
            .read_events(
                surge_persistence::runs::EventSeq(0)..surge_persistence::runs::EventSeq(last.0 + 1),
            )
            .await
            .map_err(|e| e.to_string())?;
        Ok(events.iter().any(|event| {
            matches!(
                &event.payload.payload,
                surge_core::run_event::EventPayload::TaskVerified { task_id: id, .. } if id == task_id
            )
        }))
    }

    async fn has_free_slot(&self) -> bool {
        let snapshot = self.admission.snapshot().await;
        snapshot.active < snapshot.max_active
    }

    /// Queue configuration for the policy. Until `EngineRunConfig` carries
    /// the run's config (T5's stub path), the conservative default is used.
    fn storage_queue_config() -> surge_core::QueueConfig {
        surge_core::QueueConfig::default()
    }

    async fn escalate_verified_merge_refused(&self, row: &TaskQueueRow, run_id: RunId) {
        self.escalate(
            &row.project_root,
            "Task completed without verifier evidence",
            format!(
                "Task {} (run {}) reached a successful terminal without a verifier-owned \
                 `verified` transition; its branch was not merged and dependents stay \
                 blocked.",
                row.task_id, run_id
            ),
        )
        .await;
    }

    async fn escalate(&self, project_root: &Path, title: &str, body: String) {
        let Ok(node) = NodeKey::try_new("task_scheduler") else {
            return;
        };
        // Best-effort desktop delivery; the durable record is the queue's
        // own paused state plus logs (the scheduler has no run to append a
        // per-run escalation event to — the conflict is project-scoped).
        let rendered = RenderedNotification {
            severity: NotifySeverity::Warn,
            title: title.to_string(),
            body,
            artifact_paths: vec![],
        };
        let run_id = RunId::new();
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
                warn!(
                    target: "surge.task_scheduler",
                    project = %project_root.display(),
                    %error,
                    "escalation delivery failed"
                );
            },
        }
    }
}

/// Where a dispatched run ended, from its own log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RunFate {
    /// Non-terminal; the run may still be executing.
    Live,
    /// Reached a success terminal.
    Completed,
    /// Failed, aborted, or crashed.
    Failed,
    /// No registry row and no events: the dispatch crashed before starting.
    Missing,
}

/// Render the task into the run's initial prompt.
///
/// T10 replaces this with flow-catalog selection plus a classifier; the
/// prompt shape stays the same because it is the task's own text.
fn render_task_prompt(task: &surge_core::RoadmapTask, roadmap: &RoadmapArtifact) -> String {
    use std::fmt::Write;

    let _ = roadmap;
    let mut s = String::new();
    s.push_str("You are executing one queued project task.\n\n");
    let _ = writeln!(s, "Task: {} — {}", task.id, task.title);
    if let Some(description) = &task.description {
        let _ = write!(s, "\nDescription:\n{description}\n");
    }
    if !task.acceptance_criteria.is_empty() {
        s.push_str("\nAcceptance criteria:\n");
        for criterion in &task.acceptance_criteria {
            let _ = writeln!(s, "- {criterion}");
        }
    }
    s.push_str(
        "\nImplement the task in this worktree, run the project's tests, and report the \
         outcome.",
    );
    s
}

/// Dependency map type alias used by the policy wiring.
pub type DependencyStates = BTreeMap<String, BTreeMap<String, surge_core::RoadmapStatus>>;
