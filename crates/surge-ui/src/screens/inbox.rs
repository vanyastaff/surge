//! Inbox — "what is blocked on me right now".
//!
//! The product thesis: once agents are running, the human's job is
//! *supervision by exception*. The Inbox is the single queue of things
//! that actually need a human decision — human-review gates, QA sign-off,
//! and (later) agent questions / escalations. Everything else the agents
//! handle themselves and never surfaces here.
//!
//! Data-driven from `AppState.tasks` (no mock). Ordering is *attention
//! first*: the most urgent / longest-waiting item floats to the top, not
//! the most recent — mirroring the AutoGPT "Sitrep" pattern and Surge's
//! own `flag-stuck` recovery decision.

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::{IconName, StyledExt};
use surge_core::TaskState;

use crate::app_state::AppState;
use crate::components;
use crate::theme;

/// Why an item is sitting in the inbox — determines urgency + how it reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InboxReason {
    /// A human-review gate is open: the run is paused awaiting approval.
    HumanReview,
    /// QA review in progress / awaiting sign-off.
    QaReview,
}

impl InboxReason {
    fn label(self) -> &'static str {
        match self {
            Self::HumanReview => "Awaiting your approval",
            Self::QaReview => "In QA review",
        }
    }

    fn badge(self) -> &'static str {
        match self {
            Self::HumanReview => "Review",
            Self::QaReview => "QA",
        }
    }

    fn color(self) -> Hsla {
        match self {
            // Human-review is a hard block on the user — warn color.
            Self::HumanReview => theme::warning(),
            // QA is in-flight, informational — accent.
            Self::QaReview => theme::primary(),
        }
    }

    /// Sort weight — lower sorts first (more urgent).
    fn rank(self) -> u8 {
        match self {
            Self::HumanReview => 0,
            Self::QaReview => 1,
        }
    }
}

/// One actionable row in the inbox, projected from a `TaskEntry`.
#[derive(Debug, Clone)]
struct InboxItem {
    task_id: String,
    title: String,
    agent: Option<String>,
    updated_at: String,
    reason: InboxReason,
}

/// Emitted when an inbox row is activated — the app opens the matching
/// gate-approval / task view.
#[derive(Clone, PartialEq)]
pub struct InboxItemSelected(pub String);

impl EventEmitter<InboxItemSelected> for InboxScreen {}

/// Inbox screen — the review queue.
pub struct InboxScreen {
    state: Entity<AppState>,
}

impl InboxScreen {
    pub fn new(state: Entity<AppState>, _cx: &mut Context<Self>) -> Self {
        Self { state }
    }

    /// Project the task list into inbox items, attention-first.
    fn items(&self, cx: &Context<Self>) -> Vec<InboxItem> {
        let state = self.state.read(cx);
        let mut items: Vec<InboxItem> = state
            .tasks
            .iter()
            .filter_map(|t| {
                let reason = match &t.state {
                    TaskState::HumanReview => InboxReason::HumanReview,
                    TaskState::QaReview { .. } | TaskState::QaFix { .. } => InboxReason::QaReview,
                    _ => return None,
                };
                Some(InboxItem {
                    task_id: t.id.to_string(),
                    title: t.title.clone(),
                    agent: t.agent.clone(),
                    updated_at: t.updated_at.clone(),
                    reason,
                })
            })
            .collect();
        // Attention-first: hard blocks (HumanReview) above in-flight QA.
        items.sort_by_key(|i| i.reason.rank());
        items
    }

    fn render_item(&self, item: &InboxItem, cx: &mut Context<Self>) -> Stateful<Div> {
        let color = item.reason.color();
        let id = item.task_id.clone();

        div()
            .id(SharedString::from(format!("inbox-{}", item.task_id)))
            .w_full()
            .h_flex()
            .gap_3()
            .items_center()
            .px_4()
            .py_3()
            .rounded_lg()
            .bg(theme::surface_raised())
            .border_1()
            .border_color(theme::border_subtle())
            .cursor_pointer()
            .hover(|s: StyleRefinement| s.border_color(color.opacity(0.4)))
            .on_click(cx.listener(move |_this, _e, _w, cx| {
                cx.emit(InboxItemSelected(id.clone()));
            }))
            // Urgency accent bar
            .child(div().w(px(3.0)).h(px(36.0)).rounded_full().bg(color))
            // Reason badge
            .child(components::badge(item.reason.badge(), color))
            // Title + reason
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .v_flex()
                    .gap(px(2.0))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::text_primary())
                            .child(item.title.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme::text_muted())
                            .child(item.reason.label().to_string()),
                    ),
            )
            // Agent
            .when_some(item.agent.clone(), |el, agent| {
                el.child(
                    div()
                        .text_xs()
                        .text_color(theme::text_muted().opacity(0.7))
                        .child(agent),
                )
            })
            // Time
            .child(components::meta_item(
                IconName::Calendar,
                &item.updated_at,
                theme::text_muted().opacity(0.5),
            ))
    }
}

impl Render for InboxScreen {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let items = self.items(cx);
        let count = items.len();

        let body = if items.is_empty() {
            div()
                .flex_1()
                .v_flex()
                .child(components::empty_state(
                    IconName::Inbox,
                    "Inbox zero",
                    "Nothing needs your attention — agents are handling it.",
                    None,
                ))
                .into_any_element()
        } else {
            let rows: Vec<Stateful<Div>> = items.iter().map(|i| self.render_item(i, cx)).collect();
            div()
                .id("inbox-list")
                .flex_1()
                .v_flex()
                .gap_2()
                .overflow_y_scroll()
                .children(rows)
                .into_any_element()
        };

        div()
            .size_full()
            .v_flex()
            .gap_4()
            .p_6()
            .child(components::header_row(
                components::title_with_meta(
                    "Inbox",
                    div()
                        .text_sm()
                        .text_color(theme::text_muted())
                        .child(if count == 0 {
                            "all clear".to_string()
                        } else {
                            format!("{count} awaiting you")
                        }),
                ),
                None,
            ))
            .child(body)
    }
}
