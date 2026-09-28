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
//! the item says so instead of faking a button. Empty queues stay empty.

use gpui_kit::component::StyledExt;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
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
    /// Show the plan under review full-size (Flow screen).
    OpenPlan,
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
        seq: u64,
        node: String,
        kind: DecisionKind,
        call_id: Option<String>,
    },
    /// A task sitting in HumanReview / QaReview.
    Task { task_id: String },
    /// A failed or aborted run needing triage.
    FailedRun { run_id: RunId },
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

impl InboxItem {
    fn identity(&self) -> String {
        match &self.source {
            Source::Live { run_id, seq, .. } => format!("live-{run_id}-{seq}"),
            Source::Task { task_id } => format!("task-{task_id}"),
            Source::FailedRun { run_id } => format!("failed-{run_id}"),
        }
    }
}

/// Inbox screen — the urgency-ranked operator decision queue.
pub struct InboxScreen {
    state: Entity<AppState>,
    selected: usize,
    response_input: Option<Entity<InputState>>,
    /// Verbatim feedback from the last facade call.
    action_note: Option<String>,
    /// Parsed + laid-out workflow plans and their step-list Markdown, keyed
    /// by a hash of the document. Built once per document, not per frame.
    plans: std::collections::HashMap<u64, std::rc::Rc<ReviewedPlan>>,
    /// Identity of the decision `action_note` reports on. The note renders
    /// only on that item, so "resolved · …" never lands on the next gate.
    note_owner: Option<String>,
    documents: std::collections::HashMap<std::path::PathBuf, String>,
}

