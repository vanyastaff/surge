//! Missions — one idea, planned and built (rail · overview ·
//! changes · checks · log · steer). A mission is a planning run plus the
//! implementation run it launched; the engine-level runs stay visible in
//! Log and in the run IDs.
//!
//! Adapted from the "Surge - Interactive" concept. Everything rendered
//! from live data is real: the run rail and KPI strip come from
//! [`AppState::runs`] (daemon `list_runs` + global events), **Stop run**
//! calls `EngineFacade::stop_run`, and the steer bar queues a real
//! operator message via `EngineFacade::submit_steer`.
//!
//! Live streams and restored durable events provide stage and event details.
//! Runs without recorded stage events show the known lifecycle only. The result
//! folder action reads that run's registry path and opens it through the platform.
//! When no runs exist, the cockpit stays empty and points to Fleet.

#[path = "runs/recorded_target.rs"]
mod recorded_target;
#[path = "run_changes.rs"]
mod run_changes;
#[path = "run_checks.rs"]
mod run_checks;
#[path = "run_mission.rs"]
mod run_mission;
pub(crate) use recorded_target::RecordedTarget;

use std::{collections::HashMap, path::PathBuf, time::Duration};

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Disableable, Icon, IconName, Sizable, StyledExt};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use surge_core::id::RunId;
use surge_orchestrator::engine::facade::EngineFacade as _;
use surge_orchestrator::engine::handle::RunStatus;

use crate::app_state::{AppState, UiRun};
use crate::theme;
use crate::ui;

fn continuation_label(state: surge_core::bootstrap_attempt::BootstrapAttemptState) -> &'static str {
    use surge_core::bootstrap_attempt::BootstrapAttemptState as A;
    match state {
        A::Reserved => "creation pending",
        A::ForkReady => "ready",
        A::Running => "running",
        A::NeedsAttention => "needs attention",
        A::Completed => "completed",
        A::Failed => "failed",
    }
}

/// One stage chip in the pipeline strip.
#[derive(Clone, Copy, PartialEq)]
enum StageState {
    Done,
    Running,
    Failed,
    Queued,
    /// Entered, but the run ended without an exit event for it — shown as
    /// such, never as still running.
    Unfinished,
}

impl StageState {
    fn color(self) -> Hsla {
        match self {
            Self::Done => theme::success(),
            Self::Running => theme::accent(),
            Self::Failed => theme::error(),
            Self::Queued | Self::Unfinished => theme::slate(),
        }
    }
}

#[derive(Clone)]
struct Stage {
    label: String,
    sub: String,
    state: StageState,
}

/// One event-log row.
#[derive(Clone)]
struct EventRow {
    t: String,
    kind: &'static str,
    color: Hsla,
    text: String,
    problem: bool,
}

/// Event log filter.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum LogFilter {
    All,
    Flow,
    Agent,
    You,
    Problems,
}

impl LogFilter {
    const ALL: [LogFilter; 5] = [
        Self::All,
        Self::Flow,
        Self::Agent,
        Self::You,
        Self::Problems,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Flow => "Steps",
            Self::Agent => "Agent",
            Self::You => "You",
            Self::Problems => "Problems",
        }
    }

    fn keeps(self, row: &EventRow) -> bool {
        match self {
            Self::All => true,
            Self::Flow => matches!(row.kind, "STAGE" | "EDGE" | "OUTCOME" | "END"),
            Self::Agent => matches!(row.kind, "AGENT" | "TOOL" | "ARTIFACT"),
            Self::You => matches!(row.kind, "GATE" | "STEER"),
            Self::Problems => row.problem,
        }
    }
}

/// Map a folded live stage (from the per-run stream) to a pipeline chip.
fn stage_from_stream(row: &crate::run_stream::StageRow) -> Stage {
    use crate::run_stream::StagePhase;
    let state = match row.phase {
        StagePhase::Running => StageState::Running,
        StagePhase::Done => StageState::Done,
        StagePhase::Failed => StageState::Failed,
    };
    Stage {
        label: row.node.clone(),
        sub: row.detail.clone(),
        state,
    }
}

/// Row model for the left rail and detail header, built from a real [`UiRun`].
#[derive(Clone)]
struct RunRow {
    /// Real daemon run id.
    run_id: Option<RunId>,
    id_label: String,
    title: String,
    age: String,
    status_label: &'static str,
    waiting_detail: Option<String>,
    color: Hsla,
    active: bool,
    /// 0 = needs-you first, then active, then the rest (rail ordering).
    rank: u8,
    started: String,
    elapsed: String,
    events: String,
    stages: Vec<Stage>,
    event_rows: Vec<EventRow>,
    /// Live per-run stream attached (real telemetry below).
    live_stream: bool,
    /// Known subtotals and completeness accumulated from the stream.
    usage: Option<RunUsage>,
}

#[derive(Clone, Copy)]
struct RunUsage {
    tokens_in: u64,
    tokens_out: u64,
    known_cost_usd: f64,
    usage_unknown: bool,
    cost_unknown: bool,
}

impl RunUsage {
    fn from_stream(stream: &crate::run_stream::RunStreamState) -> Self {
        Self {
            tokens_in: stream.tokens_in,
            tokens_out: stream.tokens_out,
            known_cost_usd: stream.cost_usd,
            usage_unknown: stream.usage_unknown,
            cost_unknown: stream.cost_unknown,
        }
    }

    fn token_label(self) -> String {
        if self.usage_unknown && self.tokens_in == 0 && self.tokens_out == 0 {
            return "token spend unavailable".into();
        }
        format!(
            "{} in · {} out tokens{}",
            fmt_tokens(self.tokens_in),
            fmt_tokens(self.tokens_out),
            if self.usage_unknown {
                " known · total unavailable"
            } else {
                ""
            }
        )
    }

    fn cost_label(self) -> String {
        if self.cost_unknown && self.known_cost_usd == 0.0 {
            "cost unavailable".into()
        } else {
            format!(
                "${:.2}{}",
                self.known_cost_usd,
                if self.cost_unknown {
                    " known · total unavailable"
                } else {
                    ""
                }
            )
        }
    }
}

fn status_parts(status: RunStatus) -> (&'static str, Hsla, bool, u8) {
    match status {
        RunStatus::Active => ("active", theme::accent(), true, 1),
        RunStatus::Awaiting => ("queued", theme::slate(), false, 2),
        RunStatus::Completed => ("completed", theme::success(), false, 3),
        RunStatus::Failed => ("failed", theme::error(), false, 0),
        RunStatus::Aborted => ("aborted", theme::error(), false, 0),
        _ => ("unknown", theme::slate(), false, 2),
    }
}

fn humanize_age(started: chrono::DateTime<chrono::Utc>) -> String {
    let secs = (chrono::Utc::now() - started).num_seconds().max(0);
    match secs {
        s if s < 60 => format!("{s}s"),
        s if s < 3600 => format!("{}m", s / 60),
        s if s < 86_400 => format!("{}h", s / 3600),
        s => format!("{}d", s / 86_400),
    }
}

/// Elapsed between start and `end` (or now, for runs still going).
fn humanize_elapsed(
    started: chrono::DateTime<chrono::Utc>,
    ended: Option<chrono::DateTime<chrono::Utc>>,
) -> String {
    let end = ended.unwrap_or_else(chrono::Utc::now);
    let secs = (end - started).num_seconds().max(0);
    if secs < 3600 {
        format!("{}:{:02}", secs / 60, secs % 60)
    } else {
        format!("{}:{:02}:{:02}", secs / 3600, (secs % 3600) / 60, secs % 60)
    }
}

/// Wall-clock label in the operator's local timezone — the event log
/// stamps rows with local time, so every clock in the cockpit must
/// agree.
fn local_hm(t: chrono::DateTime<chrono::Utc>) -> String {
    t.with_timezone(&chrono::Local).format("%H:%M").to_string()
}

/// Token counter that never truncates real usage to "0k".
fn fmt_tokens(n: u64) -> String {
    if n < 1_000 {
        n.to_string()
    } else if n < 100_000 {
        format!("{:.1}k", n as f64 / 1000.0)
    } else {
        format!("{}k", n / 1000)
    }
}

/// Lifecycle pipeline for a real run: what we truthfully know from
/// `RunStatus` alone. Per-node graph stages arrive with the per-run
/// event stream (follow-up daemon phase).
fn lifecycle_stages(run: &UiRun) -> Vec<Stage> {
    let accepted_sub = local_hm(run.started_at);
    let (exec, exec_sub): (StageState, &str) = match run.status {
        RunStatus::Active => (StageState::Running, "in flight"),
        RunStatus::Awaiting => (StageState::Queued, "admission queue"),
        RunStatus::Completed => (StageState::Done, "done"),
        RunStatus::Failed => (StageState::Failed, "failed"),
        RunStatus::Aborted => (StageState::Failed, "stopped"),
        _ => (StageState::Queued, "—"),
    };
    let (fin, fin_sub): (StageState, &str) = match run.status {
        RunStatus::Completed => (StageState::Done, "terminal node"),
        RunStatus::Failed => (StageState::Failed, "failure node"),
        RunStatus::Aborted => (StageState::Failed, "operator stop"),
        _ => (StageState::Queued, "pending"),
    };
    vec![
        Stage {
            label: "accepted".to_string(),
            sub: accepted_sub,
            state: StageState::Done,
        },
        Stage {
            label: "executing".to_string(),
            sub: exec_sub.to_string(),
            state: exec,
        },
        Stage {
            label: "finished".to_string(),
            sub: fin_sub.to_string(),
            state: fin,
        },
    ]
}

impl RunRow {
    fn from_run(run: &UiRun, prompt: Option<&str>) -> Self {
        let (status_label, color, active, rank) = status_parts(run.status);
        let events = run
            .last_event_seq
            .map(|s| s.to_string())
            .unwrap_or_else(|| "—".to_string());
        Self {
            run_id: Some(run.run_id),
            id_label: format!("r-{}", run.run_id.short().to_lowercase()),
            title: prompt.map_or_else(
                || format!("Mission started {}", local_hm(run.started_at)),
                |p| crate::ui::headline(p, 90),
            ),
            age: humanize_age(run.started_at),
            status_label,
            waiting_detail: None,
            color,
            active,
            rank,
            started: local_hm(run.started_at),
            // Terminal runs freeze at the observed end; if we never saw
            // the finish (already-terminal at first list), "—" is more
            // honest than a counter that keeps growing.
            elapsed: if run.is_terminal() {
                match run.ended_at {
                    Some(end) => humanize_elapsed(run.started_at, Some(end)),
                    None => "—".to_string(),
                }
            } else {
                humanize_elapsed(run.started_at, None)
            },
            events,
            stages: lifecycle_stages(run),
            event_rows: Vec::new(),
            live_stream: false,
            usage: None,
        }
    }

