//! Tasks home: project work, decisions, and result inspection.
//!
//! Displays daemon run summaries only. An empty run list is an onboarding
//! state, never a source of sample activity or invented review requests.

use gpui_kit::component::button::ButtonVariants as _;
use gpui_kit::component::{Disableable, StyledExt};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use surge_orchestrator::engine::handle::RunStatus;

use gpui_kit::component::input::{Input, InputEvent, InputState};

use crate::app_state::AppState;
use crate::theme;
use crate::ui;

/// What the operator can trigger from the Fleet surface.
#[derive(Clone)]
pub enum FleetAction {
    /// Open a run's cockpit.
    OpenRun(Option<surge_core::id::RunId>),
    /// Operator described new work in the command bar — dispatch a
    /// bootstrap run with this prompt.
    Dispatch(surge_core::RunId, String),
    /// Open the review gate for a run awaiting the operator.
    OpenGate(String),
    NewTask,
    OpenResult(surge_core::RunId, super::runs::RunTab),
}

impl EventEmitter<FleetAction> for FleetScreen {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TaskFilter {
    All,
    NeedsDecision,
    Running,
    Finished,
    Stopped,
}

impl TaskFilter {
    const ALL: [Self; 5] = [
        Self::All,
        Self::NeedsDecision,
        Self::Running,
        Self::Finished,
        Self::Stopped,
    ];
    fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::NeedsDecision => "Needs decision",
            Self::Running => "Running",
            Self::Finished => "Finished",
            Self::Stopped => "Stopped",
        }
    }
    fn id(self) -> &'static str {
        match self {
            Self::All => "filter-all",
            Self::NeedsDecision => "filter-decisions",
            Self::Running => "filter-running",
            Self::Finished => "filter-finished",
            Self::Stopped => "filter-stopped",
        }
    }
    fn includes(self, task: &WorkTask) -> bool {
        match self {
            Self::All => true,
            Self::NeedsDecision => task.needs_decision,
            Self::Running => matches!(task.run.status, RunStatus::Active | RunStatus::Awaiting),
            Self::Finished => task.run.status == RunStatus::Completed,
            Self::Stopped => matches!(task.run.status, RunStatus::Failed | RunStatus::Aborted),
        }
    }
}

#[derive(Clone)]
struct WorkTask {
    run: crate::app_state::UiRun,
    request: Option<String>,
    needs_decision: bool,
}

impl WorkTask {
    fn title(&self) -> String {
        self.request
            .as_deref()
            .and_then(|request| request.lines().find(|line| !line.trim().is_empty()))
            .map_or_else(
                || format!("Task {}", self.run.run_id.short()),
                |line| line.trim().to_string(),
            )
    }
    fn status(&self) -> &'static str {
        if self.needs_decision {
            return "Needs decision";
        }
        match self.run.status {
            RunStatus::Completed => "Finished",
            RunStatus::Failed => "Failed",
            RunStatus::Aborted => "Stopped",
            RunStatus::Active => "Running",
            _ => "Queued",
        }
    }
    fn color(&self) -> Hsla {
        if self.needs_decision {
            return theme::warning();
        }
        match self.run.status {
            RunStatus::Completed => theme::success(),
            RunStatus::Failed | RunStatus::Aborted => theme::error(),
            RunStatus::Active => theme::accent(),
            _ => theme::text_muted(),
        }
    }
}

/// Project task list and request composer, backed by daemon state.
pub struct FleetScreen {
    state: Entity<AppState>,
    filter: TaskFilter,
    selected: Option<surge_core::RunId>,
    /// Command-bar input (created lazily; needs a Window).
    command_input: Option<Entity<InputState>>,
    pending: Option<(surge_core::RunId, String)>,
    submission_error: Option<String>,
    clear_accepted: Option<String>,
}

