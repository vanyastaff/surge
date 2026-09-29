//! Missions — one idea, planned and built (rail · overview · preview ·
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

#[path = "run_changes.rs"]
mod run_changes;
#[path = "run_checks.rs"]
mod run_checks;
#[path = "run_mission.rs"]
mod run_mission;
#[path = "run_preview.rs"]
mod run_preview;

use std::time::Duration;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Icon, IconName, Sizable, StyledExt};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use surge_core::id::RunId;
use surge_orchestrator::engine::facade::EngineFacade as _;
use surge_orchestrator::engine::handle::RunStatus;

use crate::app_state::{AppState, UiRun};
use crate::theme;
use crate::ui;

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
    /// (tokens_in, tokens_out, cost_usd) accumulated from the stream.
    usage: Option<(u64, u64, f64)>,
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
        if let Some(prompt) = &stream.prompt {
            self.title = crate::ui::headline(prompt, 90);
        }
        if !stream.stages.is_empty() {
            self.stages = stream.stages.iter().map(stage_from_stream).collect();
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
        if stream.tokens_in > 0 || stream.tokens_out > 0 || stream.cost_usd > 0.0 {
            self.usage = Some((stream.tokens_in, stream.tokens_out, stream.cost_usd));
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
enum RunTab {
    /// The mission level by level (default).
    Overview,
    Preview,
    Changes,
    Checks,
    /// Raw steps and events, for developers.
    Log,
}

/// Runs screen — daemon-run cockpit.
pub struct RunsScreen {
    state: Entity<AppState>,
    /// Selected real run; falls back to the most attention-worthy.
    selected: Option<RunId>,
    steer_input: Option<Entity<InputState>>,
    /// One-line feedback from the last facade call (honest, verbatim).
    action_note: Option<String>,
    tab: RunTab,
    preview: Option<(RunId, Entity<run_preview::PreviewView>)>,
    checks: Option<(RunId, Entity<run_checks::ChecksView>)>,
    changes: Option<(RunId, Entity<run_changes::ChangesView>)>,
    mission: Option<Entity<run_mission::MissionPanel>>,
    log_filter: LogFilter,
    steps_scroll: ScrollHandle,
    /// (run, step count) last scrolled to, so a new step scrolls once.
    steps_seen: (Option<RunId>, usize),
}

/// What the Runs screen asks the app to do.
#[derive(Clone, Debug, PartialEq)]
pub enum RunsEvent {
    /// Empty state: go describe an app.
    DescribeApp,
}

impl EventEmitter<RunsEvent> for RunsScreen {}

/// Plain-language bootstrap phase.
fn phase_label(phase: surge_core::bootstrap_operation::BootstrapPhase) -> &'static str {
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
fn attention_reason(
    reason: surge_core::bootstrap_operation::BootstrapAttentionReason,
) -> (&'static str, &'static str) {
    use surge_core::bootstrap_operation::BootstrapAttentionReason as R;
    match reason {
        R::ConfigurationChanged => (
            "Settings changed since this build started",
            "Restore the agent settings it started with, then retry.",
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
            "Check the run's worktree in Worktrees, then retry.",
        ),
        R::InvalidMaterialization => (
            "The plan could not be validated",
            "Open Flow to inspect the plan, then retry.",
        ),
        R::BudgetUnconfirmed => (
            "The remaining budget could not be confirmed",
            "Check the agent's quota or budget, then retry.",
        ),
        R::StorageUnconfirmed => (
            "The result could not be recorded",
            "Check free disk space in the Surge home folder, then retry.",
        ),
    }
}

impl RunsScreen {
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
            steer_input: None,
            action_note: None,
            tab: RunTab::Overview,
            preview: None,
            checks: None,
            changes: None,
            mission: None,
            log_filter: LogFilter::All,
            steps_scroll: ScrollHandle::new(),
            steps_seen: (None, 0),
        }
    }

    fn clear_preview(&mut self, cx: &mut Context<Self>) {
        if let Some((_, preview)) = self.preview.take() {
            preview.update(cx, |view, cx| view.close(cx));
        }
    }

    pub fn close_preview(&mut self, cx: &mut Context<Self>) {
        self.clear_preview(cx);
        if self.tab == RunTab::Preview {
            self.tab = RunTab::Overview;
        }
        cx.notify();
    }

    /// Focus a specific run (used by Fleet → "Open run" deep links).
    pub fn select_run(&mut self, run_id: RunId, cx: &mut Context<Self>) {
        self.clear_preview(cx);
        self.selected = Some(run_id);
        cx.notify();
    }

    /// Actual daemon runs and the selected row index.
    fn rows(&self, cx: &Context<Self>) -> (Vec<RunRow>, usize, bool) {
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
        // One idea is one entry: a planning run whose build run is listed is
        // shown through that build run (Overview shows both).
        let listed: std::collections::HashSet<RunId> =
            rows.iter().filter_map(|r| r.run_id).collect();
        rows.retain(|r| {
            let Some(id) = r.run_id else { return true };
            !state.bootstrap_operations.values().any(|op| {
                op.planning_run == id
                    && op.implementation_run != id
                    && listed.contains(&op.implementation_run)
            })
        });
        rows.sort_by_key(|r| r.rank);
        let sel = self
            .selected
            .and_then(|id| rows.iter().position(|r| r.run_id == Some(id)))
            .unwrap_or(0);
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

    fn submit_steer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(input) = self.steer_input.clone() else {
            return;
        };
        let message = input.read(cx).value().trim().to_string();
        if message.is_empty() {
            return;
        }

        let (rows, sel, live) = self.rows(cx);
        let Some(row) = rows.get(sel) else { return };
        if !live || !row.active {
            self.action_note = Some("steer needs a live, active run".into());
            cx.notify();
            return;
        }
        let Some(run_id) = row.run_id else { return };
        let Some(facade) = self.state.read(cx).daemon_state.facade() else {
            self.action_note = Some("daemon offline — cannot steer".into());
            cx.notify();
            return;
        };

        input.update(cx, |s, cx| s.set_value("", window, cx));
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let note = match facade.submit_steer(run_id, message).await {
                Ok(steer_id) => format!("steer queued · {steer_id}"),
                Err(e) => format!("steer failed: {e}"),
            };
            cx.update(|cx| {
                let _ = this.update(cx, |t, cx| {
                    t.action_note = Some(note);
                    cx.notify();
                });
            });
        })
        .detach();
    }

    // ── panes ───────────────────────────────────────────────────────

    fn render_rail(&self, rows: &[RunRow], sel: usize, live: bool, cx: &mut Context<Self>) -> Div {
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
            let is_sel = i == sel;
            let run_id = row.run_id;
            let item = div()
                .id(SharedString::from(format!(
                    "run-row-{}",
                    row.run_id
                        .map_or_else(|| row.id_label.clone(), |id| id.to_string())
                )))
                .role(Role::Button)
                .aria_label(format!("{}, {}", row.title, row.status_label))
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
                    this.clear_preview(cx);
                    this.selected = run_id;
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
                    .child(ui::section_label(format!("Missions · {}", rows.len())).flex_1())
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
        self.state
            .read(cx)
            .bootstrap_operations
            .iter()
            .find_map(|(operation_id, status)| {
                (status.planning_run == id || status.implementation_run == id)
                    .then_some((*operation_id, status.clone()))
            })
    }

    /// The planning + implementation runs this row stands for.
    fn mission_runs(&self, row: &RunRow, cx: &Context<Self>) -> Option<run_mission::MissionRuns> {
        let run = row.run_id?;
        Some(match self.selected_bootstrap(row, cx) {
            Some((_, status)) => run_mission::MissionRuns {
                planning: Some(status.planning_run),
                implementation: status.implementation_run,
            },
            None => run_mission::MissionRuns {
                planning: None,
                implementation: run,
            },
        })
    }

    /// Title, one status, one line of facts, and the actions — nothing
    /// said twice.
    fn render_header(&self, row: &RunRow, live: bool, cx: &mut Context<Self>) -> Div {
        use surge_core::bootstrap_operation::BootstrapState as B;
        let bootstrap = self.selected_bootstrap(row, cx);

        // One status: the build's when this run belongs to one, else the run's.
        let (status_text, status_color) = match bootstrap.as_ref().map(|(_, s)| &s.state) {
            Some(B::Pending { phase }) => (phase_label(*phase).to_string(), theme::accent()),
            Some(B::NeedsAttention { .. }) => ("needs you".to_string(), theme::warning()),
            Some(B::Cancelling { .. }) => ("cancelling".to_string(), theme::warning()),
            _ => (row.status_label.to_string(), row.color),
        };
        let cancellable = bootstrap
            .as_ref()
            .is_some_and(|(_, s)| s.state.phase().is_some() && !s.cancel_requested);

        let mut facts: Vec<String> = vec![
            row.id_label.clone(),
            format!("started {}", row.started),
            format!("{} elapsed", row.elapsed),
            format!("{} events", row.events),
        ];
        if let Some((tokens_in, tokens_out, cost)) = row.usage {
            facts.push(format!(
                "{} in · {} out tokens",
                fmt_tokens(tokens_in),
                fmt_tokens(tokens_out)
            ));
            facts.push(format!("${cost:.2}"));
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
                            .child(ui::pill(
                                status_text,
                                status_color,
                                theme::tint(status_color),
                            )),
                    )
                    .child(
                        div()
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
        let B::NeedsAttention { phase, reason, .. } = status.state else {
            return None;
        };
        let (what, fix) = attention_reason(reason);
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
                        ),
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
            (RunTab::Preview, "Preview", Lucide::AppWindow),
            (RunTab::Changes, "Changes", Lucide::GitCompare),
            (RunTab::Checks, "Checks", Lucide::ShieldCheck),
            (RunTab::Log, "Log", Lucide::ScrollText),
        ] {
            let active = self.tab == tab;
            tabs = tabs.child(
                div()
                    .id(SharedString::from(format!("run-tab-{label}")))
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
                        if screen.tab != tab {
                            screen.clear_preview(cx);
                        }
                        screen.tab = tab;
                        cx.notify();
                    }))
                    .child(Icon::new(icon).size(px(13.0)))
                    .child(label),
            );
        }
        tabs
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

    fn render_steer_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        if self.steer_input.is_none() {
            let input = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("Tell the agent something — it reads it before the next step…")
            });
            cx.subscribe_in(
                &input,
                window,
                |this: &mut Self, _input, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::PressEnter { .. }) {
                        this.submit_steer(window, cx);
                    }
                },
            )
            .detach();
            self.steer_input = Some(input);
        }

        div()
            .h(px(58.0))
            .flex_shrink_0()
            .h_flex()
            .gap(px(12.0))
            .items_center()
            .px(px(16.0))
            .bg(theme::panel())
            .border_t_1()
            .border_color(theme::hairline())
            .child(
                div()
                    .flex_1()
                    .h_flex()
                    .gap(px(10.0))
                    .items_center()
                    .h(px(36.0))
                    .px(px(12.0))
                    .rounded(px(ui::R_CONTROL + 2.0))
                    .bg(theme::panel_deep())
                    .border_1()
                    .border_color(theme::hairline_strong())
                    .child(ui::role_badge("steer", theme::Semantic::You))
                    .child(
                        div().flex_1().child(
                            Input::new(self.steer_input.as_ref().unwrap())
                                .accessibility_id("steer-run")
                                .aria_label("Steer active run")
                                .appearance(false),
                        ),
                    )
                    .child(ui::kbd("↵")),
            )
    }
}

