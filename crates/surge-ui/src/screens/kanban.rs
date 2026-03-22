use gpui::*;
use gpui::prelude::FluentBuilder;
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::StyledExt;

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
            Self::Executing => "Executing",
            Self::QaReview => "QA Review",
            Self::HumanReview => "Review",
            Self::Done => "Done",
        }
    }

    pub fn color(self) -> Hsla {
        match self {
            Self::Draft => theme::TEXT_MUTED,
            Self::Planning => theme::PRIMARY,
            Self::Executing => theme::WARNING,
            Self::QaReview => theme::PRIMARY,
            Self::HumanReview => theme::WARNING,
            Self::Done => theme::SUCCESS,
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
    pub agent: Option<String>,
    pub complexity: String,
    pub progress: Option<(usize, usize)>,
    pub column: KanbanColumn,
}

/// Event emitted when a task card is clicked.
#[derive(Clone, PartialEq)]
pub struct TaskClicked(pub String);

impl EventEmitter<TaskClicked> for KanbanScreen {}

/// Column width constant.
const COLUMN_MIN_W: f32 = 220.0;

/// Kanban Board screen.
pub struct KanbanScreen {
    tasks: Vec<KanbanTask>,
}

impl KanbanScreen {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        Self {
            tasks: vec![
                KanbanTask {
                    id: "task-01".into(),
                    title: "Add auth middleware".into(),
                    agent: Some("claude-code".into()),
                    complexity: "Standard".into(),
                    progress: Some((2, 5)),
                    column: KanbanColumn::Executing,
                },
                KanbanTask {
                    id: "task-02".into(),
                    title: "Refactor DB layer".into(),
                    agent: Some("claude-code".into()),
                    complexity: "Complex".into(),
                    progress: Some((0, 8)),
                    column: KanbanColumn::Planning,
                },
                KanbanTask {
                    id: "task-03".into(),
                    title: "Fix login bug".into(),
                    agent: None,
                    complexity: "Simple".into(),
                    progress: None,
                    column: KanbanColumn::Draft,
                },
                KanbanTask {
                    id: "task-04".into(),
                    title: "Update CI pipeline".into(),
                    agent: Some("claude-code".into()),
                    complexity: "Simple".into(),
                    progress: Some((3, 3)),
                    column: KanbanColumn::QaReview,
                },
                KanbanTask {
                    id: "task-05".into(),
                    title: "Add rate limiting".into(),
                    agent: Some("claude-code".into()),
                    complexity: "Standard".into(),
                    progress: Some((4, 4)),
                    column: KanbanColumn::HumanReview,
                },
                KanbanTask {
                    id: "task-06".into(),
                    title: "Setup logging".into(),
                    agent: Some("claude-code".into()),
                    complexity: "Simple".into(),
                    progress: Some((2, 2)),
                    column: KanbanColumn::Done,
                },
                KanbanTask {
                    id: "task-07".into(),
                    title: "Add WebSocket support for real-time notifications".into(),
                    agent: Some("claude-code".into()),
                    complexity: "Complex".into(),
                    progress: Some((1, 6)),
                    column: KanbanColumn::Executing,
                },
                KanbanTask {
                    id: "task-08".into(),
                    title: "Database connection pooling".into(),
                    agent: None,
                    complexity: "Standard".into(),
                    progress: None,
                    column: KanbanColumn::Draft,
                },
            ],
        }
    }

    fn tasks_in_column(&self, col: KanbanColumn) -> Vec<&KanbanTask> {
        self.tasks.iter().filter(|t| t.column == col).collect()
    }

    fn render_task_card(&self, task: &KanbanTask, cx: &mut Context<Self>) -> Stateful<Div> {
        let id = task.id.clone();
        let complexity_color = match task.complexity.as_str() {
            "Simple" => theme::SUCCESS,
            "Standard" => theme::WARNING,
            "Complex" => theme::ERROR,
            _ => theme::TEXT_MUTED,
        };

        let has_progress = task.progress.is_some();
        let progress_done = task.progress.map(|(d, _)| d).unwrap_or(0);
        let progress_total = task.progress.map(|(_, t)| t).unwrap_or(0);
        let pct = if progress_total > 0 {
            progress_done as f32 / progress_total as f32
        } else {
            0.0
        };

        let progress_color = if pct >= 1.0 {
            theme::SUCCESS
        } else {
            theme::PRIMARY
        };

        div()
            .id(SharedString::from(format!("task-{}", task.id)))
            .w_full()
            .v_flex()
            .gap(px(8.0))
            .p_3()
            .rounded_lg()
            .bg(theme::BACKGROUND)
            .border_1()
            .border_color(theme::TEXT_MUTED.opacity(0.08))
            .cursor_pointer()
            .hover(|s: StyleRefinement| {
                s.border_color(theme::PRIMARY.opacity(0.4))
                    .shadow_sm()
            })
            .on_click(cx.listener(move |_this, _event, _window, cx| {
                cx.emit(TaskClicked(id.clone()));
            }))
            // Title
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme::TEXT_PRIMARY)
                    .line_height(relative(1.4))
                    .child(task.title.clone()),
            )
            // Progress bar (if has progress)
            .when(has_progress, |el: Stateful<Div>| {
                el.child(
                    div()
                        .w_full()
                        .v_flex()
                        .gap_1()
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
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme::TEXT_MUTED)
                                .child(format!("{progress_done}/{progress_total} subtasks")),
                        ),
                )
            })
            // Footer: badges
            .child(
                div()
                    .h_flex()
                    .gap(px(4.0))
                    .flex_wrap()
                    .when(task.agent.is_some(), |el: Div| {
                        let agent = task.agent.as_deref().unwrap_or("");
                        el.child(self.badge(agent, theme::PRIMARY))
                    })
                    .child(self.badge(&task.complexity, complexity_color)),
            )
    }

    fn badge(&self, text: &str, color: Hsla) -> Div {
        div()
            .text_xs()
            .px(px(6.0))
            .py(px(2.0))
            .rounded(px(4.0))
            .bg(color.opacity(0.12))
            .text_color(color)
            .child(text.to_string())
    }

    fn render_column(&self, col: KanbanColumn, cx: &mut Context<Self>) -> Div {
        let tasks = self.tasks_in_column(col);
        let count = tasks.len();

        let cards: Vec<Stateful<Div>> = tasks
            .iter()
            .map(|t| self.render_task_card(t, cx))
            .collect();

        div()
            .v_flex()
            .h_full()
            .w(px(COLUMN_MIN_W))
            .min_w(px(COLUMN_MIN_W))
            .flex_shrink_0()
            // Column header
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .items_center()
                    .px_2()
                    .pb_2()
                    .child(
                        div()
                            .w(px(8.0))
                            .h(px(8.0))
                            .rounded_full()
                            .bg(col.color()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme::TEXT_PRIMARY)
                            .child(col.label().to_string()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .px(px(6.0))
                            .py(px(1.0))
                            .rounded_full()
                            .bg(theme::TEXT_MUTED.opacity(0.12))
                            .text_color(theme::TEXT_MUTED)
                            .child(format!("{count}")),
                    ),
            )
            // Cards area
            .child(
                div()
                    .v_flex()
                    .gap_2()
                    .flex_1()
                    .p(px(6.0))
                    .rounded_lg()
                    .bg(theme::SURFACE.opacity(0.25))
                    .border_1()
                    .border_color(theme::TEXT_MUTED.opacity(0.05))
                    .when(cards.is_empty(), |el: Div| {
                        el.child(
                            div()
                                .py_8()
                                .flex()
                                .justify_center()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(theme::TEXT_MUTED.opacity(0.4))
                                        .child("No tasks".to_string()),
                                ),
                        )
                    })
                    .children(cards),
            )
    }
}

impl Render for KanbanScreen {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let columns: Vec<Div> = KanbanColumn::all()
            .iter()
            .map(|&col| self.render_column(col, cx))
            .collect();

        div()
            .size_full()
            .v_flex()
            .gap_3()
            .p_4()
            .pt_5()
            // Header row
            .child(
                div()
                    .h_flex()
                    .justify_between()
                    .items_center()
                    .px_2()
                    .child(
                        div()
                            .h_flex()
                            .gap_3()
                            .items_center()
                            .child(
                                div()
                                    .text_xl()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(theme::TEXT_PRIMARY)
                                    .child("Kanban Board".to_string()),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme::TEXT_MUTED)
                                    .child(format!("{} tasks", self.tasks.len())),
                            ),
                    )
                    .child(
                        Button::new("kanban-new-task")
                            .primary()
                            .compact()
                            .label("+ New Task"),
                    ),
            )
            // Columns container — horizontal scroll when columns overflow
            .child(
                div()
                    .id("kanban-columns")
                    .flex_1()
                    .h_flex()
                    .gap_3()
                    .items_start()
                    .overflow_x_scroll()
                    .children(columns),
            )
    }
}