    /// Enrich a real run's row with folded per-run stream telemetry:
    /// live stage pipeline, event log, token/cost counters.
    fn attach_stream(&mut self, stream: &crate::run_stream::RunStreamState) {
        self.live_stream = stream.live;
        let display = stream.display();
        self.status_label = display.label();
        self.waiting_detail = if display == surge_core::run_display::RunDisplayState::Unknown {
            stream
                .recorded_host_loss_detail()
                .or_else(|| display.next_action())
        } else {
            display.next_action()
        }
        .map(ToOwned::to_owned);
        if let Some((until, basis)) = display.wake()
            && let Some(detail) = &mut self.waiting_detail
        {
            detail.push_str(&format!(
                " Wake: {} ({})",
                until.to_rfc3339(),
                match basis {
                    surge_core::capacity::WakeBasis::ObservedReset => "observed provider reset",
                    surge_core::capacity::WakeBasis::PolicyBackoff => "policy backoff",
                }
            ));
        }
        if self.waiting_detail.is_some() {
            self.color = theme::warning();
            self.rank = 0;
        }

        if let Some(prompt) = &stream.prompt {
            self.title = crate::ui::headline(prompt, 90);
        }
        if !stream.stages.is_empty() {
            self.stages = stream.stages.iter().map(stage_from_stream).collect();
        }
        if matches!(
            display,
            surge_core::run_display::RunDisplayState::Waiting(
                surge_core::run_display::WaitingReason::RecoveryRequired
            ) | surge_core::run_display::RunDisplayState::Unknown
        ) {
            for stage in &mut self.stages {
                if stage.state == StageState::Running {
                    stage.state = StageState::Unfinished;
                    stage.sub = if stream.recovery().active().is_some() {
                        "paused · recovery requires review"
                    } else {
                        "last recorded progress · current state unconfirmed"
                    }
                    .into();
                }
            }
        }
        if !stream.log.is_empty() {
            self.event_rows = stream
                .log
                .iter()
                .rev()
                .map(|r| EventRow {
                    t: r.time.clone(),
                    kind: r.kind,
                    color: r.tone.color(),
                    text: r.text.clone(),
                    problem: matches!(
                        r.tone,
                        crate::run_stream::Tone::Err | crate::run_stream::Tone::Warn
                    ),
                })
                .collect();
        }
        if stream.last_seq > 0 {
            self.events = stream.last_seq.to_string();
        }
        if stream.tokens_in > 0
            || stream.tokens_out > 0
            || stream.cost_usd > 0.0
            || stream.cost_unknown
            || stream.usage_unknown
        {
            self.usage = Some(RunUsage::from_stream(stream));
        }
    }
}

async fn load_result_folder(
    home: std::path::PathBuf,
    run_id: RunId,
) -> Result<std::path::PathBuf, String> {
    let summary = surge_persistence::runs::Storage::inspect_existing_run_summary(home, run_id)
        .await
        .map_err(|error| format!("Cannot read result folder: {error}"))?
        .ok_or_else(|| "Result folder is not recorded for this run".to_string())?;
    tokio::task::spawn_blocking(move || validate_result_folder(summary.project_path))
        .await
        .map_err(|error| format!("Cannot inspect result folder: {error}"))?
}

fn validate_result_folder(path: std::path::PathBuf) -> Result<std::path::PathBuf, String> {
    if !path.is_absolute() {
        return Err("Recorded result folder is not an absolute path".into());
    }
    let metadata = std::fs::metadata(&path)
        .map_err(|error| format!("Result folder unavailable ({}): {error}", path.display()))?;
    if !metadata.is_dir() {
        return Err(format!(
            "Result folder is not a directory: {}",
            path.display()
        ));
    }
    Ok(path)
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum RunTab {
    /// The mission level by level (default).
    Overview,
    Changes,
    Checks,
    Sessions,
    /// Raw steps and events, for developers.
    Log,
}

#[cfg(test)]
type ControlledGuidance = std::rc::Rc<
    std::cell::RefCell<
        Vec<(
            RunId,
            String,
            tokio::sync::oneshot::Sender<Result<String, String>>,
        )>,
    >,
>;

#[derive(Clone, PartialEq, Eq, Hash)]
struct GuidanceKey {
    project: Option<PathBuf>,
    run: RunId,
}

struct GuidanceDraft {
    input: Entity<InputState>,
    revision: u64,
    pending: bool,
    note: Option<String>,
}

/// Runs screen — daemon-run cockpit.
pub struct RunsScreen {
    state: Entity<AppState>,
    /// Selected real run; falls back to the most attention-worthy.
    selected: Option<RunId>,
    guidance: HashMap<GuidanceKey, GuidanceDraft>,
    #[cfg(test)]
    controlled_guidance: Option<ControlledGuidance>,
    #[cfg(test)]
    results_home: Option<PathBuf>,
    #[cfg(test)]
    results_active_loads: std::rc::Rc<std::cell::Cell<usize>>,
    /// One-line feedback from the last facade call (honest, verbatim).
    action_note: Option<String>,
    continuation_pending: bool,
    continuation_requests: HashMap<RunId, RunId>,
    tab: RunTab,
    recorded_target: Option<(RunId, RecordedTarget)>,
    session_pages: HashMap<RunId, usize>,
    checks: Option<(RunId, Entity<run_checks::ChecksView>)>,
    changes: Option<(RunId, Entity<run_changes::ChangesView>)>,
    mission: Option<Entity<run_mission::MissionPanel>>,
    task_focus: Option<(
        surge_core::roadmap::MilestoneId,
        surge_core::roadmap::RoadmapTaskId,
    )>,
    log_filter: LogFilter,
    steps_scroll: ScrollHandle,
    /// (run, step count) last scrolled to, so a new step scrolls once.
    steps_seen: (Option<RunId>, usize),
}

/// What the Runs screen asks the app to do.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum RunsEvent {
    /// Empty state: go describe an app.
    DescribeApp,
    /// The App owns scoped status publication and successor observation.
    BootstrapContinuationAccepted {
        operation: RunId,
        status: Box<surge_core::bootstrap_operation::BootstrapOperationStatus>,
        scope: crate::run_observation::Scope,
    },
}

impl EventEmitter<RunsEvent> for RunsScreen {}

/// Plain-language bootstrap phase.
pub(crate) fn phase_label(phase: surge_core::bootstrap_operation::BootstrapPhase) -> &'static str {
    use surge_core::bootstrap_operation::BootstrapPhase as P;
    match phase {
        P::QueuedPlanning => "waiting to plan",
        P::PreparingPlanning => "preparing the plan",
        P::Planning => "planning",
        P::QueuedImplementation => "waiting to build",
        P::PreparingImplementation => "preparing the build",
        P::Implementing => "building",
    }
}

/// Why a build stopped for you, and what fixes it.
pub(crate) fn attention_reason(
    reason: surge_core::bootstrap_operation::BootstrapAttentionReason,
) -> (&'static str, &'static str) {
    use surge_core::bootstrap_operation::BootstrapAttentionReason as R;
    match reason {
        R::MissingBootstrapPolicy => (
            "Captured bootstrap policy is unavailable",
            "Review recovery or capture a new planning operation; the current default cannot restore the original policy.",
        ),
        R::ConfigurationChanged => (
            "Settings changed since this build started",
            "Restore the original agent settings, then retry, or start a new plan. The recorded build history is retained.",
        ),
        R::MissingCredential => (
            "A required credential is missing",
            "Add the API key or sign in for the agent in Settings → Agents, then retry.",
        ),
        R::PartialStartup => (
            "The build did not fully start",
            "Retry — Surge will re-check what already started.",
        ),
        R::WorktreeConflict => (
            "Its working copy could not be confirmed",
            "Check the run's worktree under .worktrees/ in the project, then retry.",
        ),
        R::InvalidMaterialization => (
            "The plan could not be validated",
            "Open Flow to inspect the plan, then retry.",
        ),
        R::BudgetUnconfirmed => (
            "The remaining budget could not be confirmed",
            "Check the agent's quota or budget, then retry.",
        ),
        R::RecoveryRequired => (
            "Recovery requires review",
            "Inspect recorded history before choosing how to recover. No retry has been authorized.",
        ),
        R::StorageUnconfirmed => (
            "The run outcome is unconfirmed",
            "Inspect recorded history and the connection or storage error before retrying.",
        ),
    }
}

impl RunsScreen {
    #[cfg(all(test, unix))]
    pub(crate) fn continuation_pending_for_test(&self) -> bool {
        self.continuation_pending
    }

    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        cx.observe(&state, |_this, _state, cx| cx.notify()).detach();

