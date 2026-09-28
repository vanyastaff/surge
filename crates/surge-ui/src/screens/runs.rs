//! Runs — the per-run cockpit (rail · stage pipeline · event log · steer).
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
#[path = "run_preview.rs"]
mod run_preview;

use std::time::Duration;

use gpui_kit::component::StyledExt;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState};
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
}

impl StageState {
    fn color(self) -> Hsla {
        match self {
            Self::Done => theme::success(),
            Self::Running => theme::accent(),
            Self::Failed => theme::error(),
            Self::Queued => theme::text_muted(),
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
        RunStatus::Awaiting => ("queued", theme::text_muted(), false, 2),
        RunStatus::Completed => ("completed", theme::success(), false, 3),
        RunStatus::Failed => ("failed", theme::error(), false, 0),
        RunStatus::Aborted => ("aborted", theme::error(), false, 0),
        _ => ("unknown", theme::text_muted(), false, 2),
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
    fn from_run(run: &UiRun) -> Self {
        let (status_label, color, active, rank) = status_parts(run.status);
        let events = run
            .last_event_seq
            .map(|s| s.to_string())
            .unwrap_or_else(|| "—".to_string());
        Self {
            run_id: Some(run.run_id),
            id_label: format!("r-{}", run.run_id.short().to_lowercase()),
            title: format!("started {}", local_hm(run.started_at)),
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
            self.title = crate::ui::headline(prompt, 30);
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
    Preview,
    Activity,
    Changes,
    Checks,
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
            tab: RunTab::Activity,
            preview: None,
            checks: None,
            changes: None,
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
            self.tab = RunTab::Activity;
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
                let mut row = RunRow::from_run(run);
                // An acknowledged failure is history, not "needs you".
                if row.rank == 0 && state.dismissed_runs.contains(&run.run_id) {
                    row.rank = 3;
                }
                if let Some(stream) = state.run_streams.get(&run.run_id) {
                    row.attach_stream(stream);
                }
                row
            })
            .collect();
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
            .gap(px(3.0))
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
                .px(px(10.0))
                .py(px(7.0))
                .rounded_md()
                .cursor_pointer()
                .when(is_sel, |el| {
                    el.bg(theme::panel_raised())
                        .border_1()
                        .border_color(theme::hairline_strong())
                })
                .when(!is_sel, |el| {
                    el.hover(|s: StyleRefinement| s.bg(theme::panel_raised().opacity(0.6)))
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
                        .items_center()
                        .child(ui::status_dot(row.color))
                        .child(
                            div()
                                .text_size(px(11.0))
                                .font_weight(FontWeight::BOLD)
                                .text_color(theme::text_primary())
                                .child(row.id_label.clone()),
                        )
                        .child(div().flex_1())
                        .child(
                            div()
                                .text_size(px(9.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(row.color)
                                .child(row.status_label),
                        ),
                )
                .child(
                    div()
                        .h_flex()
                        .gap(px(8.0))
                        .pl(px(15.0))
                        .pt(px(3.0))
                        .child(
                            div()
                                .flex_1()
                                .overflow_hidden()
                                .text_size(px(10.0))
                                .text_color(theme::text_muted())
                                .child(row.title.clone()),
                        )
                        .child(
                            div()
                                .text_size(px(9.5))
                                .text_color(theme::text_muted().opacity(0.8))
                                .child(row.age.clone()),
                        ),
                );
            list = list.child(item);
        }

        div()
            .w(px(264.0))
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
                    .px(px(14.0))
                    .pt(px(14.0))
                    .pb(px(10.0))
                    .child(ui::section_label(format!("RUNS · {}", rows.len())).flex_1())
                    .when(needs > 0, |el| {
                        el.child(ui::pill(
                            format!("{needs} need you"),
                            theme::accent(),
                            theme::accent().opacity(0.14),
                        ))
                    })
                    .when(!live, |el| {
                        el.child(ui::pill(
                            "Disconnected",
                            theme::text_muted(),
                            theme::panel_raised(),
                        ))
                    }),
            )
            .child(list)
    }

    fn render_header(&self, row: &RunRow, live: bool, cx: &mut Context<Self>) -> Div {
        let mut header = div()
            .h_flex()
            .gap(px(12.0))
            .items_center()
            .px(px(20.0))
            .py(px(14.0))
            .border_b_1()
            .border_color(theme::hairline())
            .child(ui::status_dot(row.color))
            .child(
                div()
                    .text_size(px(18.0))
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme::text_primary())
                    .child(row.id_label.clone()),
            )
            .child(ui::pill(
                row.status_label,
                row.color,
                row.color.opacity(0.13),
            ))
            .child(
                div()
                    .text_size(px(11.5))
                    .text_color(theme::text_muted())
                    .child(row.title.clone()),
            )
            .child(div().flex_1());

        if let Some(operation) =
            self.state
                .read(cx)
                .bootstrap_operations
                .iter()
                .find_map(|(id, status)| {
                    (status.planning_run == row.run_id?
                        || status.implementation_run == row.run_id?)
                        .then_some((*id, status.clone()))
                })
        {
            let (operation_id, status) = operation;
            let (label, detail) = match &status.state {
                surge_core::bootstrap_operation::BootstrapState::Pending { phase } => {
                    ("in progress", format!("{phase:?}"))
                },
                surge_core::bootstrap_operation::BootstrapState::NeedsAttention {
                    phase,
                    reason,
                    ..
                } => ("needs attention", format!("{phase:?} · {reason:?}")),
                surge_core::bootstrap_operation::BootstrapState::Cancelling { phase } => {
                    ("cancelling", format!("{phase:?}"))
                },
                surge_core::bootstrap_operation::BootstrapState::Completed => {
                    ("completed", "implementation run settled".into())
                },
                surge_core::bootstrap_operation::BootstrapState::Failed => {
                    ("failed", "operation settled with failure".into())
                },
                surge_core::bootstrap_operation::BootstrapState::Cancelled => {
                    ("cancelled", "operation stopped".into())
                },
            };
            header = header.child(ui::pill(
                label,
                theme::accent(),
                theme::accent().opacity(0.13),
            ));
            header = header.child(
                div()
                    .text_size(px(10.5))
                    .text_color(theme::text_muted())
                    .child(detail),
            );
            if !matches!(
                status.state,
                surge_core::bootstrap_operation::BootstrapState::Completed
                    | surge_core::bootstrap_operation::BootstrapState::Failed
                    | surge_core::bootstrap_operation::BootstrapState::Cancelled
            ) {
                header = header.child(
                    div()
                        .id("cancel-bootstrap")
                        .role(Role::Button)
                        .aria_label("Cancel application workflow")
                        .h_flex()
                        .items_center()
                        .h(px(30.0))
                        .px(px(13.0))
                        .rounded_lg()
                        .border_1()
                        .border_color(theme::error().opacity(0.35))
                        .text_color(theme::error())
                        .text_size(px(11.0))
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _e, _w, cx| {
                            this.cancel_bootstrap(operation_id, cx)
                        }))
                        .child("■ Cancel build"),
                );
            }
        }

