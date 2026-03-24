use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::{Icon, IconName, StyledExt};
use surge_core::TaskState;

use crate::app_state::AppState;
use crate::theme;

/// Kanban column state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KanbanColumn {
    Draft,
    Planning,
    Executing,
    QaReview,
    HumanReview,
    Done,
}

impl KanbanColumn {
    pub fn label(self) -> &'static str {
        match self {
            Self::Draft => "Draft",
            Self::Planning => "Planning",
            Self::Executing => "In Progress",
            Self::QaReview => "QA Review",
            Self::HumanReview => "Human Review",
            Self::Done => "Done",
        }
    }

    pub fn color(self) -> Hsla {
        match self {
            Self::Draft => theme::TEXT_MUTED,
            Self::Planning => theme::PRIMARY,
            Self::Executing => theme::WARNING,
            Self::QaReview => hsla(190.0 / 360.0, 0.8, 0.5, 1.0), // cyan
            Self::HumanReview => theme::WARNING,
            Self::Done => theme::SUCCESS,
        }
    }

    pub fn empty_icon(self) -> IconName {
        match self {
            Self::Draft => IconName::File,
            Self::Planning => IconName::Loader,
            Self::Executing => IconName::Loader,
            Self::QaReview => IconName::Search,
            Self::HumanReview => IconName::Eye,
            Self::Done => IconName::Check,
        }
    }

    pub fn empty_text(self) -> &'static str {
        match self {
            Self::Draft => "No draft tasks",
            Self::Planning => "Nothing planned",
            Self::Executing => "Nothing running\nStart a task from Planning",
            Self::QaReview => "No tasks in review\nAI will review completed tasks",
            Self::HumanReview => "No tasks awaiting review",
            Self::Done => "No completed tasks",
        }
    }

    pub fn all() -> &'static [KanbanColumn] {
        &[
            Self::Draft,
            Self::Planning,
            Self::Executing,
            Self::QaReview,
            Self::HumanReview,
            Self::Done,
        ]
    }
}

/// A task card in the kanban board.
#[derive(Debug, Clone)]
pub struct KanbanTask {
    pub id: String,
    pub title: String,
    pub description: String,
    pub agent: Option<String>,
    pub complexity: String,
    pub status_label: String,
    pub subtasks_done: usize,
    pub subtasks_total: usize,
    pub tags: Vec<(String, Hsla)>,
    pub time_ago: String,
    pub column: KanbanColumn,
}

/// Event emitted when a task card is clicked.
#[derive(Clone, PartialEq)]
pub struct TaskClicked(pub String);

impl EventEmitter<TaskClicked> for KanbanScreen {}

const COLUMN_MIN_W: f32 = 300.0;

/// Kanban Board screen.
pub struct KanbanScreen {
    state: Entity<AppState>,
}

fn state_to_column(state: &TaskState) -> KanbanColumn {
    match state {
        TaskState::Draft => KanbanColumn::Draft,
        TaskState::Planning | TaskState::Planned { .. } => KanbanColumn::Planning,
        TaskState::Executing { .. } => KanbanColumn::Executing,
        TaskState::QaReview | TaskState::QaFix { .. } => KanbanColumn::QaReview,
        TaskState::HumanReview => KanbanColumn::HumanReview,
        TaskState::Completed | TaskState::Merging => KanbanColumn::Done,
        TaskState::Failed { .. } | TaskState::Cancelled => KanbanColumn::Done,
    }
}

fn state_label(state: &TaskState) -> &'static str {
    match state {
        TaskState::Draft => "Draft",
        TaskState::Planning => "Planning",
        TaskState::Planned { .. } => "Planned",
        TaskState::Executing { .. } => "Executing",
        TaskState::QaReview => "QA Review",
        TaskState::QaFix { .. } => "QA Fix",
        TaskState::HumanReview => "Human Review",
        TaskState::Merging => "Merging",
        TaskState::Completed => "Completed",
        TaskState::Failed { .. } => "Failed",
        TaskState::Cancelled => "Cancelled",
    }
}

impl KanbanScreen {
    pub fn new(state: Entity<AppState>, _cx: &mut Context<Self>) -> Self {
        Self { state }
    }