impl Render for RunsScreen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (rows, sel, live) = self.rows(cx);
        let selected = rows.get(sel).cloned();

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
                RunTab::Preview => {
                    if let Some(run_id) = row.run_id {
                        if self.preview.as_ref().is_none_or(|(id, _)| *id != run_id) {
                            self.clear_preview(cx);
                            self.preview = Some((
                                run_id,
                                cx.new(|cx| run_preview::PreviewView::new(run_id, window, cx)),
                            ));
                        }
                        if let Some((_, preview)) = &self.preview {
                            main = main.child(preview.clone());
                        }
                    }
                },
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
                RunTab::Overview => {
                    if let Some(runs) = self.mission_runs(row, cx) {
                        let stale = self
                            .mission
                            .as_ref()
                            .is_none_or(|panel| panel.read(cx).runs() != runs);
                        if stale {
                            self.mission =
                                Some(cx.new(|cx| run_mission::MissionPanel::new(runs, cx)));
                        }
                        if let Some(panel) = &self.mission {
                            let seq = row.events.clone();
                            panel.update(cx, |panel, cx| panel.sync(&seq, cx));
                            main = main.child(panel.clone());
                        }
                    }
                },
            }
        } else {
            self.clear_preview(cx);
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
            .when(steerable, |el| el.child(self.render_steer_bar(window, cx)))
    }
}

#[cfg(test)]
mod empty_state_tests {
    use super::RunsScreen;
    use crate::app_state::AppState;
    use gpui_kit::{AppContext, TestAppContext};

    #[gpui_kit::test]
    fn empty_cockpit_never_invents_runs(cx: &mut TestAppContext) {
        let screen = cx.update(|cx| {
            let state = cx.new(|_| AppState::new());
            cx.new(|cx| RunsScreen::new(state, cx))
        });
        screen.update(cx, |screen, cx| {
            let (rows, selected, connected) = screen.rows(cx);
            assert!(rows.is_empty());
            assert_eq!(selected, 0);
            assert!(!connected);
        });
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
        let home = tempfile::tempdir().unwrap();
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