        if let Some(run_id) = row.run_id {
            header = header.child(
                Button::new("open-result-folder")
                    .ghost()
                    .label("Open result folder")
                    .accessibility_id("open-result-folder")
                    .debug_selector(|| "open-result-folder".into())
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.open_result_folder(run_id, cx);
                    })),
            );
        }

        if let Some(note) = &self.action_note {
            header = header.child(
                div()
                    .max_w(px(240.0))
                    .truncate()
                    .text_size(px(10.5))
                    .text_color(theme::accent())
                    .child(note.clone()),
            );
        }

        // Stop run — a real facade call; only offered for live active runs.
        if live
            && row.active
            && let Some(run_id) = row.run_id
        {
            header = header.child(
                div()
                    .id("stop-run")
                    .role(Role::Button)
                    .aria_label("Stop run")
                    .h_flex()
                    .gap(px(7.0))
                    .items_center()
                    .h(px(30.0))
                    .px(px(13.0))
                    .rounded_lg()
                    .border_1()
                    .border_color(theme::error().opacity(0.35))
                    .text_color(theme::error().opacity(0.95))
                    .text_size(px(11.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .cursor_pointer()
                    .hover(|s: StyleRefinement| s.border_color(theme::error().opacity(0.7)))
                    .on_click(cx.listener(move |this, _e, _w, cx| {
                        this.stop_run(run_id, cx);
                    }))
                    .child("■ Stop run"),
            );
        }

        header
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

    fn render_kpis(&self, row: &RunRow) -> Div {
        let cell = |label: &'static str, value: String, color: Hsla| {
            div()
                .v_flex()
                .gap(px(3.0))
                .justify_center()
                .pr(px(26.0))
                .child(ui::section_label(label))
                .child(
                    div()
                        .text_size(px(13.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(color)
                        .child(value),
                )
        };
        let sep = || div().w(px(1.0)).my(px(14.0)).bg(theme::hairline());

        let mut strip = div()
            .h(px(60.0))
            .flex_shrink_0()
            .h_flex()
            .px(px(20.0))
            .gap(px(0.0))
            .border_b_1()
            .border_color(theme::hairline())
            .bg(theme::panel())
            .child(cell("STARTED", row.started.clone(), theme::text_primary()))
            .child(sep())
            .child(
                div()
                    .pl(px(26.0))
                    .child(cell("ELAPSED", row.elapsed.clone(), theme::accent())),
            )
            .child(sep())
            .child(div().pl(px(26.0)).child(cell(
                "EVENTS",
                row.events.clone(),
                theme::text_primary(),
            )))
            .child(sep())
            .child(div().pl(px(26.0)).child(cell(
                "STATUS",
                row.status_label.to_uppercase(),
                row.color,
            )));

        // Token/cost counters — only when the live stream reported them.
        if let Some((tokens_in, tokens_out, cost)) = row.usage {
            strip = strip
                .child(sep())
                .child(div().pl(px(26.0)).child(cell(
                    "TOKENS",
                    format!(
                        "{} in · {} out",
                        fmt_tokens(tokens_in),
                        fmt_tokens(tokens_out)
                    ),
                    theme::text_primary(),
                )))
                .child(sep())
                .child(div().pl(px(26.0)).child(cell(
                    "COST",
                    format!("${cost:.2}"),
                    theme::success(),
                )));
        }
        strip
    }

    /// Dot-grid backdrop for the pipeline zone (bounds-driven, so it
    /// fills whatever size flexbox gives the pane).
    fn render_dot_grid(&self) -> impl IntoElement {
        let dot_color = theme::graph_line().opacity(0.5);
        canvas(
            |_bounds, _window, _cx| {},
            move |bounds, _prepaint, window, _cx| {
                let ox = bounds.origin.x;
                let oy = bounds.origin.y;
                let w = f32::from(bounds.size.width);
                let h = f32::from(bounds.size.height);
                let step = 26.0_f32;
                let mut gy = 8.0_f32;
                while gy < h {
                    let mut gx = 8.0_f32;
                    while gx < w {
                        let dot =
                            Bounds::new(point(ox + px(gx), oy + px(gy)), size(px(1.5), px(1.5)));
                        window.paint_quad(fill(dot, dot_color));
                        gx += step;
                    }
                    gy += step;
                }
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
    }

    fn render_stage_chip(&self, stage: &Stage, last: bool) -> Div {
        let color = stage.state.color();
        let is_hot = matches!(stage.state, StageState::Running);

        let dot: AnyElement = if is_hot {
            ui::status_dot(color)
                .with_animation(
                    SharedString::from(format!("stage-pulse-{}", stage.label)),
                    Animation::new(Duration::from_millis(1400)).repeat(),
                    |el, delta| el.opacity(0.4 + 0.6 * (delta * std::f32::consts::PI).sin()),
                )
                .into_any_element()
        } else {
            ui::status_dot(color).into_any_element()
        };

        let chip = div()
            .h_flex()
            .gap(px(9.0))
            .items_center()
            .px(px(14.0))
            .py(px(10.0))
            .rounded_lg()
            .bg(theme::panel_raised())
            .border_1()
            .border_color(match stage.state {
                StageState::Running => color.opacity(0.5),
                StageState::Failed => color.opacity(0.4),
                _ => theme::hairline(),
            })
            .child(dot)
            .child(
                div()
                    .v_flex()
                    .child(
                        div()
                            .text_size(px(11.5))
                            .font_weight(FontWeight::BOLD)
                            .text_color(if stage.state == StageState::Queued {
                                theme::text_muted()
                            } else {
                                theme::text_primary()
                            })
                            .child(stage.label.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(9.5))
                            .text_color(color.opacity(0.9))
                            .child(stage.sub.clone()),
                    ),
            );

        let mut cell = div().h_flex().items_center().child(chip);
        if !last {
            cell = cell.child(div().w(px(34.0)).h(px(2.0)).bg(
                if stage.state == StageState::Done {
                    theme::success().opacity(0.55)
                } else {
                    theme::graph_line()
                },
            ));
        }
        cell
    }

    fn render_pipeline(&self, row: &RunRow, live: bool) -> Div {
        let n = row.stages.len();
        let chips: Vec<Div> = row
            .stages
            .iter()
            .enumerate()
            .map(|(i, s)| self.render_stage_chip(s, i + 1 == n))
            .collect();

        let legend_item = |color: Hsla, label: &'static str| {
            div()
                .h_flex()
                .gap(px(6.0))
                .items_center()
                .child(ui::status_dot(color))
                .child(
                    div()
                        .text_size(px(9.5))
                        .text_color(theme::text_muted())
                        .child(label),
                )
        };

        div()
            .relative()
            .flex_1()
            .min_h(px(160.0))
            .overflow_hidden()
            .bg(theme::panel_deep())
            .flex()
            .items_center()
            .justify_center()
            .child(self.render_dot_grid())
            .child(div().relative().h_flex().items_center().children(chips))
            // legend (bottom-left)
            .child(
                div()
                    .absolute()
                    .left(px(20.0))
                    .bottom(px(12.0))
                    .h_flex()
                    .gap(px(14.0))
                    .items_center()
                    .px(px(13.0))
                    .py(px(7.0))
                    .rounded_full()
                    .bg(theme::panel().opacity(0.85))
                    .border_1()
                    .border_color(theme::hairline())
                    .child(legend_item(theme::success(), "done"))
                    .child(legend_item(theme::accent(), "running"))
                    .child(legend_item(theme::error(), "failed"))
                    .child(legend_item(theme::text_muted(), "queued")),
            )
            // honesty note (bottom-right)
            .child(
                div()
                    .absolute()
                    .right(px(20.0))
                    .bottom(px(16.0))
                    .text_size(px(9.5))
                    .text_color(theme::text_muted().opacity(0.8))
                    .child(if row.live_stream {
                        "live stage telemetry · since cockpit attach"
                    } else if live {
                        "run lifecycle · select while active for live stage telemetry"
                    } else {
                        "last known run lifecycle · daemon disconnected"
                    }),
            )
    }

    fn render_event_log(&self, row: &RunRow, live: bool) -> Div {
        let mut body = div()
            .flex_1()
            .id("runs-event-log")
            .overflow_y_scroll()
            .v_flex()
            .gap(px(1.0))
            .px(px(20.0))
            .pb(px(12.0));

        if row.event_rows.is_empty() {
            let text = if row.live_stream {
                "Stream attached — events will appear as the run emits them \
                 (no history replay before attach)."
            } else if live {
                "No live stream for this run — it either finished before the \
                 cockpit attached, or is queued."
            } else {
                "No events — queued behind fleet capacity."
            };
            body = body.child(
                div()
                    .py(px(16.0))
                    .text_size(px(11.0))
                    .text_color(theme::text_muted())
                    .child(text),
            );
        } else {
            for (index, e) in row.event_rows.iter().enumerate() {
                body = body.child(
                    div()
                        .id(("run-event", index))
                        .role(Role::Label)
                        .aria_label(format!("{} {} {}", e.t, e.kind, e.text))
                        .h_flex()
                        .gap(px(12.0))
                        .items_center()
                        .py(px(4.0))
                        .child(
                            div()
                                .w(px(54.0))
                                .flex_shrink_0()
                                .whitespace_nowrap()
                                .text_size(px(10.0))
                                .text_color(theme::text_muted().opacity(0.8))
                                .child(e.t.clone()),
                        )
                        .child(ui::status_dot(e.color))
                        .child(
                            div()
                                .w(px(60.0))
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
                                .overflow_hidden()
                                .text_size(px(11.5))
                                .text_color(theme::text_primary().opacity(0.9))
                                .child(e.text.clone()),
                        ),
                );
            }
        }

        div()
            .h(px(190.0))
            .flex_shrink_0()
            .v_flex()
            .bg(theme::panel())
            .border_t_1()
            .border_color(theme::hairline())
            .child(
                div()
                    .h_flex()
                    .gap(px(12.0))
                    .items_center()
                    .px(px(20.0))
                    .pt(px(11.0))
                    .pb(px(9.0))
                    .child(ui::section_label("EVENT LOG"))
                    .child(ui::meta("event-sourced · replayable"))
                    .child(div().flex_1())
                    .when(row.live_stream, |el| {
                        el.child(
                            div()
                                .h_flex()
                                .gap(px(5.0))
                                .items_center()
                                .px(px(10.0))
                                .py(px(3.0))
                                .rounded_md()
                                .bg(theme::accent().opacity(0.1))
                                .border_1()
                                .border_color(theme::accent().opacity(0.28))
                                .child(
                                    ui::status_dot(theme::accent())
                                        .with_animation(
                                            "live-tail-pulse",
                                            Animation::new(Duration::from_millis(1400)).repeat(),
                                            |el, delta| {
                                                el.opacity(
                                                    0.35 + 0.65
                                                        * (delta * std::f32::consts::PI).sin(),
                                                )
                                            },
                                        )
                                        .into_any_element(),
                                )
                                .child(
                                    div()
                                        .text_size(px(9.5))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(theme::accent())
                                        .child("LIVE TAIL"),
                                ),
                        )
                    })
                    .when(live && !row.live_stream, |el| {
                        el.child(ui::pill(
                            "LIFECYCLE FEED",
                            theme::text_muted(),
                            theme::panel_raised(),
                        ))
                    }),
            )
            .child(body)
    }

    fn render_steer_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        if self.steer_input.is_none() {
            let input = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("Steer this run — delivered at the next stage boundary…")
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
                    .rounded_lg()
                    .bg(theme::panel_deep())
                    .border_1()
                    .border_color(theme::hairline_strong())
                    .child(ui::pill(
                        "steer",
                        theme::accent(),
                        theme::accent().opacity(0.12),
                    ))
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
        let selected_bootstrap =
            selected.as_ref().and_then(|row| {
                let id = row.run_id?;
                self.state.read(cx).bootstrap_operations.iter().find_map(
                    |(operation_id, status)| {
                        (status.planning_run == id || status.implementation_run == id)
                            .then_some((*operation_id, status.clone()))
                    },
                )
            });

        let mut main = div().flex_1().min_w_0().v_flex();
        if let Some(row) = &selected {
            main = main
                .child(self.render_header(row, live, cx))
                .child(self.render_kpis(row))
                .when_some(selected_bootstrap.as_ref(), |el, (operation_id, status)| {
                    let operation_id = *operation_id;
                    let phase = match &status.state {
                        surge_core::bootstrap_operation::BootstrapState::Pending { phase } => {
                            format!("In progress · {phase:?}")
                        },
                        surge_core::bootstrap_operation::BootstrapState::NeedsAttention {
                            phase,
                            reason,
                            ..
                        } => format!("Needs attention · {phase:?} · {reason:?}"),
                        surge_core::bootstrap_operation::BootstrapState::Cancelling { phase } => {
                            format!("Cancelling · {phase:?}")
                        },
                        surge_core::bootstrap_operation::BootstrapState::Completed => {
                            "Completed · implementation verified by durable engine outcome".into()
                        },
                        surge_core::bootstrap_operation::BootstrapState::Failed => {
                            "Failed · inspect checks and run evidence".into()
                        },
                        surge_core::bootstrap_operation::BootstrapState::Cancelled => {
                            "Cancelled · no further run will launch".into()
                        },
                    };
                    let active = status.state.phase().is_some();
                    let retry_revision = status.revision;
                    el.child(
                        div()
                            .id("bootstrap-operation")
                            .v_flex()
                            .gap(px(8.0))
                            .px(px(20.0))
                            .py(px(12.0))
                            .border_b_1()
                            .border_color(theme::hairline())
                            .bg(theme::panel_deep())
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme::text_primary())
                                    .child("Application workflow"),
                            )
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(theme::text_muted())
                                    .child(phase),
                            )
                            .when(
                                matches!(status.state, surge_core::bootstrap_operation::BootstrapState::NeedsAttention { .. })
                                    && !status.cancel_requested,
                                |panel| panel.child(
                                    div()
                                        .id("retry-bootstrap")
                                        .role(Role::Button)
                                        .aria_label("Retry application workflow")
                                        .h_flex().items_center().h(px(30.0)).px(px(13.0)).rounded_lg()
                                        .border_1().border_color(theme::accent().opacity(0.35)).text_color(theme::accent())
                                        .text_size(px(11.0)).cursor_pointer()
                                        .on_click(cx.listener(move |this, _e, _w, cx| {
                                            this.retry_bootstrap(operation_id, retry_revision, cx)
                                        }))
                                        .child("↻ Retry after repair"),
                                ),
                            )
                            .when(active, |panel| {
                                panel.child(
                                    div()
                                        .id("cancel-bootstrap")
                                        .role(Role::Button)
                                        .aria_label("Cancel application workflow")
                                        .h_flex()
                                        .items_center()
                                        .h(px(30.0))
                                        .px(px(13.0))
                                        .rounded_lg()
                                        .border_1()
                                        .border_color(theme::error().opacity(0.35))
                                        .text_color(theme::error())
                                        .text_size(px(11.0))
                                        .cursor_pointer()
                                        .on_click(cx.listener(move |this, _e, _w, cx| {
                                            this.cancel_bootstrap(operation_id, cx)
                                        }))
                                        .child("■ Cancel build"),
                                )
                            }),
                    )
                })
                ;
            let mut tabs = div().h_flex().gap(px(8.0)).px(px(20.0)).py(px(6.0));
            for (tab, label) in [
                (RunTab::Preview, "Preview"),
                (RunTab::Activity, "Activity"),
                (RunTab::Changes, "Changes"),
                (RunTab::Checks, "Checks"),
            ] {
                tabs = tabs.child(
                    Button::new(SharedString::from(format!("run-tab-{label}")))
                        .ghost()
                        .label(label)
                        .when(self.tab == tab, |button| button.primary())
                        .on_click(cx.listener(move |screen, _, _, cx| {
                            if screen.tab != tab {
                                screen.clear_preview(cx);
                            }
                            screen.tab = tab;
                            cx.notify();
                        })),
                );
            }
            main = main.child(tabs);
            if self.tab == RunTab::Preview {
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
            } else if self.tab == RunTab::Checks {
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
            } else if self.tab == RunTab::Changes {
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
            } else {
                main = main
                    .child(self.render_pipeline(row, live))
                    .child(self.render_event_log(row, live));
            }
        } else {
            self.clear_preview(cx);
            main = main.child(
                div().flex_1().flex().items_center().justify_center().child(
                    div()
                        .text_size(px(12.0))
                        .text_color(theme::text_muted())
                        .child("No runs yet — describe an application in Fleet to begin."),
                ),
            );
        }

        div()
            .size_full()
            .v_flex()
            .child(
                // Plain .flex() row — gpui-component's h_flex() would
                // items_center the panes instead of stretching them to
                // full height.
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(self.render_rail(&rows, sel, live, cx))
                    .child(main),
            )
            .child(self.render_steer_bar(window, cx))
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
