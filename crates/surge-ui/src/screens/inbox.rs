//! Inbox — every decision blocked on the operator, one queue, most
//! urgent first.
//!
//! This is Surge's signature surface: competitors ship status
//! monitors; this is a *decision queue*. An item is not a run or a
//! chat — it is one thing the fleet cannot do without you.
//!
//! Real sources, in urgency order:
//! 1. **Live blocked decisions** from per-run event streams
//!    ([`AppState::pending_decisions`]): escalations, sandbox
//!    elevations, human-input requests, gate approvals, roadmap
//!    patches. Gate/input items resolve through the real
//!    `EngineFacade::resolve_human_input` IPC; errors surface verbatim.
//! 2. **Tasks awaiting review** (`HumanReview` / `QaReview`) — QA
//!    verdict + reasoning attached as evidence; approve/reject goes
//!    through the gate-decision plumbing.
//! 3. **Failed / aborted runs** — failure triage; opens the cockpit.
//!
//! Sandbox elevations have no resolve IPC yet (daemon seam missing) —
//! the item says so instead of faking a button. When nothing real
//! exists at all, a clearly-labelled sample queue shows the shape.

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::StyledExt;
use gpui_component::input::{Input, InputEvent, InputState};
use surge_core::TaskState;
use surge_core::id::RunId;
use surge_orchestrator::engine::facade::EngineFacade as _;
use surge_orchestrator::engine::handle::RunStatus;

use crate::app_state::AppState;
use crate::run_stream::DecisionKind;
use crate::theme;
use crate::ui;

/// What the Inbox can ask the app shell to do.
#[derive(Clone)]
pub enum InboxAction {
    /// Open the run cockpit focused on this run.
    OpenRun(RunId),
    /// Operator decided a task-level review gate (approve / reject).
    TaskDecision { task_id: String, approved: bool },
}

impl EventEmitter<InboxAction> for InboxScreen {}

/// Where an item came from — determines which actions are real.
#[derive(Clone)]
enum Source {
    /// Live blocked decision on an active run (per-run stream).
    Live {
        run_id: RunId,
        kind: DecisionKind,
        call_id: Option<String>,
    },
    /// A task sitting in HumanReview / QaReview.
    Task { task_id: String },
    /// A failed or aborted run needing triage.
    FailedRun { run_id: RunId },
    /// Labelled sample (offline).
    Sample,
}

/// One decision unit in the queue.
#[derive(Clone)]
struct InboxItem {
    /// Urgency class (lower = first). Live kinds 0-4, reviews 5-6,
    /// failures 7.
    rank: u8,
    badge: &'static str,
    badge_color: Hsla,
    title: String,
    meta: String,
    age: String,
    /// Evidence lines shown in the focus pane: (label, body).
    evidence: Vec<(String, String)>,
    source: Source,
}

/// Inbox screen — the urgency-ranked operator decision queue.
pub struct InboxScreen {
    state: Entity<AppState>,
    selected: usize,
    response_input: Option<Entity<InputState>>,
    /// Verbatim feedback from the last facade call.
    action_note: Option<String>,
    /// Sample items dismissed this session (sample mode only).
    dismissed_samples: Vec<String>,
}