        // 1s ticker so ELAPSED / age counters move. Notifies only while
        // a non-terminal run exists — terminal-only lists are frozen, so
        // waking the renderer for them is pure waste.
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let alive = cx.update(|cx| {
                    this.update(cx, |screen, cx| {
                        let ticking = screen.state.read(cx).runs.iter().any(|r| !r.is_terminal());
                        if ticking {
                            cx.notify();
                        }
                    })
                    .is_ok()
                });
                if !alive {
                    break;
                }
            }
        })
        .detach();

        Self {
            state,
            selected: None,
            guidance: HashMap::new(),
            #[cfg(test)]
            controlled_guidance: None,
            #[cfg(test)]
            results_home: None,
            #[cfg(test)]
            results_active_loads: Default::default(),
            action_note: None,
            continuation_pending: false,
            continuation_requests: HashMap::new(),
            tab: RunTab::Overview,
            recorded_target: None,
            session_pages: HashMap::new(),
            checks: None,
            changes: None,
            mission: None,
            task_focus: None,
            log_filter: LogFilter::All,
            steps_scroll: ScrollHandle::new(),
            steps_seen: (None, 0),
        }
    }

    #[cfg(test)]
    pub(crate) fn visible_run_for_test(&self, cx: &Context<Self>) -> Option<RunId> {
        let (rows, selected, _) = self.rows(cx);
        selected
            .and_then(|index| rows.get(index))
            .and_then(|row| row.run_id)
    }

    #[cfg(all(test, unix))]
    pub(crate) fn continuation_history_home_for_test(&mut self, home: PathBuf) {
        self.results_home = Some(home);
    }

    #[cfg(all(test, unix))]
    pub(crate) fn continuation_overview_through_for_test(&self, cx: &App) -> Option<u64> {
        self.mission
            .as_ref()?
            .read(cx)
            .continuation_through_for_test()
    }

    #[cfg(all(test, unix))]
    pub(crate) fn continuation_history_error_for_test(&self, cx: &App) -> Option<String> {
        self.mission
            .as_ref()?
            .read(cx)
            .continuation_error_for_test()
            .map(str::to_owned)
    }

    /// Focus a specific run (used by Fleet → "Open run" deep links).
    pub fn select_run(&mut self, run_id: RunId, cx: &mut Context<Self>) {
        self.selected = Some(run_id);
        self.recorded_target = None;
        self.tab = RunTab::Overview;
        self.task_focus = None;
        self.mission = None;
        cx.notify();
    }

    /// Open the recorded process of one task within its owning milestone.
    pub fn select_task(
        &mut self,
        milestone: surge_core::roadmap::MilestoneId,
        task: surge_core::roadmap::RoadmapTaskId,
        cx: &mut Context<Self>,
    ) {
        self.recorded_target = None;
        self.task_focus = Some((milestone, task));
        self.tab = RunTab::Overview;
        self.mission = None;
        cx.notify();
    }

    pub(crate) fn select_tab(&mut self, tab: RunTab, cx: &mut Context<Self>) {
        self.recorded_target = None;
        self.tab = tab;
        cx.notify();
    }

    /// Actual daemon runs and the selected row index.
    fn rows(&self, cx: &Context<Self>) -> (Vec<RunRow>, Option<usize>, bool) {
        let state = self.state.read(cx);
        let mut rows: Vec<RunRow> = state
            .project_runs()
            .into_iter()
            .map(|run| {
                let mut row = RunRow::from_run(run, state.run_prompt(&run.run_id));
                // An acknowledged failure is history, not "needs you".
                if row.rank == 0 && state.dismissed_runs.contains(&run.run_id) {
                    row.rank = 3;
                }
                if let Some(stream) = state.run_streams.get(&run.run_id) {
                    row.attach_stream(stream);
                }
                if run.is_terminal() {
                    for stage in &mut row.stages {
                        if stage.state == StageState::Running {
                            stage.state = StageState::Unfinished;
                            stage.sub = "no exit recorded".into();
                        }
                    }
                }
                row
            })
            .collect();
        rows.sort_by_key(|r| r.rank);
        let sel = match self.selected {
            Some(id) => rows.iter().position(|row| row.run_id == Some(id)),
            None => (!rows.is_empty()).then_some(0),
        };
        (rows, sel, state.daemon_state.facade().is_some())
    }

    // ── facade actions ──────────────────────────────────────────────

    fn open_result_folder(&mut self, run_id: RunId, cx: &mut Context<Self>) {
        self.selected = Some(run_id);
        let Some(home) = surge_core::home::surge_home_dir() else {
            self.action_note = Some("Cannot locate Surge home".into());
            cx.notify();
            return;
        };
        self.action_note = Some("Locating result folder…".into());
        cx.notify();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let result = load_result_folder(home, run_id).await;
            cx.update(|cx| {
                let _ = this.update(cx, |screen, cx| {
                    if screen.selected != Some(run_id) {
                        return;
                    }
                    screen.action_note = Some(match result {
                        Ok(path) => {
                            cx.open_with_system(&path);
                            format!("Opening requested: {}", path.display())
                        },
                        Err(error) => error,
                    });
                    cx.notify();
                });
            });
        })
        .detach();
    }

    fn stop_run(&mut self, run_id: RunId, cx: &mut Context<Self>) {
        let state_entity = self.state.clone();
        let (facade, operation) = {
            let state = self.state.read(cx);
            (
                state.daemon_state.facade(),
                state
                    .bootstrap_operations
                    .iter()
                    .find(|(_, op)| op.planning_run == run_id || op.implementation_run == run_id)
                    .map(|(id, _)| *id),
            )
        };
        let Some(facade) = facade else {
            self.action_note = Some("daemon offline — cannot stop".into());
            cx.notify();
            return;
        };
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let note = if let Some(operation) = operation {
                match facade.cancel_bootstrap(operation).await {
                    Ok(status) => format!(
                        "cancel recorded · {}",
                        status
                            .state
                            .phase()
                            .map_or_else(|| "settled".into(), |phase| format!("{phase:?}"),)
                    ),
                    Err(error) => format!("cancel failed: {error}"),
                }
            } else {
                match facade
                    .stop_run(run_id, "stopped from Runs cockpit".to_string())
                    .await
                {
                    Ok(()) => format!("stop requested · {}", run_id.short().to_lowercase()),
                    Err(error) => format!("stop failed: {error}"),
                }
            };
            // Refresh the run list so the rail reflects the new status.
            if let Ok(summaries) = facade.list_runs().await {
                cx.update(|cx| {
                    state_entity.update(cx, |s, cx| {
                        s.set_runs_from_summaries(&summaries);
                        cx.notify();
                    });
                });
            }
            cx.update(|cx| {
                let _ = this.update(cx, |t, cx| {
                    t.action_note = Some(note);
                    cx.notify();
                });
            });
        })
        .detach();
    }

    fn guidance_key(&self, run: RunId, cx: &Context<Self>) -> GuidanceKey {
        GuidanceKey {
            project: self.state.read(cx).project_path.clone(),
            run,
        }
    }

    fn guidance_input(
        &mut self,
        key: &GuidanceKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<InputState> {
        if let Some(draft) = self.guidance.get(key) {
            return draft.input.clone();
        }
        let input = cx
            .new(|cx| InputState::new(window, cx).placeholder("Guidance for the next agent step…"));
        let owner = key.clone();
        cx.subscribe_in(
            &input,
            window,
            move |this: &mut Self, _, event: &InputEvent, window, cx| match event {
                InputEvent::Change => {
                    if let Some(draft) = this.guidance.get_mut(&owner) {
                        draft.revision = draft.revision.saturating_add(1);
                    }
                },
                InputEvent::PressEnter { .. } => {
                    let (rows, selected, _) = this.rows(cx);
                    if selected
                        .and_then(|index| rows.get(index))
                        .and_then(|row| row.run_id)
                        .is_some_and(|run| this.guidance_key(run, cx) == owner)
                    {
                        this.submit_steer(window, cx);
                    }
                },
                _ => {},
            },
        )
        .detach();
        self.guidance.insert(
            key.clone(),
            GuidanceDraft {
                input: input.clone(),
                revision: 0,
                pending: false,
                note: None,
            },
        );
        input
    }

    fn submit_steer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (rows, sel, live) = self.rows(cx);
        let Some(row) = sel.and_then(|index| rows.get(index)) else {
            return;
        };
        let Some(run) = row.run_id else {
            return;
        };
        let key = self.guidance_key(run, cx);
        let Some(draft) = self.guidance.get_mut(&key) else {
            return;
        };
        if draft.pending {
            return;
        }
        let input = draft.input.clone();
        let exact = input.read(cx).value().to_string();
        if exact.trim().is_empty() {
            return;
        }
        #[cfg(test)]
        let live = live || self.controlled_guidance.is_some();
        if !live || !row.active {
            draft.note = Some("Guidance requires a connected, active run".into());
            cx.notify();
            return;
        }
        let revision = draft.revision;
        draft.pending = true;
        draft.note = Some("Sending guidance…".into());
        let message = exact.trim().to_string();
        let request = self.guidance_request(run, message, cx);
        let handle = window.window_handle();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let result = request.await;
            let _ = cx.update_window(handle, |_, window, cx| {
                let _ = this.update(cx, |screen, cx| {
                    let Some(draft) = screen.guidance.get_mut(&key) else {
                        return;
                    };
                    if result.is_ok()
                        && draft.revision == revision
                        && input.read(cx).value().as_ref() == exact
                    {
                        input.update(cx, |input, cx| input.set_value("", window, cx));
                    }
                    draft.pending = false;
                    draft.note = Some(match result {
                        Ok(_) => "Guidance queued for the next agent step".into(),
                        Err(error) => format!("Could not confirm guidance was queued: {error}"),
                    });
                    cx.notify();
                });
            });
        })
        .detach();
        cx.notify();
    }

    fn guidance_request(
        &self,
        run: RunId,
        message: String,
        cx: &Context<Self>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>>>> {
        #[cfg(test)]
        if let Some(responses) = &self.controlled_guidance {
            let (sender, receiver) = tokio::sync::oneshot::channel();
            responses.borrow_mut().push((run, message, sender));
            return Box::pin(async move {
                receiver
                    .await
                    .unwrap_or_else(|_| Err("facade disconnected".into()))
            });
        }
        let facade = self.state.read(cx).daemon_state.facade();
        Box::pin(async move {
            let facade = facade.ok_or("daemon offline — cannot send guidance")?;
            facade
                .submit_steer(run, message)
                .await
                .map_err(|error| error.to_string())
        })
    }

    // ── panes ───────────────────────────────────────────────────────

    fn render_rail(
        &self,
        rows: &[RunRow],
        sel: Option<usize>,
        live: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let needs = rows.iter().filter(|r| r.rank == 0).count();

        let mut list = div()
            .flex_1()
            .id("runs-rail")
            .overflow_y_scroll()
            .v_flex()
            .gap(px(2.0))
            .px(px(8.0))
            .pb(px(8.0));

        for (i, row) in rows.iter().enumerate() {
            let is_sel = Some(i) == sel;
            let run_id = row.run_id;
            let kind = self.selected_bootstrap(row, cx).map(|(_, status)| {
                if row.run_id == Some(status.planning_run) {
                    "Planning"
                } else {
                    "Implementation"
                }
            });
            let item = div()
                .id(SharedString::from(format!(
                    "run-row-{}",
                    row.run_id
                        .map_or_else(|| row.id_label.clone(), |id| id.to_string())
                )))
                .test_support()
                .role(Role::Button)
                .aria_label(format!(
                    "{}{}, {}",
                    kind.map_or(String::new(), |kind| format!("{kind} · ")),
                    row.title,
                    row.status_label
                ))
                .v_flex()
                .gap(px(4.0))
                .px(px(10.0))
                .py(px(8.0))
                .rounded(px(ui::R_CONTROL))
                .border_1()
                .border_color(if is_sel {
                    theme::hairline_strong()
                } else {
                    transparent_black()
                })
                .cursor_pointer()
                .when(is_sel, |el| el.bg(theme::surface()))
                .when(!is_sel, |el| {
                    el.hover(|s: StyleRefinement| s.bg(theme::surface().opacity(0.6)))
                })
                .on_click(cx.listener(move |this, _e, _w, cx| {
                    this.selected = run_id;
                    this.task_focus = None;
                    this.mission = None;
                    this.action_note = None;
                    cx.notify();
                }))
                .child(
                    div()
                        .h_flex()
                        .gap(px(8.0))
                        .items_start()
                        .child(div().pt(px(4.0)).child(if row.active {
                            ui::live_dot(row.color)
                        } else {
                            ui::status_dot(row.color)
                        }))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .text_size(px(12.0))
                                .line_height(px(16.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme::text_primary())
                                .line_clamp(2)
                                .child(row.title.clone()),
                        ),
                )
                .children(kind.map(ui::meta))
                .child(
                    div()
                        .h_flex()
                        .gap(px(8.0))
                        .pl(px(15.0))
                        .text_size(px(10.0))
                        .child(div().text_color(row.color).child(row.status_label))
                        .child(
                            div()
                                .text_color(theme::text_dim())
                                .child(row.id_label.clone()),
                        )
                        .child(div().flex_1())
                        .child(
                            div()
                                .text_color(theme::text_dim())
                                .child(format!("{} ago", row.age)),
                        ),
                );
            list = list.child(item);
        }

        div()
            .w(px(280.0))
            .flex_shrink_0()
            .v_flex()
            .bg(theme::panel())
            .border_r_1()
            .border_color(theme::hairline())
            .child(
                div()
                    .h_flex()
                    .gap(px(8.0))
                    .items_center()
                    .px(px(16.0))
                    .pt(px(14.0))
                    .pb(px(10.0))
                    .child(
                        ui::section_label(format!("Runs · {}", rows.len()))
                            .id("runs-count")
                            .test_support()
                            .role(Role::Label)
                            .aria_label(format!("Runs · {}", rows.len()))
                            .flex_1(),
                    )
                    .when(needs > 0, |el| {
                        el.child(ui::role_badge(
                            format!("{needs} need you"),
                            theme::Semantic::You,
                        ))
                    })
                    .when(!live, |el| {
                        el.child(ui::role_badge("offline", theme::Semantic::External))
                    }),
            )
            .child(list)
    }

    fn selected_bootstrap(
        &self,
        row: &RunRow,
        cx: &Context<Self>,
    ) -> Option<(
        RunId,
        surge_core::bootstrap_operation::BootstrapOperationStatus,
    )> {
        let id = row.run_id?;
        let state = self.state.read(cx);
        let mut matching = state.bootstrap_operations.values().filter(|status| {
            [status.planning_run, status.implementation_run].contains(&id)
                || status.continuations.iter().any(|attempt| attempt.run == id)
        });
        let cached = matching.next();
        if matching.next().is_some() {
            return None;
        }
        cached
            .cloned()
            .or_else(|| {
                self.mission
                    .as_ref()
                    .and_then(|panel| {
                        let panel = panel.read(cx);
                        (panel.request().project == state.project_path)
                            .then(|| panel.operation_for(id))
                            .flatten()
                    })
                    .cloned()
            })
            .map(|status| (status.operation_id, status))
    }

    /// Resolve operation ownership from disk when Results loads, including cold history.
    fn mission_request(
        &self,
        row: &RunRow,
        cx: &Context<Self>,
    ) -> Option<run_mission::MissionRequest> {
        Some(run_mission::MissionRequest {
            selected: row.run_id?,
            project: self.state.read(cx).project_path.clone(),
        })
    }

    /// Selected run lifecycle and independently attributed operation phase.
    fn render_header(&self, row: &RunRow, live: bool, cx: &mut Context<Self>) -> Div {
        let bootstrap = self.selected_bootstrap(row, cx);

        let (status_text, status_color) = (row.status_label.to_string(), row.color);
        let cancellable = bootstrap
            .as_ref()
            .is_some_and(|(_, s)| s.state.phase().is_some() && !s.cancel_requested);

        let mut facts: Vec<String> = vec![
            row.run_id
                .map_or_else(|| row.id_label.clone(), |id| id.to_string()),
            format!("started {}", row.started),
            format!("{} elapsed", row.elapsed),
            format!("{} events", row.events),
        ];
        if let Some((_, status)) = &bootstrap
            && let Some(attempt) = status
                .continuations
                .iter()
                .find(|attempt| Some(attempt.run) == row.run_id)
        {
            facts.push(format!(
                "Continuation: {} · original operation remains failed",
                continuation_label(attempt.state)
            ));
        }
        if let Some(detail) = &row.waiting_detail {
            facts.push(detail.clone());
        }
        if let Some(usage) = row.usage {
            facts.push(usage.token_label());
            facts.push(usage.cost_label());
        }

        let mut actions = div().h_flex().gap(px(6.0)).items_center().flex_none();
        if let Some(note) = &self.action_note {
            actions = actions.child(
                div()
                    .max_w(px(260.0))
                    .truncate()
                    .text_size(px(10.5))
                    .text_color(theme::text_muted())
                    .child(note.clone()),
            );
        }
        if let Some((operation, status)) = &bootstrap
            && status.state == surge_core::bootstrap_operation::BootstrapState::Failed
            && let Some(source) = row.run_id
            && source != status.planning_run
            && (matches!(&status.result, Some(surge_core::bootstrap_continuation::BootstrapTerminal::Failed { run_id, .. }) if *run_id == source)
                || status.continuations.iter().any(|attempt| {
                    attempt.run == source
                        && attempt.state
                            == surge_core::bootstrap_attempt::BootstrapAttemptState::Failed
                }))
        {
            let operation = *operation;
            let revision = status.revision;
            let existing = status
                .continuations
                .iter()
                .find(|attempt| attempt.source_run == source);
            if let Some(attempt) = existing {
                let run = attempt.run;
                actions = actions.child(
                    Button::new("open-bootstrap-continuation")
                        .debug_selector(|| "open-bootstrap-continuation".into())
                        .label(format!("Successor · {}", continuation_label(attempt.state)))
                        .small()
                        .on_click(cx.listener(move |this, _, _, cx| this.select_run(run, cx))),
                );
            } else {
                actions = actions.child(div().v_flex()
                    .child(div().text_size(px(10.0)).child("Isolated saved checkpoint, using current project settings. Later source edits stay in the original folder."))
                    .child(Button::new("continue-bootstrap").debug_selector(|| "continue-bootstrap".into())
                        .label("Continue from saved checkpoint")
                        .small()
                        .disabled(self.continuation_pending)
                        .accessibility_id("continue-bootstrap")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.continue_bootstrap(operation, revision, source, cx)
                        }))),
                );
            }
        }
        if let Some(run_id) = row.run_id {
            actions = actions.child(
                Button::new("open-result-folder")
                    .ghost()
                    .small()
                    .icon(IconName::FolderOpen)
                    .tooltip("Open the result folder")
                    .accessibility_id("open-result-folder")
                    .debug_selector(|| "open-result-folder".into())
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.open_result_folder(run_id, cx);
                    })),
            );
        }
        if let Some((operation_id, _)) = bootstrap.as_ref().filter(|_| cancellable) {
            let operation_id = *operation_id;
            actions =
                actions.child(
                    Button::new("cancel-bootstrap")
                        .outline()
                        .small()
                        .danger()
                        .label("Cancel build")
                        .accessibility_id("cancel-bootstrap")
                        .on_click(cx.listener(move |this, _e, _w, cx| {
                            this.cancel_bootstrap(operation_id, cx)
                        })),
                );
        } else if live
            && row.active
            && let Some(run_id) = row.run_id
        {
            actions = actions.child(
                Button::new("stop-run")
                    .outline()
                    .small()
                    .danger()
                    .label("Stop run")
                    .accessibility_id("stop-run")
                    .on_click(cx.listener(move |this, _e, _w, cx| this.stop_run(run_id, cx))),
            );
        }

        div()
            .h_flex()
            .gap(px(16.0))
            .items_start()
            .px(px(20.0))
            .pt(px(16.0))
            .pb(px(12.0))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .v_flex()
                    .gap(px(6.0))
                    .child(
                        div()
                            .w_full()
                            .h_flex()
                            .gap(px(10.0))
                            .items_center()
                            .overflow_hidden()
                            .child(
                                div()
                                    .min_w(px(0.0))
                                    .flex_shrink(1.0)
                                    .text_size(px(16.0))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(theme::text_primary())
                                    .truncate()
                                    .child(row.title.clone()),
                            )
                            .child(
                                ui::pill(
                                    status_text.clone(),
                                    status_color,
                                    theme::tint(status_color),
                                )
                                .id("selected-run-status")
                                .test_support()
                                .role(Role::Label)
                                .aria_label(status_text),
                            ),
                    )
                    .children(bootstrap.as_ref().map(|(_, status)| {
                        let message = run_mission::operation_message(status);
                        div()
                            .id("selected-operation-status")
                            .test_support()
                            .role(Role::Label)
                            .aria_label(message.clone())
                            .child(ui::meta(message))
                    }))
                    .child(
                        div()
                            .id("run-header-facts")
                            .test_support()
                            .role(Role::Group)
                            .aria_label(facts.join("  ·  "))
                            .text_size(px(10.5))
                            .text_color(theme::text_dim())
                            .child(facts.join("  ·  ")),
                    ),
            )
            .child(actions)
    }

    /// Only when the build is blocked on you: why, and the one button that
    /// moves it.
    fn render_attention(&self, row: &RunRow, cx: &mut Context<Self>) -> Option<Div> {
        use surge_core::bootstrap_operation::BootstrapState as B;
        let (operation_id, status) = self.selected_bootstrap(row, cx)?;
        let B::NeedsAttention {
            phase,
            reason,
            run_id,
        } = status.state
        else {
            return None;
        };
        let (what, fix) = attention_reason(reason);
        let recovery = run_id.and_then(|run_id| self.state.read(cx).run_streams.get(&run_id))
            .and_then(|stream| crate::mission::recovery_problem(stream.recovery(),
                reason == surge_core::bootstrap_operation::BootstrapAttentionReason::StorageUnconfirmed));
        let recovery = recovery.or_else(|| {
            (reason == surge_core::bootstrap_operation::BootstrapAttentionReason::RecoveryRequired)
                .then(|| {
                    "Recorded recovery diagnostic is unavailable; inspect the run history."
                        .to_string()
                })
        });
        let revision = status.revision;
        Some(
            ui::node_card(theme::warning())
                .mx(px(20.0))
                .mb(px(10.0))
                .h_flex()
                .gap(px(12.0))
                .items_center()
                .px(px(14.0))
                .py(px(10.0))
                .child(
                    Icon::new(IconName::TriangleAlert)
                        .size(px(15.0))
                        .text_color(theme::warning()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .v_flex()
                        .gap(px(2.0))
                        .child(
                            div()
                                .text_size(px(12.5))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme::text_primary())
                                .child(format!("Stopped while {} — {what}", phase_label(phase))),
                        )
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(theme::text_muted())
                                .child(fix),
                        )
                        .when_some(recovery, |el, message| {
                            el.child(
                                div()
                                    .id("run-recovery-problem")
                                    .test_support()
                                    .role(Role::Group)
                                    .aria_label(message.clone())
                                    .text_size(px(11.0))
                                    .text_color(theme::warning())
                                    .child(message),
                            )
                        }),
                )
                .when(!status.cancel_requested, |el| {
                    el.child(
                        Button::new("retry-bootstrap")
                            .primary()
                            .small()
                            .icon(IconName::RefreshCw)
                            .label("Retry")
                            .accessibility_id("retry-bootstrap")
                            .on_click(cx.listener(move |this, _e, _w, cx| {
                                this.retry_bootstrap(operation_id, revision, cx)
                            })),
                    )
                }),
        )
    }

    fn cancel_bootstrap(&mut self, operation_id: RunId, cx: &mut Context<Self>) {
        let Some(facade) = self.state.read(cx).daemon_state.facade() else {
            self.action_note = Some("daemon offline — cannot cancel this build".into());
            cx.notify();
            return;
        };
        let state = self.state.clone();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let outcome = facade.cancel_bootstrap(operation_id).await;
            if let Ok(status) = &outcome {
                cx.update(|cx| {
                    state.update(cx, |state, cx| {
                        state
                            .bootstrap_operations
                            .insert(operation_id, status.clone());
                        cx.notify();
                    })
                });
            }
            let note = match outcome {
                Ok(status) if status.cancel_requested => {
                    "Cancellation recorded by daemon".to_string()
                },
                Ok(_) => "Build already settled".to_string(),
                Err(error) => format!("cancel failed: {error}"),
            };
            cx.update(|cx| {
                let _ = this.update(cx, |screen, cx| {
                    screen.action_note = Some(note);
                    cx.notify();
                });
            });
        })
        .detach();
    }

    fn continue_bootstrap(
        &mut self,
        operation: RunId,
        revision: u64,
        source: RunId,
        cx: &mut Context<Self>,
    ) {
        if self.continuation_pending {
            return;
        }
        let Some(facade) = self.state.read(cx).daemon_state.facade() else {
            self.action_note = Some("Daemon offline; continuation was not submitted".into());
            cx.notify();
            return;
        };
        let Some(scope) = self.state.update(cx, |state, _| state.observation_scope()) else {
            self.action_note =
                Some("Project observation is unavailable; continuation was not submitted".into());
            cx.notify();
            return;
        };
        let request = *self.continuation_requests.entry(source).or_default();
        self.continuation_pending = true;
        self.action_note = Some("Creating an isolated saved-checkpoint successor; later source edits remain in the original folder".into());
        let state = self.state.clone();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let result = facade.continue_bootstrap_implementation(operation, revision, source, request).await;
            cx.update(|cx| { let _ = this.update(cx, |screen, cx| {
                screen.continuation_pending = false;
                if state.update(cx, |state, _| state.observation_scope()).as_ref() != Some(&scope) {
                    cx.notify();
                    return;
                }
                match result {
                    Ok(status) => {
                        let successor = status.continuations.iter().find(|attempt| attempt.continuation_id == request).map(|attempt| attempt.run);
                        cx.emit(RunsEvent::BootstrapContinuationAccepted { operation, status: Box::new(status), scope });
                        screen.action_note = Some("Successor reserved; inspect its actual progress. Original failed history and later source edits are retained.".into());
                        if let Some(run) = successor { screen.select_run(run, cx); }
                    },
                    Err(error) => screen.action_note = Some(format!("Continuation unconfirmed: {error}. Retry uses the same request.")),
                }
                cx.notify();
            }); });
        }).detach();
        cx.notify();
    }

    fn retry_bootstrap(&mut self, operation_id: RunId, revision: u64, cx: &mut Context<Self>) {
        let Some(facade) = self.state.read(cx).daemon_state.facade() else {
            self.action_note = Some("daemon offline — cannot retry this build".into());
            cx.notify();
            return;
        };
        let state = self.state.clone();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let outcome = facade.retry_bootstrap(operation_id, revision).await;
            if let Ok(status) = &outcome {
                cx.update(|cx| {
                    state.update(cx, |state, cx| {
                        state
                            .bootstrap_operations
                            .insert(operation_id, status.clone());
                        cx.notify();
                    })
                });
            }
            let note = match outcome {
                Ok(status) => format!("retry accepted · {:?}", status.state),
                Err(error) => format!("retry failed: {error}"),
            };
            cx.update(|cx| {
                let _ = this.update(cx, |screen, cx| {
                    screen.action_note = Some(note);
                    cx.notify();
                });
            });
        })
        .detach();
    }

    fn render_tabs(&self, cx: &mut Context<Self>) -> Div {
        let mut tabs = div()
            .h_flex()
            .gap(px(4.0))
            .px(px(16.0))
            .border_b_1()
            .border_color(theme::hairline());
        for (tab, label, icon) in [
            (RunTab::Overview, "Overview", Lucide::Workflow),
            (RunTab::Changes, "Changes", Lucide::GitCompare),
            (RunTab::Checks, "Checks", Lucide::ShieldCheck),
            (RunTab::Log, "Log", Lucide::ScrollText),
            (RunTab::Sessions, "Sessions", Lucide::ScrollText),
        ] {
            let active = self.tab == tab;
            tabs = tabs.child(
                div()
                    .id(SharedString::from(format!("run-tab-{label}")))
                    .test_support()
                    .role(Role::Tab)
                    .aria_label(label)
                    .h_flex()
                    .gap(px(6.0))
                    .items_center()
                    .px(px(10.0))
                    .h(px(36.0))
                    .border_b_2()
                    .border_color(if active {
                        theme::accent()
                    } else {
                        transparent_black()
                    })
                    .text_size(px(12.0))
                    .text_color(if active {
                        theme::text_primary()
                    } else {
                        theme::text_muted()
                    })
                    .cursor_pointer()
                    .hover(|s: StyleRefinement| s.text_color(theme::text_primary()))
                    .on_click(cx.listener(move |screen, _, _, cx| {
                        screen.select_tab(tab, cx);
                    }))
                    .child(Icon::new(icon).size(px(13.0)))
                    .child(label),
            );
        }
        tabs
    }

    fn render_sessions(&self, run: RunId, cx: &Context<Self>) -> impl IntoElement {
        let panel = div()
            .id("recorded-session-history")
            .test_support()
            .debug_selector(|| "recorded-session-history".into())
            .v_flex()
            .gap(px(8.0))
            .p(px(16.0))
            .overflow_y_scroll();
        let state = self.state.read(cx);
        let Some(stream) = state.run_streams.get(&run) else {
            if let Some((
                _,
                RecordedTarget::Session {
                    opened, connection, ..
                },
            )) = self.recorded_target
            {
                return panel.child(recorded_target::unavailable(format!("Requested session opening #{opened} ({connection}) has not been hydrated for {run}.")));
            }
            return panel.child(ui::meta(
                "Session history has not been hydrated for this run.",
            ));
        };
        let last_page = stream.sessions.len().saturating_sub(1) / ui::SESSIONS_PER_PAGE;
        let focused = match self.recorded_target {
            Some((
                target_run,
                RecordedTarget::Session {
                    entered,
                    opened,
                    connection,
                },
            )) if target_run == run => {
                if !self.recorded_session_known(run, entered, opened, connection, cx) {
                    return panel.child(recorded_target::unavailable(format!("Requested session opening #{opened} is unavailable in its exact recorded stage.")));
                }
                Some((opened, connection))
            },
            _ => None,
        };
        let page = if let Some((opened, connection)) = focused {
            match recorded_target::session_page(stream,opened,connection) {
                Some(page)=>page,
                None=>return panel.child(recorded_target::unavailable(format!("Requested session opening #{opened} ({connection}) is unavailable or its history has not been fully confirmed."))),
            }
        } else {
            self.session_pages
                .get(&run)
                .copied()
                .unwrap_or(0)
                .min(last_page)
        };
        let panel = if let Some((opened, connection)) = focused {
            panel.child(
                div()
                    .id("focused-recorded-session")
                    .test_support()
                    .debug_selector(|| "focused-recorded-session".into())
                    .role(Role::Region)
                    .aria_label(format!("Requested session opening #{opened}: {connection}"))
                    .child(ui::meta(format!(
                        "Requested session opening #{opened} · {connection}"
                    ))),
            )
        } else {
            panel
        };
        panel.child(ui::recorded_session_page(stream, page)).child(
            div()
                .h_flex()
                .gap(px(8.0))
                .child(
                    gpui_kit::component::button::Button::new("run-newer-sessions")
                        .label("Newer sessions")
                        .disabled(page == 0)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.recorded_target = None;
                            this.session_pages.insert(run, page.saturating_sub(1));
                            cx.notify();
                        })),
                )
                .child(
                    gpui_kit::component::button::Button::new("run-older-sessions")
                        .label("Older sessions")
                        .disabled(page >= last_page)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.recorded_target = None;
                            this.session_pages.insert(run, page + 1);
                            cx.notify();
                        })),
                ),
        )
    }

    fn render_stage_chip(&self, stage: &Stage, last: bool) -> Div {
        let color = stage.state.color();
        let is_hot = matches!(stage.state, StageState::Running);

        let dot: AnyElement = if is_hot {
            ui::live_dot(color)
                .with_animation(
                    SharedString::from(format!("stage-pulse-{}", stage.label)),
                    Animation::new(Duration::from_millis(1400)).repeat(),
                    |el, delta| el.opacity(0.45 + 0.55 * (delta * std::f32::consts::PI).sin()),
                )
                .into_any_element()
        } else {
            ui::status_dot(color).into_any_element()
        };

        let chip = match stage.state {
            StageState::Queued | StageState::Unfinished => {
                ui::panel().rounded(px(ui::R_CONTROL + 2.0))
            },
            _ => ui::node_card(color),
        }
        .h_flex()
        .flex_none()
        .gap(px(9.0))
        .items_center()
        .px(px(12.0))
        .py(px(8.0))
        .child(dot)
        .child(
            div()
                .v_flex()
                .child(
                    div()
                        .text_size(px(11.5))
                        .font_weight(FontWeight::BOLD)
                        .text_color(
                            if matches!(stage.state, StageState::Queued | StageState::Unfinished) {
                                theme::text_muted()
                            } else {
                                theme::text_primary()
                            },
                        )
                        .whitespace_nowrap()
                        .child(stage.label.replace('_', " ")),
                )
                .child(
                    div()
                        .text_size(px(9.5))
                        .text_color(color)
                        .whitespace_nowrap()
                        .child(ui::headline(&stage.sub, 28)),
                ),
        );

        let mut cell = div().h_flex().flex_none().items_center().child(chip);
        if !last {
            cell = cell.child(div().w(px(22.0)).h(px(1.0)).bg(
                if stage.state == StageState::Done {
                    theme::stroke(theme::success())
                } else {
                    theme::graph_line()
                },
            ));
        }
        cell
    }

    /// Steps as a horizontal, scrollable strip on the grid — left to right
    /// in execution order, the latest kept in view.
    fn render_pipeline(&self, row: &RunRow) -> Div {
        let n = row.stages.len();
        let chips: Vec<Div> = row
            .stages
            .iter()
            .enumerate()
            .map(|(i, s)| self.render_stage_chip(s, i + 1 == n))
            .collect();
        let done = row
            .stages
            .iter()
            .filter(|s| s.state == StageState::Done)
            .count();

        div()
            .relative()
            .flex_none()
            .h(px(116.0))
            .overflow_hidden()
            .bg(theme::panel_deep())
            .border_b_1()
            .border_color(theme::hairline())
            .child(ui::grid_backdrop(24.0))
            .child(
                div()
                    .absolute()
                    .top(px(10.0))
                    .left(px(20.0))
                    .h_flex()
                    .gap(px(8.0))
                    .child(ui::section_label(format!("Steps · {done}/{n} done"))),
            )
            .child(
                div()
                    .id("run-steps")
                    .absolute()
                    .top(px(34.0))
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .overflow_x_scroll()
                    .track_scroll(&self.steps_scroll)
                    .child(
                        div()
                            .h_flex()
                            .items_center()
                            .h_full()
                            .px(px(20.0))
                            .children(chips),
                    ),
            )
    }

    fn render_event_log(&self, row: &RunRow, cx: &mut Context<Self>) -> Div {
        let filter = self.log_filter;
        let rows: Vec<&EventRow> = row.event_rows.iter().filter(|e| filter.keeps(e)).collect();
        let mut body = div()
            .flex_1()
            .id("runs-event-log")
            .overflow_y_scroll()
            .v_flex()
            .px(px(20.0))
            .pb(px(12.0));

        if row.event_rows.is_empty() {
            body = body.child(ui::empty_state(
                "≡",
                if row.live_stream {
                    "Waiting for the first event"
                } else {
                    "No events recorded"
                },
                if row.live_stream {
                    "Events appear here as the run emits them."
                } else {
                    "This run has no recorded step events yet."
                },
            ));
        } else if rows.is_empty() {
            body = body.child(
                div()
                    .py(px(16.0))
                    .text_size(px(11.5))
                    .text_color(theme::text_muted())
                    .child(format!("No {} events.", filter.label().to_lowercase())),
            );
        } else {
            for (index, e) in rows.into_iter().enumerate() {
                body = body.child(
                    div()
                        .id(("run-event", index))
                        .test_support()
                        .role(Role::Label)
                        .aria_label(format!("{} {} {}", e.t, e.kind, e.text))
                        .h_flex()
                        .gap(px(12.0))
                        .items_start()
                        .py(px(5.0))
                        .border_b_1()
                        .border_color(theme::hairline().opacity(0.5))
                        .child(
                            div()
                                .w(px(58.0))
                                .flex_shrink_0()
                                .whitespace_nowrap()
                                .text_size(px(10.5))
                                .text_color(theme::text_dim())
                                .child(e.t.clone()),
                        )
                        .child(
                            div()
                                .w(px(64.0))
                                .flex_shrink_0()
                                .whitespace_nowrap()
                                .text_size(px(9.5))
                                .font_weight(FontWeight::BOLD)
                                .text_color(e.color)
                                .child(e.kind),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .text_size(px(11.5))
                                .line_height(px(16.0))
                                .text_color(theme::text_primary())
                                .child(e.text.clone()),
                        ),
                );
            }
        }

        let mut filters = div().h_flex().gap(px(2.0));
        for f in LogFilter::ALL {
            let count = row.event_rows.iter().filter(|e| f.keeps(e)).count();
            let active = f == filter;
            filters = filters.child(
                div()
                    .id(SharedString::from(format!("log-filter-{}", f.label())))
                    .role(Role::Button)
                    .px(px(8.0))
                    .py(px(3.0))
                    .rounded(px(ui::R_CONTROL))
                    .text_size(px(10.5))
                    .cursor_pointer()
                    .when(active, |el| {
                        el.bg(theme::surface()).text_color(theme::text_primary())
                    })
                    .when(!active, |el| {
                        el.text_color(theme::text_muted())
                            .hover(|s: StyleRefinement| s.text_color(theme::text_primary()))
                    })
                    .on_click(cx.listener(move |this, _e, _w, cx| {
                        this.log_filter = f;
                        cx.notify();
                    }))
                    .child(format!("{} {count}", f.label())),
            );
        }

        div()
            .flex_1()
            .min_h(px(0.0))
            .v_flex()
            .child(
                div()
                    .h_flex()
                    .gap(px(12.0))
                    .items_center()
                    .px(px(20.0))
                    .pt(px(10.0))
                    .pb(px(6.0))
                    .child(ui::section_label("Events"))
                    .when(row.live_stream, |el| {
                        el.child(ui::role_badge("live", theme::Semantic::Agent))
                    })
                    .child(div().flex_1())
                    .child(filters),
            )
            .child(body)
    }

    fn render_steer_bar(&mut self, run: RunId, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let key = self.guidance_key(run, cx);
        let input = self.guidance_input(&key, window, cx);
        let (pending, note) = self
            .guidance
            .get(&key)
            .map_or((false, None), |draft| (draft.pending, draft.note.clone()));
        div()
            .flex_shrink_0()
            .v_flex()
            .gap(px(6.0))
            .p(px(12.0))
            .bg(theme::panel())
            .border_t_1()
            .border_color(theme::hairline())
            .child(
                div()
                    .h_flex()
                    .gap(px(10.0))
                    .items_center()
                    .child(
                        div().flex_1().child(
                            Input::new(&input)
                                .accessibility_id("steer-run")
                                .aria_label("Guidance for the next agent step"),
                        ),
                    )
                    .child(
                        Button::new("send-guidance")
                            .primary()
                            .label("Send guidance")
                            .disabled(pending)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.submit_steer(window, cx)),
                            ),
                    ),
            )
            .children(note.map(|note| {
                div()
                    .text_size(px(13.0))
                    .text_color(theme::text_muted())
                    .child(note)
            }))
    }
}

