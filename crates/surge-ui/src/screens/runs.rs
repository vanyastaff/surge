//! Runs — the per-run cockpit (rail · stage pipeline · event log ·
//! artifacts · steer).
//!
//! Everything rendered from live data is real: the run rail and KPI
//! strip come from [`AppState::runs`] (daemon `list_runs` + global
//! events), **Stop run** calls `EngineFacade::stop_run`, and the steer
//! bar queues a real operator message via `EngineFacade::submit_steer`.
//!
//! The stage pipeline and event log come from two honest sources:
//! the **live per-run stream** when attached (folded telemetry), or a
//! **durable replay** ([`crate::replay_link`]) folded from the run's
//! event log in `~/.surge/runs/<id>/` — loaded in the background for
//! every selected run without a live stream, so a run that finished
//! before the cockpit opened is still fully reconstructable. A queued
//! run has no log yet; the pane says so.
//!
//! The artifacts pane lists the run's durable artifacts
//! (`RunReader::artifacts`); clicking one renders its text content
//! inline (UTF-8, size-capped). When there are no runs at all, a
//! clearly-labelled sample cockpit shows the design intent offline.

use std::time::Duration;

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::StyledExt;
use gpui_component::input::{Input, InputEvent, InputState};
use surge_core::id::RunId;
use surge_orchestrator::engine::facade::EngineFacade as _;
use surge_orchestrator::engine::handle::RunStatus;

use crate::app_state::{AppState, UiRun};
use crate::replay_link::{ReplayArtifact, ReplaySnapshot};
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