    /// Build kanban tasks from AppState.
    fn build_tasks(&self, cx: &Context<Self>) -> Vec<KanbanTask> {
        let app_state = self.state.read(cx);
        app_state
            .tasks
            .iter()
            .map(|t| {
                let (subtasks_done, subtasks_total) = match &t.state {
                    TaskState::Executing { completed, total } => (*completed, *total),
                    _ => (0, 0),
                };
                KanbanTask {
                    id: t.id.to_string(),
                    title: t.title.clone(),
                    description: t.description.clone(),
                    agent: t.agent.clone(),
                    complexity: t.complexity.clone(),
                    status_label: state_label(&t.state).to_string(),
                    subtasks_done,
                    subtasks_total,
                    tags: Vec::new(),
                    time_ago: t.updated_at.clone(),
                    column: state_to_column(&t.state),
                }
            })
            .collect()
    }

    fn tasks_in_column<'a>(
        &self,
        col: KanbanColumn,
        tasks: &'a [KanbanTask],
    ) -> Vec<&'a KanbanTask> {
        tasks.iter().filter(|t| t.column == col).collect()
    }

    fn render_task_card(&self, task: &KanbanTask, cx: &mut Context<Self>) -> Stateful<Div> {
        let id = task.id.clone();
        let pct = if task.subtasks_total > 0 {
            task.subtasks_done as f32 / task.subtasks_total as f32
        } else {
            0.0
        };
        let progress_color = if pct >= 1.0 {
            theme::SUCCESS
        } else {
            theme::PRIMARY
        };

        // Subtask dots
        let dots: Vec<Div> = (0..task.subtasks_total.min(12))
            .map(|i| {
                let done = i < task.subtasks_done;
                div().w(px(7.0)).h(px(7.0)).rounded_full().bg(if done {
                    progress_color
                } else {
                    theme::TEXT_MUTED.opacity(0.2)
                })
            })
            .collect();

        let extra_subtasks = if task.subtasks_total > 12 {
            Some(format!("+{}", task.subtasks_total - 12))
        } else {
            None
        };

        div()
            .id(SharedString::from(format!("task-{}", task.id)))
            .w_full()
            .v_flex()
            .gap(px(10.0))
            .p_3()
            .rounded_lg()
            .bg(theme::SURFACE)
            .border_1()
            .border_color(theme::TEXT_MUTED.opacity(0.06))
            .cursor_pointer()
            .hover(|s: StyleRefinement| {
                s.bg(theme::SURFACE)
                    .border_color(theme::TEXT_MUTED.opacity(0.15))
            })
            .on_click(cx.listener(move |_this, _event, _window, cx| {
                cx.emit(TaskClicked(id.clone()));
            }))
            // Row 1: Title
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme::TEXT_PRIMARY)
                    .line_height(relative(1.3))
                    .child(task.title.clone()),
            )
            // Tags row
            .when(!task.tags.is_empty(), |el: Stateful<Div>| {
                let tags_el: Vec<Div> = task
                    .tags
                    .iter()
                    .map(|(label, color)| {
                        div()
                            .text_xs()
                            .px(px(6.0))
                            .py(px(2.0))
                            .rounded(px(4.0))
                            .bg(color.opacity(0.15))
                            .text_color(*color)
                            .child(label.clone())
                    })
                    .collect();
                el.child(div().h_flex().gap_1().flex_wrap().children(tags_el))
            })
            // Description (2 lines max)
            .child(
                div()
                    .text_xs()
                    .text_color(theme::TEXT_MUTED)
                    .line_height(relative(1.5))
                    .max_h(px(34.0))
                    .overflow_hidden()
                    .child(task.description.clone()),
            )
            // Row 2: Progress
            .when(task.subtasks_total > 0, |el: Stateful<Div>| {
                el.child(
                    div()
                        .v_flex()
                        .gap(px(6.0))
                        // Progress label
                        .child(
                            div()
                                .h_flex()
                                .justify_between()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(theme::TEXT_MUTED)
                                        .child("Progress".to_string()),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(theme::TEXT_PRIMARY)
                                        .child(format!("{}%", (pct * 100.0) as u32)),
                                ),
                        )
                        // Progress bar
                        .child(
                            div()
                                .w_full()
                                .h(px(4.0))
                                .rounded_full()
                                .bg(theme::TEXT_MUTED.opacity(0.1))
                                .child(
                                    div()
                                        .h_full()
                                        .rounded_full()
                                        .bg(progress_color)
                                        .w(relative(pct)),
                                ),
                        )
                        // Subtask dots
                        .child(
                            div()
                                .h_flex()
                                .gap(px(3.0))
                                .items_center()
                                .children(dots)
                                .when(extra_subtasks.is_some(), |el: Div| {
                                    el.child(
                                        div()
                                            .text_xs()
                                            .text_color(theme::TEXT_MUTED)
                                            .child(extra_subtasks.unwrap_or_default()),
                                    )
                                }),
                        ),
                )
            })
            // Row 3: Footer — time ago
            .child(
                div()
                    .h_flex()
                    .gap(px(4.0))
                    .items_center()
                    .child(
                        Icon::new(IconName::Calendar)
                            .size_3()
                            .text_color(theme::TEXT_MUTED.opacity(0.5)),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme::TEXT_MUTED.opacity(0.6))
                            .child(task.time_ago.clone()),
                    ),
            )
    }

    fn render_empty_column(&self, col: KanbanColumn) -> Div {
        div()
            .flex_1()
            .v_flex()
            .items_center()
            .justify_center()
            .gap_2()
            .py_8()
            .child(
                Icon::new(col.empty_icon())
                    .size_6()
                    .text_color(theme::TEXT_MUTED.opacity(0.25)),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme::TEXT_MUTED.opacity(0.4))
                    .text_center()
                    .child(col.empty_text().to_string()),
            )
    }

    fn render_column_with_tasks(
        &self,
        col: KanbanColumn,
        all_tasks: &[KanbanTask],
        cx: &mut Context<Self>,
    ) -> Div {
        let tasks = self.tasks_in_column(col, all_tasks);
        let count = tasks.len();
        let is_empty = tasks.is_empty();

        let cards: Vec<Stateful<Div>> =
            tasks.iter().map(|t| self.render_task_card(t, cx)).collect();

        div()
            .v_flex()
            .h_full()
            .flex_1()
            .min_w(px(COLUMN_MIN_W))
            .flex_shrink_0()
            .rounded_lg()
            .bg(theme::BACKGROUND.opacity(0.5))
            .border_1()
            .border_color(theme::TEXT_MUTED.opacity(0.06))
            // Colored top border
            .child(div().w_full().h(px(3.0)).rounded_t_lg().bg(col.color()))
            // Column header
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .items_center()
                    .justify_between()
                    .px_3()
                    .py_2()
                    .child(
                        div()
                            .h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(theme::TEXT_PRIMARY)
                                    .child(col.label().to_string()),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme::TEXT_MUTED)
                                    .child(format!("{count}")),
                            ),
                    )
                    .when(col == KanbanColumn::Draft, |el: Div| {
                        el.child(
                            Icon::new(IconName::Plus)
                                .size_4()
                                .text_color(theme::TEXT_MUTED),
                        )
                    }),
            )
            // Cards area
            .child(
                div()
                    .v_flex()
                    .gap_2()
                    .flex_1()
                    .px_2()
                    .pb_2()
                    .when(is_empty, |el: Div| el.child(self.render_empty_column(col)))
                    .children(cards),
            )
    }
}

impl Render for KanbanScreen {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tasks = self.build_tasks(cx);
        let is_empty = tasks.is_empty();

        if is_empty {
            return div()
                .size_full()
                .v_flex()
                .items_center()
                .justify_center()
                .gap_3()
                .child(
                    Icon::new(IconName::Inbox)
                        .size_8()
                        .text_color(theme::TEXT_MUTED.opacity(0.3)),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(theme::TEXT_MUTED)
                        .child("No tasks yet. Create a spec to get started.".to_string()),
                )
                .into_any_element();
        }

        // Store tasks temporarily for column rendering.
        let columns: Vec<Div> = KanbanColumn::all()
            .iter()
            .map(|&col| self.render_column_with_tasks(col, &tasks, cx))
            .collect();

        div()
            .size_full()
            .v_flex()
            .gap_3()
            .p_4()
            // Columns — horizontal scroll
            .child(
                div()
                    .id("kanban-columns")
                    .flex_1()
                    .h_flex()
                    .gap_3()
                    .items_start()
                    .overflow_x_scroll()
                    .pb_2()
                    .children(columns),
            )
            .into_any_element()
    }
}
