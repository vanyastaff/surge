use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use surge_acp::{
    AgentHealth, AgentPool, DetectedAgent, HealthTracker, PermissionPolicy, Registry, RegistryEntry,
};
use surge_core::SurgeConfig;
use surge_core::id::RunId;
use surge_orchestrator::engine::handle::{RunStatus, RunSummary};
use surge_orchestrator::engine::ipc::GlobalDaemonEvent;

use crate::daemon_link::ConnectionState;

/// Read-side membership only; conflicting accepted references never become ownership.
pub(crate) fn bootstrap_related_runs(
    status: &surge_core::bootstrap_operation::BootstrapOperationStatus,
) -> Result<Vec<RunId>, &'static str> {
    let mut runs = vec![status.planning_run, status.implementation_run];
    if status.planning_run == status.implementation_run {
        return Err("operation has conflicting original runs");
    }
    let mut requests = std::collections::HashSet::new();
    let mut sources = std::collections::HashSet::new();
    for attempt in &status.continuations {
        if !requests.insert(attempt.continuation_id)
            || !sources.insert(attempt.source_run)
            || attempt.operation_id != status.operation_id
            || runs.contains(&attempt.run)
            || attempt.run == status.operation_id
            || !runs.contains(&attempt.source_run)
            || attempt.source_run == status.planning_run
        {
            return Err("operation has conflicting continuation associations");
        }
        runs.push(attempt.run);
    }
    Ok(runs)
}

pub(crate) fn bootstrap_related_path(
    record: &surge_persistence::runs::bootstrap_operations::BootstrapOperationRecord,
    home: &std::path::Path,
    run: RunId,
) -> Result<PathBuf, &'static str> {
    use surge_persistence::runs::bootstrap_operations::BootstrapStoredPayload;
    if !bootstrap_related_runs(&record.status)?.contains(&run) {
        return Err("run is not associated with this operation");
    }
    let BootstrapStoredPayload::V1 { capture, .. } = &record.payload else {
        return Err("unsupported operation payload");
    };
    if run == record.status.planning_run {
        return Ok(capture.fields().planning_worktree.clone());
    }
    if run == record.status.implementation_run {
        return Ok(capture.fields().implementation_worktree.clone());
    }
    // The native host reserves this namespace; journal paths are checked against it.
    let path = home.join("worktrees").join(run.to_string());
    let repository = capture
        .fields()
        .git_common_dir
        .canonicalize()
        .map_err(|_| "accepted repository is unavailable")?;
    if crate::project::git_common_dir(&path).as_ref() != Some(&repository) {
        return Err("continuation worktree does not match the accepted repository");
    }
    Ok(path)
}