impl Render for RunsScreen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (rows, sel, live) = self.rows(cx);
        let selected = sel.and_then(|index| rows.get(index)).cloned();

        // Keep the newest step in view as a live run advances.
        if let Some(row) = &selected {
            let n = row.stages.len();
            if self.steps_seen != (row.run_id, n) {
                self.steps_seen = (row.run_id, n);
                if n > 0 {
                    self.steps_scroll.scroll_to_item(n - 1);
                }
            }
        }

        let mut main = div().flex_1().min_w_0().v_flex();
        let mut steerable = false;
        if let Some(row) = &selected {
            steerable = live && row.active;
            main = main
                .child(self.render_header(row, live, cx))
                .children(self.render_attention(row, cx))
                .child(self.render_tabs(cx));
            match self.tab {
                RunTab::Checks => {
                    if let Some(run_id) = row.run_id {
                        if self.checks.as_ref().is_none_or(|(id, _)| *id != run_id) {
                            self.checks = Some((
                                run_id,
                                cx.new(|cx| run_checks::ChecksView::new(run_id, window, cx)),
                            ));
                        }
                        if let Some((_, checks)) = &self.checks {
                            main = main.child(checks.clone());
                        }
                    }
                },
                RunTab::Changes => {
                    if let Some(run_id) = row.run_id {
                        if self.changes.as_ref().is_none_or(|(id, _)| *id != run_id) {
                            self.changes = Some((
                                run_id,
                                cx.new(|cx| run_changes::ChangesView::new(run_id, window, cx)),
                            ));
                        }
                        if let Some((_, changes)) = &self.changes {
                            main = main.child(changes.clone());
                        }
                    }
                },
                RunTab::Log => {
                    main = main
                        .child(self.render_pipeline(row))
                        .child(self.render_event_log(row, cx));
                },
                RunTab::Sessions => {
                    if let Some(run) = row.run_id {
                        main = main.child(self.render_sessions(run, cx));
                    }
                },
                RunTab::Overview => {
                    if matches!(self.recorded_target,Some((run,RecordedTarget::Stage{..})) if row.run_id==Some(run))
                    {
                        main = main.child(self.render_recorded_stage(cx));
                    } else if let Some(request) = self.mission_request(row, cx) {
                        let stale = self
                            .mission
                            .as_ref()
                            .is_none_or(|panel| panel.read(cx).request() != &request);
                        if stale {
                            let home = surge_core::home::surge_home_dir();
                            #[cfg(test)]
                            let home = self.results_home.clone().or(home);
                            self.mission = Some(cx.new(|cx| {
                                run_mission::MissionPanel::new(
                                    home,
                                    request,
                                    self.task_focus.clone(),
                                    #[cfg(test)]
                                    self.results_active_loads.clone(),
                                    cx,
                                )
                            }));
                        }
                        if let Some(panel) = &self.mission {
                            let state = self.state.clone();
                            panel.update(cx, |panel, cx| {
                                let refresh = panel.refresh_key(state.read(cx));
                                panel.sync(refresh, cx);
                            });
                            main = main.child(panel.clone());
                        }
                    }
                },
            }
        } else if let Some(run) = self.selected {
            main=main.child(recorded_target::unavailable(format!("Requested run {run} is unavailable in the current project history. Refresh or choose a different run explicitly.")));
        } else {
            main = main.child(
                div().flex_1().flex().items_center().justify_center().child(
                    ui::empty_state(
                        "▷",
                        "No missions yet",
                        "Every build shows up here with its steps, live events, changed files and checks.",
                    )
                    .child(
                        Button::new("runs-describe-app")
                            .primary()
                            .icon(IconName::Plus)
                            .label("Describe an app")
                            .on_click(cx.listener(|_this, _e, _w, cx| cx.emit(RunsEvent::DescribeApp))),
                    ),
                ),
            );
        }

        div()
            .size_full()
            .v_flex()
            .bg(theme::background())
            .child(
                // Plain .flex() row — gpui-component's h_flex() would
                // items_center the panes instead of stretching them to
                // full height.
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .when(!rows.is_empty(), |el| {
                        el.child(self.render_rail(&rows, sel, live, cx))
                    })
                    .child(main),
            )
            .when(steerable, |el| {
                if let Some(run) = selected.as_ref().and_then(|row| row.run_id) {
                    el.child(self.render_steer_bar(run, window, cx))
                } else {
                    el
                }
            })
    }
}

