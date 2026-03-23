use gpui::*;
use gpui::prelude::FluentBuilder;
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::StyledExt;

use crate::theme;

/// Tabs in the task detail view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DetailTab {
    Overview,
    Subtasks,
    Files,
    Logs,
}

impl DetailTab {
    fn label(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Subtasks => "Subtasks",
            Self::Files => "Files",
            Self::Logs => "Logs",
        }
    }

    fn all() -> &'static [DetailTab] {
        &[Self::Overview, Self::Subtasks, Self::Files, Self::Logs]
    }
}

/// A subtask entry.
#[derive(Debug, Clone)]
pub struct SubtaskInfo {
    pub title: String,
    pub status: String,
    pub agent: Option<String>,
    pub duration: Option<String>,
}

/// Task Detail Modal content.
pub struct TaskDetailScreen {
    task_id: String,
    title: String,
    status: String,
    complexity: String,
    agent: Option<String>,
    description: String,
    subtasks: Vec<SubtaskInfo>,
    active_tab: DetailTab,
}

impl TaskDetailScreen {
    pub fn new(task_id: &str, _cx: &mut Context<Self>) -> Self {
        // Demo data.
        Self {
            task_id: task_id.to_string(),
            title: "Add auth middleware".to_string(),
            status: "Executing".to_string(),
            complexity: "Standard".to_string(),
            agent: Some("claude-acp".to_string()),
            description: "Implement JWT-based authentication middleware for all API endpoints. \
                Must support token refresh and role-based access control.".to_string(),
            subtasks: vec![
                SubtaskInfo {
                    title: "Create JWT validation module".into(),
                    status: "Completed".into(),
                    agent: Some("claude-acp".into()),
                    duration: Some("2m 15s".into()),
                },
                SubtaskInfo {
                    title: "Add middleware to router".into(),
                    status: "Completed".into(),
                    agent: Some("claude-acp".into()),
                    duration: Some("1m 45s".into()),
                },
                SubtaskInfo {
                    title: "Implement role-based guards".into(),
                    status: "Executing".into(),
                    agent: Some("claude-acp".into()),
                    duration: None,
                },
                SubtaskInfo {
                    title: "Add token refresh endpoint".into(),
                    status: "Pending".into(),
                    agent: None,
                    duration: None,
                },
                SubtaskInfo {
                    title: "Write integration tests".into(),
                    status: "Pending".into(),
                    agent: None,
                    duration: None,
                },
            ],
            active_tab: DetailTab::Overview,
        }
    }