impl FleetScreen {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        // Re-render when the run list / daemon link changes.
        cx.observe(&state, |_this, _state, cx| cx.notify()).detach();
        Self {
            state,
            filter: TaskFilter::All,
            selected: None,
            command_input: None,
            pending: None,
            submission_error: None,
            clear_accepted: None,
        }
    }

    /// Preserve the exact draft until its operation is acknowledged.
    fn submit_command(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(input) = self.command_input.clone() else {
            return;
        };
        let prompt = input.read(cx).value().to_string();
        if self.pending.is_some() || self.clear_accepted.is_some() || prompt.trim().is_empty() {
            return;
        }
        let operation_id = surge_core::RunId::new();
        self.pending = Some((operation_id, prompt.clone()));
        self.submission_error = None;
        cx.emit(FleetAction::Dispatch(operation_id, prompt));
        cx.notify();
    }

    #[cfg(test)]
    pub(crate) fn draft(&self, cx: &App) -> String {
        self.command_input
            .as_ref()
            .map_or_else(String::new, |input| input.read(cx).value().to_string())
    }

    #[cfg(test)]
    pub(crate) fn error(&self) -> Option<&str> {
        self.submission_error.as_deref()
    }

    pub fn finish_submission(
        &mut self,
        operation_id: surge_core::RunId,
        result: Result<(), String>,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self
            .pending
            .as_ref()
            .is_some_and(|(id, _)| *id == operation_id)
        {
            return false;
        }
        let Some((_, prompt)) = self.pending.take() else {
            return false;
        };
        let unchanged = self
            .command_input
            .as_ref()
            .is_some_and(|input| input.read(cx).value().as_ref() == prompt);
        match result {
            Ok(()) if unchanged => self.clear_accepted = Some(prompt),
            Ok(()) => {},
            Err(error) => self.submission_error = Some(error),
        }
        cx.notify();
        unchanged
    }

    fn tasks(&self, cx: &App) -> Vec<WorkTask> {
        let state = self.state.read(cx);
        let decisions: std::collections::HashSet<_> = state
            .pending_decisions()
            .into_iter()
            .map(|(id, _)| id)
            .chain(
                state
                    .unacknowledged_failures()
                    .into_iter()
                    .map(|run| run.run_id),
            )
            .collect();
        state
            .project_runs()
            .into_iter()
            .map(|run| WorkTask {
                run: run.clone(),
                request: state.run_prompt(&run.run_id).map(str::to_owned),
                needs_decision: decisions.contains(&run.run_id),
            })
            .collect()
    }

    fn render_filters(&self, tasks: &[WorkTask], cx: &mut Context<Self>) -> Div {
        div()
            .h_flex()
            .flex_wrap()
            .gap(px(6.0))
            .children(TaskFilter::ALL.into_iter().map(|filter| {
                let count = tasks.iter().filter(|task| filter.includes(task)).count();
                gpui_kit::component::button::Button::new(filter.id())
                    .ghost()
                    .label(format!("{}  {count}", filter.label()))
                    .accessibility_id(filter.id())
                    .debug_selector(move || filter.id().into())
                    .when(self.filter == filter, |button| {
                        button.bg(theme::panel_raised()).text_color(theme::accent())
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.filter = filter;
                        this.selected = None;
                        cx.notify();
                    }))
            }))
    }

    fn render_task_row(&self, task: &WorkTask, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let run_id = task.run.run_id;
        let selected = self.selected == Some(run_id);
        let selector = format!("task-{run_id}");
        div()
            .id(SharedString::from(format!("task-row-{run_id}")))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| {
                this.selected = Some(run_id);
                cx.notify();
            }))
            .v_flex()
            .gap(px(8.0))
            .p(px(16.0))
            .border_b_1()
            .border_color(theme::hairline())
            .when(selected, |row| {
                row.bg(theme::panel_raised())
                    .border_l_2()
                    .border_color(theme::accent())
            })
            .child(
                div()
                    .h_flex()
                    .items_center()
                    .gap(px(12.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .truncate()
                            .text_size(px(15.0))
                            .font_weight(FontWeight::MEDIUM)
                            .child(task.title()),
                    )
                    .child(
                        gpui_kit::component::button::Button::new(SharedString::from(
                            selector.clone(),
                        ))
                        .ghost()
                        .label("View task")
                        .accessibility_id(selector.clone())
                        .debug_selector(|| "fleet-task-row".into())
                        .text_size(px(15.0))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.selected = Some(run_id);
                            cx.notify();
                        })),
                    ),
            )
            .child(
                div()
                    .h_flex()
                    .gap(px(12.0))
                    .child(ui::pill(task.status(), task.color(), theme::panel()))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(theme::text_muted())
                            .child(
                                task.run
                                    .started_at
                                    .with_timezone(&chrono::Local)
                                    .format("%b %-d, %H:%M")
                                    .to_string(),
                            ),
                    ),
            )
    }

    fn render_detail(&self, task: &WorkTask, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let run_id = task.run.run_id;
        div()
            .id("task-detail")
            .test_support()
            .debug_selector(|| "task-detail".into())
            .v_flex()
            .gap(px(20.0))
            .w(px(340.0))
            .min_h(px(0.0))
            .flex_shrink_0()
            .p(px(24.0))
            .border_l_1()
            .border_color(theme::hairline())
            .overflow_y_scroll()
            .child(
                div()
                    .text_size(px(22.0))
                    .line_height(px(30.0))
                    .flex_shrink_0()
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme::text_primary())
                    .child(task.title()),
            )
            .child(ui::pill(task.status(), task.color(), theme::panel_raised()))
            .when(task.needs_decision, |detail| {
                detail.child(
                    gpui_kit::component::button::Button::new("fleet-inspector-primary")
                        .primary()
                        .label("Review decision")
                        .accessibility_id("fleet-inspector-primary")
                        .on_click(cx.listener(move |_, _, _, cx| {
                            cx.emit(FleetAction::OpenGate(run_id.to_string()))
                        })),
                )
            })
            .child(
                gpui_kit::component::button::Button::new("task-open")
                    .primary()
                    .label(if task.run.status == RunStatus::Completed {
                        "Open result"
                    } else {
                        "Open task"
                    })
                    .accessibility_id("task-open")
                    .debug_selector(|| "task-open".into())
                    .on_click(
                        cx.listener(move |_, _, _, cx| cx.emit(FleetAction::OpenRun(Some(run_id)))),
                    ),
            )
            .when(task.run.status == RunStatus::Completed, |detail| {
                detail.child(
                    div().v_flex().gap(px(8.0)).children(
                        [
                            (
                                "task-review-changes",
                                "Review changes",
                                super::runs::RunTab::Changes,
                            ),
                            (
                                "task-review-checks",
                                "Review checks",
                                super::runs::RunTab::Checks,
                            ),
                        ]
                        .into_iter()
                        .map(|(id, label, tab)| {
                            gpui_kit::component::button::Button::new(id)
                                .label(label)
                                .accessibility_id(id)
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    cx.emit(FleetAction::OpenResult(run_id, tab))
                                }))
                        }),
                    ),
                )
            })
            .child(
                div()
                    .v_flex()
                    .gap(px(8.0))
                    .child(
                        div()
                            .text_size(px(14.0))
                            .font_weight(FontWeight::BOLD)
                            .child("Original request"),
                    )
                    .child(
                        div()
                            .p(px(14.0))
                            .rounded(px(8.0))
                            .bg(theme::panel())
                            .text_size(px(15.0))
                            .text_color(theme::text_primary())
                            .child(task.request.clone().unwrap_or_else(|| {
                                "The original request has not arrived from the daemon yet.".into()
                            })),
                    ),
            )
            .child(ui::meta(format!(
                "Started {}",
                task.run
                    .started_at
                    .with_timezone(&chrono::Local)
                    .format("%b %-d, %Y at %H:%M")
            )))
            .when_some(task.run.ended_at, |detail, ended| {
                detail.child(ui::meta(format!(
                    "Ended {}",
                    ended
                        .with_timezone(&chrono::Local)
                        .format("%b %-d, %Y at %H:%M")
                )))
            })
            .when(task.run.status == RunStatus::Completed, |detail| {
                detail.child(ui::meta(
                    "The task finished. Open the result to inspect changes and reported checks.",
                ))
            })
    }

    fn render_command_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        if self.command_input.is_none() {
            let input = cx
                .new(|cx| InputState::new(window, cx).placeholder("Describe what to build next…"));
            cx.subscribe_in(
                &input,
                window,
                |this: &mut Self, _input, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::PressEnter { .. }) {
                        this.submit_command(window, cx);
                    }
                    cx.notify();
                },
            )
            .detach();
            self.command_input = Some(input);
        }

        if let Some(accepted) = self.clear_accepted.take()
            && let Some(input) = &self.command_input
            && input.read(cx).value().as_ref() == accepted
        {
            input.update(cx, |input, cx| input.set_value("", window, cx));
        }
        div()
            .v_flex()
            .when_some(self.submission_error.clone(), |el, error| {
                el.child(
                    div()
                        .id("fleet-submission-error")
                        .role(Role::Label)
                        .aria_label(error.clone())
                        .px(px(16.0))
                        .py(px(8.0))
                        .text_color(theme::error())
                        .child(error),
                )
            })
            .child(
                div()
                    .min_h(px(60.0))
                    .flex_shrink_0()
                    .h_flex()
                    .gap(px(12.0))
                    .items_center()
                    .px(px(16.0))
                    .bg(theme::panel())
                    .border_t_1()
                    .border_color(theme::hairline())
                    .child(
                        div()
                            .flex_1()
                            .h_flex()
                            .gap(px(10.0))
                            .items_center()
                            .h(px(38.0))
                            .px(px(12.0))
                            .rounded(px(ui::R_CONTROL + 2.0))
                            .bg(theme::panel_deep())
                            .border_1()
                            .border_color(theme::hairline_strong())
                            .child(ui::role_badge("new task", theme::Semantic::Agent))
                            .child(
                                div()
                                    .id("fleet-prompt-region")
                                    .test_support()
                                    .debug_selector(|| "fleet-prompt-region".into())
                                    .flex_1()
                                    .children(self.command_input.as_ref().map(|input| {
                                        Input::new(input)
                                            .accessibility_id("fleet-task")
                                            .aria_label("Describe a task")
                                            .appearance(false)
                                    })),
                            )
                            .child(ui::kbd("↵")),
                    )
                    .child(
                        gpui_kit::component::button::Button::new("fleet-start")
                            .primary()
                            .label(if self.pending.is_some() {
                                "Starting…"
                            } else {
                                "Start"
                            })
                            .accessibility_id("fleet-start")
                            .debug_selector(|| "fleet-start".into())
                            .disabled(
                                self.pending.is_some()
                                    || self.command_input.as_ref().is_none_or(|input| {
                                        input.read(cx).value().trim().is_empty()
                                    }),
                            )
                            .on_click(
                                cx.listener(|this, _, window, cx| this.submit_command(window, cx)),
                            ),
                    ),
            )
    }
}