#[cfg(test)]
mod empty_state_tests {
    use super::RunsScreen;
    use crate::app_state::AppState;
    use gpui_kit::{AppContext, TestAppContext};

    #[test]
    fn guidance_retains_draft_until_delayed_facade_acknowledges() {
        let mut cx = TestAppContext::single();
        cx.update(gpui_kit::init);
        let run = surge_core::RunId::new();
        let state = cx.new(|_| {
            let mut state = AppState::new();
            state.runs.push(crate::app_state::UiRun {
                run_id: run,
                status: surge_orchestrator::engine::handle::RunStatus::Active,
                started_at: chrono::Utc::now(),
                last_event_seq: None,
                ended_at: None,
            });
            state
        });
        let screen = cx.new(|cx| RunsScreen::new(state.clone(), cx));
        let (_, window) = cx.add_window_view(|window, cx| {
            gpui_kit::component::Root::new(screen.clone(), window, cx)
        });
        let responses = super::ControlledGuidance::default();
        window.update(|window, cx| {
            screen.update(cx, |screen, cx| {
                screen.controlled_guidance = Some(responses.clone());
                screen.selected = Some(run);
                let key = screen.guidance_key(run, cx);
                let input = screen.guidance_input(&key, window, cx);
                input.update(cx, |input, cx| {
                    input.set_value("  Keep this exact guidance  ", window, cx)
                });

                screen.submit_steer(window, cx);
                assert_eq!(
                    input.read(cx).value().as_ref(),
                    "  Keep this exact guidance  "
                );
            })
        });
        window.run_until_parked();
        assert_eq!(responses.borrow().len(), 1);
        let (sent_run, message, sender) = responses.borrow_mut().remove(0);
        assert_eq!(sent_run, run);
        assert_eq!(message, "Keep this exact guidance");
        sender.send(Err("acknowledgment lost".into())).unwrap();
        window.run_until_parked();
        window.update(|window, cx| {
            screen.update(cx, |screen, cx| {
                let key = screen.guidance_key(run, cx);
                let draft = screen.guidance.get(&key).unwrap();
                assert_eq!(
                    draft.input.read(cx).value().as_ref(),
                    "  Keep this exact guidance  "
                );
                assert!(!draft.pending);
                assert!(draft.note.as_ref().unwrap().contains("Could not confirm"));
                screen.submit_steer(window, cx);
                screen.submit_steer(window, cx);
            })
        });
        assert_eq!(
            responses.borrow().len(),
            1,
            "pending submission must not duplicate"
        );
        let (_, _, sender) = responses.borrow_mut().remove(0);
        let other_run = surge_core::RunId::new();
        window.update(|_, cx| {
            state.update(cx, |state, _| {
                state.runs.push(crate::app_state::UiRun {
                    run_id: other_run,
                    status: surge_orchestrator::engine::handle::RunStatus::Active,
                    started_at: chrono::Utc::now(),
                    last_event_seq: None,
                    ended_at: None,
                })
            })
        });
        window.update(|window, cx| {
            screen.update(cx, |screen, cx| {
                let key = screen.guidance_key(run, cx);
                let input = screen.guidance.get(&key).unwrap().input.clone();
                input.update(cx, |input, cx| {
                    use gpui_kit::EntityInputHandler as _;
                    let len = input.value().encode_utf16().count();
                    input.replace_text_in_range(Some(0..len), "revised during pending", window, cx)
                });
                input.update(cx, |input, cx| {
                    use gpui_kit::EntityInputHandler as _;
                    let len = input.value().encode_utf16().count();
                    input.replace_text_in_range(
                        Some(0..len),
                        "  Keep this exact guidance  ",
                        window,
                        cx,
                    )
                });
                screen.select_run(other_run, cx);
                let other = screen.guidance_key(other_run, cx);
                let input = screen.guidance_input(&other, window, cx);
                input.update(cx, |input, cx| {
                    input.set_value("Other run draft", window, cx)
                });
            })
        });
        window.run_until_parked();
        sender.send(Ok("queued-one".into())).unwrap();
        window.run_until_parked();
        window.update(|_, cx| {
            screen.update(cx, |screen, cx| {
                let key = screen.guidance_key(run, cx);
                assert_eq!(
                    screen.guidance[&key].input.read(cx).value().as_ref(),
                    "  Keep this exact guidance  ",
                    "retyping the same text is a newer revision"
                );
                let other = screen.guidance_key(other_run, cx);
                assert_eq!(
                    screen.guidance[&other].input.read(cx).value().as_ref(),
                    "Other run draft"
                );
                assert!(
                    screen.guidance[&other].note.is_none(),
                    "late acknowledgment belongs to the original run"
                );
            })
        });
        window.update(|window, cx| {
            screen.update(cx, |screen, cx| {
                screen.select_run(run, cx);
                screen.submit_steer(window, cx);
            })
        });
        let (_, _, sender) = responses.borrow_mut().remove(0);
        window.update(|window, cx| {
            screen.update(cx, |screen, cx| {
                screen.state.update(cx, |state, _| {
                    state.project_path = Some(std::path::PathBuf::from("/other-project"))
                });
                let key = screen.guidance_key(run, cx);
                let input = screen.guidance_input(&key, window, cx);
                assert!(input.read(cx).value().is_empty());
                input.update(cx, |input, cx| {
                    input.set_value("Other project draft", window, cx)
                });
            })
        });
        window.run_until_parked();
        sender.send(Ok("queued-two".into())).unwrap();
        window.run_until_parked();
        window.update(|_, cx| {
            screen.update(cx, |screen, cx| {
                let current = screen.guidance_key(run, cx);
                assert_eq!(
                    screen.guidance[&current].input.read(cx).value().as_ref(),
                    "Other project draft"
                );
                assert!(screen.guidance[&current].note.is_none());
                let previous = super::GuidanceKey { project: None, run };
                assert!(
                    screen.guidance[&previous].input.read(cx).value().is_empty(),
                    "unchanged acknowledged draft clears in its own project"
                );
                screen.guidance[&previous].input.update(cx, |_, cx| {
                    cx.emit(gpui_kit::component::input::InputEvent::PressEnter {
                        secondary: false,
                        shift: false,
                    });
                });
            })
        });
        window.run_until_parked();
        assert!(
            responses.borrow().is_empty(),
            "an old input Enter must not submit another project's draft"
        );
    }

