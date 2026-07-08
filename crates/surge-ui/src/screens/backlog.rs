//! Backlog — the dispatch queue: Triage → Ready → In flight → Done.
//!
//! Not a classic kanban (parallel agents move too fast for hand-drag
//! stages — even kanban-first competitors concede the columns go
//! stale). Columns here are *derived* from the real [`TaskState`] FSM,
//! so a card can never disagree with the engine:
//!
//! - **Triage** — `Draft` (unscoped intake)
//! - **Ready** — `Planning` / `Planned` (scoped, waiting for capacity)
//! - **In flight** — `Executing`/`QaReview`/`QaFix`/`HumanReview`/`Merging`
//! - **Done** — terminal states, labelled truthfully
//!   (completed / failed / cancelled)
//!
//! The capacity strip compares installed agents (lanes) against active
//! daemon runs. "New task" opens the Spec wizard; created drafts land
//! here in Triage. A labelled sample board renders when the project
//! has no tasks yet.

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::StyledExt;
use surge_core::TaskState;
use surge_orchestrator::engine::handle::RunStatus;

use crate::app_state::AppState;
use crate::theme;
use crate::ui;

/// What the board can ask the app shell to do.
#[derive(Clone)]
pub enum BacklogAction {
    /// Open the task-detail overlay for a task id.
    OpenTask(String),
    /// Start the Spec wizard (New task).
    NewTask,
}

impl EventEmitter<BacklogAction> for BacklogScreen {}

#[derive(Clone, Copy, PartialEq)]
enum Column {
    Triage,
    Ready,
    InFlight,
    Done,
}

impl Column {
    fn title(self) -> &'static str {
        match self {
            Self::Triage => "Triage",
            Self::Ready => "Ready",
            Self::InFlight => "In flight",
            Self::Done => "Done",
        }
    }

    fn note(self) -> &'static str {
        match self {
            Self::Triage => "unscoped",
            Self::Ready => "scoped",
            Self::InFlight => "→ Runs",
            Self::Done => "terminal",
        }
    }

    fn dot(self) -> Hsla {
        match self {
            Self::Triage => theme::text_muted(),
            Self::Ready | Self::InFlight => theme::accent(),
            Self::Done => theme::success(),
        }
    }

    fn for_state(state: &TaskState) -> Self {
        match state {
            TaskState::Draft => Self::Triage,
            TaskState::Planning | TaskState::Planned { .. } => Self::Ready,
            TaskState::Executing { .. }
            | TaskState::QaReview { .. }
            | TaskState::QaFix { .. }
            | TaskState::HumanReview
            | TaskState::Merging => Self::InFlight,
            TaskState::Completed | TaskState::Failed { .. } | TaskState::Cancelled => Self::Done,
        }
    }
}

/// Board card view-model (from a real task or the sample set).
#[derive(Clone)]
struct Card {
    /// Real task id (None in sample mode).
    task_id: Option<String>,
    id_label: String,
    title: String,
    /// Status pill: label + color (state-specific truth).
    pill: (String, Hsla),
    /// Secondary meta line.
    meta: String,
    /// Executing progress, when the state carries it.
    progress: Option<(usize, usize)>,
    /// This card is blocked on the operator (review states).
    needs_you: bool,
    column: Column,
}

fn card_from_task(task: &crate::app_state::TaskEntry) -> Card {
    let column = Column::for_state(&task.state);
    let (pill, needs_you): ((String, Hsla), bool) = match &task.state {
        TaskState::Draft => (("draft".into(), theme::text_muted()), false),
        TaskState::Planning => (("planning".into(), theme::accent()), false),
        TaskState::Planned { subtask_count } => {
            ((format!("{subtask_count} subtasks"), theme::accent()), false)
        },
        TaskState::Executing { completed, total } => {
            ((format!("{completed}/{total}"), theme::accent()), false)
        },
        TaskState::QaReview { .. } => (("qa review".into(), theme::warning()), true),
        TaskState::QaFix { iteration, .. } => {
            ((format!("qa fix #{iteration}"), theme::warning()), false)
        },
        TaskState::HumanReview => (("needs you".into(), theme::accent()), true),
        TaskState::Merging => (("merging".into(), theme::accent()), false),
        TaskState::Completed => (("merged".into(), theme::success()), false),
        TaskState::Failed { .. } => (("failed".into(), theme::error()), false),
        TaskState::Cancelled => (("cancelled".into(), theme::text_muted()), false),
    };

    let progress = match &task.state {
        TaskState::Executing { completed, total } if *total > 0 => Some((*completed, *total)),
        _ => None,
    };

    let agent = task.agent.clone().unwrap_or_else(|| "unassigned".into());
    let meta = match &task.state {
        TaskState::Failed { reason } => reason.clone(),
        _ => format!("{} · {}", agent, task.complexity),
    };

    Card {
        task_id: Some(task.id.to_string()),
        id_label: format!("t-{}", task.id.short().to_lowercase()),
        title: task.title.clone(),
        pill,
        meta,
        progress,
        needs_you,
        column,
    }
}