impl Render for FleetScreen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tasks = self.tasks(cx);
        let visible: Vec<_> = tasks
            .iter()
            .filter(|task| self.filter.includes(task))
            .collect();
        if !visible
            .iter()
            .any(|task| Some(task.run.run_id) == self.selected)
        {
            self.selected = visible.first().map(|task| task.run.run_id);
        }
        let selected = visible
            .iter()
            .find(|task| Some(task.run.run_id) == self.selected)
            .copied();
        let live = self.state.read(cx).daemon_state.facade().is_some();
        let project_error = self.state.read(cx).project_load_error.clone();
        div().v_flex().size_full().bg(theme::surface()).text_color(theme::text_primary())
            .child(div().h_flex().items_center().flex_shrink_0().gap(px(16.0)).p(px(24.0))
                .child(div().v_flex().gap(px(6.0)).flex_1()
                    .child(div().text_size(px(26.0)).font_weight(FontWeight::BOLD).child("Your work"))
                    .child(ui::meta("Follow task progress, review results, and make decisions.")))
                .child(gpui_kit::component::button::Button::new("fleet-new-task").primary().label("New task")
                    .accessibility_id("fleet-new-task").on_click(cx.listener(|_, _, _, cx| cx.emit(FleetAction::NewTask)))))
            .when(!live, |screen| screen.child(div().px(px(24.0)).py(px(8.0)).text_size(px(13.0)).text_color(theme::text_muted())
                .child("Daemon offline · showing last known tasks. Start the daemon from the sidebar to begin work.")))
            .when_some(project_error, |screen, error| screen.child(div().px(px(24.0)).py(px(8.0)).text_color(theme::error()).child(error)))
            .child(div().h_flex().items_stretch().flex_1().min_h(px(0.0))
                .child(div().v_flex().flex_1().min_h(px(0.0)).min_w(px(0.0))
                    .child(div().px(px(20.0)).pb(px(16.0)).child(self.render_filters(&tasks, cx)))
                    .child(div().id("task-list").flex_1().min_h(px(0.0)).overflow_y_scroll()
                        .when(visible.is_empty(), |list| list.child(div().id("fleet-empty").test_support().aria_label("Create your first application")
                            .v_flex().p(px(32.0)).gap(px(12.0))
                            .child(div().text_size(px(20.0)).font_weight(FontWeight::BOLD).child(if tasks.is_empty() { "Create your first application" } else { "No tasks in this view" }))
                            .child(ui::meta(if tasks.is_empty() { "Describe what you want to build below, then select Start." } else { "Choose another filter to see more tasks." }))))
                        .children(visible.iter().map(|task| self.render_task_row(task, cx)))))
                .children(selected.map(|task| self.render_detail(task, cx))))
            .child(self.render_command_bar(window, cx))
    }
}