    #[gpui_kit::test]
    fn task_deep_link_resets_previous_tab_and_scope(cx: &mut TestAppContext) {
        let screen = cx.update(|cx| {
            let state = cx.new(|_| AppState::new());
            cx.new(|cx| RunsScreen::new(state, cx))
        });
        screen.update(cx, |screen, cx| {
            screen.tab = super::RunTab::Log;
            screen.select_task("old".into(), "shared".into(), cx);
            let run = surge_core::RunId::new();
            screen.select_run(run, cx);
            assert_eq!(screen.selected, Some(run));
            assert!(matches!(screen.tab, super::RunTab::Overview));
            assert!(screen.task_focus.is_none());
            screen.select_task("m2".into(), "shared".into(), cx);
            assert_eq!(screen.task_focus, Some(("m2".into(), "shared".into())));
        });
    }

    #[gpui_kit::test]
    fn empty_cockpit_never_invents_runs(cx: &mut TestAppContext) {
        let screen = cx.update(|cx| {
            let state = cx.new(|_| AppState::new());
            cx.new(|cx| RunsScreen::new(state, cx))
        });
        screen.update(cx, |screen, cx| {
            let (rows, selected, connected) = screen.rows(cx);
            assert!(rows.is_empty());
            assert_eq!(selected, None);
            assert!(!connected);
        });
    }
}