impl InboxScreen {
    fn load_review_documents(&mut self, cx: &mut Context<Self>) {
        let paths: Vec<_> = {
            let state = self.state.read(cx);
            state
                .pending_decisions()
                .into_iter()
                .filter_map(|(run, pending)| {
                    let DecisionKind::HumanInput { schema, .. } = pending.kind else {
                        return None;
                    };
                    let (_, name) = review_artifact(schema.as_ref())?;
                    state.run_streams.get(&run)?.artifacts.get(name).cloned()
                })
                .collect()
        };
        for path in paths {
            if self.documents.contains_key(&path) {
                continue;
            }
            self.documents
                .insert(path.clone(), "Loading document…".into());
            let read_path = path.clone();
            let read = cx
                .background_executor()
                .spawn(async move { read_review_document(&read_path) });
            cx.spawn(async move |this, cx| {
                let content = read.await;
                let _ = this.update(cx, |this, cx| {
                    this.documents.insert(path, content);
                    cx.notify();
                });
            })
            .detach();
        }
    }

    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        cx.observe(&state, |_this, _state, cx| cx.notify()).detach();
        Self {
            state,
            selected: 0,
            response_input: None,
            action_note: None,
            note_owner: None,
            plans: std::collections::HashMap::new(),
            documents: std::collections::HashMap::new(),
        }
    }

    /// Assemble the queue from actual decisions, reviews, and failed runs.
    fn items(&self, cx: &Context<Self>) -> (Vec<InboxItem>, bool) {
        let state = self.state.read(cx);
        let mut items: Vec<InboxItem> = Vec::new();

        // 1. Live blocked decisions (already urgency-sorted).
        for (run_id, p) in state.pending_decisions() {
            let (badge_color, title, evidence): (Hsla, String, Vec<(String, String)>) = match &p
                .kind
            {
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
                    if let Some((label, name)) = review_artifact(schema.as_ref()) {
                        let content = state.run_streams.get(&run_id)
                                .and_then(|stream| stream.artifacts.get(name))
                                .map(|path| self.documents.get(path).cloned()
                                    .unwrap_or_else(|| "Loading document…".into()))
                                .unwrap_or_else(|| "The review document has not arrived yet. Wait for the run stream to load before deciding.".into());
                        ev.push((label.to_owned(), content));
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
                    seq: p.seq,
                    node: p.node.clone(),
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
        for run in state.unacknowledged_failures() {
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
                title: format!("Run ended {label}"),
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

        let live = state.daemon_state.facade().is_some();
        items.sort_by_key(|a| a.rank);
        (items, live)
    }

    // ── real actions ────────────────────────────────────────────────

    /// Resolve a live human-input / gate decision over IPC.
    fn resolve_live(
        &mut self,
        run_id: RunId,
        node: String,
        call_id: Option<String>,
        response: serde_json::Value,
        cx: &mut Context<Self>,
    ) {
        let Ok(node) = surge_core::keys::NodeKey::try_from(node.as_str()) else {
            self.action_note = Some("invalid request node — refresh this run".into());
            cx.notify();
            return;
        };
        let Some(facade) = self.state.read(cx).daemon_state.facade() else {
            self.action_note = Some("daemon offline — cannot resolve".into());
            cx.notify();
            return;
        };
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let note = match facade
                .resolve_requested_input(run_id, node, call_id, response)
                .await
            {
                Ok(()) => format!("resolved · r-{}", run_id.short().to_lowercase()),
                Err(e) => format!("resolve failed: {e}"),
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
        self.note_owner = Some(item.identity());
        let comment = self.take_comment(window, cx);

        match &item.source {
            Source::Live {
                run_id,
                node,
                call_id,
                ..
            } => {
                // HumanGate resolution contract (engine.rs
                // resolve_human_input): `{"outcome": <declared key>}`,
                // free-form fields ride along — the bootstrap driver
                // sends the comment as `comment`.
                let mut payload = serde_json::json!({ "outcome": outcome });
                if !comment.is_empty() {
                    payload["comment"] = serde_json::Value::String(comment);
                }
                // Plan edits made in the Flow inspector ride along with the
                // approval of that plan (and only with an approval).
                if outcome == "approve" && node.as_str() == "flow_gate" {
                    let edits = self
                        .state
                        .update(cx, |state, _| state.plan_edits.remove(run_id));
                    if let Some(edits) = edits.filter(|e| !e.is_empty())
                        && let Ok(value) = serde_json::to_value(&edits)
                    {
                        payload["node_overrides"] = value;
                    }
                }
                self.resolve_live(*run_id, node.clone(), call_id.clone(), payload, cx);
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
        }
    }

    /// Send the free-text response for a live tool-driven human-input
    /// item (a scoped tool call ID). Typed gate requests go through
    /// the outcome buttons instead — the engine requires `{"outcome"}`.
    fn send_response(&mut self, item: &InboxItem, window: &mut Window, cx: &mut Context<Self>) {
        self.note_owner = Some(item.identity());
        if let Source::Live {
            run_id,
            node,
            call_id: Some(call_id),
            kind: DecisionKind::HumanInput { .. },
            ..
        } = &item.source
            && surge_core::id::GateRequestId::from_event_call_id(call_id).is_none()
        {
            let run_id = *run_id;
            let node = node.clone();
            let call_id = call_id.clone();
            let text = self.take_comment(window, cx);
            if text.is_empty() {
                return;
            }
            self.resolve_live(
                run_id,
                node,
                Some(call_id),
                serde_json::Value::String(text),
                cx,
            );
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
                            .child(if live { "Inbox zero" } else { "Inbox offline" }),
                    )
                    .child(
                        div()
                            .id("decision-status")
                            .role(Role::Label)
                            .aria_label(if live {
                                "Nothing blocked on you"
                            } else {
                                "Reconnect to check decisions"
                            })
                            .text_size(px(10.5))
                            .line_height(px(16.0))
                            .text_color(theme::text_muted())
                            .text_center()
                            .child(if live {
                                "No pending decisions. New requests will appear here."
                            } else {
                                "Reconnect the daemon to check for new decisions."
                            }),
                    ),
            );
        }

        for (i, item) in items.iter().enumerate() {
            let is_sel = i == self.selected.min(items.len().saturating_sub(1));
            let row = div()
                .id(SharedString::from(format!(
                    "inbox-item-{}",
                    item.identity()
                )))
                .role(Role::Button)
                .aria_label(item.title.clone())
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
                            "Disconnected",
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
                call_id: Some(id),
                kind: DecisionKind::HumanInput { .. },
                ..
            } if surge_core::id::GateRequestId::from_event_call_id(id).is_none()
        );
        let is_gate_input = matches!(
            &item.source,
            Source::Live {
                call_id: Some(id),
                kind: DecisionKind::HumanInput { .. },
                ..
            } if surge_core::id::GateRequestId::from_event_call_id(id).is_some()
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

        // The document scrolls; the response field and decision buttons live
        // in a pinned footer so a long plan never pushes Approve off-screen.
        let mut pane = div()
            .flex_1()
            .min_h_0()
            .w_full()
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
                    .children(
                        self.action_note
                            .clone()
                            .filter(|_| self.note_owner.as_deref() == Some(item.identity().as_str()))
                            .map(|n| {
                        div()
                            .id("decision-status")
                            .role(Role::Label)
                            .aria_label(n.clone())
                            .text_size(px(10.5))
                            .text_color(theme::accent())
                            .child(n)
                    })),
            )
            // title
            .child(
                div()
                    .id("decision-prompt")
                    .role(Role::Label)
                    .aria_label(item.title.clone())
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
            // A generated workflow is reviewed as a diagram first; the
            // step list and raw TOML follow for detail.
            let is_workflow = label == WORKFLOW_LABEL;
            let reviewed = is_workflow.then(|| self.reviewed_plan(body));
            if let Some(plan) = reviewed.as_ref().and_then(|r| r.prepared.clone()) {
                pane = pane.child(
                    div()
                        .mt(px(14.0))
                        .v_flex()
                        .gap(px(8.0))
                        .child(
                            div()
                                .h_flex()
                                .items_center()
                                .child(ui::section_label("Plan".to_string()))
                                .child(div().flex_1())
                                .child(
                                    div()
                                        .id("decision-open-plan")
                                        .role(Role::Button)
                                        .aria_label("Open full plan")
                                        .cursor_pointer()
                                        .text_size(px(10.5))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(theme::accent())
                                        .hover(|s: StyleRefinement| s.opacity(0.8))
                                        .child("Open full plan ↗")
                                        .on_click(cx.listener(|_this, _e, _w, cx| {
                                            cx.emit(InboxAction::OpenPlan);
                                        })),
                                ),
                        )
                        .children({
                            let pending_edits = match &item.source {
                                Source::Live { run_id, .. } => self
                                    .state
                                    .read(cx)
                                    .plan_edits
                                    .get(run_id)
                                    .map_or(0, |edits| edits.0.len()),
                                _ => 0,
                            };
                            (pending_edits > 0).then(|| {
                                div()
                                    .px(px(10.0))
                                    .py(px(6.0))
                                    .rounded_md()
                                    .bg(theme::accent().opacity(0.12))
                                    .text_size(px(11.5))
                                    .text_color(theme::accent())
                                    .child(format!(
                                        "{pending_edits} plan edit(s) from the Flow inspector will be applied when you approve"
                                    ))
                            })
                        })
                        .children(plan.rationale.clone().map(|why| {
                            div()
                                .text_size(px(11.5))
                                .text_color(theme::text_primary().opacity(0.85))
                                .child(format!("Why this plan: {why}"))
                        }))
                        .child(crate::flow_diagram::render_prepared(&plan, "decision-flow-diagram")),
                );
            }
            pane = pane.child(
                div()
                    .id(SharedString::from(format!("decision-evidence-{label}")))
                    .role(Role::Label)
                    .aria_label(format!("{label}: {body}"))
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
                            .child(match &reviewed {
                                Some(plan) => {
                                    crate::markdown::render_markdown(&plan.review_markdown)
                                },
                                None => crate::markdown::render_markdown(body),
                            }),
                    ),
            );
        }

        // response / comment input — only where a decision can carry it
        let mut footer = div()
            .flex_shrink_0()
            .v_flex()
            .px(px(26.0))
            .border_t_1()
            .border_color(theme::hairline());
        if is_tool_input || is_gate_input || is_task {
            footer = footer.child(
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
                    .child(
                        div().flex_1().child(
                            Input::new(self.response_input.as_ref().unwrap())
                                .appearance(false)
                                .accessibility_id("decision-response")
                                .aria_label("Decision response"),
                        ),
                    )
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
                .role(Role::Button)
                .aria_label(label.clone())
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
                .role(Role::Button)
                .aria_label(label.clone())
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
                .role(Role::Button)
                .aria_label(label.clone())
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
            if let Source::FailedRun { run_id } = item.source {
                actions = actions.child(
                    secondary("inbox-dismiss-failure".into(), "Dismiss".to_string()).on_click(
                        cx.listener(move |this, _e, _w, cx| {
                            this.state.update(cx, |state, cx| {
                                state.dismissed_runs.insert(run_id);
                                cx.notify();
                            });
                            this.action_note = None;
                            cx.notify();
                        }),
                    ),
                );
            }
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
                    primary(
                        format!("inbox-outcome-{outcome}").into(),
                        outcome_label(&outcome),
                    )
                } else {
                    secondary(
                        format!("inbox-outcome-{outcome}").into(),
                        outcome_label(&outcome),
                    )
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
                    danger(
                        format!("inbox-outcome-{outcome}").into(),
                        outcome_label(&outcome),
                    )
                    .on_click(cx.listener(move |this, _e, window, cx| {
                        this.decide(&item_o, &outcome_for_click, window, cx);
                    })),
                );
            }
        } else {
            // Task review gates: canonical approve / reject.
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

        div()
            .id("inbox-focus-column")
            .flex_1()
            .min_w_0()
            .h_full()
            .v_flex()
            .child(pane.pb(px(18.0)))
            .child(footer.child(actions))
    }

    fn render_all_clear(&self, live: bool) -> Div {
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
                    .child(if live {
                        "No pending decisions — describe new work in Fleet."
                    } else {
                        "Daemon disconnected — reconnect to check pending decisions."
                    }),
            )
    }
}