#[cfg(test)]
mod accessibility_tests {
    use super::{FleetAction, FleetScreen};
    use crate::app_state::AppState;
    use gpui_kit::test::TestWindowExt;
    use gpui_kit::{AppContext, SharedString, TestAppContext, WindowOptions};
    use std::{cell::Cell, rc::Rc};
    use surge_orchestrator::engine::handle::RunStatus;

    #[gpui_kit::test]
    fn empty_fleet_contains_no_invented_runs(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (handle, view) = cx.update(|cx| {
            let state = cx.new(|_| AppState::new());
            gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| {
                cx.new(|cx| FleetScreen::new(state, cx))
            })
            .unwrap()
        });
        view.update(cx, |view, cx| {
            assert!(
                view.tasks(cx).is_empty(),
                "empty daemon data must stay empty"
            );
        });
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(
                window.find("fleet-empty").label(),
                Some("Create your first application")
            );
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn fleet_submission_preserves_exact_draft_and_guards_duplicates(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let state = cx.new(|_| AppState::new());
        let view = cx.new(|cx| FleetScreen::new(state, cx));
        let (_, window) = cx
            .add_window_view(|window, cx| gpui_kit::component::Root::new(view.clone(), window, cx));
        let submissions = Rc::new(std::cell::RefCell::new(Vec::new()));
        let copy = submissions.clone();
        window.update(|_, cx| {
            cx.subscribe(&view, move |_, event: &FleetAction, _| {
                if let FleetAction::Dispatch(id, prompt) = event {
                    copy.borrow_mut().push((*id, prompt.clone()));
                }
            })
            .detach();
        });
        let prompt = "  Build a timer with controls.  ";
        let bounds = window.debug_bounds("fleet-prompt-region").unwrap();
        window.simulate_click(bounds.center(), gpui_kit::Modifiers::default());
        window.simulate_input(prompt);
        let start = window.debug_bounds("fleet-start").unwrap();
        window.simulate_click(start.center(), gpui_kit::Modifiers::default());
        window.simulate_click(start.center(), gpui_kit::Modifiers::default());
        assert_eq!(submissions.borrow().len(), 1);
        let id = submissions.borrow()[0].0;
        assert_eq!(submissions.borrow()[0].1, prompt);
        window.update(|_, cx| {
            view.update(cx, |fleet, cx| {
                assert_eq!(
                    fleet
                        .command_input
                        .as_ref()
                        .unwrap()
                        .read(cx)
                        .value()
                        .as_ref(),
                    prompt
                );
                assert!(!fleet.finish_submission(surge_core::RunId::new(), Ok(()), cx));
                assert!(fleet.finish_submission(
                    id,
                    Err("Daemon offline. Start it, then retry.".into()),
                    cx
                ));
                assert_eq!(
                    fleet
                        .command_input
                        .as_ref()
                        .unwrap()
                        .read(cx)
                        .value()
                        .as_ref(),
                    prompt
                );
                assert!(fleet.submission_error.as_ref().unwrap().contains("retry"));
            })
        });
        window.simulate_click(bounds.center(), gpui_kit::Modifiers::default());
        window.simulate_keystrokes("enter enter");
        assert_eq!(submissions.borrow().len(), 2);
        let retry_id = submissions.borrow()[1].0;
        window.update(|window, cx| {
            view.update(cx, |fleet, cx| {
                fleet
                    .command_input
                    .as_ref()
                    .unwrap()
                    .update(cx, |input, cx| input.set_value("newer draft", window, cx));
                assert!(!fleet.finish_submission(retry_id, Ok(()), cx));
                let _ = fleet.render_command_bar(window, cx);
                assert_eq!(
                    fleet
                        .command_input
                        .as_ref()
                        .unwrap()
                        .read(cx)
                        .value()
                        .as_ref(),
                    "newer draft"
                );
            })
        });
    }