impl InboxScreen {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        cx.observe(&state, |_this, _state, cx| cx.notify()).detach();
        Self {
            state,
            selected: 0,
            response_input: None,
            action_note: None,
            dismissed_samples: Vec::new(),
        }
    }

    /// Assemble the queue from all real sources; sample only when
    /// everything is empty.
    fn items(&self, cx: &Context<Self>) -> (Vec<InboxItem>, bool) {
        let state = self.state.read(cx);
        let mut items: Vec<InboxItem> = Vec::new();

        // 1. Live blocked decisions (already urgency-sorted).
        for (run_id, p) in state.pending_decisions() {
            let (badge_color, title, evidence): (Hsla, String, Vec<(String, String)>) =
                match &p.kind {
                    DecisionKind::Escalation { reason } => (
                        theme::error(),
                        "Engine escalation — automated path exhausted".to_string(),
                        vec![("REASON".to_string(), reason.clone())],
                    ),
                    DecisionKind::Elevation { capability } => (
                        theme::warning(),
                        format!("Sandbox elevation: {capability}"),
                        vec![(
                            "NOTE".to_string(),
                            "No in-UI resolve seam yet — decide via the configured \
                             approval channel (Telegram / desktop). Timeout applies."
                                .to_string(),
                        )],
                    ),
                    DecisionKind::HumanInput {
                        prompt,
                        schema,
                        call_id,
                    } => {
                        let mut ev = Vec::new();
                        let outcomes = p.kind.gate_outcomes();
                        if !outcomes.is_empty() {
                            ev.push(("OUTCOMES".to_string(), outcomes.join(" · ")));
                        } else if let Some(schema) = schema {
                            ev.push((
                                "EXPECTED RESPONSE (SCHEMA)".to_string(),
                                serde_json::to_string_pretty(schema)
                                    .unwrap_or_else(|_| schema.to_string()),
                            ));
                        }
                        let _ = call_id;
                        (theme::accent(), prompt.clone(), ev)
                    },
                    DecisionKind::RoadmapPatch { patch_id } => (
                        theme::accent(),
                        format!("Roadmap patch {patch_id} awaits approval"),
                        vec![(
                            "NOTE".to_string(),
                            "Roadmap patches resolve through the amendment flow \
                             (`surge feature` → submit_roadmap_amendment) — no in-UI \
                             seam yet."
                                .to_string(),
                        )],
                    ),
                };
            let call_id = match &p.kind {
                DecisionKind::HumanInput { call_id, .. } => call_id.clone(),
                _ => None,
            };
            items.push(InboxItem {
                rank: p.kind.rank(),
                badge: p.kind.badge(),
                badge_color,
                title,
                meta: format!("run r-{} · {}", run_id.short().to_lowercase(), p.node),
                age: p.time.clone(),
                evidence,
                source: Source::Live {
                    run_id,
                    kind: p.kind.clone(),
                    call_id,
                },
            });
        }

        // 2. Tasks blocked in review states.
        for task in &state.tasks {
            let (badge, rank, evidence): (&'static str, u8, Vec<(String, String)>) =
                match &task.state {
                    TaskState::HumanReview => ("review gate", 5, Vec::new()),
                    TaskState::QaReview { verdict, reasoning } => {
                        let mut ev = Vec::new();
                        if let Some(v) = verdict {
                            ev.push(("QA VERDICT".to_string(), v.clone()));
                        }
                        if let Some(r) = reasoning {
                            ev.push(("QA REASONING".to_string(), r.clone()));
                        }
                        ("qa gate", 6, ev)
                    },
                    _ => continue,
                };
            items.push(InboxItem {
                rank,
                badge,
                badge_color: theme::accent(),
                title: task.title.clone(),
                meta: format!(
                    "task t-{} · {}",
                    task.id.short().to_lowercase(),
                    task.agent.clone().unwrap_or_else(|| "unassigned".into())
                ),
                age: task.updated_at.clone(),
                evidence,
                source: Source::Task {
                    task_id: task.id.to_string(),
                },
            });
        }

        // 3. Failed / aborted runs — failure triage.
        for run in &state.runs {
            if !matches!(run.status, RunStatus::Failed | RunStatus::Aborted) {
                continue;
            }
            let label = if matches!(run.status, RunStatus::Failed) {
                "failed"
            } else {
                "aborted"
            };
            // Attach the tail of the live log as evidence if we have it.
            let mut evidence = Vec::new();
            if let Some(stream) = state.run_streams.get(&run.run_id) {
                let tail: Vec<String> = stream
                    .log
                    .iter()
                    .rev()
                    .take(4)
                    .map(|r| format!("{} {} {}", r.time, r.kind, r.text))
                    .collect();
                if !tail.is_empty() {
                    evidence.push(("LAST EVENTS".to_string(), tail.join("\n")));
                }
            }
            items.push(InboxItem {
                rank: 7,
                badge: "failure triage",
                badge_color: theme::error(),
                title: format!("Run ended {label} without merging"),
                meta: format!(
                    "run r-{} · started {}",
                    run.run_id.short().to_lowercase(),
                    run.started_at.with_timezone(&chrono::Local).format("%H:%M")
                ),
                age: String::new(),
                evidence,
                source: Source::FailedRun { run_id: run.run_id },
            });
        }

        let live = !items.is_empty() || !state.runs.is_empty() || !state.tasks.is_empty();
        if !live {
            let samples: Vec<InboxItem> = sample_items()
                .into_iter()
                .filter(|i| !self.dismissed_samples.contains(&i.title))
                .collect();
            return (samples, false);
        }

        items.sort_by_key(|a| a.rank);
        (items, true)
    }

    // ── real actions ────────────────────────────────────────────────

    /// Resolve a live human-input / gate decision over IPC.
    fn resolve_live(
        &mut self,
        run_id: RunId,
        call_id: Option<String>,
        response: serde_json::Value,
        cx: &mut Context<Self>,
    ) {
        let Some(facade) = self.state.read(cx).daemon_state.facade() else {
            self.action_note = Some("daemon offline — cannot resolve".into());
            cx.notify();
            return;
        };
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let note = match facade.resolve_human_input(run_id, call_id, response).await {
                Ok(()) => format!("resolved · r-{}", run_id.short().to_lowercase()),
                Err(e) => format!("resolve failed: {e}"),
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

    /// Read the shared comment box and clear it — a comment belongs to
    /// exactly one decision, never the next one too.
    fn take_comment(&mut self, window: &mut Window, cx: &mut Context<Self>) -> String {
        let Some(input) = self.response_input.clone() else {
            return String::new();
        };
        let text = input.read(cx).value().trim().to_string();
        if !text.is_empty() {
            input.update(cx, |s, cx| s.set_value("", window, cx));
        }
        text
    }

    fn decide(
        &mut self,
        item: &InboxItem,
        outcome: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let comment = self.take_comment(window, cx);

        match &item.source {
            Source::Live {
                run_id, call_id, ..
            } => {
                // HumanGate resolution contract (engine.rs
                // resolve_human_input): `{"outcome": <declared key>}`,
                // free-form fields ride along — the bootstrap driver
                // sends the comment as `comment`.
                let mut payload = serde_json::json!({ "outcome": outcome });
                if !comment.is_empty() {
                    payload["comment"] = serde_json::Value::String(comment);
                }
                self.resolve_live(*run_id, call_id.clone(), payload, cx);
            },
            Source::Task { task_id } => {
                cx.emit(InboxAction::TaskDecision {
                    task_id: task_id.clone(),
                    approved: outcome == "approve",
                });
            },
            Source::FailedRun { run_id } => {
                cx.emit(InboxAction::OpenRun(*run_id));
            },
            Source::Sample => {
                self.dismissed_samples.push(item.title.clone());
                self.action_note = Some("sample item dismissed".into());
                self.selected = 0;
                cx.notify();
            },
        }
    }

    /// Send the free-text response for a live tool-driven human-input
    /// item (`call_id: Some`). Gate pauses (`call_id: None`) go through
    /// the outcome buttons instead — the engine requires `{"outcome"}`.
    fn send_response(&mut self, item: &InboxItem, window: &mut Window, cx: &mut Context<Self>) {
        if let Source::Live {
            run_id,
            call_id: Some(call_id),
            kind: DecisionKind::HumanInput { .. },
        } = &item.source
        {
            let run_id = *run_id;
            let call_id = call_id.clone();
            let text = self.take_comment(window, cx);
            if text.is_empty() {
                return;
            }
            self.resolve_live(run_id, Some(call_id), serde_json::Value::String(text), cx);
        }
    }

    // ── panes ───────────────────────────────────────────────────────

    fn render_queue(&self, items: &[InboxItem], live: bool, cx: &mut Context<Self>) -> Div {
        let mut list = div()
            .flex_1()
            .id("inbox-queue")
            .overflow_y_scroll()
            .v_flex()
            .gap(px(5.0))
            .p(px(10.0));

        if items.is_empty() {
            list = list.child(
                div()
                    .v_flex()
                    .gap(px(10.0))
                    .items_center()
                    .py(px(60.0))
                    .px(px(20.0))
                    .child(
                        div()
                            .w(px(34.0))
                            .h(px(34.0))
                            .rounded_lg()
                            .bg(theme::success().opacity(0.14))
                            .border_1()
                            .border_color(theme::success().opacity(0.3))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(theme::success())
                            .child("✓"),
                    )
                    .child(
                        div()
                            .text_size(px(12.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::text_primary())
                            .child("Inbox zero"),
                    )
                    .child(
                        div()
                            .text_size(px(10.5))
                            .line_height(px(16.0))
                            .text_color(theme::text_muted())
                            .text_center()
                            .child("Nothing blocked on you.\nThe fleet is flying itself."),
                    ),
            );
        }

        for (i, item) in items.iter().enumerate() {
            let is_sel = i == self.selected.min(items.len().saturating_sub(1));
            let row = div()
                .id(SharedString::from(format!("inbox-item-{i}")))
                .h_flex()
                .gap(px(10.0))
                .p(px(11.0))
                .rounded_lg()
                .border_1()
                .border_color(if is_sel {
                    theme::accent().opacity(0.5)
                } else {
                    theme::hairline()
                })
                .bg(if is_sel {
                    theme::panel_raised()
                } else {
                    theme::panel_raised().opacity(0.55)
                })
                .cursor_pointer()
                .hover(|s: StyleRefinement| s.border_color(theme::hairline_strong()))
                .on_click(cx.listener(move |this, _e, window, cx| {
                    if this.selected != i {
                        // A half-typed comment belongs to the item it was
                        // written for — never carry it to the next one.
                        if let Some(input) = this.response_input.clone() {
                            input.update(cx, |s, cx| s.set_value("", window, cx));
                        }
                    }
                    this.selected = i;
                    this.action_note = None;
                    cx.notify();
                }))
                .child(
                    div()
                        .v_flex()
                        .gap(px(3.0))
                        .items_center()
                        .w(px(30.0))
                        .flex_shrink_0()
                        .child(
                            div()
                                .text_size(px(13.0))
                                .font_weight(FontWeight::BOLD)
                                .text_color(if item.rank <= 2 {
                                    theme::error()
                                } else if item.rank <= 4 {
                                    theme::accent()
                                } else {
                                    theme::text_muted()
                                })
                                .child(format!("P{}", item.rank.min(3))),
                        ),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .v_flex()
                        .gap(px(5.0))
                        .child(
                            div()
                                .h_flex()
                                .gap(px(7.0))
                                .items_center()
                                .child(ui::pill(
                                    item.badge,
                                    item.badge_color,
                                    item.badge_color.opacity(0.13),
                                ))
                                .child(div().flex_1())
                                .child(
                                    div()
                                        .text_size(px(9.0))
                                        .text_color(theme::text_muted().opacity(0.8))
                                        .child(item.age.clone()),
                                ),
                        )
                        .child(
                            div()
                                .text_size(px(12.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .line_height(px(17.0))
                                .overflow_hidden()
                                .text_color(theme::text_primary())
                                .child(item.title.clone()),
                        )
                        .child(
                            div()
                                .text_size(px(9.5))
                                .text_color(theme::text_muted())
                                .child(item.meta.clone()),
                        ),
                );
            list = list.child(row);
        }

        div()
            .w(px(390.0))
            .flex_shrink_0()
            .v_flex()
            .bg(theme::panel())
            .border_r_1()
            .border_color(theme::hairline())
            .child(
                div()
                    .h_flex()
                    .gap(px(10.0))
                    .items_baseline()
                    .px(px(16.0))
                    .pt(px(15.0))
                    .pb(px(11.0))
                    .border_b_1()
                    .border_color(theme::hairline())
                    .child(
                        div()
                            .text_size(px(15.0))
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme::text_primary())
                            .child("Inbox"),
                    )
                    .child(ui::meta(format!("{} blocked on you", items.len())))
                    .child(div().flex_1())
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

    fn render_focus(
        &mut self,
        item: &InboxItem,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        // Lazily create the shared response/comment input.
        if self.response_input.is_none() {
            let input = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("Optional comment / response — sent with your decision…")
            });
            cx.subscribe_in(
                &input,
                window,
                |this: &mut Self, _input, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::PressEnter { .. }) {
                        let (items, _) = this.items(cx);
                        if let Some(item) = items
                            .get(this.selected.min(items.len().saturating_sub(1)))
                            .cloned()
                        {
                            this.send_response(&item, window, cx);
                        }
                    }
                },
            )
            .detach();
            self.response_input = Some(input);
        }

        // Tool-driven request (call_id present): the caller defines the
        // response shape — free-text send. Gate pause (call_id absent):
        // the engine requires {"outcome": <declared key>} — buttons.
        let is_tool_input = matches!(
            &item.source,
            Source::Live {
                call_id: Some(_),
                kind: DecisionKind::HumanInput { .. },
                ..
            }
        );
        let is_gate_input = matches!(
            &item.source,
            Source::Live {
                call_id: None,
                kind: DecisionKind::HumanInput { .. },
                ..
            }
        );
        let is_elevation = matches!(
            &item.source,
            Source::Live {
                kind: DecisionKind::Elevation { .. },
                ..
            }
        );
        let is_roadmap_patch = matches!(
            &item.source,
            Source::Live {
                kind: DecisionKind::RoadmapPatch { .. },
                ..
            }
        );
        let is_failure = matches!(&item.source, Source::FailedRun { .. });
        let is_escalation = matches!(
            &item.source,
            Source::Live {
                kind: DecisionKind::Escalation { .. },
                ..
            }
        );
        let is_task = matches!(&item.source, Source::Task { .. });

        let mut pane = div()
            .flex_1()
            .min_w_0()
            .id("inbox-focus")
            .overflow_y_scroll()
            .v_flex()
            .px(px(26.0))
            .pt(px(22.0))
            // header
            .child(
                div()
                    .h_flex()
                    .gap(px(10.0))
                    .items_center()
                    .child(ui::pill(
                        item.badge,
                        item.badge_color,
                        item.badge_color.opacity(0.13),
                    ))
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(theme::text_muted())
                            .child(item.meta.clone()),
                    )
                    .child(div().flex_1())
                    .children(self.action_note.clone().map(|n| {
                        div()
                            .text_size(px(10.5))
                            .text_color(theme::accent())
                            .child(n)
                    })),
            )
            // title
            .child(
                div()
                    .pt(px(14.0))
                    .pb(px(6.0))
                    .text_size(px(20.0))
                    .font_weight(FontWeight::BOLD)
                    .line_height(px(28.0))
                    .text_color(theme::text_primary())
                    .child(item.title.clone()),
            );

        // evidence blocks
        for (label, body) in &item.evidence {
            pane = pane.child(
                div()
                    .mt(px(14.0))
                    .v_flex()
                    .gap(px(8.0))
                    .child(ui::section_label(label.clone()))
                    .child(
                        div()
                            .p(px(13.0))
                            .rounded_lg()
                            .bg(theme::panel_raised())
                            .border_1()
                            .border_color(theme::hairline())
                            .text_size(px(11.0))
                            .line_height(px(17.0))
                            .text_color(theme::text_primary().opacity(0.85))
                            .child(body.clone()),
                    ),
            );
        }

        // response / comment input — only where a decision can carry it
        if is_tool_input || is_gate_input || is_task {
            pane =
                pane.child(
                    div()
                        .mt(px(18.0))
                        .h_flex()
                        .gap(px(10.0))
                        .items_center()
                        .h(px(38.0))
                        .px(px(12.0))
                        .rounded_lg()
                        .bg(theme::panel_deep())
                        .border_1()
                        .border_color(theme::hairline_strong())
                        .child(div().flex_1().child(
                            Input::new(self.response_input.as_ref().unwrap()).appearance(false),
                        ))
                        .when(is_tool_input, |el| el.child(ui::kbd("↵ send"))),
                );
        }

        // action row
        let mut actions = div()
            .mt(px(18.0))
            .mb(px(22.0))
            .h_flex()
            .gap(px(10.0))
            .items_center();

        let primary = |id: SharedString, label: String| {
            div()
                .id(id)
                .h(px(38.0))
                .px(px(18.0))
                .rounded_lg()
                .bg(theme::accent())
                .flex()
                .items_center()
                .gap(px(8.0))
                .text_color(theme::on_accent())
                .text_size(px(12.0))
                .font_weight(FontWeight::BOLD)
                .cursor_pointer()
                .hover(|s: StyleRefinement| s.bg(theme::accent().opacity(0.85)))
                .child(label)
        };
        let secondary = |id: SharedString, label: String| {
            div()
                .id(id)
                .h(px(38.0))
                .px(px(15.0))
                .rounded_lg()
                .border_1()
                .border_color(theme::hairline_strong())
                .flex()
                .items_center()
                .text_color(theme::text_primary().opacity(0.85))
                .text_size(px(11.5))
                .font_weight(FontWeight::SEMIBOLD)
                .cursor_pointer()
                .hover(|s: StyleRefinement| s.border_color(theme::accent().opacity(0.5)))
                .child(label)
        };
        let danger = |id: SharedString, label: String| {
            div()
                .id(id)
                .h(px(38.0))
                .px(px(16.0))
                .rounded_lg()
                .border_1()
                .border_color(theme::error().opacity(0.35))
                .flex()
                .items_center()
                .text_color(theme::error().opacity(0.95))
                .text_size(px(11.5))
                .font_weight(FontWeight::BOLD)
                .cursor_pointer()
                .hover(|s: StyleRefinement| s.border_color(theme::error().opacity(0.7)))
                .child(label)
        };
        let looks_destructive =
            |o: &str| o.contains("reject") || o.contains("abort") || o.contains("fail");

        if is_failure {
            let item_open = item.clone();
            actions = actions.child(
                primary("inbox-open-run".into(), "Open cockpit".to_string()).on_click(cx.listener(
                    move |this, _e, window, cx| {
                        this.decide(&item_open, "open", window, cx);
                    },
                )),
            );
        } else if is_elevation || is_escalation || is_roadmap_patch {
            if let Source::Live { run_id, .. } = item.source {
                actions = actions.child(
                    secondary("inbox-open-run-2".into(), "Open cockpit".to_string()).on_click(
                        cx.listener(move |_this, _e, _w, cx| {
                            cx.emit(InboxAction::OpenRun(run_id));
                        }),
                    ),
                );
            }
        } else if is_tool_input {
            let item_send = item.clone();
            actions = actions.child(
                primary("inbox-send-response".into(), "Send response".to_string()).on_click(
                    cx.listener(move |this, _e, window, cx| {
                        this.send_response(&item_send, window, cx);
                    }),
                ),
            );
        } else if is_gate_input {
            // One button per outcome the gate actually declares
            // (schema.properties.outcome.enum) — destructive-looking
            // keys pushed right and styled red.
            let mut outcomes = match &item.source {
                Source::Live { kind, .. } => kind.gate_outcomes(),
                _ => Vec::new(),
            };
            if outcomes.is_empty() {
                // Free-text gate (allow_freetext) — offer the canonical pair.
                outcomes = vec!["approve".to_string(), "reject".to_string()];
            }
            let (destructive, normal): (Vec<String>, Vec<String>) =
                outcomes.into_iter().partition(|o| looks_destructive(o));
            for (i, outcome) in normal.into_iter().enumerate() {
                let item_o = item.clone();
                let outcome_for_click = outcome.clone();
                let button = if i == 0 {
                    primary(format!("inbox-outcome-{outcome}").into(), outcome.clone())
                } else {
                    secondary(format!("inbox-outcome-{outcome}").into(), outcome.clone())
                };
                actions =
                    actions.child(button.on_click(cx.listener(move |this, _e, window, cx| {
                        this.decide(&item_o, &outcome_for_click, window, cx);
                    })));
            }
            actions = actions.child(div().flex_1());
            for outcome in destructive {
                let item_o = item.clone();
                let outcome_for_click = outcome.clone();
                actions = actions.child(
                    danger(format!("inbox-outcome-{outcome}").into(), outcome.clone()).on_click(
                        cx.listener(move |this, _e, window, cx| {
                            this.decide(&item_o, &outcome_for_click, window, cx);
                        }),
                    ),
                );
            }
        } else {
            // Task review gates + samples: canonical approve / reject.
            let item_approve = item.clone();
            let item_reject = item.clone();
            actions = actions
                .child(
                    primary("inbox-approve".into(), "Approve".to_string()).on_click(cx.listener(
                        move |this, _e, window, cx| {
                            this.decide(&item_approve, "approve", window, cx);
                        },
                    )),
                )
                .child(div().flex_1())
                .child(
                    danger("inbox-reject".into(), "Reject".to_string()).on_click(cx.listener(
                        move |this, _e, window, cx| {
                            this.decide(&item_reject, "reject", window, cx);
                        },
                    )),
                );
        }

        pane.child(actions)
    }

    fn render_all_clear(&self) -> Div {
        div()
            .flex_1()
            .flex()
            .items_center()
            .justify_center()
            .bg(theme::panel_deep())
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(theme::text_muted())
                    .child("All clear — head back to the Fleet."),
            )
    }
}