fn outcome_label(outcome: &str) -> String {
    match outcome {
        "approve" => "Approve",
        "edit" => "Request changes",
        "reject" => "Reject",
        other => other,
    }
    .to_owned()
}

/// A workflow document prepared once for review.
struct ReviewedPlan {
    prepared: Option<crate::flow_diagram::PreparedPlan>,
    review_markdown: String,
}

impl InboxScreen {
    /// Cached diagram + step list for `body` (built on first sight).
    fn reviewed_plan(&mut self, body: &str) -> std::rc::Rc<ReviewedPlan> {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        body.hash(&mut hasher);
        let key = hasher.finish();
        if self.plans.len() > 16 {
            self.plans.clear();
        }
        std::rc::Rc::clone(self.plans.entry(key).or_insert_with(|| {
            std::rc::Rc::new(ReviewedPlan {
                prepared: toml::from_str::<surge_core::graph::Graph>(body)
                    .ok()
                    .map(|graph| crate::flow_diagram::PreparedPlan::new(&graph)),
                review_markdown: crate::flow_review::flow_review_markdown(body),
            })
        }))
    }
}

/// Evidence label of the generated workflow under review.
const WORKFLOW_LABEL: &str = "Workflow";

fn review_artifact(schema: Option<&serde_json::Value>) -> Option<(&'static str, &'static str)> {
    match schema?.get("x-surge-bootstrap-stage")?.as_str()? {
        "description" => Some(("Description", "description")),
        "roadmap" => Some(("Roadmap", "roadmap_md")),
        "flow" => Some((WORKFLOW_LABEL, "flow")),
        _ => None,
    }
}