#[cfg(test)]
mod recorded_session_tests {
    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{AppContext as _, TestAppContext};
    use surge_core::run_event::{EventPayload, VersionedEventPayload};

    #[test]
    fn sessions_tab_reaches_oldest_stored_opening_beyond_log_limit() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let guard = runtime.enter();
        let home = crate::runtime_home_fixture::FixtureHome::new().unwrap();
        let run = surge_core::RunId::new();
        let oldest = surge_core::SessionId::new();
        let newest = surge_core::SessionId::new();
        let events = runtime.block_on(async {
            let storage = surge_persistence::runs::Storage::open(home.path())
                .await
                .unwrap();
            let writer = storage.create_run(run, home.path(), None).await.unwrap();
            let mut payloads = vec![EventPayload::RunStarted {
                pipeline_template: None,
                project_path: home.path().into(),
                initial_prompt: "Inspect all recorded openings".into(),
                config: surge_core::RunConfig {
                    budget: Default::default(),
                    sandbox_default: surge_core::sandbox::SandboxMode::WorkspaceWrite,
                    approval_default: surge_core::approvals::ApprovalPolicy::OnRequest,
                    auto_pr: false,
                    mcp_servers: Vec::new(),
                    bootstrap_edit_loop_cap: None,
                },
            }];
            for index in 0..261 {
                payloads.push(EventPayload::SessionOpened {
                    node: surge_core::NodeKey::try_from("agent").unwrap(),
                    session: if index == 0 {
                        oldest
                    } else if index == 260 {
                        newest
                    } else {
                        surge_core::SessionId::new()
                    },
                    agent: "implementer@1.0".into(),
                    agent_id: Some("recorded-runtime".into()),
                    opened: None,
                    handoff: None,
                });
            }
            writer
                .append_events(
                    payloads
                        .into_iter()
                        .map(VersionedEventPayload::new)
                        .collect(),
                )
                .await
                .unwrap();
            let events = surge_persistence::runs::Storage::inspect_existing_run_events(
                home.path().join("runs"),
                run,
            )
            .await
            .unwrap();
            writer.close().await.unwrap();
            events
        });
        home.close().unwrap();
        assert_eq!(events.len(), 262);
        let mut stream = crate::run_stream::RunStreamState::default();
        stream.begin_display_history(run, None);
        for event in events {
            stream.apply_recorded(
                &surge_orchestrator::engine::handle::EngineRunEvent::Persisted {
                    seq: event.seq.as_u64(),
                    payload: Box::new(event.payload.payload),
                },
                0,
            );
        }
        stream.finish_display_history(262);
        assert!(stream.session_history_confirmed());
        assert_eq!(stream.sessions.len(), 261);
        assert_eq!(stream.sessions[0].session, oldest);
        let mut cx = TestAppContext::single();
        cx.update(gpui_kit::init);
        crate::theme::init();
        let state = cx.new(|_| {
            let mut state = crate::app_state::AppState::new();
            state.runs.push(crate::app_state::UiRun {
                run_id: run,
                status: surge_orchestrator::engine::handle::RunStatus::Active,
                started_at: chrono::Utc::now(),
                last_event_seq: Some(262),
                ended_at: None,
            });
            state.run_streams.insert(run, stream);
            state
        });
        let screen = cx.new(|cx| super::RunsScreen::new(state, cx));
        screen.update(&mut cx, |screen, _| {
            screen.selected = Some(run);
            screen.tab = super::RunTab::Sessions;
        });
        let (_, window) = cx.add_window_view(|window, cx| {
            gpui_kit::component::Root::new(screen.clone(), window, cx)
        });
        window.update(|window, cx| window.draw(cx).clear(cx));
        window.update(|window, cx| {
            window.render_frame(cx);
            assert_eq!(
                window.find("session-opening-262").label(),
                Some(format!("Opening #262 · internal {newest} · provider unrecorded").as_str())
            );
            assert!(window.try_find("session-opening-2").is_none());
        });
        // Fixed 261-opening fixture requires thirteen actual pagination clicks.
        for _ in 0..13 {
            window.update(|window, cx| {
                window.scroll(
                    "recorded-session-history",
                    gpui_kit::ScrollDelta::Pixels(gpui_kit::point(
                        gpui_kit::px(0.0),
                        gpui_kit::px(-4000.0),
                    )),
                    cx,
                );
            });
            window.update(|window, cx| {
                window.click("run-older-sessions", cx);
            });
        }
        window.update(|window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("session-opening-262").is_none());
            assert_eq!(
                window.find("session-opening-2").label(),
                Some(format!("Opening #2 · internal {oldest} · provider unrecorded").as_str())
            );
        });
        screen.update(window, |screen, _| {
            assert_eq!(screen.session_pages.get(&run), Some(&13));
        });
        drop(screen);
        drop(cx);
        drop(guard);
    }
}

