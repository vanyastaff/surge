use gpui::*;
use gpui::prelude::FluentBuilder;
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::StyledExt;
use surge_core::TaskState;

use crate::app_state::AppState;
use crate::theme;

/// Summary counts for the Project Health card.
#[derive(Debug, Clone, Default)]
pub struct TaskCounts {
    pub draft: u32,
    pub planning: u32,
    pub executing: u32,
    pub qa_review: u32,
    pub human_review: u32,
    pub completed: u32,
    pub failed: u32,
}

impl TaskCounts {
    pub fn total(&self) -> u32 {
        self.draft + self.planning + self.executing + self.qa_review
            + self.human_review + self.completed + self.failed
    }

    pub fn active(&self) -> u32 {
        self.planning + self.executing + self.qa_review + self.human_review
    }
}

/// Agent summary for the dashboard.
#[derive(Debug, Clone)]
pub struct AgentSummary {
    pub name: String,
    pub connected: bool,
    pub active_tasks: u32,
}

/// A recent activity entry.
#[derive(Debug, Clone)]
pub struct ActivityEntry {
    pub message: String,
    pub timestamp: String,
    pub kind: ActivityKind,
}

#[derive(Debug, Clone, Copy)]
pub enum ActivityKind {
    TaskUpdate,
    AgentEvent,
    GitEvent,
}

/// Dashboard screen — project overview.
pub struct DashboardScreen {
    state: Entity<AppState>,
}

impl DashboardScreen {
    pub fn new(state: Entity<AppState>, _cx: &mut Context<Self>) -> Self {
        Self { state }
    }

    /// Build task counts from AppState tasks.
    fn task_counts(&self, cx: &Context<Self>) -> TaskCounts {
        let state = self.state.read(cx);
        let mut counts = TaskCounts::default();
        for task in &state.tasks {
            match &task.state {
                TaskState::Draft => counts.draft += 1,
                TaskState::Planning | TaskState::Planned { .. } => counts.planning += 1,
                TaskState::Executing { .. } => counts.executing += 1,
                TaskState::QaReview | TaskState::QaFix { .. } => counts.qa_review += 1,
                TaskState::HumanReview => counts.human_review += 1,
                TaskState::Completed | TaskState::Merging => counts.completed += 1,
                TaskState::Failed { .. } | TaskState::Cancelled => counts.failed += 1,
            }
        }
        counts
    }

    /// Build agent summaries from AppState installed_agents.
    fn agent_summaries(&self, cx: &Context<Self>) -> Vec<AgentSummary> {
        let state = self.state.read(cx);
        state
            .installed_agents
            .iter()
            .map(|a| AgentSummary {
                name: a.entry.id.clone(),
                connected: false, // Real connection status will come from health monitor later.
                active_tasks: 0,
            })
            .collect()
    }

    /// Build recent activity from AppState events.
    fn recent_activity(&self, cx: &Context<Self>) -> Vec<ActivityEntry> {
        let state = self.state.read(cx);
        state
            .recent_events
            .iter()
            .rev()
            .take(10)
            .map(|event| {
                let (message, kind) = match event {
                    surge_core::SurgeEvent::TaskStateChanged { task_id, new_state, .. } => {
                        (format!("Task {} moved to {:?}", task_id, new_state), ActivityKind::TaskUpdate)
                    }
                    surge_core::SurgeEvent::AgentConnected { agent_name } => {
                        (format!("{} connected", agent_name), ActivityKind::AgentEvent)
                    }
                    _ => (format!("{:?}", event), ActivityKind::TaskUpdate),
                };
                ActivityEntry {
                    message,
                    timestamp: String::new(),
                    kind,
                }
            })
            .collect()
    }

