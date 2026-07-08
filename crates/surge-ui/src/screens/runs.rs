//! Runs — the per-run cockpit (rail · stage pipeline · event log · steer).
//!
//! Adapted from the "Surge - Interactive" concept. Everything rendered
//! from live data is real: the run rail and KPI strip come from
//! [`AppState::runs`] (daemon `list_runs` + global events), **Stop run**
//! calls `EngineFacade::stop_run`, and the steer bar queues a real
//! operator message via `EngineFacade::submit_steer`.
//!
//! What the daemon does NOT expose yet (per-run stage/event streaming —
//! `daemon_link.rs` stops at global lifecycle events) is shown honestly:
//! the pipeline renders the run *lifecycle* (accepted → executing →
//! terminal) rather than fake graph stages, and the event pane says so.
//! When there are no runs at all, a clearly-labelled sample cockpit
//! shows the design intent offline (same policy as the Fleet screen).

use std::time::Duration;

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::StyledExt;
use gpui_component::input::{Input, InputEvent, InputState};
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
    Gate,
    Failed,
    Queued,
}

impl StageState {
    fn color(self) -> Hsla {
        match self {
            Self::Done => theme::success(),
            Self::Running | Self::Gate => theme::accent(),
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

/// Row model for the left rail + detail header, built either from a
/// real [`UiRun`] or from the sample set.
#[derive(Clone)]
struct RunRow {
    /// Real daemon run id (None in sample mode).
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

fn humanize_elapsed(started: chrono::DateTime<chrono::Utc>) -> String {
    let secs = (chrono::Utc::now() - started).num_seconds().max(0);
    if secs < 3600 {
        format!("{}:{:02}", secs / 60, secs % 60)
    } else {
        format!("{}:{:02}:{:02}", secs / 3600, (secs % 3600) / 60, secs % 60)
    }
}

/// Lifecycle pipeline for a real run: what we truthfully know from
/// `RunStatus` alone. Per-node graph stages arrive with the per-run
/// event stream (follow-up daemon phase).
fn lifecycle_stages(run: &UiRun) -> Vec<Stage> {
    let accepted_sub = run.started_at.format("%H:%M").to_string();
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
            title: format!("started {}", run.started_at.format("%H:%M")),
            age: humanize_age(run.started_at),
            status_label,
            color,
            active,
            rank,
            started: run.started_at.format("%H:%M").to_string(),
            elapsed: humanize_elapsed(run.started_at),
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

/// Runs screen — daemon-run cockpit.
pub struct RunsScreen {
    state: Entity<AppState>,
    /// Selected real run; falls back to the most attention-worthy.
    selected: Option<RunId>,
    /// Selection within the sample set (offline mode).
    sample_selected: usize,
    steer_input: Option<Entity<InputState>>,
    /// One-line feedback from the last facade call (honest, verbatim).
    action_note: Option<String>,
}

impl RunsScreen {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        cx.observe(&state, |_this, _state, cx| cx.notify()).detach();

        // 1s ticker so ELAPSED / age counters move while runs are active.
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let alive = cx.update(|cx| this.update(cx, |_, cx| cx.notify()).is_ok());
                if !matches!(alive, Ok(true)) {
                    break;
                }
            }
        })
        .detach();

        Self {
            state,
            selected: None,
            sample_selected: 0,
            steer_input: None,
            action_note: None,
        }
    }

    /// Focus a specific run (used by Fleet → "Open run" deep links).
    pub fn select_run(&mut self, run_id: RunId, cx: &mut Context<Self>) {
        self.selected = Some(run_id);
        cx.notify();
    }

    /// Rail rows + selected index. Real when any runs exist, else sample.
    fn rows(&self, cx: &Context<Self>) -> (Vec<RunRow>, usize, bool) {
        let state = self.state.read(cx);
        if state.runs.is_empty() {
            let rows = sample_rows();
            let sel = self.sample_selected.min(rows.len().saturating_sub(1));
            return (rows, sel, false);
        }

        let mut rows: Vec<RunRow> = state
            .runs
            .iter()
            .map(|run| {
                let mut row = RunRow::from_run(run);
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
        (rows, sel, true)
    }

    // ── facade actions ──────────────────────────────────────────────

    fn stop_run(&mut self, run_id: RunId, cx: &mut Context<Self>) {
        let Some(facade) = self.state.read(cx).daemon_state.facade() else {
            self.action_note = Some("daemon offline — cannot stop".into());
            cx.notify();
            return;
        };
        let state = self.state.clone();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let note = match facade
                .stop_run(run_id, "stopped from Runs cockpit".to_string())
                .await
            {
                Ok(()) => format!("stop requested · {}", run_id.short().to_lowercase()),
                Err(e) => format!("stop failed: {e}"),
            };
            // Refresh the run list so the rail reflects the new status.
            if let Ok(summaries) = facade.list_runs().await {
                let _ = cx.update(|cx| {
                    state.update(cx, |s, cx| {
                        s.set_runs_from_summaries(&summaries);
                        cx.notify();
                    });
                });
            }
            let _ = cx.update(|cx| {
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
            let _ = cx.update(|cx| {
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
                .id(SharedString::from(format!("run-row-{i}")))
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
                    match run_id {
                        Some(id) => this.selected = Some(id),
                        None => this.sample_selected = i,
                    }
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
                            "sample",
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

        if let Some(note) = &self.action_note {
            header = header.child(
                div()
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
                    format!("{}k in · {}k out", tokens_in / 1000, tokens_out / 1000),
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
        let is_hot = matches!(stage.state, StageState::Running | StageState::Gate);

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
                StageState::Running | StageState::Gate => color.opacity(0.5),
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
                        "sample pipeline · start the daemon for live runs"
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
            for e in &row.event_rows {
                body = body.child(
                    div()
                        .h_flex()
                        .gap(px(12.0))
                        .items_center()
                        .py(px(4.0))
                        .child(
                            div()
                                .w(px(38.0))
                                .flex_shrink_0()
                                .text_size(px(10.0))
                                .text_color(theme::text_muted().opacity(0.8))
                                .child(e.t.clone()),
                        )
                        .child(ui::status_dot(e.color))
                        .child(
                            div()
                                .w(px(48.0))
                                .flex_shrink_0()
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
                            Input::new(self.steer_input.as_ref().unwrap()).appearance(false),
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

        let mut main = div().flex_1().min_w_0().v_flex();
        if let Some(row) = &selected {
            main = main
                .child(self.render_header(row, live, cx))
                .child(self.render_kpis(row))
                .child(self.render_pipeline(row, live))
                .child(self.render_event_log(row, live));
        } else {
            main = main.child(
                div().flex_1().flex().items_center().justify_center().child(
                    div()
                        .text_size(px(12.0))
                        .text_color(theme::text_muted())
                        .child("No runs yet — dispatch something from the Backlog."),
                ),
            );
        }

        div()
            .size_full()
            .v_flex()
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .h_flex()
                    .child(self.render_rail(&rows, sel, live, cx))
                    .child(main),
            )
            .child(self.render_steer_bar(window, cx))
    }
}

/// Clearly-labelled sample cockpit (offline / no daemon runs). Mirrors
/// the concept's run set so the three-pane idea reads without live data.
fn sample_rows() -> Vec<RunRow> {
    let amber = theme::accent();
    let green = theme::success();
    let red = theme::error();
    let muted = theme::text_muted();

    let stages = |list: Vec<(&'static str, &str, StageState)>| -> Vec<Stage> {
        list.into_iter()
            .map(|(label, sub, state)| Stage {
                label: label.to_string(),
                sub: sub.to_string(),
                state,
            })
            .collect()
    };
    let ev = |t: &str, kind: &'static str, color: Hsla, text: &str| EventRow {
        t: t.to_string(),
        kind,
        color,
        text: text.to_string(),
    };

    vec![
        RunRow {
            run_id: None,
            id_label: "r-9c1e".into(),
            title: "Rate limiter middleware".into(),
            age: "15m".into(),
            status_label: "review gate",
            color: amber,
            active: false,
            rank: 0,
            started: "14:21".into(),
            elapsed: "41:12".into(),
            events: "38".into(),
            stages: stages(vec![
                ("intake", "done", StageState::Done),
                ("plan gate", "approved", StageState::Done),
                ("implement", "claude-1 · done", StageState::Done),
                ("qa suite", "42/42", StageState::Done),
                ("review gate", "needs you", StageState::Gate),
                ("merge", "queued", StageState::Queued),
            ]),
            event_rows: vec![
                ev("14:36", "TEST", green, "Tests passed — 42/42 green"),
                ev(
                    "14:34",
                    "WRITE",
                    amber,
                    "Wrote src/middleware/rate_limit.rs +186",
                ),
                ev("14:31", "LINT", green, "Lint clean"),
                ev("14:28", "PLAN", amber, "Plan approved at gate — 6 stages"),
                ev("14:21", "START", muted, "Run accepted · worktree wt-9c1e"),
            ],
            live_stream: false,
            usage: None,
        },
        RunRow {
            run_id: None,
            id_label: "r-b2e8".into(),
            title: "Retry logic patch".into(),
            age: "8m".into(),
            status_label: "active",
            color: amber,
            active: true,
            rank: 1,
            started: "14:29".into(),
            elapsed: "12:44".into(),
            events: "21".into(),
            stages: stages(vec![
                ("intake", "done", StageState::Done),
                ("plan gate", "approved", StageState::Done),
                ("implement", "gpt-runner", StageState::Running),
                ("qa suite", "5/6", StageState::Queued),
                ("review gate", "pending", StageState::Queued),
                ("merge", "queued", StageState::Queued),
            ]),
            event_rows: vec![
                ev("14:41", "WRITE", amber, "Patching src/retry/backoff.rs"),
                ev("14:38", "READ", muted, "Scanning call sites of retry()"),
                ev("14:29", "START", muted, "Run accepted · worktree wt-b2e8"),
            ],
            live_stream: false,
            usage: None,
        },
        RunRow {
            run_id: None,
            id_label: "r-77b0".into(),
            title: "Config loader refactor".into(),
            age: "32m".into(),
            status_label: "failed",
            color: red,
            active: false,
            rank: 0,
            started: "13:58".into(),
            elapsed: "17:03".into(),
            events: "29".into(),
            stages: stages(vec![
                ("intake", "done", StageState::Done),
                ("plan gate", "approved", StageState::Done),
                ("implement", "claude-1 · done", StageState::Done),
                ("qa suite", "3/6 failing", StageState::Failed),
                ("review gate", "blocked", StageState::Queued),
                ("merge", "blocked", StageState::Queued),
            ]),
            event_rows: vec![
                ev(
                    "14:15",
                    "FAIL",
                    red,
                    "qa suite failed — 3/6: config_env round-trip",
                ),
                ev(
                    "14:12",
                    "TEST",
                    red,
                    "test_env_override ✗ expected \"prod\", got \"dev\"",
                ),
                ev("13:58", "START", muted, "Run accepted · worktree wt-77b0"),
            ],
            live_stream: false,
            usage: None,
        },
        RunRow {
            run_id: None,
            id_label: "r-4f2a".into(),
            title: "Session cache".into(),
            age: "2h".into(),
            status_label: "merged",
            color: green,
            active: false,
            rank: 3,
            started: "12:02".into(),
            elapsed: "38:20".into(),
            events: "54".into(),
            stages: stages(vec![
                ("intake", "done", StageState::Done),
                ("plan gate", "approved", StageState::Done),
                ("implement", "claude-1 · done", StageState::Done),
                ("qa suite", "31/31", StageState::Done),
                ("review gate", "approved", StageState::Done),
                ("merge", "→ main", StageState::Done),
            ]),
            event_rows: vec![
                ev("12:40", "MERGE", green, "Merged to main · +214 −40"),
                ev("12:31", "GATE", green, "Review approved by operator"),
                ev("12:02", "START", muted, "Run accepted · worktree wt-4f2a"),
            ],
            live_stream: false,
            usage: None,
        },
        RunRow {
            run_id: None,
            id_label: "r-c3d9".into(),
            title: "CSV import v2".into(),
            age: "1m".into(),
            status_label: "queued",
            color: muted,
            active: false,
            rank: 2,
            started: "14:41".into(),
            elapsed: "0:48".into(),
            events: "0".into(),
            stages: stages(vec![
                ("intake", "queued", StageState::Queued),
                ("plan gate", "—", StageState::Queued),
                ("implement", "—", StageState::Queued),
                ("qa suite", "—", StageState::Queued),
                ("review gate", "—", StageState::Queued),
                ("merge", "—", StageState::Queued),
            ]),
            event_rows: Vec::new(),
            live_stream: false,
            usage: None,
        },
    ]
}