#[cfg(test)]
mod result_folder_tests {
    #[test]
    fn result_folder_button_dispatches_selected_run() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _guard = runtime.enter();
        use gpui_kit::{AppContext as _, Modifiers, TestAppContext};
        let mut cx = TestAppContext::single();
        cx.update(gpui_kit::init);
        crate::theme::init();
        let id = surge_core::RunId::new();
        let state = cx.new(|_| {
            let mut state = crate::app_state::AppState::new();
            state.runs.push(crate::app_state::UiRun {
                run_id: id,
                status: surge_orchestrator::engine::handle::RunStatus::Completed,
                started_at: chrono::Utc::now(),
                ended_at: Some(chrono::Utc::now()),
                last_event_seq: None,
            });
            state
        });
        let screen = cx.new(|cx| super::RunsScreen::new(state, cx));
        let (_, window) = cx.add_window_view(|window, cx| {
            gpui_kit::component::Root::new(screen.clone(), window, cx)
        });
        window.update(|window, cx| window.draw(cx).clear(cx));
        let button = window
            .debug_bounds("open-result-folder")
            .expect("result action rendered");
        window.simulate_click(button.center(), Modifiers::default());
        screen.update(window, |screen, _| {
            assert_eq!(screen.selected, Some(id));
            assert!(screen.action_note.is_some());
        });
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn result_folder_uses_persisted_custom_path() {
        let home = crate::runtime_home_fixture::FixtureHome::new().unwrap();
        let output = tempfile::tempdir().unwrap();
        let storage = surge_persistence::runs::Storage::open(home.path())
            .await
            .unwrap();
        let run = surge_core::RunId::new();
        let writer = storage
            .create_run(run, output.path().to_str().unwrap(), None)
            .await
            .unwrap();
        writer.close().await.unwrap();
        assert_eq!(
            super::load_result_folder(home.path().into(), run)
                .await
                .unwrap(),
            output.path()
        );
        assert!(
            super::load_result_folder(home.path().into(), surge_core::RunId::new())
                .await
                .unwrap_err()
                .contains("not recorded")
        );
        drop(storage);
        home.close().unwrap();
        output.close().unwrap();
    }

    #[test]
    fn result_folder_rejects_missing_file_and_relative_paths() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("file");
        std::fs::write(&file, "data").unwrap();
        for path in [
            file,
            root.path().join("missing"),
            std::path::PathBuf::from("relative"),
        ] {
            assert!(super::validate_result_folder(path).is_err());
        }
        assert_eq!(
            super::validate_result_folder(root.path().into()).unwrap(),
            root.path()
        );
    }
}

#[cfg(test)]
mod usage_completeness_tests {
    use super::RunUsage;
    use crate::run_stream::RunStreamState;

    #[test]
    fn missing_only_usage_renders_unavailable() {
        let mut stream = RunStreamState::default();
        stream.usage_unknown = true;
        stream.cost_unknown = true;
        let usage = RunUsage::from_stream(&stream);
        assert_eq!(usage.token_label(), "token spend unavailable");
        assert_eq!(usage.cost_label(), "cost unavailable");
    }

    #[test]
    fn partial_usage_retains_known_token_and_cost_subtotals() {
        let mut stream = RunStreamState::default();
        stream.tokens_in = 10;
        stream.tokens_out = 5;
        stream.cost_usd = 0.5;
        stream.usage_unknown = true;
        stream.cost_unknown = true;
        let usage = RunUsage::from_stream(&stream);
        assert_eq!(
            usage.token_label(),
            "10 in · 5 out tokens known · total unavailable"
        );
        assert_eq!(usage.cost_label(), "$0.50 known · total unavailable");
    }

    #[test]
    fn complete_usage_renders_reported_totals() {
        let mut stream = RunStreamState::default();
        stream.tokens_in = 10;
        stream.tokens_out = 5;
        stream.cost_usd = 0.5;
        let usage = RunUsage::from_stream(&stream);
        assert_eq!(usage.token_label(), "10 in · 5 out tokens");
        assert_eq!(usage.cost_label(), "$0.50");
    }
}

#[cfg(test)]
mod recovery_observability_tests {
    #[gpui_kit::test]
    fn recovery_observability_known_crash_and_stream_error_keep_native_recorded_fact(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use gpui_kit::AppContext as _;
        use gpui_kit::test::TestWindowExt as _;
        use surge_orchestrator::engine::handle::EngineRunEvent;
        cx.update(gpui_kit::init);
        let run = surge_core::RunId::new();
        let mut stream = crate::run_stream::RunStreamState::default();
        stream.live = true;
        stream.apply(&EngineRunEvent::StreamError {
            message: "journal read unavailable".into(),
        });
        stream.begin_display_history(run, Some(surge_core::RunStatus::Crashed));
        assert!(!stream.live);
        assert_eq!(stream.display().label(), "Run state is unconfirmed");
        let state = cx.new(|_| {
            let mut state = crate::app_state::AppState::new();
            state.runs.push(crate::app_state::UiRun {
                run_id: run,
                status: surge_orchestrator::engine::handle::RunStatus::Active,
                started_at: chrono::Utc::now(),
                last_event_seq: None,
                ended_at: None,
            });
            state.run_streams.insert(run, stream);
            state
        });
        let screen = cx.new(|cx| {
            let mut screen = super::RunsScreen::new(state, cx);
            screen.selected = Some(run);
            screen.tab = super::RunTab::Log;
            screen
        });
        let (_, window) = cx.add_window_view(|window, cx| {
            gpui_kit::component::Root::new(screen.clone(), window, cx)
        });
        window.update(|window, cx| {
            window.render_frame(cx);
            let rail = window
                .find(format!("run-row-{run}"))
                .label()
                .unwrap()
                .to_owned();
            assert!(rail.contains("Run state is unconfirmed"));
            let facts = window.find("run-header-facts").label().unwrap().to_owned();
            assert!(
                facts.contains("Recorded loss of daemon ownership"),
                "known registry fact missing from rendered card: {facts}"
            );
            assert!(facts.contains("current observation is unconfirmed"));
            screen.update(cx, |screen, cx| {
                let (rows, _, _) = screen.rows(cx);
                let row = rows.iter().find(|row| row.run_id == Some(run)).unwrap();
                assert!(!row.live_stream);
                assert!(
                    row.stages
                        .iter()
                        .all(|stage| stage.state != super::StageState::Running)
                );
                assert!(!row.status_label.contains("Running"));
                assert!(!row.waiting_detail.as_ref().unwrap().contains("settled"));
            });
        });
    }

    #[test]
    fn recovery_observability_known_crash_detail_preserves_neutral_fresh_and_terminal_states() {
        use surge_core::EventPayload;
        use surge_orchestrator::engine::handle::{EngineRunEvent, RunStatus};
        let run = surge_core::RunId::new();
        let row_for = |stream: &crate::run_stream::RunStreamState| {
            let mut row = super::RunRow::from_run(
                &crate::app_state::UiRun {
                    run_id: run,
                    status: RunStatus::Active,
                    started_at: chrono::Utc::now(),
                    last_event_seq: None,
                    ended_at: None,
                },
                None,
            );
            row.attach_stream(stream);
            row
        };
        let mut neutral = crate::run_stream::RunStreamState::default();
        neutral.apply(&EngineRunEvent::StreamError {
            message: "read failed".into(),
        });
        let row = row_for(&neutral);
        assert_eq!(row.status_label, "Run state is unconfirmed");
        assert_eq!(
            row.waiting_detail.as_deref(),
            Some("Inspect the run journal before taking action.")
        );
        let mut fresh_crash = crate::run_stream::RunStreamState::default();
        fresh_crash.begin_display_history(run, Some(surge_core::RunStatus::Crashed));
        let row = row_for(&fresh_crash);
        assert_eq!(row.status_label, "Recovery requires an operator");
        assert_eq!(
            row.waiting_detail.as_deref(),
            Some("Inspect the run and choose whether to resume or abort.")
        );
        let mut terminal = crate::run_stream::RunStreamState::default();
        terminal.begin_display_history(run, Some(surge_core::RunStatus::Crashed));
        for (seq, payload) in [
            (
                1,
                EventPayload::RunStarted {
                    pipeline_template: None,
                    project_path: "/project".into(),
                    initial_prompt: "terminal".into(),
                    config: surge_core::RunConfig {
                        budget: Default::default(),
                        sandbox_default: surge_core::sandbox::SandboxMode::WorkspaceWrite,
                        approval_default: surge_core::approvals::ApprovalPolicy::OnRequest,
                        auto_pr: false,
                        mcp_servers: Vec::new(),
                        bootstrap_edit_loop_cap: None,
                    },
                },
            ),
            (
                2,
                EventPayload::RunAborted {
                    reason: "recorded stop".into(),
                },
            ),
        ] {
            terminal.apply(&EngineRunEvent::Persisted {
                seq,
                payload: Box::new(payload),
            });
        }
        terminal.apply(&EngineRunEvent::StreamError {
            message: "later read failed".into(),
        });
        let row = row_for(&terminal);
        assert_eq!(row.status_label, "Aborted");
        assert!(
            row.waiting_detail.is_none(),
            "independent crash/error detail cannot replace durable terminal outcome"
        );
        assert!(!row.live_stream);
    }

    #[test]
    fn recovery_observability_actual_run_card_and_problem_log_use_shared_state() {
        use surge_core::EventPayload;
        use surge_orchestrator::engine::handle::EngineRunEvent;
        let run = surge_core::RunId::new();
        let mut stream = crate::run_stream::RunStreamState::default();
        stream.begin_display_history(run, None);
        for (seq, payload) in [
            (
                1,
                EventPayload::RunStarted {
                    pipeline_template: None,
                    project_path: "/project".into(),
                    initial_prompt: "card".into(),
                    config: surge_core::RunConfig {
                        budget: Default::default(),
                        sandbox_default: surge_core::sandbox::SandboxMode::WorkspaceWrite,
                        approval_default: surge_core::approvals::ApprovalPolicy::OnRequest,
                        auto_pr: false,
                        mcp_servers: Vec::new(),
                        bootstrap_edit_loop_cap: None,
                    },
                },
            ),
            (
                2,
                EventPayload::RunRecoveryRequired {
                    control_generation: 0,
                    diagnostic: "card durable conflict".into(),
                },
            ),
        ] {
            stream.apply(&EngineRunEvent::Persisted {
                seq,
                payload: Box::new(payload),
            });
        }
        stream.finish_display_history(2);
        let mut row = super::RunRow::from_run(
            &crate::app_state::UiRun {
                run_id: run,
                status: surge_orchestrator::engine::handle::RunStatus::Active,
                started_at: chrono::Utc::now(),
                last_event_seq: Some(2),
                ended_at: None,
            },
            None,
        );
        row.attach_stream(&stream);
        assert_eq!(row.status_label, "Recovery requires an operator");
        assert!(!row.live_stream);
        assert!(row.event_rows.iter().any(|row| row.kind == "RECOVERY"
            && row.problem
            && row.text == "card durable conflict"));
        assert!(
            row.stages
                .iter()
                .all(|stage| stage.state != super::StageState::Running)
        );
    }

    #[test]
    fn recovery_observability_unknown_storage_copy_does_not_invent_disk_diagnosis() {
        let (title, detail) = super::attention_reason(
            surge_core::bootstrap_operation::BootstrapAttentionReason::StorageUnconfirmed,
        );
        assert!(title.contains("unconfirmed"));
        assert!(!detail.contains("free disk"));
        assert!(detail.contains("history"));
    }
}

#[cfg(test)]
#[path = "runs/continuation_tests.rs"]
mod continuation_tests;