fn read_review_document(path: &std::path::Path) -> String {
    use std::io::Read as _;
    let read = || -> std::io::Result<String> {
        let file = std::fs::File::open(path)?;
        let mut content = String::new();
        file.take(2 * 1024 * 1024 + 1)
            .read_to_string(&mut content)?;
        if content.len() > 2 * 1024 * 1024 {
            return Err(std::io::Error::other(
                "document exceeds the 2 MiB preview limit",
            ));
        }
        Ok(content)
    };
    read().unwrap_or_else(|error| format!("Could not load the review document: {error}"))
}

impl Render for InboxScreen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.load_review_documents(cx);
        let (items, live) = self.items(cx);
        let selected = items
            .get(self.selected.min(items.len().saturating_sub(1)))
            .cloned();

        let focus: AnyElement = match &selected {
            Some(item) => self.render_focus(item, window, cx).into_any_element(),
            None => self.render_all_clear(live).into_any_element(),
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

#[cfg(test)]
mod tests {
    use super::InboxScreen;
    use crate::app_state::AppState;
    use gpui_kit::{AppContext, TestAppContext};

    #[gpui_kit::test]
    fn bootstrap_review_shows_document_and_edit_action(cx: &mut TestAppContext) {
        use crate::run_stream::{DecisionKind, PendingDecision, RunStreamState};
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("immutable-description");
        std::fs::write(&path, "## Goal\nBuild an offline timer.").unwrap();
        let kind = DecisionKind::HumanInput {
            call_id: None,
            prompt: "Approve description?".into(),
            schema: Some(serde_json::json!({"x-surge-bootstrap-stage":"description"})),
        };
        assert_eq!(kind.gate_outcomes(), ["approve", "edit", "reject"]);
        let inbox = cx.update(|cx| {
            let state = cx.new(|_| {
                let mut state = AppState::new();
                let mut stream = RunStreamState::default();
                stream.artifacts.insert("description".into(), path.clone());
                stream.pending.push(PendingDecision {
                    seq: 1,
                    time: String::new(),
                    node: "description_gate".into(),
                    kind,
                });
                state.run_streams.insert(surge_core::RunId::new(), stream);
                state
            });
            cx.new(|cx| InboxScreen::new(state, cx))
        });
        inbox.update(cx, |inbox, cx| {
            inbox
                .documents
                .insert(path.clone(), super::read_review_document(&path));
            let (items, _) = inbox.items(cx);
            assert_eq!(items.len(), 1);
            assert!(
                items[0]
                    .evidence
                    .iter()
                    .any(|(label, body)| label == "Description"
                        && body.contains("Build an offline timer."))
            );
            assert!(
                !items[0]
                    .evidence
                    .iter()
                    .any(|(label, _)| label.contains("SCHEMA"))
            );
        });
    }

    #[gpui_kit::test]
    fn empty_inbox_never_invents_approval_requests(cx: &mut TestAppContext) {
        let inbox = cx.update(|cx| {
            let state = cx.new(|_| AppState::new());
            cx.new(|cx| InboxScreen::new(state, cx))
        });
        inbox.update(cx, |inbox, cx| {
            let (items, connected) = inbox.items(cx);
            assert!(items.is_empty());
            assert!(!connected);
        });
    }
}