/// Artifact size label ("1.2 kB", "340 kB", "2.1 MB").
fn format_artifact_size(bytes: u64) -> String {
    if bytes < 1_000 {
        format!("{bytes} B")
    } else if bytes < 1_000_000 {
        format!("{:.1} kB", bytes as f64 / 1000.0)
    } else {
        format!("{:.1} MB", bytes as f64 / 1_000_000.0)
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

    /// Enrich a row with a durable replay (folded event log): same
    /// pipeline/log/usage projection as the live stream. Only fills
    /// gaps — a live stream wins where both exist.
    fn attach_replay(&mut self, replay: &crate::replay_link::ReplaySnapshot) {
        if self.stages.len() <= 1 {
            let stages: Vec<Stage> = replay.stages.iter().map(stage_from_stream).collect();
            if !stages.is_empty() {
                self.stages = stages;
            }
        }
        if self.event_rows.is_empty() && !replay.rows.is_empty() {
            self.event_rows = replay
                .rows
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
        if replay.last_seq > 0 {
            self.events = replay.last_seq.to_string();
        }
        if self.usage.is_none()
            && (replay.tokens_in > 0 || replay.tokens_out > 0 || replay.cost_usd > 0.0)
        {
            self.usage = Some((replay.tokens_in, replay.tokens_out, replay.cost_usd));
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
    /// Durable replays folded from the event log, keyed by run.
    replays: crate::replay_link::ReplayCache,
    /// Replay loads in flight (no double-fetch per render).
    replay_loading: std::collections::HashSet<RunId>,
    /// Runs whose replay load failed (missing log / no runtime) —
    /// retried only on an explicit re-select, not on every render.
    replay_failed: std::collections::HashSet<RunId>,
    /// Artifacts of the selected run, loaded in the background.
    artifacts: Vec<ReplayArtifact>,
    /// Runs whose artifact listing failed — not retried per render.
    artifacts_failed: std::collections::HashSet<RunId>,
    /// Artifact load in flight.
    artifacts_loading: bool,
    /// Text of the currently opened artifact (inline viewer).
    artifact_view: Option<(String, String)>,
    /// Artifact loads in flight.
    artifact_loading: Option<String>,
}

impl RunsScreen {
    /// One ticker beat: notify only while a non-terminal run exists —
    /// terminal-only lists are frozen, so waking the renderer for them
    /// is pure waste.
    fn tick(&mut self, cx: &mut Context<Self>) {
        let ticking = self.state.read(cx).runs.iter().any(|r| !r.is_terminal());
        if ticking {
            cx.notify();
        }
    }

    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        cx.observe(&state, |_this, _state, cx| cx.notify()).detach();

        // 1s ticker so ELAPSED / age counters move.
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let alive = cx.update(|cx| this.update(cx, RunsScreen::tick)).is_ok();
                if !alive {
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
            replays: std::collections::HashMap::new(),
            replay_loading: std::collections::HashSet::new(),
            replay_failed: std::collections::HashSet::new(),
            artifacts: Vec::new(),
            artifacts_failed: std::collections::HashSet::new(),
            artifacts_loading: false,
            artifact_view: None,
            artifact_loading: None,
        }
    }

    /// Focus a specific run (used by Fleet → "Open run" deep links and
    /// rail clicks). Resets per-run panes so a switch does not show the
    /// previous run's artifacts.
    pub fn select_run(&mut self, run_id: RunId, cx: &mut Context<Self>) {
        if self.selected != Some(run_id) {
            self.selected = Some(run_id);
            self.artifact_view = None;
            self.artifacts.clear();
        }
        cx.notify();
    }

    /// Kick a durable replay fold for the selected run when it has no
    /// live stream and no cached replay yet. Runs in the background;
    /// the render path only reads the cache.
    fn ensure_replay(&mut self, run_id: RunId, cx: &mut Context<Self>) {
        let has_live_stream = self
            .state
            .read(cx)
            .run_streams
            .get(&run_id)
            .is_some_and(|s| s.live);
        if has_live_stream
            || self.replays.contains_key(&run_id)
            || self.replay_failed.contains(&run_id)
            || !self.replay_loading.insert(run_id)
        {
            return;
        }
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let snap = crate::replay_link::load_replay(run_id).await;
            cx.update(|cx| {
                let _ = this.update(cx, |screen, cx| screen.apply_replay(run_id, snap, cx));
            });
        })
        .detach();
    }

    /// Store (or log) a finished replay load. A failure is *remembered*
    /// for the session: retrying on every render would spin the loop a
    /// no-runtime / no-log environment lives in.
    fn apply_replay(
        &mut self,
        run_id: RunId,
        snap: Result<ReplaySnapshot, String>,
        cx: &mut Context<Self>,
    ) {
        self.replay_loading.remove(&run_id);
        match snap {
            Ok(s) => {
                self.replays.insert(run_id, s);
            },
            Err(e) => {
                tracing::debug!(run_id = %run_id, "replay load failed: {e}");
                self.replay_failed.insert(run_id);
            },
        }
        cx.notify();
    }

    /// Kick an artifact listing for the selected run.
    fn ensure_artifacts(&mut self, run_id: RunId, cx: &mut Context<Self>) {
        if self.artifacts_loading || self.artifacts_failed.contains(&run_id) {
            return;
        }
        self.artifacts_loading = true;
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let result = crate::replay_link::load_artifacts(run_id).await;
            cx.update(|cx| {
                let _ = this.update(cx, |screen, cx| screen.apply_artifacts(run_id, result, cx));
            });
        })
        .detach();
    }

    /// Store (or log) a finished artifact listing.
    fn apply_artifacts(
        &mut self,
        run_id: RunId,
        result: Result<Vec<ReplayArtifact>, String>,
        cx: &mut Context<Self>,
    ) {
        self.artifacts_loading = false;
        match result {
            Ok(a) => self.artifacts = a,
            Err(e) => {
                tracing::debug!(run_id = %run_id, "artifact list failed: {e}");
                self.artifacts.clear();
                self.artifacts_failed.insert(run_id);
            },
        }
        cx.notify();
    }

    /// Open one artifact's text inline (background load, capped size).
    fn open_artifact(&mut self, run_id: RunId, artifact: &ReplayArtifact, cx: &mut Context<Self>) {
        let hash = artifact.content_hash.clone();
        let name = artifact.name.clone();
        if self.artifact_loading.as_deref() == Some(hash.as_str()) {
            return;
        }
        self.artifact_loading = Some(hash.clone());
        self.artifact_view = None;
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let result = crate::replay_link::read_artifact_text(run_id, hash.clone()).await;
            cx.update(|cx| {
                let _ = this.update(cx, |screen, cx| {
                    screen.apply_artifact_text(name, result, cx)
                });
            });
        })
        .detach();
    }

    /// Store a finished artifact text load.
    fn apply_artifact_text(
        &mut self,
        name: String,
        result: Result<Option<String>, String>,
        cx: &mut Context<Self>,
    ) {
        self.artifact_loading = None;
        match result {
            Ok(Some(text)) => self.artifact_view = Some((name, text)),
            Ok(None) => {
                self.artifact_view = Some((name, "(binary or too large to display)".into()));
            },
            Err(e) => self.artifact_view = Some((name, format!("read failed: {e}"))),
        }
        cx.notify();
    }

    /// Rail rows + selected index. Real when any runs exist, else sample.
    fn rows(&mut self, cx: &Context<Self>) -> (Vec<RunRow>, usize, bool) {
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
                if let Some(replay) = self.replays.get(&run.run_id) {
                    row.attach_replay(replay);
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
                cx.update(|cx| {
                    state.update(cx, |s, cx| s.refresh_runs(&summaries, cx));
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
                        Some(id) => this.select_run(id, cx),
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

    /// Artifacts strip for the selected run: durable artifact list
    /// (metadata from `RunReader::artifacts`) with an inline text
    /// viewer. Hidden when the run has no artifacts.
    fn render_artifacts(&self, row: &RunRow, cx: &mut Context<Self>) -> Div {
        let Some(run_id) = row.run_id else {
            return div();
        };

        let mut pane = div()
            .flex_shrink_0()
            .h_flex()
            .gap(px(8.0))
            .items_center()
            .px(px(20.0))
            .pb(px(10.0))
            .child(
                div()
                    .text_size(px(9.0))
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme::text_muted())
                    .child("ARTIFACTS"),
            );

        if self.artifacts_loading && self.artifacts.is_empty() {
            pane = pane.child(ui::meta("loading…"));
            return pane;
        }
        if self.artifacts.is_empty() {
            pane = pane.child(ui::meta("none yet"));
            return pane;
        }

        for (i, artifact) in self.artifacts.iter().take(8).enumerate() {
            let artifact_name = artifact.name.clone();
            let artifact_size = artifact.size_bytes;
            let artifact_for_click = artifact.clone();
            let is_open = self
                .artifact_view
                .as_ref()
                .is_some_and(|(n, _)| *n == artifact.name);
            pane = pane.child(
                div()
                    .id(SharedString::from(format!("runs-artifact-{i}")))
                    .h_flex()
                    .gap(px(6.0))
                    .items_center()
                    .px(px(10.0))
                    .py(px(5.0))
                    .rounded_md()
                    .border_1()
                    .border_color(if is_open {
                        theme::accent().opacity(0.5)
                    } else {
                        theme::hairline()
                    })
                    .bg(if is_open {
                        theme::accent().opacity(0.08)
                    } else {
                        theme::panel_raised()
                    })
                    .cursor_pointer()
                    .hover(|s: StyleRefinement| s.border_color(theme::accent().opacity(0.5)))
                    .on_click(cx.listener(move |this, _e, _w, cx| {
                        this.open_artifact(run_id, &artifact_for_click, cx);
                    }))
                    .child(
                        div()
                            .text_size(px(10.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::text_primary())
                            .child(artifact_name),
                    )
                    .child(ui::meta(format_artifact_size(artifact_size))),
            );
        }

        pane
    }

    /// Inline artifact text viewer (below the artifacts strip).
    fn render_artifact_view(&self) -> Option<Stateful<Div>> {
        let (name, text) = self.artifact_view.as_ref()?;
        Some(
            div()
                .mx(px(20.0))
                .mb(px(12.0))
                .flex_1()
                .min_h_0()
                .id("runs-artifact-view")
                .overflow_y_scroll()
                .v_flex()
                .gap(px(6.0))
                .p(px(12.0))
                .rounded_lg()
                .bg(theme::panel_deep())
                .border_1()
                .border_color(theme::hairline())
                .child(
                    div()
                        .h_flex()
                        .gap(px(8.0))
                        .items_center()
                        .child(
                            div()
                                .text_size(px(10.0))
                                .font_weight(FontWeight::BOLD)
                                .text_color(theme::text_muted())
                                .child(name.clone()),
                        )
                        .child(ui::meta("durable artifact · content-addressed")),
                )
                .child(
                    div()
                        .text_size(px(10.5))
                        .line_height(px(16.0))
                        .font_family(crate::ui::MONO)
                        .text_color(theme::text_primary().opacity(0.85))
                        .child(text.clone()),
                ),
        )
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

        // Background fills for the selected run: durable replay (when no
        // live stream) + artifact listing. Idempotent — guarded by the
        // in-flight sets inside each `ensure_*`.
        if let Some(run_id) = selected.as_ref().and_then(|row| row.run_id) {
            let has_live_stream = self
                .state
                .read(cx)
                .run_streams
                .get(&run_id)
                .is_some_and(|s| s.live);
            if !has_live_stream {
                self.ensure_replay(run_id, cx);
            }
            self.ensure_artifacts(run_id, cx);
        }

        let mut main = div().flex_1().min_w_0().v_flex();
        if let Some(row) = &selected {
            main = main
                .child(self.render_header(row, live, cx))
                .child(self.render_kpis(row))
                .child(self.render_pipeline(row, live))
                .child(self.render_artifacts(row, cx))
                .children(self.render_artifact_view())
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
