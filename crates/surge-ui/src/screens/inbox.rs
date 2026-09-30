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
    OpenPlan { run_id: RunId, decision_seq: u64 },
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
    /// A failed or aborted run needing triage.
    FailedRun { run_id: RunId },
}

/// One decision unit in the queue.
#[derive(Clone)]
struct InboxItem {
    /// Urgency class (lower = first). Live kinds 0-4, failures 7.
    rank: u8,
    badge: &'static str,
    badge_color: Hsla,
    title: String,
    /// The mission this belongs to (the request, as a headline).
    mission: Option<String>,
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
            Source::FailedRun { run_id } => format!("failed-{run_id}"),
        }
    }
}

#[derive(Default)]
enum DecisionSubmission {
    #[default]
    Editing,
    Pending,
    Failed(String),
    Acknowledged {
        comment: String,
    },
}

#[derive(Default)]
struct DecisionDraft {
    comment: String,
    submission: DecisionSubmission,
}

struct SubmittedDecision {
    identity: String,
    run_id: RunId,
    comment: String,
    edits: Option<surge_core::node_overrides::NodeOverrides>,
}

#[cfg(test)]
type ControlledResponses = std::rc::Rc<
    std::cell::RefCell<
        Vec<(
            serde_json::Value,
            tokio::sync::oneshot::Sender<Result<(), String>>,
        )>,
    >,
>;