    fn render_health_card(&self, counts: &TaskCounts) -> Div {
        let items = [
            ("Draft", counts.draft, theme::TEXT_MUTED),
            ("Planning", counts.planning, theme::PRIMARY),
            ("Executing", counts.executing, theme::WARNING),
            ("QA Review", counts.qa_review, theme::PRIMARY),
            ("Human Review", counts.human_review, theme::WARNING),
            ("Completed", counts.completed, theme::SUCCESS),
            ("Failed", counts.failed, theme::ERROR),
        ];

        let bars: Vec<Div> = items
            .iter()
            .map(|(label, count, color)| {
                div()
                    .h_flex()
                    .justify_between()
                    .items_center()
                    .child(
                        div()
                            .h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .w(px(8.0))
                                    .h(px(8.0))
                                    .rounded_full()
                                    .bg(*color),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(theme::TEXT_MUTED)
                                    .child(label.to_string()),
                            ),
                    )
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::TEXT_PRIMARY)
                            .child(format!("{count}")),
                    )
            })
            .collect();

        self.card(
            "Project Health",
            &format!("{} total · {} active", counts.total(), counts.active()),
            div().v_flex().gap_2().children(bars),
        )
    }

    fn render_agents_card(&self, agents: &[AgentSummary]) -> Div {
        let items: Vec<Div> = agents
            .iter()
            .map(|a| {
                let status_color = if a.connected { theme::SUCCESS } else { theme::ERROR };
                let status_text = if a.connected { "Online" } else { "Offline" };

                div()
                    .h_flex()
                    .justify_between()
                    .items_center()
                    .child(
                        div()
                            .h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .w(px(8.0))
                                    .h(px(8.0))
                                    .rounded_full()
                                    .bg(status_color),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(theme::TEXT_PRIMARY)
                                    .child(a.name.clone()),
                            ),
                    )
                    .child(
                        div()
                            .h_flex()
                            .gap_2()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(status_color)
                                    .child(status_text.to_string()),
                            )
                            .when(a.active_tasks > 0, |el: Div| {
                                el.child(
                                    div()
                                        .text_xs()
                                        .px_2()
                                        .py_0p5()
                                        .rounded_md()
                                        .bg(theme::PRIMARY.opacity(0.15))
                                        .text_color(theme::PRIMARY)
                                        .child(format!("{} tasks", a.active_tasks)),
                                )
                            }),
                    )
            })
            .collect();

        self.card(
            "Active Agents",
            &format!("{} configured", agents.len()),
            div().v_flex().gap_2().children(items),
        )
    }

    fn render_quick_actions(&self) -> Div {
        self.card(
            "Quick Actions",
            "",
            div()
                .v_flex()
                .gap_2()
                .child(
                    Button::new("qa-new-task")
                        .primary()
                        .w_full()
                        .label("New Task"),
                )
                .child(
                    Button::new("qa-continue")
                        .w_full()
                        .label("Continue Last"),
                )
                .child(
                    Button::new("qa-review")
                        .w_full()
                        .label("Review Queue (1)"),
                ),
        )
    }

    fn render_activity(&self, activity: &[ActivityEntry]) -> Div {
        let items: Vec<Div> = activity
            .iter()
            .map(|entry| {
                let icon = match entry.kind {
                    ActivityKind::TaskUpdate => "→",
                    ActivityKind::AgentEvent => "●",
                    ActivityKind::GitEvent => "⎇",
                };

                div()
                    .h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme::TEXT_MUTED)
                            .child(icon.to_string()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_sm()
                            .text_color(theme::TEXT_PRIMARY)
                            .child(entry.message.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme::TEXT_MUTED.opacity(0.6))
                            .child(entry.timestamp.clone()),
                    )
            })
            .collect();

        self.card(
            "Recent Activity",
            "",
            div().v_flex().gap_2().children(items),
        )
    }

    fn card(&self, title: &str, subtitle: &str, content: Div) -> Div {
        div()
            .v_flex()
            .gap_3()
            .p_4()
            .rounded_lg()
            .bg(theme::SURFACE)
            .border_1()
            .border_color(theme::TEXT_MUTED.opacity(0.1))
            .child(
                div()
                    .v_flex()
                    .gap_0p5()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::TEXT_PRIMARY)
                            .child(title.to_string()),
                    )
                    .when(!subtitle.is_empty(), |el: Div| {
                        el.child(
                            div()
                                .text_xs()
                                .text_color(theme::TEXT_MUTED)
                                .child(subtitle.to_string()),
                        )
                    }),
            )
            .child(content)
    }
}

impl Render for DashboardScreen {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let counts = self.task_counts(cx);
        let agents = self.agent_summaries(cx);
        let activity = self.recent_activity(cx);

        div()
            .size_full()
            .p_6()
            .v_flex()
            .gap_6()
            .overflow_hidden()
            // Title
            .child(
                div()
                    .text_2xl()
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme::TEXT_PRIMARY)
                    .child("Dashboard".to_string()),
            )
            // Grid: 2 columns
            .child(
                div()
                    .h_flex()
                    .gap_4()
                    .items_start()
                    // Left column
                    .child(
                        div()
                            .flex_1()
                            .v_flex()
                            .gap_4()
                            .child(self.render_health_card(&counts))
                            .child(self.render_activity(&activity)),
                    )
                    // Right column
                    .child(
                        div()
                            .w(px(300.0))
                            .v_flex()
                            .gap_4()
                            .child(self.render_agents_card(&agents))
                            .child(self.render_quick_actions()),
                    ),
            )
    }
}
