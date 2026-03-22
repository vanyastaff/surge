use gpui::*;
use gpui::prelude::FluentBuilder;
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::StyledExt;

use crate::theme;

/// Kanban column state — maps to simplified TaskState groups.
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
            Self::HumanReview => "Human Review",
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
    pub progress: Option<(usize, usize)>, // (completed, total)
    pub column: KanbanColumn,
}

/// Event emitted when a task card is clicked.
#[derive(Clone, PartialEq)]
pub struct TaskClicked(pub String);

impl EventEmitter<TaskClicked> for KanbanScreen {}

/// Kanban Board screen.
pub struct KanbanScreen {
    tasks: Vec<KanbanTask>,
}

impl KanbanScreen {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        // Demo data.
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

        let mut card = div()
            .id(SharedString::from(format!("task-{}", task.id)))
            .v_flex()
            .gap(px(6.0))
            .p(px(10.0))
            .w_full()
            .rounded_lg()
            .bg(theme::BACKGROUND)
            .border_1()
            .border_color(theme::TEXT_MUTED.opacity(0.08))
            .cursor_pointer()
            .hover(|s: StyleRefinement| {
                s.border_color(theme::PRIMARY.opacity(0.3))
                    .bg(theme::BACKGROUND)
            })
            .on_click(cx.listener(move |_this, _event, _window, cx| {
                cx.emit(TaskClicked(id.clone()));
            }))
            // Title
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme::TEXT_PRIMARY)
                    .child(task.title.clone()),
            );

        // Progress bar
        if let Some((done, total)) = task.progress {
            let pct = if total > 0 { (done as f32 / total as f32) * 100.0 } else { 0.0 };
            card = card.child(
                div()
                    .v_flex()
                    .gap_0p5()
                    .child(
                        div()
                            .w_full()
                            .h(px(3.0))
                            .rounded_full()
                            .bg(theme::TEXT_MUTED.opacity(0.1))
                            .child(
                                div()
                                    .h_full()
                                    .rounded_full()
                                    .bg(theme::PRIMARY)
                                    .w(relative(pct / 100.0)),
                            ),
                    )
                    .child(
                        div()
                            .text_color(theme::TEXT_MUTED)
                            .child(SharedString::from(format!("{done}/{total}")))
                    ),
            );
        }

        // Footer: agent + complexity badges
        card = card.child(
            div()
                .h_flex()
                .gap_1()
                .when(task.agent.is_some(), |el: Div| {
                    let agent = task.agent.as_deref().unwrap_or("");
                    el.child(
                        div()
                            .text_xs()
                            .px(px(6.0))
                            .py_0p5()
                            .rounded_md()
                            .bg(theme::PRIMARY.opacity(0.1))
                            .text_color(theme::PRIMARY)
                            .child(agent.to_string()),
                    )
                })
                .child(
                    div()
                        .text_xs()
                        .px(px(6.0))
                        .py_0p5()
                        .rounded_md()
                        .bg(complexity_color.opacity(0.1))
                        .text_color(complexity_color)
                        .child(task.complexity.clone()),
                ),
        );

        card
    }

    fn render_column(&self, col: KanbanColumn, cx: &mut Context<Self>) -> Div {
        let tasks = self.tasks_in_column(col);
        let count = tasks.len();

        let cards: Vec<Stateful<Div>> = tasks
            .iter()
            .map(|t| self.render_task_card(t, cx))
            .collect();

        div()
            .flex_1()
            .v_flex()
            .h_full()
            .min_w(px(180.0))
            .gap_0()
            // Column header — sticky top
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .items_center()
                    .px_2()
                    .pb_3()
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
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::TEXT_PRIMARY)
                            .child(col.label().to_string()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .px(px(6.0))
                            .py_0p5()
                            .rounded_full()
                            .bg(theme::TEXT_MUTED.opacity(0.12))
                            .text_color(theme::TEXT_MUTED)
                            .child(format!("{count}")),
                    ),
            )
            // Cards area — fills remaining height
            .child(
                div()
                    .v_flex()
                    .gap_2()
                    .flex_1()
                    .p_2()
                    .rounded_lg()
                    .bg(theme::SURFACE.opacity(0.3))
                    .border_1()
                    .border_color(theme::TEXT_MUTED.opacity(0.06))
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
            // Columns — stretch to fill all available height
            .child(
                div()
                    .flex_1()
                    .h_flex()
                    .gap_2()
                    .items_start()
                    .children(columns),
            )
    }
}