    #[gpui_kit::test]
    fn full_task_list_filters_and_selection_include_older_decisions(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let stopped_id = surge_core::RunId::new();
        let completed_id = surge_core::RunId::new();
        let second_active_id = surge_core::RunId::new();
        let state = cx.new(|_| {
            let mut state = AppState::new();
            for index in 0..9 {
                let (run_id, status) = match index {
                    0 => (stopped_id, RunStatus::Failed),
                    1 => (completed_id, RunStatus::Completed),
                    7 => (second_active_id, RunStatus::Active),
                    _ => (surge_core::RunId::new(), RunStatus::Active),
                };
                state.runs.push(crate::app_state::UiRun {
                    run_id,
                    status,
                    started_at: chrono::Utc::now() + chrono::Duration::seconds(index),
                    last_event_seq: None,
                    ended_at: None,
                });
                state.run_streams.entry(run_id).or_default().prompt =
                    Some(format!("Task {index}\nFull original request for {index}"));
            }
            state
        });
        let view = cx.new(|cx| FleetScreen::new(state, cx));
        let (_, window) = cx
            .add_window_view(|window, cx| gpui_kit::component::Root::new(view.clone(), window, cx));
        window.update(|_, cx| assert_eq!(view.read(cx).tasks(cx).len(), 9));
        window.update(|window, cx| {
            window.render_frame(cx);
            window.click(SharedString::from(format!("task-{second_active_id}")), cx);
            assert_eq!(view.read(cx).selected, Some(second_active_id));
        });
        let decisions = window.debug_bounds("filter-decisions").unwrap();
        window.simulate_click(decisions.center(), gpui_kit::Modifiers::default());
        window.update(|_, cx| {
            assert_eq!(view.read(cx).filter, super::TaskFilter::NeedsDecision);
            assert_eq!(view.read(cx).selected, Some(stopped_id));
        });
        assert!(window.debug_bounds("fleet-task-row").is_some());
        let finished = window.debug_bounds("filter-finished").unwrap();
        window.simulate_click(finished.center(), gpui_kit::Modifiers::default());
        let completed = window.debug_bounds("fleet-task-row").unwrap();
        window.simulate_click(completed.center(), gpui_kit::Modifiers::default());
        window.update(|_, cx| {
            assert_eq!(view.read(cx).selected, Some(completed_id));
            assert_eq!(
                view.read(cx)
                    .tasks(cx)
                    .iter()
                    .find(|task| task.run.run_id == completed_id)
                    .unwrap()
                    .request
                    .as_deref(),
                Some("Task 1\nFull original request for 1")
            );
        });
        assert!(window.debug_bounds("task-detail").is_some());
        for (selector, filter, expected) in [
            ("filter-running", super::TaskFilter::Running, 7),
            ("filter-stopped", super::TaskFilter::Stopped, 1),
            ("filter-all", super::TaskFilter::All, 9),
        ] {
            let filter_button = window.debug_bounds(selector).unwrap();
            window.simulate_click(filter_button.center(), gpui_kit::Modifiers::default());
            window.update(|_, cx| {
                let fleet = view.read(cx);
                assert_eq!(fleet.filter, filter);
                assert_eq!(
                    fleet
                        .tasks(cx)
                        .iter()
                        .filter(|task| filter.includes(task))
                        .count(),
                    expected
                );
            });
        }
    }