/// Backlog screen — dispatch queue over the real task FSM.
pub struct BacklogScreen {
    state: Entity<AppState>,
}

impl BacklogScreen {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        cx.observe(&state, |_this, _state, cx| cx.notify()).detach();
        Self { state }
    }

    fn cards(&self, cx: &Context<Self>) -> (Vec<Card>, bool) {
        let state = self.state.read(cx);
        if state.tasks.is_empty() {
            return (sample_cards(), false);
        }
        (state.tasks.iter().map(card_from_task).collect(), true)
    }

    fn render_header(&self, total: usize, live: bool, cx: &mut Context<Self>) -> Div {
        div()
            .h_flex()
            .gap(px(12.0))
            .items_center()
            .px(px(20.0))
            .pt(px(15.0))
            .pb(px(12.0))
            .child(
                div()
                    .text_size(px(15.0))
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme::text_primary())
                    .child("Backlog"),
            )
            .child(ui::meta(format!("{total} tasks")))
            .when(!live, |el| {
                el.child(ui::pill(
                    "sample · create a task to start",
                    theme::text_muted(),
                    theme::panel_raised(),
                ))
            })
            .child(div().flex_1())
            .child(
                div()
                    .id("backlog-new-task")
                    .h_flex()
                    .gap(px(7.0))
                    .items_center()
                    .h(px(31.0))
                    .px(px(14.0))
                    .rounded_lg()
                    .bg(theme::accent())
                    .text_color(hsla(0.0, 0.0, 0.08, 1.0))
                    .text_size(px(11.0))
                    .font_weight(FontWeight::BOLD)
                    .cursor_pointer()
                    .hover(|s: StyleRefinement| s.bg(theme::accent().opacity(0.85)))
                    .on_click(cx.listener(|_this, _e, _w, cx| {
                        cx.emit(BacklogAction::NewTask);
                    }))
                    .child("+ New task"),
            )
    }

    /// Fleet capacity strip: real agents (lanes) vs active daemon runs.
    fn render_capacity(&self, cx: &Context<Self>) -> Div {
        let state = self.state.read(cx);
        let lanes = state.installed_agents.len();
        let active = state
            .runs
            .iter()
            .filter(|r| matches!(r.status, RunStatus::Active))
            .count();

        let text = if lanes == 0 {
            "no agents detected — install one to open a lane".to_string()
        } else {
            format!("{active} of {lanes} lanes busy — idle lanes pull riskiest ready work first")
        };

        let mut lane_dots = div().h_flex().gap(px(4.0));
        for i in 0..lanes.min(8) {
            let busy = i < active;
            lane_dots = lane_dots.child(
                div()
                    .w(px(14.0))
                    .h(px(6.0))
                    .rounded_sm()
                    .bg(if busy {
                        theme::accent()
                    } else {
                        theme::panel_raised()
                    })
                    .border_1()
                    .border_color(if busy {
                        theme::accent().opacity(0.6)
                    } else {
                        theme::hairline_strong()
                    }),
            );
        }

        div()
            .h_flex()
            .gap(px(12.0))
            .items_center()
            .mx(px(20.0))
            .mb(px(14.0))
            .px(px(14.0))
            .py(px(10.0))
            .rounded_lg()
            .bg(theme::accent().opacity(0.06))
            .border_1()
            .border_color(theme::accent().opacity(0.22))
            .child(
                div()
                    .text_size(px(11.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme::accent())
                    .child("Fleet capacity"),
            )
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(theme::text_muted())
                    .child(text),
            )
            .child(lane_dots)
            .child(div().flex_1())
            .child(
                div()
                    .text_size(px(10.0))
                    .text_color(theme::text_muted())
                    .child("riskiest-ready-first"),
            )
    }

    fn render_card(&self, card: &Card, cx: &mut Context<Self>) -> Stateful<Div> {
        let task_id = card.task_id.clone();
        let (pill_label, pill_color) = card.pill.clone();
        let is_done = card.column == Column::Done;

        let mut el = div()
            .id(SharedString::from(format!("bl-card-{}", card.id_label)))
            .v_flex()
            .gap(px(8.0))
            .p(px(12.0))
            .rounded_lg()
            .bg(theme::panel_raised())
            .border_1()
            .border_color(if card.needs_you {
                theme::accent().opacity(0.45)
            } else {
                theme::hairline()
            })
            .cursor_pointer()
            .hover(|s: StyleRefinement| s.border_color(theme::accent().opacity(0.5)))
            .when(is_done, |el| el.opacity(0.85))
            .on_click(cx.listener(move |_this, _e, _w, cx| {
                if let Some(id) = &task_id {
                    cx.emit(BacklogAction::OpenTask(id.clone()));
                }
            }))
            // header row: id + pill
            .child(
                div()
                    .h_flex()
                    .gap(px(7.0))
                    .items_center()
                    .child(
                        div()
                            .text_size(px(10.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::text_muted())
                            .child(card.id_label.clone()),
                    )
                    .child(div().flex_1())
                    .child(ui::pill(pill_label, pill_color, pill_color.opacity(0.13))),
            )
            // title
            .child(
                div()
                    .text_size(px(12.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .line_height(px(17.0))
                    .text_color(theme::text_primary())
                    .child(card.title.clone()),
            );

        if let Some((done, total)) = card.progress {
            let pct = done as f32 / total as f32;
            el = el.child(
                div()
                    .h(px(4.0))
                    .rounded_full()
                    .bg(theme::panel_deep())
                    .overflow_hidden()
                    .child(
                        div()
                            .h_full()
                            .rounded_full()
                            .bg(theme::accent())
                            .w(relative(pct)),
                    ),
            );
        }

        el.child(
            div()
                .text_size(px(9.5))
                .text_color(theme::text_muted())
                .child(card.meta.clone()),
        )
    }

    fn render_column(&self, column: Column, cards: &[Card], cx: &mut Context<Self>) -> Div {
        let in_col: Vec<&Card> = cards.iter().filter(|c| c.column == column).collect();
        let count = in_col.len();
        let needs = in_col.iter().filter(|c| c.needs_you).count();

        let mut list = div()
            .flex_1()
            .min_h_0()
            .id(SharedString::from(format!("bl-col-{}", column.title())))
            .overflow_y_scroll()
            .v_flex()
            .gap(px(9.0));

        if in_col.is_empty() {
            list = list.child(
                div()
                    .p(px(12.0))
                    .rounded_lg()
                    .border_1()
                    .border_color(theme::hairline())
                    .text_size(px(10.5))
                    .text_color(theme::text_muted().opacity(0.7))
                    .child("empty"),
            );
        }
        for card in &in_col {
            list = list.child(self.render_card(card, cx));
        }

        div()
            .flex_1()
            .min_w_0()
            .v_flex()
            .gap(px(10.0))
            .child(
                div()
                    .h_flex()
                    .gap(px(8.0))
                    .items_center()
                    .px(px(4.0))
                    .child(ui::status_dot(column.dot()))
                    .child(
                        div()
                            .text_size(px(11.0))
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme::text_primary())
                            .child(column.title()),
                    )
                    .child(ui::pill(
                        format!("{count}"),
                        theme::text_muted(),
                        theme::panel_raised(),
                    ))
                    .when(needs > 0, |el| {
                        el.child(ui::pill(
                            format!("{needs} need you"),
                            theme::accent(),
                            theme::accent().opacity(0.13),
                        ))
                    })
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_size(px(9.0))
                            .text_color(theme::text_muted().opacity(0.8))
                            .child(column.note()),
                    ),
            )
            .child(list)
    }
}