/// Inbox screen — the urgency-ranked operator decision queue.
pub struct InboxScreen {
    state: Entity<AppState>,
    selected: usize,
    response_input: Option<Entity<InputState>>,
    response_owner: Option<String>,
    drafts: std::collections::HashMap<String, DecisionDraft>,
    #[cfg(test)]
    controlled_responses: Option<ControlledResponses>,
    /// Parsed + laid-out workflow plans and their step-list Markdown, keyed
    /// by a hash of the document. Built once per document, not per frame.
    plans: std::collections::HashMap<u64, std::rc::Rc<ReviewedPlan>>,
    documents: std::collections::HashMap<std::path::PathBuf, String>,
    /// Decisions already made in this project, newest first.
    history: Vec<crate::decisions::Decision>,
    /// Project runs (id + status) the history was read for.
    history_for: Option<Vec<String>>,
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
            response_owner: None,
            drafts: std::collections::HashMap::new(),
            #[cfg(test)]
            controlled_responses: None,
            plans: std::collections::HashMap::new(),
            documents: std::collections::HashMap::new(),
            history: Vec::new(),
            history_for: None,
        }
    }

    /// Re-read decision history when a project run appears or changes
    /// status (a decision always moves its run forward).
    fn reload_history_if_changed(&mut self, cx: &mut Context<Self>) {
        let (key, runs): (Vec<String>, Vec<RunId>) = {
            let state = self.state.read(cx);
            let runs = state.project_runs();
            let mut ids: Vec<RunId> = runs.iter().map(|r| r.run_id).collect();
            // A mission's approvals live in its planning run, which is not
            // always listed as a project run once the build has started.
            for op in state.bootstrap_operations.values() {
                if ids.contains(&op.implementation_run) && !ids.contains(&op.planning_run) {
                    ids.push(op.planning_run);
                }
            }
            (
                runs.iter()
                    .map(|r| format!("{}:{:?}", r.run_id, r.status))
                    .collect(),
                ids,
            )
        };
        if self.history_for.as_ref() == Some(&key) {
            return;
        }
        self.history_for = Some(key);
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let history = crate::decisions::recent(runs, 12).await;
            cx.update(|cx| {
                let _ = this.update(cx, |screen, cx| {
                    screen.history = history;
                    cx.notify();
                });
            });
        })
        .detach();
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
                    "Surge could not continue on its own".to_string(),
                    vec![("What happened".to_string(), reason.clone())],
                ),
                DecisionKind::Elevation { capability } => (
                    theme::warning(),
                    format!("An agent asks for more access: {capability}"),
                    vec![(
                        "How to answer".to_string(),
                        "Answer on the approval channel set up for this project (Telegram or \
                         the desktop prompt). If nobody answers in time, the request is declined."
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
                    (theme::warning(), prompt.clone(), ev)
                },
                DecisionKind::RoadmapPatch { patch_id } => (
                    theme::violet(),
                    "A change to the roadmap waits for approval".to_string(),
                    vec![(
                        "How to answer".to_string(),
                        format!(
                            "Roadmap changes are approved from the terminal for now: \
                             `surge feature show {patch_id}`."
                        ),
                    )],
                ),
            };
            let call_id = match &p.kind {
                DecisionKind::HumanInput { call_id, .. } => call_id.clone(),
                _ => None,
            };
            let badge = match &p.kind {
                DecisionKind::HumanInput { schema, .. } => match review_artifact(schema.as_ref()) {
                    Some(_) => "approve plan",
                    None if p.kind.badge() == "human input" => "question",
                    None => "approval",
                },
                DecisionKind::Elevation { .. } => "permission",
                DecisionKind::RoadmapPatch { .. } => "roadmap change",
                DecisionKind::Escalation { .. } => "needs help",
            };
            items.push(InboxItem {
                rank: p.kind.rank(),
                badge,
                badge_color,
                title,
                mission: state.run_prompt(&run_id).map(|p| ui::headline(p, 80)),
                meta: format!(
                    "{} · r-{}",
                    p.node.replace('_', " "),
                    run_id.short().to_lowercase()
                ),
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

        // 2. Failed / aborted runs — failure triage.
        for run in state.unacknowledged_failures() {
            if !matches!(run.status, RunStatus::Failed | RunStatus::Aborted) {
                continue;
            }
            let label = if matches!(run.status, RunStatus::Failed) {
                "failed"
            } else {
                "aborted"
            };
            // The run's own last word, when its stream is loaded.
            let mut evidence = Vec::new();
            let mut reason: Option<String> = None;
            if let Some(stream) = state.run_streams.get(&run.run_id) {
                reason = stream
                    .log
                    .iter()
                    .rev()
                    .find(|r| r.kind == "END")
                    .map(|r| r.text.clone());
                let tail: Vec<String> = stream
                    .log
                    .iter()
                    .rev()
                    .take(6)
                    .map(|r| format!("{}  {}  {}", r.time, r.kind, r.text))
                    .collect();
                if let Some(reason) = &reason {
                    evidence.push(("What happened".to_string(), reason.clone()));
                }
                if !tail.is_empty() {
                    evidence.push((
                        "Last events".to_string(),
                        format!("```text\n{}\n```", tail.join("\n")),
                    ));
                }
            }
            let headline = reason.as_deref().map_or(
                if label == "failed" {
                    "Stopped with an error"
                } else {
                    "Stopped by request"
                },
                |r| crate::mission::failure_headline(r),
            );
            items.push(InboxItem {
                rank: 7,
                badge: label,
                badge_color: theme::error(),
                title: headline.to_string(),
                mission: state.run_prompt(&run.run_id).map(|p| ui::headline(p, 80)),
                meta: format!(
                    "started {} · r-{}",
                    run.started_at.with_timezone(&chrono::Local).format("%H:%M"),
                    run.run_id.short().to_lowercase()
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
        submitted: SubmittedDecision,
        node: String,
        call_id: Option<String>,
        response: serde_json::Value,
        cx: &mut Context<Self>,
    ) {
        let Ok(node) = surge_core::keys::NodeKey::try_from(node.as_str()) else {
            self.finish_resolution(
                submitted,
                Err("invalid request node — refresh this run".into()),
                cx,
            );
            return;
        };
        #[cfg(test)]
        if let Some(responses) = &self.controlled_responses {
            let (sender, receiver) = tokio::sync::oneshot::channel();
            responses.borrow_mut().push((response, sender));
            cx.spawn(async move |this, cx| {
                let result = receiver
                    .await
                    .unwrap_or_else(|_| Err("resolver disconnected".into()));
                let _ = this.update(cx, |screen, cx| {
                    screen.finish_resolution(submitted, result, cx)
                });
            })
            .detach();
            return;
        }
        let Some(facade) = self.state.read(cx).daemon_state.facade() else {
            self.finish_resolution(submitted, Err("daemon offline — cannot resolve".into()), cx);
            return;
        };
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let result = facade
                .resolve_requested_input(submitted.run_id, node, call_id, response)
                .await
                .map_err(|error| error.to_string());
            let _ = this.update(cx, |screen, cx| {
                screen.finish_resolution(submitted, result, cx)
            });
        })
        .detach();
    }

    fn finish_resolution(
        &mut self,
        submitted: SubmittedDecision,
        result: Result<(), String>,
        cx: &mut Context<Self>,
    ) {
        let draft = self.drafts.entry(submitted.identity.clone()).or_default();
        if !matches!(draft.submission, DecisionSubmission::Pending) {
            return;
        }
        draft.submission = match result {
            Ok(()) => {
                if draft.comment == submitted.comment {
                    draft.comment.clear();
                }
                if let Some(edits) = submitted.edits {
                    self.state.update(cx, |state, cx| {
                        let newer_flow_decision = state
                            .run_streams
                            .get(&submitted.run_id)
                            .is_some_and(|stream| {
                                stream.pending.iter().any(|decision| {
                                    decision.node == "flow_gate"
                                        && format!("live-{}-{}", submitted.run_id, decision.seq)
                                            != submitted.identity
                                })
                            });
                        if !newer_flow_decision
                            && state.plan_edits.get(&submitted.run_id) == Some(&edits)
                        {
                            state.plan_edits.remove(&submitted.run_id);
                            cx.notify();
                        }
                    });
                }
                DecisionSubmission::Acknowledged {
                    comment: submitted.comment,
                }
            },
            Err(error) => DecisionSubmission::Failed(format!("Resolve failed: {error}")),
        };
        cx.notify();
    }

    fn comment(&self, cx: &Context<Self>) -> String {
        self.response_input
            .as_ref()
            .map_or_else(String::new, |input| input.read(cx).value().to_string())
    }

    fn begin_submission(
        &mut self,
        item: &InboxItem,
        edits: Option<surge_core::node_overrides::NodeOverrides>,
        cx: &mut Context<Self>,
    ) -> Option<SubmittedDecision> {
        let Source::Live { run_id, .. } = &item.source else {
            return None;
        };
        let identity = item.identity();
        // Stale event handlers cannot submit another decision's draft.
        if self.response_owner.as_ref() != Some(&identity)
            || !self
                .items(cx)
                .0
                .iter()
                .any(|current| current.identity() == identity)
        {
            return None;
        }
        let comment = self.comment(cx);
        let draft = self.drafts.entry(identity.clone()).or_default();
        if matches!(
            draft.submission,
            DecisionSubmission::Pending | DecisionSubmission::Acknowledged { .. }
        ) {
            return None;
        }
        draft.comment = comment.clone();
        draft.submission = DecisionSubmission::Pending;
        cx.notify();
        Some(SubmittedDecision {
            identity,
            run_id: *run_id,
            comment,
            edits,
        })
    }

    fn decide(
        &mut self,
        item: &InboxItem,
        outcome: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Source::Live {
            run_id,
            node,
            call_id,
            ..
        } = &item.source
        else {
            if let Source::FailedRun { run_id } = item.source {
                cx.emit(InboxAction::OpenRun(run_id));
            }
            return;
        };
        let edits = if outcome == "approve" && node == "flow_gate" {
            self.state
                .read(cx)
                .plan_edits
                .get(run_id)
                .filter(|edits| !edits.is_empty())
                .cloned()
        } else {
            None
        };
        let Some(submitted) = self.begin_submission(item, edits, cx) else {
            return;
        };
        let mut payload = serde_json::json!({ "outcome": outcome });
        if !submitted.comment.trim().is_empty() {
            payload["comment"] = serde_json::Value::String(submitted.comment.clone());
        }
        if let Some(edits) = &submitted.edits {
            match serde_json::to_value(edits) {
                Ok(value) => payload["node_overrides"] = value,
                Err(error) => {
                    self.finish_resolution(
                        submitted,
                        Err(format!("Could not serialize plan edits: {error}")),
                        cx,
                    );
                    return;
                },
            }
        }
        self.resolve_live(submitted, node.clone(), call_id.clone(), payload, cx);
    }

    fn send_response(&mut self, item: &InboxItem, _window: &mut Window, cx: &mut Context<Self>) {
        if let Source::Live {
            node,
            call_id: Some(call_id),
            kind: DecisionKind::HumanInput { .. },
            ..
        } = &item.source
            && surge_core::id::GateRequestId::from_event_call_id(call_id).is_none()
            && !self.comment(cx).trim().is_empty()
            && let Some(submitted) = self.begin_submission(item, None, cx)
        {
            let payload = serde_json::Value::String(submitted.comment.clone());
            self.resolve_live(submitted, node.clone(), Some(call_id.clone()), payload, cx);
        }
    }

    /// Synchronize the shared input even when the live queue changes without a click.
    fn sync_response_owner(
        &mut self,
        owner: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(previous) = &self.response_owner {
            let comment = self.comment(cx);
            let draft = self.drafts.entry(previous.clone()).or_default();
            let acknowledged = matches!(&draft.submission,
                DecisionSubmission::Acknowledged { comment: submitted } if submitted == &comment);
            let clear_input = acknowledged && !comment.is_empty();
            draft.comment = if acknowledged { String::new() } else { comment };
            if clear_input && let Some(input) = self.response_input.clone() {
                input.update(cx, |input, cx| input.set_value("", window, cx));
            }
        }
        if self.response_owner != owner {
            let comment = owner
                .as_ref()
                .and_then(|identity| self.drafts.get(identity))
                .map_or_else(String::new, |draft| draft.comment.clone());
            if let Some(input) = self.response_input.clone() {
                input.update(cx, |input, cx| input.set_value(comment, window, cx));
            }
            self.response_owner = owner;
        }
    }

    // ── panes ───────────────────────────────────────────────────────

    fn render_queue(&self, items: &[InboxItem], live: bool, cx: &mut Context<Self>) -> Div {
        let mut list = div()
            .flex_1()
            .id("inbox-queue")
            .overflow_y_scroll()
            .v_flex()
            .gap(px(4.0))
            .p(px(8.0));

        for (i, item) in items.iter().enumerate() {
            let is_sel = i == self.selected.min(items.len().saturating_sub(1));
            let row = div()
                .id(SharedString::from(format!("inbox-item-{}", item.identity())))
                .role(Role::Button)
                .aria_label(item.title.clone())
                .h_flex()
                .items_stretch()
                .gap(px(10.0))
                .rounded(px(ui::R_CONTROL + 2.0))
                .border_1()
                .border_color(if is_sel { theme::hairline_strong() } else { theme::hairline() })
                .bg(if is_sel { theme::surface() } else { theme::panel_raised() })
                .overflow_hidden()
                .cursor_pointer()
                .hover(|s: StyleRefinement| s.border_color(theme::hairline_strong()))
                .on_click(cx.listener(move |this, _e, window, cx| {
                    this.select(i, window, cx);
                }))
                // Urgency is the color of the edge, not a "P0" code.
                .child(div().w(px(3.0)).flex_none().bg(item.badge_color))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .v_flex()
                        .gap(px(4.0))
                        .py(px(10.0))
                        .pr(px(12.0))
                        .child(
                            div()
                                .h_flex()
                                .gap(px(7.0))
                                .items_center()
                                .child(ui::pill(item.badge, item.badge_color, theme::tint(item.badge_color)))
                                .child(div().flex_1())
                                .child(div().text_size(px(10.0)).text_color(theme::text_dim()).child(item.age.clone())),
                        )
                        .child(
                            div()
                                .text_size(px(12.5))
                                .font_weight(FontWeight::SEMIBOLD)
                                .line_height(px(17.0))
                                .text_color(theme::text_primary())
                                .child(item.title.clone()),
                        )
                        .children(item.mission.clone().map(|m| {
                            div()
                                .text_size(px(11.0))
                                .text_color(theme::text_muted())
                                .truncate()
                                .child(m)
                        })),
                );
            list = list.child(row);
        }

        div()
            .w(px(380.0))
            .flex_shrink_0()
            .v_flex()
            .bg(theme::panel())
            .border_r_1()
            .border_color(theme::hairline())
            .child(
                div()
                    .h_flex()
                    .gap(px(10.0))
                    .items_center()
                    .px(px(16.0))
                    .pt(px(16.0))
                    .pb(px(10.0))
                    .child(
                        div()
                            .text_size(px(16.0))
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme::text_primary())
                            .child("Inbox"),
                    )
                    .child(ui::role_badge(
                        format!("{} waiting", items.len()),
                        theme::Semantic::You,
                    ))
                    .child(div().flex_1())
                    .when(!live, |el| {
                        el.child(ui::role_badge("offline", theme::Semantic::External))
                    })
                    .child(div().h_flex().gap(px(4.0)).child(ui::kbd("↑↓"))),
            )
            .child(list)
            .child(self.render_history(true, cx))
    }

    /// "Recently decided": your last answers, newest first.
    fn render_history(&self, compact: bool, cx: &Context<Self>) -> Div {
        if self.history.is_empty() {
            return div();
        }
        let state = self.state.read(cx);
        let state_rows: Vec<Div> = self
            .history
            .iter()
            .take(if compact { 5 } else { 12 })
            .map(|d| {
                let when = chrono::DateTime::from_timestamp_millis(d.at_ms)
                    .map(|t| {
                        t.with_timezone(&chrono::Local)
                            .format("%b %d %H:%M")
                            .to_string()
                    })
                    .unwrap_or_default();
                let color = d.role.color();
                div()
                    .h_flex()
                    .gap(px(10.0))
                    .items_start()
                    .py(px(5.0))
                    .child(div().pt(px(5.0)).child(ui::status_dot(color)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .v_flex()
                            .gap(px(1.0))
                            .child(
                                div()
                                    .text_size(px(11.5))
                                    .text_color(theme::text_primary())
                                    .child(d.what.clone()),
                            )
                            .children(state.run_prompt(&d.run).map(|p| {
                                div()
                                    .text_size(px(10.5))
                                    .text_color(theme::text_dim())
                                    .truncate()
                                    .child(ui::headline(p, 70))
                            }))
                            .children(d.comment.clone().map(|c| {
                                div()
                                    .text_size(px(10.5))
                                    .text_color(theme::text_muted())
                                    .truncate()
                                    .child(format!("“{c}”"))
                            })),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(10.0))
                            .text_color(theme::text_dim())
                            .child(when),
                    )
            })
            .collect();
        div()
            .flex_none()
            .v_flex()
            .gap(px(2.0))
            .px(px(16.0))
            .py(px(12.0))
            .border_t_1()
            .border_color(theme::hairline())
            .child(
                div()
                    .pb(px(4.0))
                    .child(ui::section_label("Recently decided")),
            )
            .children(state_rows)
    }

    /// Select queue item `i`, preserving each decision's draft separately.
    fn select(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.selected = i;
        let (items, _) = self.items(cx);
        self.sync_response_owner(items.get(i).map(InboxItem::identity), window, cx);
        cx.notify();
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
        let is_failure = matches!(&item.source, Source::FailedRun { .. });

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
                        self.drafts.get(&item.identity()).and_then(|draft| match &draft.submission {
                            DecisionSubmission::Editing => None,
                            DecisionSubmission::Pending => Some("Sending decision…".to_owned()),
                            DecisionSubmission::Failed(error) => Some(error.clone()),
                            DecisionSubmission::Acknowledged { .. } => Some("Decision acknowledged".to_owned()),
                        })
                            .map(|n| {
                        div()
                            .id("decision-status")
                            .test_support().debug_selector(|| "decision-status".into())
                            .role(Role::Label)
                            .aria_label(n.clone())
                            .text_size(px(10.5))
                            .text_color(theme::accent())
                            .child(n)
                    })),
            )
            .children(item.mission.clone().map(|m| {
                div()
                    .pt(px(12.0))
                    .h_flex()
                    .gap(px(8.0))
                    .child(ui::section_label("Mission"))
                    .child(div().text_size(px(12.0)).text_color(theme::text_muted()).child(m))
            }))
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
            let target = match item.source {
                Source::Live { run_id, seq, .. } => Some((run_id, seq)),
                _ => None,
            };
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
                                        .test_support().debug_selector(|| "decision-open-plan".into())
                                        .role(Role::Button)
                                        .aria_label("Open full plan")
                                        .cursor_pointer()
                                        .text_size(px(10.5))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(theme::accent())
                                        .hover(|s: StyleRefinement| s.opacity(0.8))
                                        .child("Open full plan ↗")
                                        .on_click(cx.listener(move |_this, _e, _w, cx| {
                                            if let Some((run_id, decision_seq)) = target {
                                                cx.emit(InboxAction::OpenPlan { run_id, decision_seq });
                                            }
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
        if is_tool_input || is_gate_input {
            footer = footer.child(
                div()
                    .id("decision-response")
                    .test_support()
                    .debug_selector(|| "decision-response".into())
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

        let blocked = self.drafts.get(&item.identity()).is_some_and(|draft| {
            matches!(
                draft.submission,
                DecisionSubmission::Pending | DecisionSubmission::Acknowledged { .. }
            )
        });
        let primary = |id: SharedString, label: String| {
            div()
                .id(id.clone())
                .test_support()
                .debug_selector(|| id.to_string())
                .accessibility_id(id)
                .role(Role::Button)
                .aria_label(label.clone())
                .when(blocked, |element| {
                    element
                        .opacity(0.5)
                        .aria_description("Decision already submitted")
                })
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
                .id(id.clone())
                .test_support()
                .debug_selector(|| id.to_string())
                .accessibility_id(id)
                .role(Role::Button)
                .aria_label(label.clone())
                .when(blocked, |element| {
                    element
                        .opacity(0.5)
                        .aria_description("Decision already submitted")
                })
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
                .id(id.clone())
                .test_support()
                .debug_selector(|| id.to_string())
                .accessibility_id(id)
                .role(Role::Button)
                .aria_label(label.clone())
                .when(blocked, |element| {
                    element
                        .opacity(0.5)
                        .aria_description("Decision already submitted")
                })
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
                            cx.notify();
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
            // Elevations, escalations, roadmap patches and unbound legacy
            // requests are answered in the run itself, not here.
            if let Source::Live { run_id, .. } = item.source {
                actions = actions.child(
                    secondary("inbox-open-run-2".into(), "Open cockpit".to_string()).on_click(
                        cx.listener(move |_this, _e, _w, cx| {
                            cx.emit(InboxAction::OpenRun(run_id));
                        }),
                    ),
                );
            }
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

    /// Nothing waits on you: one calm page, with what you decided last.
    fn render_all_clear(&self, live: bool, cx: &Context<Self>) -> Stateful<Div> {
        div()
            .id("inbox-clear")
            .size_full()
            .overflow_y_scroll()
            .bg(theme::background())
            .flex()
            .justify_center()
            .px(px(28.0))
            .pt(px(48.0))
            .child(
                div()
                    .v_flex()
                    .gap(px(18.0))
                    .w_full()
                    .max_w(px(640.0))
                    .child(
                        ui::panel().child(
                            div()
                                .id("decision-status")
                                .role(Role::Label)
                                .aria_label(if live {
                                    "Nothing blocked on you"
                                } else {
                                    "Reconnect to check decisions"
                                })
                                .child(ui::empty_state(
                                    "✓",
                                    if live { "Nothing waits on you" } else { "Inbox offline" },
                                    if live {
                                        "Plans to approve, questions from agents and failed missions land here, most urgent first."
                                    } else {
                                        "Start the engine from the sidebar to see what waits on you."
                                    },
                                )),
                        ),
                    )
                    .when(!self.history.is_empty(), |el| {
                        el.child(ui::panel().overflow_hidden().child(self.render_history(false, cx)))
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
        self.reload_history_if_changed(cx);
        let (items, live) = self.items(cx);
        self.sync_response_owner(
            items
                .get(self.selected.min(items.len().saturating_sub(1)))
                .map(InboxItem::identity),
            window,
            cx,
        );
        if items.is_empty() {
            return div().size_full().child(self.render_all_clear(live, cx));
        }
        let selected = items
            .get(self.selected.min(items.len().saturating_sub(1)))
            .cloned();
        let focus: AnyElement = match &selected {
            Some(item) => self.render_focus(item, window, cx).into_any_element(),
            None => div().into_any_element(),
        };
        let count = items.len();

        // Plain .flex() row so both panes stretch to full height
        // (h_flex would vertically center them).
        div()
            .size_full()
            .flex()
            .bg(theme::background())
            // ↑/↓ walk the queue (the comment field ignores them).
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                let current = this.selected.min(count.saturating_sub(1));
                match event.keystroke.key.as_str() {
                    "down" if current + 1 < count => this.select(current + 1, window, cx),
                    "up" if current > 0 => this.select(current - 1, window, cx),
                    _ => {},
                }
            }))
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
    fn offline_approval_preserves_exact_comment_and_plan_edits(cx: &mut TestAppContext) {
        let (state, inbox, run, _) = controlled_fixture(cx, false);
        inbox.update(cx, |screen, _| screen.controlled_responses = None);
        state.update(cx, |state, _| {
            state.plan_edits.insert(run, edits("offline"));
        });
        let (_, window) = cx.add_window_view(|window, cx| {
            gpui_kit::component::Root::new(inbox.clone(), window, cx)
        });
        write_comment(window, &inbox, "  Preserve this comment  ");
        click(window, "inbox-outcome-approve");
        assert_comment(window, &inbox, "  Preserve this comment  ");
        window.update(|_, cx| {
            inbox.update(cx, |screen, cx| {
                assert_eq!(state.read(cx).plan_edits.get(&run), Some(&edits("offline")));
                let owner = screen.response_owner.as_ref().unwrap();
                assert!(matches!(&screen.drafts[owner].submission,
                super::DecisionSubmission::Failed(error) if error.contains("daemon offline")));
            })
        });
        assert!(window.debug_bounds("decision-status").is_some());
    }

    fn init_components(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::init();
            crate::theme::sync_component_theme(cx);
        });
    }

    fn decision(seq: u64, tool: bool) -> crate::run_stream::PendingDecision {
        crate::run_stream::PendingDecision {
            seq,
            time: String::new(),
            node: "flow_gate".into(),
            kind: crate::run_stream::DecisionKind::HumanInput {
                call_id: Some(if tool {
                    format!("tool-{seq}")
                } else {
                    surge_core::id::GateRequestId::new().to_string()
                }),
                prompt: format!("Decision {seq}"),
                schema: (!tool).then(|| serde_json::json!({"x-surge-bootstrap-stage":"flow"})),
            },
        }
    }

    fn controlled_fixture(
        cx: &mut TestAppContext,
        tool: bool,
    ) -> (
        gpui_kit::Entity<AppState>,
        gpui_kit::Entity<InboxScreen>,
        surge_core::RunId,
        super::ControlledResponses,
    ) {
        init_components(cx);
        let run = surge_core::RunId::new();
        let state = cx.new(|_| {
            let mut state = AppState::new();
            let mut stream = crate::run_stream::RunStreamState::default();
            stream.pending.push(decision(1, tool));
            state.run_streams.insert(run, stream);
            state
        });
        let responses = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let inbox = cx.new(|cx| {
            let mut screen = InboxScreen::new(state.clone(), cx);
            screen.controlled_responses = Some(responses.clone());
            screen
        });
        (state, inbox, run, responses)
    }

    fn write_comment(
        window: &mut gpui_kit::VisualTestContext,
        inbox: &gpui_kit::Entity<InboxScreen>,
        text: &str,
    ) {
        assert!(window.debug_bounds("decision-response").is_some());
        window.update(|window, cx| {
            inbox.update(cx, |screen, cx| {
                screen
                    .response_input
                    .clone()
                    .unwrap()
                    .update(cx, |input, cx| input.set_value(text, window, cx));
            })
        });
    }

    fn click(window: &mut gpui_kit::VisualTestContext, id: &'static str) {
        let bounds = window.debug_bounds(id).unwrap();
        window.simulate_click(bounds.center(), gpui_kit::Modifiers::default());
    }

    fn assert_comment(
        window: &mut gpui_kit::VisualTestContext,
        inbox: &gpui_kit::Entity<InboxScreen>,
        expected: &str,
    ) {
        window.update(|_, cx| {
            inbox.update(cx, |screen, cx| assert_eq!(screen.comment(cx), expected))
        });
    }

    fn edits(model: &str) -> surge_core::node_overrides::NodeOverrides {
        surge_core::node_overrides::NodeOverrides(std::collections::BTreeMap::from([(
            "implement".into(),
            surge_core::node_overrides::NodeOverride {
                model: Some(model.into()),
                ..Default::default()
            },
        )]))
    }

    #[gpui_kit::test]
    fn approval_rejection_preserves_edits_and_retry_acknowledges_once(cx: &mut TestAppContext) {
        let (state, inbox, run, responses) = controlled_fixture(cx, false);
        state.update(cx, |state, _| {
            state.plan_edits.insert(run, edits("first"));
        });
        let (_, window) = cx.add_window_view(|window, cx| {
            gpui_kit::component::Root::new(inbox.clone(), window, cx)
        });
        write_comment(window, &inbox, "  exact comment  ");
        click(window, "inbox-outcome-approve");
        click(window, "inbox-outcome-approve");
        assert_eq!(responses.borrow().len(), 1);
        let (payload, sender) = responses.borrow_mut().remove(0);
        assert_eq!(payload["comment"], "  exact comment  ");
        assert_eq!(
            payload["node_overrides"],
            serde_json::to_value(edits("first")).unwrap()
        );
        sender.send(Err("queue rejected".into())).unwrap();
        window.run_until_parked();
        assert_comment(window, &inbox, "  exact comment  ");
        window.update(|_, cx| inbox.update(cx, |screen, cx| {
            let owner = screen.response_owner.as_ref().unwrap();
            assert!(matches!(&screen.drafts[owner].submission, super::DecisionSubmission::Failed(error) if error.contains("queue rejected")));
            assert_eq!(state.read(cx).plan_edits.get(&run), Some(&edits("first")));
        }));
        click(window, "inbox-outcome-approve");
        responses.borrow_mut().remove(0).1.send(Ok(())).unwrap();
        window.run_until_parked();
        assert_comment(window, &inbox, "");
        window.update(|_, cx| assert!(!state.read(cx).plan_edits.contains_key(&run)));
        click(window, "inbox-outcome-approve");
        assert!(responses.borrow().is_empty());
    }

    #[gpui_kit::test]
    fn pending_tool_response_blocks_enter_and_late_ack_preserves_changed_text(
        cx: &mut TestAppContext,
    ) {
        let (_, inbox, _, responses) = controlled_fixture(cx, true);
        let (_, window) = cx.add_window_view(|window, cx| {
            gpui_kit::component::Root::new(inbox.clone(), window, cx)
        });
        write_comment(window, &inbox, "  exact response  ");
        click(window, "inbox-send-response");
        let bounds = window.debug_bounds("decision-response").unwrap();
        window.simulate_click(bounds.center(), gpui_kit::Modifiers::default());
        window.simulate_keystrokes("enter enter");
        assert_eq!(responses.borrow().len(), 1);
        let (payload, sender) = responses.borrow_mut().remove(0);
        assert_eq!(payload, serde_json::json!("  exact response  "));
        write_comment(window, &inbox, "new response");
        sender.send(Ok(())).unwrap();
        window.run_until_parked();
        assert_comment(window, &inbox, "new response");
    }

    #[gpui_kit::test]
    fn switching_decisions_preserves_drafts_and_scopes_late_error(cx: &mut TestAppContext) {
        let (state, inbox, run, responses) = controlled_fixture(cx, false);
        state.update(cx, |state, _| {
            state
                .run_streams
                .get_mut(&run)
                .unwrap()
                .pending
                .push(decision(2, false))
        });
        let (_, window) = cx.add_window_view(|window, cx| {
            gpui_kit::component::Root::new(inbox.clone(), window, cx)
        });
        write_comment(window, &inbox, "first draft");
        click(window, "inbox-outcome-approve");
        let first_owner = window.update(|_, cx| inbox.read(cx).response_owner.clone().unwrap());
        window.update(|window, cx| inbox.update(cx, |screen, cx| screen.select(1, window, cx)));
        assert_comment(window, &inbox, "");
        write_comment(window, &inbox, "second draft");
        responses
            .borrow_mut()
            .remove(0)
            .1
            .send(Err("first rejection".into()))
            .unwrap();
        window.run_until_parked();
        assert_comment(window, &inbox, "second draft");
        window.update(|window, cx| inbox.update(cx, |screen, cx| {
            assert_ne!(screen.response_owner.as_ref(), Some(&first_owner));
            assert!(matches!(&screen.drafts[&first_owner].submission, super::DecisionSubmission::Failed(error) if error.contains("first rejection")));
            screen.select(0, window, cx);
        }));
        assert_comment(window, &inbox, "first draft");
    }

    #[gpui_kit::test]
    fn acknowledged_approval_does_not_remove_changed_overrides(cx: &mut TestAppContext) {
        let (state, inbox, run, responses) = controlled_fixture(cx, false);
        state.update(cx, |state, _| {
            state.plan_edits.insert(run, edits("first"));
        });
        let (_, window) = cx.add_window_view(|window, cx| {
            gpui_kit::component::Root::new(inbox.clone(), window, cx)
        });
        click(window, "inbox-outcome-approve");
        window.update(|_, cx| {
            state.update(cx, |state, _| {
                state.plan_edits.insert(run, edits("changed"));
            })
        });
        responses.borrow_mut().remove(0).1.send(Ok(())).unwrap();
        window.run_until_parked();
        window.update(|_, cx| {
            assert_eq!(state.read(cx).plan_edits.get(&run), Some(&edits("changed")))
        });
    }

    #[gpui_kit::test]
    fn queue_replacement_preserves_same_valued_new_plan_edits_and_comment(cx: &mut TestAppContext) {
        let (state, inbox, run, responses) = controlled_fixture(cx, false);
        state.update(cx, |state, _| {
            state.plan_edits.insert(run, edits("first"));
        });
        let (_, window) = cx.add_window_view(|window, cx| {
            gpui_kit::component::Root::new(inbox.clone(), window, cx)
        });
        write_comment(window, &inbox, "old draft");
        click(window, "inbox-outcome-approve");
        window.update(|_, cx| {
            state.update(cx, |state, cx| {
                state.run_streams.get_mut(&run).unwrap().pending = vec![decision(2, false)];
                state.plan_edits.insert(run, edits("first"));
                cx.notify();
            })
        });
        window.run_until_parked();
        assert_comment(window, &inbox, "");
        write_comment(window, &inbox, "new decision draft");
        responses.borrow_mut().remove(0).1.send(Ok(())).unwrap();
        window.run_until_parked();
        assert_comment(window, &inbox, "new decision draft");
        window
            .update(|_, cx| assert_eq!(state.read(cx).plan_edits.get(&run), Some(&edits("first"))));
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