    #[gpui_kit::test]
    fn acknowledgment_before_render_cannot_dispatch_the_same_draft_twice(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let view = cx.new(|cx| {
            let state = cx.new(|_| AppState::new());
            FleetScreen::new(state, cx)
        });
        let (_, window) = cx
            .add_window_view(|window, cx| gpui_kit::component::Root::new(view.clone(), window, cx));
        let count = Rc::new(Cell::new(0));
        let captured = count.clone();
        window.update(|window, cx| {
            cx.subscribe(&view, move |_, event: &FleetAction, _| {
                if matches!(event, FleetAction::Dispatch(_, _)) {
                    captured.set(captured.get() + 1);
                }
            })
            .detach();
            view.update(cx, |fleet, cx| {
                fleet
                    .command_input
                    .as_ref()
                    .unwrap()
                    .update(cx, |input, cx| input.set_value("exact request", window, cx));
                fleet.submit_command(window, cx);
                let id = fleet.pending.as_ref().unwrap().0;
                assert!(fleet.finish_submission(id, Ok(()), cx));
                fleet.submit_command(window, cx);
                assert!(fleet.pending.is_none());
                let _ = fleet.render_command_bar(window, cx);
                assert_eq!(fleet.draft(cx), "");
            });
        });
        assert_eq!(count.get(), 1);
    }

    #[gpui_kit::test]
    fn inspector_names_the_inbox_action_it_dispatches(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (handle, view) = cx.update(|cx| {
            let state = cx.new(|_| {
                let mut state = AppState::new();
                state.runs.push(crate::app_state::UiRun {
                    run_id: surge_core::id::RunId::new(),
                    status: surge_orchestrator::engine::handle::RunStatus::Failed,
                    started_at: chrono::Utc::now(),
                    last_event_seq: None,
                    ended_at: None,
                });
                state
            });
            gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| {
                cx.new(|cx| FleetScreen::new(state, cx))
            })
            .unwrap()
        });
        let inbox_actions = Rc::new(Cell::new(0));
        let captured = inbox_actions.clone();
        cx.update(|cx| {
            cx.subscribe(&view, move |_, event: &FleetAction, _| {
                assert!(matches!(event, FleetAction::OpenGate(_)));
                captured.set(captured.get() + 1);
            })
            .detach();
        });
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(
                window.find("fleet-inspector-primary").label(),
                Some("Review decision")
            );
            window.click("fleet-inspector-primary", cx);
        })
        .unwrap();
        cx.update(|_| assert_eq!(inbox_actions.get(), 1));
    }
}