pub(crate) fn validate_bootstrap_history(
    record: &surge_persistence::runs::bootstrap_operations::BootstrapOperationRecord,
    run: RunId,
    events: &[surge_persistence::runs::ReadEvent],
    home: Option<&std::path::Path>,
) -> Result<(), String> {
    let surge_persistence::runs::bootstrap_operations::BootstrapStoredPayload::V1 {
        capture, ..
    } = &record.payload
    else {
        return Err("unsupported operation payload".into());
    };
    let planning = run == record.status.planning_run;
    let pins = capture.fields();
    let path = if planning {
        pins.planning_worktree.clone()
    } else if run == record.status.implementation_run {
        pins.implementation_worktree.clone()
    } else {
        crate::app_state::bootstrap_related_path(
            record,
            home.ok_or("continuation namespace is unavailable")?,
            run,
        )
        .map_err(str::to_owned)?
    };
    let hash = if planning {
        Some(pins.graph.digest)
    } else {
        record.child.as_ref().map(|child| child.graph().digest)
    };
    let starts: Vec<_> = events
        .iter()
        .filter_map(|event| match &event.payload.payload {
            surge_core::EventPayload::RunStarted { project_path, .. } => {
                Some((event.seq.as_u64(), project_path))
            },
            _ => None,
        })
        .collect();
    let graphs: Vec<_> = events
        .iter()
        .filter_map(|event| match &event.payload.payload {
            surge_core::EventPayload::PipelineMaterialized { graph, graph_hash } => {
                Some((graph, graph_hash))
            },
            _ => None,
        })
        .collect();
    if starts != vec![(1, &path)]
        || graphs.is_empty()
        || hash != Some(*graphs[0].1)
        || events.iter().any(|event| {
            matches!(
                event.payload.payload,
                surge_core::EventPayload::WorkItemAttemptBound { .. }
            )
        })
    {
        return Err("related journal does not match the accepted run identity".into());
    }
    for (graph, hash) in graphs {
        if graph.identity().map_err(|error| error.to_string())? != *hash {
            return Err("related journal graph content differs from its recorded identity".into());
        }
    }
    Ok(())
}
/// Check a successor's registry and startup observations before confirming its history.
/// This never derives a read pathname from the journal or changes lifecycle state.
pub(crate) async fn validate_bootstrap_successor(
    record: &surge_persistence::runs::bootstrap_operations::BootstrapOperationRecord,
    home: &std::path::Path,
    run: RunId,
    events: &[surge_core::RunEvent],
) -> Result<(), String> {
    if !record
        .status
        .continuations
        .iter()
        .any(|attempt| attempt.run == run)
    {
        return Err("run is not an accepted successor".into());
    }
    let expected_path = bootstrap_related_path(record, home, run).map_err(str::to_owned)?;
    let summary = surge_persistence::runs::Storage::inspect_existing_run_summary(home.into(), run)
        .await
        .map_err(|error| error.to_string())?
        .ok_or("successor registry row is absent")?;
    if summary.id != run || summary.project_path != expected_path {
        return Err("successor registry workspace differs from its accepted namespace".into());
    }
    let starts: Vec<_> = events
        .iter()
        .filter_map(|event| match &event.payload {
            surge_core::EventPayload::RunStarted { project_path, .. } => {
                Some((event.seq, project_path))
            },
            _ => None,
        })
        .collect();
    let graphs: Vec<_> = events
        .iter()
        .filter_map(|event| match &event.payload {
            surge_core::EventPayload::PipelineMaterialized { graph, graph_hash } => {
                Some((graph, graph_hash))
            },
            _ => None,
        })
        .collect();
    let expected_hash = record.child.as_ref().map(|child| child.graph().digest);
    if starts != vec![(1, &expected_path)]
        || events.iter().any(|event| event.run_id != run)
        || graphs.is_empty()
        || expected_hash != Some(*graphs[0].1)
        || events.iter().any(|event| {
            matches!(
                event.payload,
                surge_core::EventPayload::WorkItemAttemptBound { .. }
            )
        })
    {
        return Err("successor startup does not match its accepted identity".into());
    }
    for (graph, hash) in graphs {
        if graph.identity().map_err(|error| error.to_string())? != *hash {
            return Err("successor graph content differs from its recorded identity".into());
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReadDisposition {
    Obsolete,
    Confirmed,
    Absent,
    Incomplete,
    Retired,
}
pub(crate) struct ReadCompletion {
    pub(crate) token: crate::run_observation::Token,
    pub(crate) disposition: ReadDisposition,
    diagnostic: Option<(u64, String)>,
}
impl ReadCompletion {
    pub(crate) fn obsolete(token: &crate::run_observation::Token) -> Self {
        Self {
            token: token.clone(),
            disposition: ReadDisposition::Obsolete,
            diagnostic: None,
        }
    }
    pub(crate) fn confirmed(&self) -> bool {
        self.disposition == ReadDisposition::Confirmed
    }
}
#[derive(Default)]
pub(crate) struct OperationInspection {
    record: Option<String>,
    histories: HashMap<RunId, HistoryInspection>,
}
struct HistoryInspection {
    token: crate::run_observation::Token,
    error_revision: u64,
    reason: String,
    exhausted: bool,
}

/// Central application state shared across all UI screens.
///
/// Screens hold `Entity<AppState>` and read data from it.
/// When data changes, `cx.notify()` triggers UI re-render.
pub struct AppState {
    pub(crate) runtime_home: Option<PathBuf>,
    pub(crate) observations: crate::run_observation::Observations,
    pub(crate) rejected_inspections: HashMap<RunId, (crate::run_observation::Token, String)>,
    #[cfg(test)]
    pub(crate) rejected_history_reads: HashMap<RunId, usize>,
    #[cfg(test)]
    pub(crate) related_history_reads: HashMap<RunId, usize>,
    pub(crate) tasks: crate::work_items::TaskCache,
    pub(crate) project_history: crate::project_lifecycle::ProjectHistory,
    // ── Project ──
    pub project_path: Option<PathBuf>,
    /// Ownership rule for runs of the open project; see
    /// [`AppState::project_runs`].
    pub project_scope: Option<crate::project::ProjectScope>,
    pub project_name: String,
    pub config: Option<SurgeConfig>,
    /// Failure loading this project's configuration or ACP pool, for UI notifications.
    pub project_load_error: Option<String>,
    pub current_branch: String,

    // ── Agents ──
    /// Full registry catalog (builtin agents).
    pub registry: Registry,
    /// Agents detected on system PATH.
    pub installed_agents: Vec<DetectedAgent>,
    /// Registration-only fallback tracker, used **only** when there is no
    /// `agent_pool` yet (agents detected on PATH but no `surge.toml`/pool
    /// created for this project). Nothing ever calls `record_success`/
    /// `record_failure` on this tracker — `AgentPool` owns the live one,
    /// updated by its workers with real request outcomes. Do not read this
    /// field directly; go through [`AppState::agent_health`], which picks
    /// the pool's tracker over this one whenever a pool exists. Do not
    /// sync the two by copying between them — this one is a fallback, not
    /// a cache.
    pub fallback_health: HealthTracker,

    // ── ACP ──
    /// Agent pool for ACP connections (created when project has agents configured).
    pub agent_pool: Option<Arc<AgentPool>>,

    // ── Daemon (runtime UI client) ──
    /// Connection state with `surge-daemon`. The runtime UI is a daemon
    /// client: it lists / watches runs that the daemon hosts rather than
    /// running them in-process. See `crate::daemon_link::try_connect`.
    pub daemon_state: ConnectionState,
    /// Run summaries last fetched / observed from the daemon. Updated by
    /// the connect task on `ListRuns` and incrementally by the global-event
    /// subscription as `RunAccepted` / `RunFinished` arrive.
    ///
    /// We store our own `UiRun` rather than `RunSummary` directly because
    /// the latter is `#[non_exhaustive]` (engine can grow new fields) and
    /// would prevent us from synthesising a stub when a `RunAccepted` for
    /// an unknown id arrives ahead of a `ListRuns` refresh.
    pub runs: Vec<UiRun>,
    /// Folded per-run live streams (`subscribe_to_run`). Keyed by run;
    /// present once the UI has attached a subscription for that run.
    /// See [`crate::run_stream`].
    pub run_streams: HashMap<RunId, crate::run_stream::RunStreamState>,
    /// Failed runs the operator acknowledged; excluded from "needs you".
    pub dismissed_runs: crate::dismissed::DismissedRuns,
    /// Unapproved plan edits (provider per step), keyed by the planning run
    /// whose flow gate is waiting. Sent with the approval.
    pub plan_edits: HashMap<RunId, surge_core::node_overrides::NodeOverrides>,
    /// Latest durable bootstrap operation snapshots observed by this UI session.
    pub(crate) operation_inspections: HashMap<RunId, OperationInspection>,
    pub bootstrap_operations:
        HashMap<RunId, surge_core::bootstrap_operation::BootstrapOperationStatus>,
}

/// UI-side projection of a daemon-hosted run. Mirrors the orchestrator's
/// `RunSummary` (which is `#[non_exhaustive]` and therefore not
/// constructible from outside) but with only the fields the runtime UI
/// reads today. Add fields here as the UI starts surfacing more state.
#[derive(Debug, Clone)]
pub struct UiRun {
    pub run_id: RunId,
    pub status: RunStatus,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub last_event_seq: Option<u64>,
    /// When the UI observed the run reach a terminal status. The engine
    /// summary carries no end time, so this is our best truth — it lets
    /// ELAPSED freeze instead of counting wall-time forever. `None` for
    /// non-terminal runs, or terminal runs first seen already-finished.
    pub ended_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl UiRun {
    pub fn is_terminal(&self) -> bool {
        matches!(
            self.status,
            RunStatus::Completed | RunStatus::Failed | RunStatus::Aborted
        )
    }
}

impl From<&RunSummary> for UiRun {
    fn from(s: &RunSummary) -> Self {
        Self {
            run_id: s.run_id,
            status: s.status,
            started_at: s.started_at,
            last_event_seq: s.last_event_seq,
            ended_at: None,
        }
    }
}

impl AppState {
    /// Create initial state with empty data and real registry.
    pub fn new() -> Self {
        let registry = Registry::builtin();
        let installed_agents = registry.detect_installed_with_paths();

        // Register installed agents in the no-pool fallback tracker.
        let mut fallback_health = HealthTracker::new();
        for agent in &installed_agents {
            fallback_health.register(&agent.entry.id);
        }

        Self {
            project_path: None,
            project_scope: None,
            project_name: String::new(),
            config: None,
            project_load_error: None,
            current_branch: "main".to_string(),
            registry,
            installed_agents,
            fallback_health,
            agent_pool: None,
            daemon_state: ConnectionState::default(),
            runs: Vec::new(),
            run_streams: HashMap::new(),
            dismissed_runs: crate::dismissed::DismissedRuns::load(),
            plan_edits: HashMap::new(),
            operation_inspections: HashMap::new(),
            bootstrap_operations: HashMap::new(),
            runtime_home: surge_core::home::surge_home_dir(),
            observations: Default::default(),
            rejected_inspections: HashMap::new(),
            #[cfg(test)]
            rejected_history_reads: HashMap::new(),
            #[cfg(test)]
            related_history_reads: HashMap::new(),
            tasks: crate::work_items::TaskCache::default(),
            project_history: crate::project_lifecycle::ProjectHistory::default(),
        }
    }

    /// Apply a `GlobalDaemonEvent` to the run list.
    ///
    /// `RunAccepted` inserts a fresh `RunSummary` if the id isn't
    /// already known (or marks an existing one as `Active`).
    /// `RunFinished` flips an existing run's status to a terminal
    /// variant matching the outcome — keeps the entry around so the
    /// UI can show "recently finished" runs without re-querying
    /// `ListRuns`.
    ///
    /// Future variants are tolerated by ignoring (the wire enum is
    /// `#[non_exhaustive]`).
    pub fn apply_global_event(&mut self, event: &GlobalDaemonEvent) {
        use surge_orchestrator::engine::handle::RunOutcome;

        match event {
            GlobalDaemonEvent::RunAccepted { run_id } => {
                self.run_streams.entry(*run_id).or_default().mark_started();
                if let Some(existing) = self.runs.iter_mut().find(|r| &r.run_id == run_id) {
                    existing.status = RunStatus::Active;
                } else {
                    self.runs.push(UiRun {
                        run_id: *run_id,
                        status: RunStatus::Active,
                        started_at: chrono::Utc::now(),
                        last_event_seq: None,
                        ended_at: None,
                    });
                }
            },
            GlobalDaemonEvent::RunFinished {
                run_id,
                outcome: RunOutcome::SettlementFailed { failures, .. },
            } => {
                tracing::warn!(%run_id, ?failures, "run completion unconfirmed; keeping current status");
                if let Some(stream) = self.run_streams.get_mut(run_id) {
                    stream.apply(
                        &surge_orchestrator::engine::handle::EngineRunEvent::StreamError {
                            message: format!("resource settlement failed: {failures:?}"),
                        },
                    );
                }
            },
            GlobalDaemonEvent::RunFinished { run_id, outcome } => {
                // `RunOutcome` is `#[non_exhaustive]`. For known variants
                // we map to the matching `RunStatus`. For an unknown
                // future variant we deliberately do NOT collapse to
                // `Aborted` — that would misrepresent e.g. a future
                // `TimedOut` outcome as user-cancelled. Instead, leave
                // the existing status untouched (typical case: it was
                // `Active`, so it stays `Active`; the UI will surface
                // the staleness when it next refreshes via `ListRuns`)
                // and emit a tracing::debug so the gap is visible.
                let new_status = match outcome {
                    RunOutcome::Completed { .. } => Some(RunStatus::Completed),
                    RunOutcome::Failed { .. } => Some(RunStatus::Failed),
                    RunOutcome::Aborted { .. } => Some(RunStatus::Aborted),
                    _ => {
                        tracing::debug!(
                            run_id = %run_id,
                            "RunFinished with unknown RunOutcome variant; keeping current status"
                        );
                        None
                    },
                };
                if let Some(existing) = self.runs.iter_mut().find(|r| &r.run_id == run_id) {
                    if let Some(status) = new_status {
                        existing.status = status;
                        existing.ended_at.get_or_insert_with(chrono::Utc::now);
                    }
                } else {
                    // Unknown run id AND an outcome that doesn't map to a
                    // known `RunStatus` (e.g. `RunOutcome::Parked`, Task 12
                    // — not a failure or cancellation, and this UI-facing
                    // `RunStatus` has no variant for it yet): the `_` arm
                    // three lines above already refused to guess `Aborted`
                    // for the *known*-run case for exactly this reason
                    // (review finding: `unwrap_or` here was quietly
                    // reversing that decision for the unknown-run case).
                    // `Active` is the same "least specific, least false"
                    // choice applied consistently — it does not claim the
                    // run failed or was cancelled, which `Aborted` would.
                    // Real `started_at` is unrecoverable here either way.
                    let stub_status = new_status.unwrap_or(RunStatus::Active);
                    self.runs.push(UiRun {
                        run_id: *run_id,
                        status: stub_status,
                        started_at: chrono::Utc::now(),
                        last_event_seq: None,
                        ended_at: Some(chrono::Utc::now()),
                    });
                }
            },
            _ => {
                // `GlobalDaemonEvent` is `#[non_exhaustive]`. Future
                // variants are silently dropped here; add cases as
                // they're introduced.
            },
        }
    }

    /// Replace the run list from a fresh `ListRuns` response.
    pub fn set_runs_from_summaries(&mut self, summaries: &[RunSummary]) {
        let old_ends: HashMap<RunId, chrono::DateTime<chrono::Utc>> = self
            .runs
            .iter()
            .filter_map(|r| r.ended_at.map(|t| (r.run_id, t)))
            .collect();
        // ListRuns describes the daemon's currently hosted runs, not durable
        // history. Keep terminal rows recovered from bootstrap or prior events.
        let history: Vec<_> = self
            .runs
            .iter()
            .filter(|run| {
                (run.is_terminal()
                    || self.bootstrap_operations.values().any(|operation| {
                        bootstrap_related_runs(operation)
                            .is_ok_and(|runs| runs.contains(&run.run_id))
                    }))
                    && !summaries.iter().any(|s| s.run_id == run.run_id)
            })
            .cloned()
            .collect();
        self.runs = summaries
            .iter()
            .map(|s| {
                let mut run = UiRun::from(s);
                // Keep the end stamp we observed earlier — a list refresh
                // must not un-freeze a finished run's elapsed time.
                if run.is_terminal() {
                    run.ended_at = old_ends.get(&run.run_id).copied();
                }
                run
            })
            .collect();
        self.runs.extend(history);
    }

    /// Whether `run_id` belongs to the open project. Runs whose
    /// `RunStarted` has not been folded yet are kept visible rather than
    /// hidden, so a just-accepted run never disappears from the UI.
    pub fn run_in_project(&self, run_id: &RunId) -> bool {
        let Some(scope) = &self.project_scope else {
            return true;
        };
        let Some(stream) = self.run_streams.get(run_id) else {
            return true;
        };
        scope
            .owns(stream.run_path.as_deref(), stream.git_common_dir.as_deref())
            .unwrap_or(true)
    }

    /// Runs of the open project, newest first. The daemon and the durable
    /// run registry are shared by every project, so screens must read
    /// this rather than [`AppState::runs`].
    pub fn project_runs(&self) -> Vec<&UiRun> {
        let mut runs: Vec<&UiRun> = self
            .runs
            .iter()
            .filter(|run| self.run_in_project(&run.run_id))
            .collect();
        runs.sort_by_key(|run| std::cmp::Reverse(run.started_at));
        runs
    }

    /// Decisions blocked on the operator in the open project: live gates
    /// and failed/aborted runs. One number for the Inbox badge and the
    /// Fleet chip so the two can never disagree.
    pub fn needs_you_count(&self) -> usize {
        self.pending_decisions().len()
            + self.unacknowledged_failures().len()
            + self.inspection_runs().len()
            + self.operation_problems().len()
            + self
                .operation_inspections
                .keys()
                .filter(|id| {
                    !self.bootstrap_operations.contains_key(id)
                        && self.operation_inspection_reason(**id).is_some()
                })
                .count()
    }

    /// Runs requiring inspection, including crash evidence absent from coarse lifecycle rows.
    pub fn inspection_runs(&self) -> Vec<(RunId, surge_core::run_display::RunDisplayState)> {
        self.run_streams
            .iter()
            .filter(|(id, _)| self.run_in_project(id))
            .filter_map(|(id, stream)| {
                let display = stream.display();
                // Existing exact requests already occupy the operator queue;
                // unknown display evidence must not duplicate or replace them.
                if display == surge_core::run_display::RunDisplayState::Unknown
                    && !stream.pending.is_empty()
                {
                    return None;
                }
                if self.bootstrap_operations.values().any(|operation| matches!(operation.state, surge_core::bootstrap_operation::BootstrapState::NeedsAttention { run_id: Some(run), .. } if run == *id) && self.operation_related_links(operation).contains(id)) { return None; }
                matches!(
                    display,
                    surge_core::run_display::RunDisplayState::Unknown
                        | surge_core::run_display::RunDisplayState::Waiting(
                            surge_core::run_display::WaitingReason::RecoveryRequired
                        )
                )
                .then_some((*id, display))
            })
            .collect()
    }

    /// Failed/aborted runs of the open project the operator has not
    /// dismissed yet — the Inbox's failure-triage queue.
    pub fn unacknowledged_failures(&self) -> Vec<&UiRun> {
        self.project_runs()
            .into_iter()
            .filter(|r| self.failure_terminal(r.run_id).is_some())
            .filter(|r| !self.dismissed_runs.contains(&r.run_id))
            .collect()
    }

    pub(crate) fn failure_terminal(&self, run: RunId) -> Option<surge_core::TerminalReason> {
        use surge_core::{TerminalReason, run_display::RunDisplayState};
        match self.run_streams.get(&run)?.display() {
            RunDisplayState::Done(kind @ (TerminalReason::Failed | TerminalReason::Aborted)) => {
                Some(kind)
            },
            _ => None,
        }
    }

    pub(crate) fn operation_related_links(
        &self,
        operation: &surge_core::bootstrap_operation::BootstrapOperationStatus,
    ) -> Vec<RunId> {
        [operation.planning_run, operation.implementation_run]
            .into_iter()
            .filter(|run| {
                self.run_in_project(run)
                    && self
                        .run_streams
                        .get(run)
                        .is_some_and(|stream| stream.session_history_confirmed())
            })
            .collect()
    }
    pub(crate) fn run_inspection_evidence(&self, run: RunId) -> Vec<(String, String)> {
        let Some(stream) = self.run_streams.get(&run) else {
            return Vec::new();
        };
        let display = stream.display();
        let mut facts = vec![
            ("Run state".into(), display.label().into()),
            (
                "Next action".into(),
                display.next_action().unwrap_or_default().into(),
            ),
        ];
        if let Some(error) = &stream.observation_error {
            facts.push(("Run observation".into(), error.clone()));
        }
        if stream.current_registry_hint() == Some(surge_core::RunStatus::Crashed)
            && !matches!(display, surge_core::run_display::RunDisplayState::Done(_))
        {
            facts.push((
                "Recorded host loss".into(),
                "Recorded loss of daemon ownership; current run outcome is unconfirmed".into(),
            ));
        }
        if let Some((token, reason)) = self.rejected_inspections.get(&run)
            && self.observations.last_read_is_current(token)
        {
            facts.push(("History recovery".into(), reason.clone()));
        }
        let mut selected = Vec::new();
        for (label, text) in facts {
            if !text.is_empty() && !selected.iter().any(|(_, previous)| previous == &text) {
                selected.push((label, text));
            }
        }
        selected
    }

    pub(crate) fn operation_problems(
        &self,
    ) -> Vec<&surge_core::bootstrap_operation::BootstrapOperationStatus> {
        use surge_core::{
            TerminalReason,
            bootstrap_continuation::{BootstrapFailure as F, BootstrapTerminal as T},
            bootstrap_operation::BootstrapState as S,
        };
        self.bootstrap_operations.values().filter(|operation| {
            if self.operation_inspection_reason(operation.operation_id).is_some() { return true; }
            match &operation.state {
                S::NeedsAttention { .. } => true,
                S::Failed => !matches!(&operation.result,
                    Some(T::Failed { run_id, reason: F::RunFailed }) if self.failure_terminal(*run_id) == Some(TerminalReason::Failed))
                    && !matches!(&operation.result,
                    Some(T::Failed { run_id, reason: F::RunAborted }) if self.failure_terminal(*run_id) == Some(TerminalReason::Aborted)),
                _ => false,
            }
        }).collect()
    }

    pub(crate) fn observation_scope(&mut self) -> Option<crate::run_observation::Scope> {
        let home = self.runtime_home.clone()?;
        let project = self
            .project_scope
            .as_ref()
            .map(|scope| scope.root().to_path_buf())
            .or_else(|| self.project_path.clone())?;
        let project = project.canonicalize().unwrap_or(project);
        let repository = self
            .project_scope
            .as_ref()
            .and_then(|scope| scope.repository().map(std::path::Path::to_path_buf));
        Some(self.observations.scope(home, project, repository))
    }

    pub(crate) fn accepts_observation(
        &mut self,
        token: &crate::run_observation::Token,
        read: bool,
    ) -> bool {
        let Some(scope) = self.observation_scope() else {
            return false;
        };
        scope == token.scope
            && if read {
                self.observations.accepts_read(token)
            } else {
                self.observations.accepts(token)
            }
    }

    pub(crate) fn begin_history_read(
        &mut self,
        owner: &crate::run_observation::Token,
    ) -> Option<crate::run_observation::Token> {
        if !self.accepts_observation(owner, false) {
            return None;
        }
        let error_revision = self
            .run_streams
            .get(&owner.run)
            .map_or(0, crate::run_stream::RunStreamState::observation_revision);
        self.observations.begin_read(owner, error_revision)
    }

    pub(crate) fn retire_history_read(
        &mut self,
        token: &crate::run_observation::Token,
    ) -> ReadCompletion {
        let mut completion = ReadCompletion::obsolete(token);
        if self.accepts_observation(token, true) {
            self.observations.finish_read(token, false);
            completion.disposition = ReadDisposition::Retired;
        }
        completion
    }

    pub(crate) fn complete_history_read(
        &mut self,
        token: &crate::run_observation::Token,
        result: Result<Option<Vec<surge_persistence::runs::ReadEvent>>, String>,
    ) -> ReadCompletion {
        let mut completion = ReadCompletion {
            token: token.clone(),
            disposition: ReadDisposition::Obsolete,
            diagnostic: None,
        };
        if !self.accepts_observation(token, true) {
            return completion;
        }
        let prior_revision = self
            .run_streams
            .get(&token.run)
            .map_or(0, crate::run_stream::RunStreamState::observation_revision);
        let read_error = result
            .as_ref()
            .err()
            .map(|error| surge_core::recovery_observation::recovery_diagnostic(error));
        completion.disposition = match result {
            Ok(Some(events)) if self.publish_history(token, &events) => ReadDisposition::Confirmed,
            Ok(Some(_)) => ReadDisposition::Incomplete,
            Ok(None) => ReadDisposition::Absent,
            Err(message) => {
                if let Some(stream) = self.run_streams.get_mut(&token.run) {
                    stream.history_unconfirmed_if_unchanged(&message, token.error_revision);
                }
                ReadDisposition::Incomplete
            },
        };
        if let Some(stream) = self.run_streams.get_mut(&token.run) {
            if completion.confirmed() {
                stream.clear_observation_error_if_unchanged(token.error_revision);
            }
            if completion.disposition == ReadDisposition::Incomplete {
                let revision = stream.observation_revision();
                // A read-owned publication advances the stream revision; the captured
                // token remains the original last-read identity after finish_read.
                if prior_revision == token.error_revision
                    && revision == token.error_revision.saturating_add(1)
                    && token.error_revision != u64::MAX
                {
                    completion.diagnostic = stream
                        .observation_error
                        .clone()
                        .map(|error| (revision, error));
                } else if revision == token.error_revision && stream.observation_error.is_none() {
                    completion.diagnostic = Some((
                        revision,
                        "Related run history is incomplete; inspection retry required".into(),
                    ));
                }
            }
        }
        if completion.disposition == ReadDisposition::Incomplete
            && !self.run_streams.contains_key(&token.run)
        {
            completion.diagnostic = Some((
                0,
                read_error.unwrap_or_else(|| {
                    "Related run history is incomplete; inspection retry required".into()
                }),
            ));
        }
        let terminal = completion.confirmed()
            && matches!(
                self.run_streams
                    .get(&token.run)
                    .map(|stream| stream.display()),
                Some(surge_core::run_display::RunDisplayState::Done(_))
            );
        self.observations.finish_read(token, terminal);
        completion
    }

    pub(crate) fn set_operation_refusal(&mut self, operation: RunId, reason: String) {
        self.operation_inspections
            .entry(operation)
            .or_default()
            .record = Some(reason);
    }
    pub(crate) fn confirm_operation_record(&mut self, operation: RunId) {
        if let Some(inspection) = self.operation_inspections.get_mut(&operation) {
            inspection.record = None;
        }
        self.prune_operation_inspections();
    }
    fn prune_operation_inspections(&mut self) {
        self.operation_inspections.retain(|_, inspection| {
            inspection.record.is_some() || !inspection.histories.is_empty()
        });
    }
    pub(crate) fn complete_related_read(
        &mut self,
        operation: RunId,
        token: &crate::run_observation::Token,
        result: Result<Option<Vec<surge_persistence::runs::ReadEvent>>, String>,
        expected_started: bool,
    ) -> ReadCompletion {
        let result = if expected_started && matches!(result, Ok(None)) {
            Err("Expected related run journal is absent".into())
        } else {
            result
        };
        let completion = self.complete_history_read(token, result);
        match completion.disposition {
            ReadDisposition::Confirmed | ReadDisposition::Absent => {
                if let Some(inspection) = self.operation_inspections.get_mut(&operation) {
                    inspection.histories.remove(&token.run);
                }
                self.prune_operation_inspections();
            },
            ReadDisposition::Incomplete => {
                if let Some((revision, reason)) = &completion.diagnostic {
                    self.operation_inspections
                        .entry(operation)
                        .or_default()
                        .histories
                        .insert(
                            token.run,
                            HistoryInspection {
                                token: token.clone(),
                                error_revision: *revision,
                                reason: reason.clone(),
                                exhausted: false,
                            },
                        );
                }
            },
            ReadDisposition::Obsolete | ReadDisposition::Retired => {},
        }
        completion
    }
    pub(crate) fn exhaust_related_retries(
        &mut self,
        operation: RunId,
        tokens: &[crate::run_observation::Token],
    ) {
        if let Some(inspection) = self.operation_inspections.get_mut(&operation) {
            for token in tokens {
                if self.observations.last_read_is_current(token)
                    && let Some(history) = inspection.histories.get_mut(&token.run)
                    && history.token == *token
                {
                    history.exhausted = true;
                }
            }
        }
    }
    pub(crate) fn operation_inspection_reason(&self, operation: RunId) -> Option<String> {
        let inspection = self.operation_inspections.get(&operation)?;
        let mut reasons: Vec<_> = inspection
            .histories
            .values()
            .filter(|history| {
                self.observations.last_read_is_current(&history.token)
                    && self
                        .run_streams
                        .get(&history.token.run)
                        .map_or(history.error_revision == 0, |stream| {
                            stream.observation_revision() == history.error_revision
                        })
            })
            .map(|history| {
                (
                    history.token.run,
                    format!(
                        "{}{}",
                        history.reason,
                        if history.exhausted {
                            "; inspection retries exhausted"
                        } else {
                            ""
                        }
                    ),
                )
            })
            .collect();
        reasons.sort_by_key(|(run, _)| run.to_string());
        let mut selected: Vec<_> = inspection.record.iter().cloned().collect();
        selected.extend(reasons.into_iter().map(|(_, reason)| reason));
        (!selected.is_empty()).then(|| selected.join("; "))
    }

    pub(crate) fn publish_history(
        &mut self,
        token: &crate::run_observation::Token,
        events: &[surge_persistence::runs::ReadEvent],
    ) -> bool {
        if !self.accepts_observation(token, true) {
            return false;
        }
        let candidate = crate::run_stream::RunStreamState::replay_candidate(token.run, events);
        let result = self
            .run_streams
            .entry(token.run)
            .or_default()
            .publish_candidate(candidate);
        match result {
            Err("older prefix") => return false,
            Err(message) => {
                let stream = self.run_streams.entry(token.run).or_default();
                stream.invalidate_display();
                stream.history_unconfirmed_if_unchanged(message, token.error_revision);
                return false;
            },
            Ok(()) => {},
        }
        let Some(first) = events.first().filter(|event| {
            event.seq.as_u64() == 1
                && matches!(
                    event.payload.payload,
                    surge_core::EventPayload::RunStarted { .. }
                )
        }) else {
            return false;
        };
        let Some(started_at) = chrono::DateTime::from_timestamp_millis(first.timestamp_ms) else {
            return false;
        };
        let display = self.run_streams[&token.run].display();
        let status = match display {
            surge_core::run_display::RunDisplayState::Done(
                surge_core::TerminalReason::Completed,
            ) => RunStatus::Completed,
            surge_core::run_display::RunDisplayState::Done(surge_core::TerminalReason::Failed) => {
                RunStatus::Failed
            },
            surge_core::run_display::RunDisplayState::Done(surge_core::TerminalReason::Aborted) => {
                RunStatus::Aborted
            },
            _ => RunStatus::Active,
        };
        let verdict = surge_core::run_verdict::TerminalVerdict::of(
            events
                .iter()
                .map(|event| (event.seq.as_u64(), &event.payload.payload)),
        );
        let ended_at = verdict
            .terminal()
            .and_then(|claim| events.iter().find(|event| event.seq.as_u64() == claim.seq))
            .and_then(|event| chrono::DateTime::from_timestamp_millis(event.timestamp_ms));
        let row = UiRun {
            run_id: token.run,
            status,
            started_at,
            ended_at,
            last_event_seq: events.last().map(|event| event.seq.as_u64()),
        };
        if let Some(existing) = self.runs.iter_mut().find(|run| run.run_id == token.run) {
            *existing = row;
        } else {
            self.runs.push(row);
        }
        true
    }

    /// Operator's request for a run, when its `RunStarted` was observed.
    pub fn run_prompt(&self, run_id: &RunId) -> Option<&str> {
        self.run_streams.get(run_id)?.prompt.as_deref()
    }

    /// Human-facing mission label; the recorded agent prompt remains unchanged.
    pub fn run_mission_title(&self, run_id: &RunId) -> Option<&str> {
        let stream = self.run_streams.get(run_id)?;
        let title = stream.trusted_work_item().and_then(|item| {
            let scope = self.tasks.scope.as_ref()?;
            if self.project_scope.as_ref().is_some_and(|project| {
                project.owns(stream.run_path.as_deref(), stream.git_common_dir.as_deref())
                    != Some(true)
            }) {
                return None;
            }
            self.tasks.records.iter().find_map(|record| {
                (record.id == item
                    && record.workspace.repository == scope.repository
                    && stream.git_common_dir.as_ref() == Some(&scope.repository))
                .then_some(record.title.as_str())
            })
        });
        title.or(stream.prompt.as_deref())
    }

    /// Load project from a directory path.
    pub fn load_project(&mut self, path: &std::path::Path) {
        self.observations.clear();
        self.bootstrap_operations.clear();
        self.operation_inspections.clear();
        self.rejected_inspections.clear();
        self.tasks.clear_project();
        self.project_history.clear();
        self.config = None;
        self.agent_pool = None;
        self.project_load_error = None;
        self.project_path = Some(path.to_path_buf());
        self.project_scope = Some(crate::project::ProjectScope::resolve(path));
        self.project_name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "project".to_string());

        // Try loading surge.toml config and create AgentPool. The pool is
        // built from the unified catalog (user `[agents.*]` over builtins),
        // so a builtin provider like ollama-acp is usable from the desktop
        // chat without first being copied into surge.toml.
        let config_path = path.join("surge.toml");
        let config_absent = match std::fs::symlink_metadata(&config_path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
            metadata => {
                let loaded = metadata
                    .map_err(|error| surge_core::SurgeError::Config(error.to_string()))
                    .and_then(|_| load_project_runtime(path));
                match loaded {
                    Ok((config, pool)) => {
                        self.config = Some(config);
                        self.agent_pool = Some(Arc::new(pool));
                    },
                    Err(error) => self.record_project_load_error(error),
                }
                false
            },
        };

        // Re-detect installed agents (might have changed).
        self.installed_agents = self.registry.detect_installed_with_paths();

        // If no pool yet (no surge.toml), create one from installed agents.
        if config_absent && !self.installed_agents.is_empty() {
            let mut agents = std::collections::HashMap::new();
            let mut default_agent = String::new();
            for detected in &self.installed_agents {
                let config = detected.entry.to_agent_config();
                if default_agent.is_empty() {
                    default_agent = detected.entry.id.clone();
                }
                agents.insert(detected.entry.id.clone(), config);
            }
            match AgentPool::new(
                agents,
                default_agent,
                path.to_path_buf(),
                PermissionPolicy::default(),
                surge_core::config::ResilienceConfig::default(),
            ) {
                Ok(pool) => self.agent_pool = Some(Arc::new(pool)),
                Err(error) => self.record_project_load_error(error),
            }
        }

        // Try detecting current git branch.
        self.current_branch = detect_branch(path).unwrap_or_else(|| "main".to_string());
    }

    fn record_project_load_error(&mut self, error: surge_core::SurgeError) {
        let reason = match error {
            surge_core::SurgeError::Config(_) => "configuration could not be read or validated",
            surge_core::SurgeError::AgentConnection(_) => "agent runtime could not be initialized",
            _ => "project runtime could not be loaded",
        };
        let diagnostic =
            format!("Project {reason}. Check surge.toml and reload configuration in Settings.");
        tracing::warn!(project = ?self.project_path, reason, "Project runtime unavailable");
        self.project_load_error = Some(diagnostic);
    }

    /// Update config in-place and save to disk.
    ///
    /// Validates + persists BEFORE replacing the in-memory config so a
    /// validation or IO failure leaves the UI holding the previous,
    /// known-good state instead of an invalid one that was never
    /// written. Requires `project_path` to be set — otherwise there is
    /// no place to save and silently mutating in-memory only would
    /// diverge from disk.
    pub fn update_config(&mut self, config: SurgeConfig) -> Result<(), surge_core::SurgeError> {
        if self.project_load_error.is_some() {
            return Err(surge_core::SurgeError::Config(
                "Check surge.toml and reload configuration in Settings before saving changes."
                    .into(),
            ));
        }
        let project_path = self.project_path.as_ref().ok_or_else(|| {
            surge_core::SurgeError::Config(
                "No project loaded; cannot save config without project_path".into(),
            )
        })?;
        config.save(&project_path.join("surge.toml"))?;
        self.config = Some(config);
        Ok(())
    }

    // ── Computed accessors ──

    /// Registry entries NOT installed (for Available tab).
    pub fn available_agents(&self) -> Vec<&RegistryEntry> {
        let installed_ids: Vec<&str> = self
            .installed_agents
            .iter()
            .map(|a| a.entry.id.as_str())
            .collect();
        self.registry
            .list()
            .iter()
            .filter(|e| !installed_ids.contains(&e.id.as_str()))
            .collect()
    }

    /// Health for a specific agent — the single way UI code should read
    /// agent health; screens must not touch `agent_pool`'s tracker or
    /// `fallback_health` directly.
    ///
    /// Reads from `AgentPool`'s live tracker (the one its workers actually
    /// call `record_success`/`record_failure` against) whenever a pool
    /// exists; falls back to this `AppState`'s own registration-only
    /// tracker only when it does not (agents detected but no
    /// `surge.toml`/pool created yet — that path must keep working). A
    /// pool that exists but whose tracker lock cannot be taken right now
    /// (contended — never actually poisoned, `tokio::sync::Mutex` doesn't
    /// poison, but treated the same way regardless) degrades to `None`
    /// ("unknown" health) rather than falling back to the fallback
    /// tracker, which would silently show stale/fabricated zeros, or
    /// blocking a synchronous render frame on an async lock.
    #[must_use]
    pub fn agent_health(&self, name: &str) -> Option<AgentHealth> {
        match &self.agent_pool {
            Some(pool) => pool.try_health(name),
            None => self.fallback_health.get_health(name).cloned(),
        }
    }

    /// All pending operator decisions across live run streams, most
    /// urgent first (kind rank, then age). Powers the Inbox and the
    /// "needs you" counters.
    pub fn pending_decisions(&self) -> Vec<(RunId, crate::run_stream::PendingDecision)> {
        let mut all: Vec<(RunId, crate::run_stream::PendingDecision)> = self
            .run_streams
            .iter()
            .filter(|(run_id, _)| self.run_in_project(run_id))
            .flat_map(|(run_id, stream)| stream.pending.iter().map(move |p| (*run_id, p.clone())))
            .collect();
        // Deterministic total order: run_streams is a HashMap, and a
        // (rank, seq) tie across runs would otherwise leak its random
        // iteration order into the Inbox — while the Inbox selects by
        // index, so a between-frame reorder could retarget a decision.
        all.sort_by(|(ra, pa), (rb, pb)| {
            (pa.rank(), pa.seq, ra.to_string()).cmp(&(pb.rank(), pb.seq, rb.to_string()))
        });
        all
    }
}

/// Load configuration and its pool together so failures cannot leave a partial runtime.
fn load_project_runtime(
    path: &std::path::Path,
) -> Result<(SurgeConfig, AgentPool), surge_core::SurgeError> {
    let config = SurgeConfig::load(&path.join("surge.toml"))?;
    let registry = Registry::for_run(&config);
    let default_agent = registry
        .normalize_agent_id(&config.default_agent)
        .unwrap_or_else(|| config.default_agent.clone());
    let pool = AgentPool::new(
        registry.agent_configs(),
        default_agent,
        path.to_path_buf(),
        PermissionPolicy::default(),
        config.resilience.clone(),
    )?;
    Ok((config, pool))
}

/// Detect current git branch name from a path.
fn detect_branch(path: &std::path::Path) -> Option<String> {
    let head_file = path.join(".git").join("HEAD");
    if let Ok(content) = std::fs::read_to_string(head_file)
        && let Some(ref_str) = content.strip_prefix("ref: refs/heads/")
    {
        return Some(ref_str.trim().to_string());
    }
    None
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use surge_core::config::{AgentConfig, ResilienceConfig, Transport};

    use super::*;

    #[test]
    fn settlement_failure_cannot_mark_existing_or_unknown_run_finished() {
        use surge_orchestrator::engine::handle::{RunOutcome, RunSettlementFailure};
        let mut state = AppState::new();
        let known = RunId::new();
        state.apply_global_event(&GlobalDaemonEvent::RunAccepted { run_id: known });
        for run_id in [known, RunId::new()] {
            state.apply_global_event(&GlobalDaemonEvent::RunFinished {
                run_id,
                outcome: RunOutcome::SettlementFailed {
                    original: Box::new(RunOutcome::Completed {
                        terminal: "end".try_into().unwrap(),
                    }),
                    failures: vec![RunSettlementFailure::WriterSeal],
                },
            });
        }
        assert_eq!(state.runs.len(), 1);
        assert_eq!(state.runs[0].status, RunStatus::Active);
        assert!(state.runs[0].ended_at.is_none());
    }

    fn project_config(path: &std::path::Path, agent: &str, budget: f64) {
        let mut config = SurgeConfig {
            default_agent: agent.into(),
            ..SurgeConfig::default()
        };
        let mut runtime = test_agent_config();
        runtime.command = format!("{agent}-command");
        config.agents.insert(agent.into(), runtime);
        config.analytics.budget_usd = Some(budget);
        config
            .save(&path.join("surge.toml"))
            .expect("valid fixture");
    }

    #[test]
    fn project_switch_drops_config_and_pool_when_config_missing_or_invalid() {
        let first = tempfile::tempdir().expect("first project");
        let second = tempfile::tempdir().expect("second project");
        project_config(first.path(), "project-a", 17.0);
        let mut state = AppState::new();
        state.registry = Registry::empty();
        for invalid in [false, true] {
            state.load_project(first.path());
            assert_eq!(
                state.config.as_ref().unwrap().analytics.budget_usd,
                Some(17.0)
            );
            assert_eq!(
                state.agent_pool.as_ref().unwrap().default_agent(),
                "project-a"
            );
            if invalid {
                std::fs::write(second.path().join("surge.toml"), "[invalid").unwrap();
            }
            state.load_project(second.path());
            assert_eq!(state.project_path.as_deref(), Some(second.path()));
            assert!(
                state.config.is_none(),
                "previous project config must be dropped"
            );
            assert!(
                state.agent_pool.is_none(),
                "previous project pool must be dropped"
            );
            assert_eq!(state.project_load_error.is_some(), invalid);
        }
    }

    #[test]
    fn project_switch_replaces_valid_config_and_pool() {
        let first = tempfile::tempdir().unwrap();
        let third = tempfile::tempdir().unwrap();
        project_config(first.path(), "project-a", 17.0);
        project_config(third.path(), "project-c", 31.0);
        let mut state = AppState::new();
        state.registry = Registry::empty();
        state.load_project(first.path());
        let first_pool = Arc::clone(state.agent_pool.as_ref().unwrap());
        state.load_project(third.path());
        assert_eq!(state.project_path.as_deref(), Some(third.path()));
        let config = state.config.as_ref().unwrap();
        assert_eq!(config.default_agent, "project-c");
        assert_eq!(config.analytics.budget_usd, Some(31.0));
        assert_eq!(config.agents["project-c"].command, "project-c-command");
        assert_eq!(config.agents["project-c"].args, ["test"]);
        let pool = state.agent_pool.as_ref().unwrap();
        assert_eq!(pool.default_agent(), "project-c");
        assert!(!Arc::ptr_eq(&first_pool, pool));
        assert!(state.project_load_error.is_none());
    }

    #[test]
    fn project_switch_unreadable_config_blocks_installed_fallback_and_recovers() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        project_config(first.path(), "project-a", 17.0);
        let mut state = AppState::new();
        let mut installed_config = test_agent_config();
        installed_config.command = std::env::current_exe().unwrap().display().to_string();
        state.registry =
            Registry::from_config(HashMap::from([("installed-test".into(), installed_config)]));
        state.load_project(first.path());
        let first_pool = Arc::clone(state.agent_pool.as_ref().unwrap());
        std::fs::create_dir(second.path().join("surge.toml")).unwrap();
        state.load_project(second.path());
        assert!(!state.installed_agents.is_empty(), "fixture is installed");
        assert!(state.config.is_none());
        assert!(state.agent_pool.is_none(), "read failure forbids fallback");
        assert!(state.project_load_error.is_some());
        std::fs::remove_dir(second.path().join("surge.toml")).unwrap();
        state.load_project(second.path());
        assert!(state.config.is_none());
        assert!(state.project_load_error.is_none());
        let fallback_pool = state.agent_pool.as_ref().unwrap();
        assert_eq!(fallback_pool.default_agent(), "installed-test");
        assert!(!Arc::ptr_eq(&first_pool, fallback_pool));
        assert_eq!(state.project_path.as_deref(), Some(second.path()));
    }

    #[test]
    fn project_load_diagnostic_does_not_echo_configuration_values() {
        let project = tempfile::tempdir().unwrap();
        let sensitive = "sensitive-runtime-value";
        std::fs::write(
            project.path().join("surge.toml"),
            format!("[pipeline]\nmax_parallel = \"{sensitive}\"\n"),
        )
        .unwrap();
        let mut state = AppState::new();
        state.registry = Registry::empty();
        state.load_project(project.path());
        let diagnostic = state.project_load_error.as_deref().unwrap();
        assert!(!diagnostic.contains(sensitive));
        assert!(diagnostic.contains("reload configuration"));
        assert!(state.config.is_none());
        assert!(state.agent_pool.is_none());
    }

    #[test]
    fn update_config_preserves_a_failed_project_file_until_reload() {
        let project = tempfile::tempdir().unwrap();
        let path = project.path().join("surge.toml");
        let broken = "[broken";
        std::fs::write(&path, broken).unwrap();
        let mut state = AppState::new();
        state.registry = Registry::empty();
        state.load_project(project.path());
        assert!(state.project_load_error.is_some());
        assert!(state.update_config(SurgeConfig::default()).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), broken);
        assert!(state.config.is_none());
        std::fs::remove_file(&path).unwrap();
        state.load_project(project.path());
        assert!(state.project_load_error.is_none());
        assert!(state.update_config(SurgeConfig::default()).is_ok());
        assert!(SurgeConfig::load(&path).is_ok());
    }

    #[test]
    fn active_run_refresh_preserves_finished_history() {
        let mut state = AppState::new();
        let completed = RunId::new();
        let failed = RunId::new();
        let active = RunId::new();
        let ended = chrono::Utc::now();
        for (run_id, status) in [
            (completed, RunStatus::Completed),
            (failed, RunStatus::Failed),
            (active, RunStatus::Active),
        ] {
            state.runs.push(UiRun {
                run_id,
                status,
                started_at: ended,
                last_event_seq: Some(12),
                ended_at: (run_id != active).then_some(ended),
            });
        }
        state.set_runs_from_summaries(&[]);
        assert_eq!(state.runs.len(), 2);
        assert!(
            state
                .runs
                .iter()
                .all(|run| run.is_terminal() && run.ended_at == Some(ended))
        );
        state.set_runs_from_summaries(&[RunSummary::queued(active, ended)]);
        assert_eq!(state.runs.len(), 3);
        state.set_runs_from_summaries(&[RunSummary::queued(active, ended)]);
        assert_eq!(state.runs.len(), 3, "refresh must not duplicate history");
    }

    fn test_agent_config() -> AgentConfig {
        AgentConfig {
            command: "echo".to_string(),
            args: vec!["test".to_string()],
            transport: Transport::Stdio,
            mcp_servers: vec![],
            capabilities: vec![],
            env: std::collections::BTreeMap::new(),
            settings_files: vec![],
            capacity_route: None,
        }
    }

    /// The defect this fix targets: `AppState` used to own a second,
    /// never-written-to `HealthTracker` and read that one unconditionally,
    /// so the Agents screen showed permanent zero requests / zero errors
    /// no matter what the agent actually did. `agent_health` must read
    /// the pool's live tracker — the one `AgentPool`'s workers actually
    /// call `record_success`/`record_failure` against — whenever a pool
    /// exists.
    #[test]
    fn agent_health_reads_the_pool_tracker_when_a_pool_exists() {
        let mut state = AppState::new();

        let mut configs = HashMap::new();
        configs.insert("test-agent".to_string(), test_agent_config());
        let pool = AgentPool::new(
            configs,
            "test-agent".to_string(),
            PathBuf::from("/tmp/test"),
            PermissionPolicy::default(),
            ResilienceConfig::default(),
        )
        .expect("valid default agent");

        pool.health()
            .try_lock()
            .expect("uncontended in a single-threaded test")
            .record_success("test-agent", Duration::from_millis(75));

        state.agent_pool = Some(Arc::new(pool));

        // The fallback tracker never saw this agent at all — proves the
        // read below is not coming from it.
        assert!(state.fallback_health.get_health("test-agent").is_none());

        let health = state
            .agent_health("test-agent")
            .expect("pool recorded a request for this agent");
        assert_eq!(health.total_requests, 1);
    }

    /// The no-pool path (agents detected, but no `surge.toml`/pool created
    /// yet) must keep working, reading the registration-only fallback
    /// tracker instead of panicking or fabricating data.
    #[test]
    fn agent_health_falls_back_to_the_local_tracker_without_a_pool() {
        let mut state = AppState::new();
        assert!(state.agent_pool.is_none());
        state.fallback_health.register("claude");

        let health = state
            .agent_health("claude")
            .expect("registered in the fallback tracker");
        assert_eq!(health.total_requests, 0);
        assert_eq!(health.status(), surge_acp::HealthStatus::Healthy);
    }

    /// Neither path fabricates health for an agent nobody registered
    /// anywhere.
    #[test]
    fn agent_health_is_none_for_an_unregistered_agent_without_a_pool() {
        let state = AppState::new();
        assert!(state.agent_health("never-heard-of-it").is_none());
    }
}