impl Render for BacklogScreen {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (cards, live) = self.cards(cx);

        div()
            .size_full()
            .v_flex()
            .bg(theme::panel_deep())
            .child(self.render_header(cards.len(), live, cx))
            .child(self.render_capacity(cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .h_flex()
                    .gap(px(14.0))
                    .px(px(20.0))
                    .pb(px(18.0))
                    .child(self.render_column(Column::Triage, &cards, cx))
                    .child(self.render_column(Column::Ready, &cards, cx))
                    .child(self.render_column(Column::InFlight, &cards, cx))
                    .child(self.render_column(Column::Done, &cards, cx)),
            )
    }
}

/// Clearly-labelled sample board (no tasks yet) — mirrors the concept
/// so the four-lane idea reads before a project has real work.
fn sample_cards() -> Vec<Card> {
    let mk = |id: &str,
              title: &str,
              pill: (&str, Hsla),
              meta: &str,
              needs_you: bool,
              column: Column| Card {
        task_id: None,
        id_label: id.to_string(),
        title: title.to_string(),
        pill: (pill.0.to_string(), pill.1),
        meta: meta.to_string(),
        progress: None,
        needs_you,
        column,
    };

    vec![
        mk(
            "t-201",
            "Webhook retry queue drains too slowly under load",
            ("draft", theme::text_muted()),
            "intake · unscoped",
            false,
            Column::Triage,
        ),
        mk(
            "t-198",
            "Add structured logging to payment worker",
            ("draft", theme::text_muted()),
            "intake · unscoped",
            false,
            Column::Triage,
        ),
        mk(
            "t-195",
            "Migrate sessions table to composite key",
            ("6 subtasks", theme::accent()),
            "planned · high risk",
            false,
            Column::Ready,
        ),
        mk(
            "t-194",
            "CSV import v2 — streaming parser",
            ("4 subtasks", theme::accent()),
            "planned · medium",
            false,
            Column::Ready,
        ),
        mk(
            "t-142",
            "Rate limiter middleware",
            ("needs you", theme::accent()),
            "claude-1 · review gate",
            true,
            Column::InFlight,
        ),
        mk(
            "t-140",
            "Retry logic patch",
            ("3/6", theme::accent()),
            "gpt-runner · executing",
            false,
            Column::InFlight,
        ),
        mk(
            "t-133",
            "Session cache",
            ("merged", theme::success()),
            "claude-1 · +214 −40",
            false,
            Column::Done,
        ),
        mk(
            "t-129",
            "Config loader refactor",
            ("failed", theme::error()),
            "qa suite 3/6 — needs re-triage",
            false,
            Column::Done,
        ),
    ]
}