    fn render_header(&self) -> Div {
        let status_color = match self.status.as_str() {
            "Draft" => theme::TEXT_MUTED,
            "Planning" | "Planned" => theme::PRIMARY,
            "Executing" => theme::WARNING,
            "QaReview" | "HumanReview" => theme::PRIMARY,
            "Completed" => theme::SUCCESS,
            "Failed" => theme::ERROR,
            _ => theme::TEXT_MUTED,
        };

        div()
            .v_flex()
            .gap_2()
            .pb_4()
            .border_b_1()
            .border_color(theme::TEXT_MUTED.opacity(0.1))
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .items_center()
                    // ID badge
                    .child(
                        div()
                            .text_xs()
                            .px_2()
                            .py_0p5()
                            .rounded_md()
                            .bg(theme::TEXT_MUTED.opacity(0.15))
                            .text_color(theme::TEXT_MUTED)
                            .child(self.task_id.clone()),
                    )
                    // Status badge
                    .child(
                        div()
                            .text_xs()
                            .px_2()
                            .py_0p5()
                            .rounded_md()
                            .bg(status_color.opacity(0.15))
                            .text_color(status_color)
                            .child(self.status.clone()),
                    )
                    // Complexity
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme::TEXT_MUTED)
                            .child(self.complexity.clone()),
                    ),
            )
            .child(
                div()
                    .text_xl()
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme::TEXT_PRIMARY)
                    .child(self.title.clone()),
            )
            .when(self.agent.is_some(), |el: Div| {
                let agent = self.agent.as_deref().unwrap_or("");
                el.child(
                    div()
                        .h_flex()
                        .gap_1()
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme::TEXT_MUTED)
                                .child("Agent:".to_string()),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme::PRIMARY)
                                .child(agent.to_string()),
                        ),
                )
            })
    }

    fn render_tabs(&self, cx: &mut Context<Self>) -> Div {
        let tabs: Vec<Stateful<Div>> = DetailTab::all()
            .iter()
            .map(|&tab| {
                let is_active = tab == self.active_tab;
                div()
                    .id(SharedString::from(format!("tab-{}", tab.label())))
                    .px_3()
                    .py(px(6.0))
                    .cursor_pointer()
                    .rounded_md()
                    .text_sm()
                    .text_color(if is_active { theme::PRIMARY } else { theme::TEXT_MUTED })
                    .bg(if is_active { theme::PRIMARY.opacity(0.1) } else { gpui::transparent_black() })
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.active_tab = tab;
                        cx.notify();
                    }))
                    .child(tab.label().to_string())
            })
            .collect();

        div().h_flex().gap_1().children(tabs)
    }

    fn render_tab_content(&self) -> Div {
        match self.active_tab {
            DetailTab::Overview => self.render_overview(),
            DetailTab::Subtasks => self.render_subtasks(),
            DetailTab::Files => self.render_placeholder("Changed files will appear here"),
            DetailTab::Logs => self.render_placeholder("Execution logs will appear here"),
        }
    }

    fn render_overview(&self) -> Div {
        let completed = self.subtasks.iter().filter(|s| s.status == "Completed").count();
        let total = self.subtasks.len();

        div()
            .v_flex()
            .gap_4()
            .child(
                div()
                    .v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::TEXT_PRIMARY)
                            .child("Description".to_string()),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme::TEXT_MUTED)
                            .child(self.description.clone()),
                    ),
            )
            .child(
                div()
                    .v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::TEXT_PRIMARY)
                            .child("Progress".to_string()),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme::TEXT_MUTED)
                            .child(format!("{completed}/{total} subtasks completed")),
                    )
                    .child(
                        div()
                            .w_full()
                            .h(px(6.0))
                            .rounded_full()
                            .bg(theme::TEXT_MUTED.opacity(0.15))
                            .child(
                                div()
                                    .h_full()
                                    .rounded_full()
                                    .bg(theme::PRIMARY)
                                    .w(relative(if total > 0 {
                                        completed as f32 / total as f32
                                    } else {
                                        0.0
                                    })),
                            ),
                    ),
            )
    }

    fn render_subtasks(&self) -> Div {
        let items: Vec<Div> = self
            .subtasks
            .iter()
            .map(|st| {
                let status_icon = match st.status.as_str() {
                    "Completed" => "✓",
                    "Executing" => "▶",
                    "Pending" => "○",
                    _ => "?",
                };
                let status_color = match st.status.as_str() {
                    "Completed" => theme::SUCCESS,
                    "Executing" => theme::WARNING,
                    _ => theme::TEXT_MUTED,
                };

                div()
                    .h_flex()
                    .gap_3()
                    .items_center()
                    .py(px(6.0))
                    .border_b_1()
                    .border_color(theme::TEXT_MUTED.opacity(0.05))
                    .child(
                        div()
                            .text_sm()
                            .text_color(status_color)
                            .w(px(16.0))
                            .child(status_icon.to_string()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_sm()
                            .text_color(theme::TEXT_PRIMARY)
                            .child(st.title.clone()),
                    )
                    .when(st.agent.is_some(), |el: Div| {
                        let agent = st.agent.as_deref().unwrap_or("");
                        el.child(
                            div()
                                .text_xs()
                                .text_color(theme::PRIMARY)
                                .child(agent.to_string()),
                        )
                    })
                    .when(st.duration.is_some(), |el: Div| {
                        let dur = st.duration.as_deref().unwrap_or("");
                        el.child(
                            div()
                                .text_xs()
                                .text_color(theme::TEXT_MUTED)
                                .child(dur.to_string()),
                        )
                    })
            })
            .collect();

        div().v_flex().children(items)
    }

    fn render_placeholder(&self, msg: &str) -> Div {
        div()
            .flex_1()
            .flex()
            .items_center()
            .justify_center()
            .text_sm()
            .text_color(theme::TEXT_MUTED)
            .child(msg.to_string())
    }
}

impl Render for TaskDetailScreen {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .v_flex()
            .gap_4()
            .p_6()
            .overflow_hidden()
            .child(self.render_header())
            .child(self.render_tabs(cx))
            .child(self.render_tab_content())
            // Actions
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .pt_4()
                    .border_t_1()
                    .border_color(theme::TEXT_MUTED.opacity(0.1))
                    .child(Button::new("td-open-ide").ghost().label("Open in IDE"))
                    .child(Button::new("td-terminal").ghost().label("Terminal"))
                    .child(Button::new("td-create-pr").primary().label("Create PR")),
            )
    }
}