impl Render for InboxScreen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (items, live) = self.items(cx);
        let selected = items
            .get(self.selected.min(items.len().saturating_sub(1)))
            .cloned();

        let focus: AnyElement = match &selected {
            Some(item) => self.render_focus(item, window, cx).into_any_element(),
            None => self.render_all_clear().into_any_element(),
        };

        // Plain .flex() row so both panes stretch to full height
        // (h_flex would vertically center them).
        div()
            .size_full()
            .flex()
            .child(self.render_queue(&items, live, cx))
            .child(focus)
    }
}

/// Clearly-labelled sample queue (offline, nothing real to decide) —
/// mirrors the concept so the decision-unit idea reads.
fn sample_items() -> Vec<InboxItem> {
    vec![
        InboxItem {
            rank: 1,
            badge: "elevation",
            badge_color: theme::warning(),
            title: "Sandbox elevation: network egress for `cargo audit`".to_string(),
            meta: "run r-b2e8 · verify stage".to_string(),
            age: "2m".to_string(),
            evidence: vec![(
                "REQUEST".to_string(),
                "verifier wants WorkspaceNetwork to fetch the advisory DB. \
                 Deny keeps the run offline; it will retry with the cached DB."
                    .to_string(),
            )],
            source: Source::Sample,
        },
        InboxItem {
            rank: 3,
            badge: "review gate",
            badge_color: theme::accent(),
            title: "Sessions-table migration ready to merge".to_string(),
            meta: "run r-9c1e · review gate".to_string(),
            age: "15m".to_string(),
            evidence: vec![
                (
                    "EVIDENCE".to_string(),
                    "qa suite 42/42 green · lint clean · +186 −12 across 4 files".to_string(),
                ),
                (
                    "RISK".to_string(),
                    "touches auth-adjacent middleware; reviewed diff attached in cockpit"
                        .to_string(),
                ),
            ],
            source: Source::Sample,
        },
        InboxItem {
            rank: 7,
            badge: "failure triage",
            badge_color: theme::error(),
            title: "Config loader refactor failed QA (3/6)".to_string(),
            meta: "run r-77b0 · qa suite".to_string(),
            age: "32m".to_string(),
            evidence: vec![(
                "LAST EVENTS".to_string(),
                "14:15 FAIL qa suite failed — 3/6\n14:12 TEST test_env_override ✗".to_string(),
            )],
            source: Source::Sample,
        },
    ]
}
