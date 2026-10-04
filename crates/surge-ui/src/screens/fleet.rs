//! Tasks home: project work, decisions, and result inspection.
//!
//! Displays daemon run summaries only. An empty run list is an onboarding
//! state, never a source of sample activity or invented review requests.

use gpui_kit::component::button::ButtonVariants as _;
use gpui_kit::component::{Disableable, StyledExt};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use surge_orchestrator::engine::handle::RunStatus;

use gpui_kit::component::input::{Input, InputEvent, InputState, TextareaState};

struct TaskDraft {
    comment: Entity<TextareaState>,
    requirements: Entity<TextareaState>,
    criteria: Entity<TextareaState>,
    flow: Entity<TextareaState>,
    version: u64,
    revision: u64,
    accepted_hash: surge_core::ContentHash,
}

struct NewTaskDraft {
    title: Entity<InputState>,
    requirements: Entity<TextareaState>,
    criteria: Entity<TextareaState>,
}

use crate::app_state::AppState;
use crate::theme;
use crate::ui;

#[path = "task_create.rs"]
mod durable_create;
#[path = "task_detail.rs"]
mod durable_tasks;
#[cfg(all(test, unix))]
#[path = "task_ui_tests.rs"]
mod task_ui_tests;

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
    selected_item: Option<surge_core::id::WorkItemId>,
    task_submissions:
        std::collections::HashMap<surge_core::id::WorkItemId, crate::work_items::TaskSubmission>,
    task_busy: bool,
    task_error: Option<String>,
    task_rejections: std::collections::HashSet<surge_core::id::WorkItemId>,
    task_refresh_identity: Option<(usize, Option<std::path::PathBuf>)>,
    task_drafts: std::collections::HashMap<surge_core::id::WorkItemId, TaskDraft>,
    task_feedback: std::collections::HashMap<surge_core::id::WorkItemId, String>,
    task_clear_comment: std::collections::HashMap<surge_core::id::WorkItemId, String>,
    session_pages: std::collections::HashMap<surge_core::RunId, usize>,
    new_task_draft: Option<NewTaskDraft>,
    creating_task: bool,
    creation_submission: Option<crate::work_items::TaskSubmission>,
    creation_project: Option<std::path::PathBuf>,
    creation_busy: bool,
    creation_error: Option<String>,
    task_last_render: Option<std::time::Instant>,
    expanded_task_editors: std::collections::HashSet<surge_core::id::WorkItemId>,
    expanded_task_metadata: std::collections::HashSet<surge_core::id::WorkItemId>,
}

impl FleetScreen {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        // Re-render when the run list / daemon link changes.
        cx.observe(&state, |this, _state, cx| {
            this.refresh_tasks_if_changed(cx);
            cx.notify();
        })
        .detach();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(5))
                    .await;
                let alive = this.update(cx, |this, cx| {
                    let visible = this
                        .task_last_render
                        .is_some_and(|time| time.elapsed() < std::time::Duration::from_secs(6));
                    if visible
                        && !this.task_busy
                        && !this.creation_busy
                        && !this.state.read(cx).tasks.loading
                    {
                        this.refresh_durable_tasks(cx);
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
        Self {
            state,
            filter: TaskFilter::All,
            selected: None,
            command_input: None,
            pending: None,
            submission_error: None,
            clear_accepted: None,
            selected_item: None,
            task_submissions: std::collections::HashMap::new(),
            task_busy: false,
            task_error: None,
            task_rejections: std::collections::HashSet::new(),
            task_refresh_identity: None,
            task_drafts: std::collections::HashMap::new(),
            task_feedback: std::collections::HashMap::new(),
            task_clear_comment: std::collections::HashMap::new(),
            session_pages: std::collections::HashMap::new(),
            new_task_draft: None,
            creating_task: false,
            creation_submission: None,
            creation_project: None,
            creation_busy: false,
            creation_error: None,
            task_last_render: None,
            expanded_task_editors: std::collections::HashSet::new(),
            expanded_task_metadata: std::collections::HashSet::new(),
        }
    }

    /// Open a new durable task while retaining any existing input drafts.
    pub fn start_new_task(&mut self, cx: &mut Context<Self>) {
        self.selected = None;
        self.selected_item = None;
        self.creating_task = true;
        cx.notify();
    }

    /// Open a saved task from the project sidebar.
    pub fn open_saved_task(&mut self, item: surge_core::id::WorkItemId, cx: &mut Context<Self>) {
        self.filter = TaskFilter::All;
        self.selected = None;
        self.creating_task = false;
        self.select_item(item, cx);
    }

    /// Open recorded run history from the project sidebar.
    pub fn open_run_task(&mut self, run: surge_core::RunId, cx: &mut Context<Self>) {
        self.filter = TaskFilter::All;
        self.select_run(run, cx);
    }

    fn select_run(&mut self, run: surge_core::RunId, cx: &mut Context<Self>) {
        self.selected = Some(run);
        self.selected_item = None;
        self.creating_task = false;
        cx.notify();
    }

    /// Return to the project list while retaining all unsent drafts.
    pub fn show_task_list(&mut self, cx: &mut Context<Self>) {
        self.selected = None;
        self.selected_item = None;
        self.creating_task = false;
        cx.notify();
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
        let task_runs: std::collections::HashSet<_> =
            state
                .tasks
                .records
                .iter()
                .filter_map(|item| item.active_run)
                .chain(
                    state.tasks.histories.values().flat_map(|history| {
                        history.attempts.entries.iter().map(|attempt| attempt.run)
                    }),
                )
                .chain(state.run_streams.iter().filter_map(|(run, stream)| {
                    let item = stream.trusted_work_item()?;
                    state
                        .tasks
                        .records
                        .iter()
                        .any(|record| {
                            record.id == item
                                && state.run_in_project(run)
                                && stream.git_common_dir.as_ref().is_none_or(|repository| {
                                    repository == &record.workspace.repository
                                })
                        })
                        .then_some(*run)
                }))
                .collect();
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
            .filter(|run| !task_runs.contains(&run.run_id))
            .map(|run| WorkTask {
                run: run.clone(),
                request: state.run_prompt(&run.run_id).map(str::to_owned),
                needs_decision: decisions.contains(&run.run_id),
            })
            .collect()
    }

    fn includes_durable(
        &self,
        filter: TaskFilter,
        item: &surge_core::work_item::WorkItemRecord,
        cx: &Context<Self>,
    ) -> bool {
        let status = self.durable_status(item, cx);
        match filter {
            TaskFilter::All => true,
            TaskFilter::NeedsDecision => {
                status == "Attention"
                    || item.active_run.is_some_and(|run| {
                        self.state
                            .read(cx)
                            .pending_decisions()
                            .iter()
                            .any(|(id, _)| *id == run)
                    })
            },
            TaskFilter::Running => matches!(
                status,
                "Running"
                    | "Suspending"
                    | "Continuing"
                    | "Waiting for capacity"
                    | "Waiting for human input"
            ),
            TaskFilter::Finished => status == "Completed",
            TaskFilter::Stopped => matches!(
                status,
                "Failed" | "Stopped" | "Aborted" | "Rejected" | "Suspended"
            ),
        }
    }

    fn render_filters(&self, tasks: &[WorkTask], cx: &mut Context<Self>) -> Div {
        div()
            .h_flex()
            .flex_wrap()
            .gap(px(6.0))
            .children(TaskFilter::ALL.into_iter().map(|filter| {
                let count = tasks.iter().filter(|task| filter.includes(task)).count()
                    + self
                        .state
                        .read(cx)
                        .tasks
                        .records
                        .iter()
                        .filter(|item| self.includes_durable(filter, item, cx))
                        .count();
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

    fn run_ownership_unconfirmed(&self, run: surge_core::RunId, cx: &App) -> bool {
        let state = self.state.read(cx);
        !state.tasks.records.is_empty()
            && !state
                .run_streams
                .get(&run)
                .is_some_and(|stream| stream.task_ownership_confirmed())
    }

    fn render_task_row(&self, task: &WorkTask, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let run_id = task.run.run_id;
        let selected = self.selected == Some(run_id);
        let selector = format!("task-{run_id}");
        div()
            .id(SharedString::from(format!("task-row-{run_id}")))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| {
                this.select_run(run_id, cx);
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
                            this.select_run(run_id, cx);
                        })),
                    ),
            )
            .child(
                div()
                    .h_flex()
                    .gap(px(12.0))
                    .child(div().h_flex().items_start().child(ui::pill(
                        task.status(),
                        task.color(),
                        theme::panel(),
                    )))
                    .when(self.run_ownership_unconfirmed(run_id, cx), |row| {
                        row.child(
                            ui::meta("Task ownership unconfirmed")
                                .id(SharedString::from(format!(
                                    "ownership-unconfirmed-{run_id}"
                                )))
                                .test_support()
                                .aria_label("Task ownership unconfirmed")
                                .debug_selector(move || format!("ownership-unconfirmed-{run_id}")),
                        )
                    })
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

    fn render_detail(
        &self,
        task: &WorkTask,
        width: Pixels,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let run_id = task.run.run_id;
        div()
            .id("task-detail")
            .v_flex()
            .gap(px(20.0))
            .w(width)
            .h_full()
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
                    .line_clamp(3)
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
            .items_center()
            .px(px(28.0))
            .pb(px(24.0))
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
                    .w_full()
                    .max_w(px(800.0))
                    .min_h(px(72.0))
                    .flex_shrink_0()
                    .h_flex()
                    .gap(px(12.0))
                    .items_center()
                    .px(px(14.0))
                    .bg(theme::panel())
                    .rounded(px(24.0))
                    .border_1()
                    .border_color(theme::hairline_strong())
                    .child(
                        div()
                            .flex_1()
                            .h_flex()
                            .gap(px(10.0))
                            .items_center()
                            .h(px(48.0))
                            .px(px(12.0))
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
                            ),
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

impl FleetScreen {
    fn render_task_list(
        &self,
        tasks: &[WorkTask],
        visible: &[&WorkTask],
        durable: &[surge_core::work_item::WorkItemRecord],
        cx: &mut Context<Self>,
    ) -> Div {
        div()
            .v_flex()
            .flex_1()
            .min_h(px(0.0))
            .min_w(px(0.0))
            .child(
                div()
                    .px(px(28.0))
                    .pb(px(16.0))
                    .child(self.render_filters(tasks, cx)),
            )
            .when(self.state.read(cx).tasks.next_cursor.is_some(), |list| {
                list.child(div().px(px(28.0)).pb(px(8.0)).child(ui::meta(
                    "Counts cover loaded items. Load more to include older tasks.",
                )))
            })
            .child(
                div()
                    .id("task-list")
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .child(
                        div()
                            .v_flex()
                            .w_full()
                            .max_w(px(960.0))
                            .px(px(28.0))
                            .children(durable.iter().map(|item| self.render_durable_row(item, cx)))
                            .when(self.state.read(cx).tasks.next_cursor.is_some(), |list| {
                                list.child(
                                    gpui_kit::component::button::Button::new("more-durable-tasks")
                                        .label("Load more tasks")
                                        .disabled(self.state.read(cx).tasks.loading)
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.load_durable_tasks(true, cx)
                                        })),
                                )
                            })
                            .when(!tasks.is_empty() && !durable.is_empty(), |list| {
                                list.child(ui::meta("Additional run history"))
                            })
                            .when(visible.is_empty() && durable.is_empty(), |list| {
                                list.child(
                                    div()
                                        .id("fleet-empty")
                                        .test_support()
                                        .aria_label("Create your first application")
                                        .v_flex()
                                        .items_center()
                                        .justify_center()
                                        .min_h(px(300.0))
                                        .gap(px(12.0))
                                        .p(px(32.0))
                                        .child(
                                            div()
                                                .text_size(px(28.0))
                                                .font_weight(FontWeight::MEDIUM)
                                                .child(if tasks.is_empty() {
                                                    "What would you like to build?"
                                                } else {
                                                    "No tasks in this view"
                                                }),
                                        )
                                        .child(ui::meta(if tasks.is_empty() {
                                            "Describe your application below to get started."
                                        } else {
                                            "Choose another filter to see more tasks."
                                        })),
                                )
                            })
                            .children(visible.iter().map(|task| self.render_task_row(task, cx))),
                    ),
            )
    }

    fn render_legacy_workspace(
        &self,
        task: &WorkTask,
        compact: bool,
        inspector_width: Pixels,
        cx: &mut Context<Self>,
    ) -> Div {
        let thread = div()
            .id("task-conversation")
            .flex_shrink_0()
            .v_flex()
            .gap(px(24.0))
            .w_full()
            .max_w(px(720.0))
            .p(px(32.0))
            .child(
                div()
                    .id("task-conversation-title")
                    .test_support()
                    .debug_selector(|| "task-conversation-title".into())
                    .text_size(px(28.0))
                    .line_height(px(36.0))
                    .flex_shrink_0()
                    .font_weight(FontWeight::SEMIBOLD)
                    .line_clamp(2)
                    .child(task.title()),
            )
            .child(div().h_flex().items_start().child(ui::pill(
                task.status(),
                task.color(),
                theme::panel(),
            )))
            .child(durable_tasks::thread_message(
                "Original request".into(),
                task.request.clone().unwrap_or_else(|| {
                    "The original request has not arrived from the daemon yet.".into()
                }),
            ));
        let thread = self.render_task_progress(thread, Some(task.run.run_id), cx);
        div()
            .h_flex()
            .items_stretch()
            .flex_1()
            .min_h(px(0.0))
            .min_w(px(0.0))
            .when(compact, |layout| layout.v_flex())
            .child(
                div()
                    .id("legacy-task-thread-scroll")
                    .flex_1()
                    .min_h(px(0.0))
                    .min_w(px(0.0))
                    .overflow_y_scroll()
                    .h_flex()
                    .items_start()
                    .justify_center()
                    .child(
                        thread
                            .test_support()
                            .debug_selector(|| "task-conversation".into()),
                    ),
            )
            .child(
                div()
                    .id("legacy-task-inspector-scroll")
                    .flex_shrink_0()
                    .when(compact, |pane| {
                        pane.w_full().max_h(px(260.0)).overflow_y_scroll()
                    })
                    .child(
                        self.render_detail(task, inspector_width, cx)
                            .when(compact, |pane| pane.w_full().border_l_0().border_t_1())
                            .test_support()
                            .debug_selector(|| "task-detail".into()),
                    ),
            )
    }

    fn render_task_notices(&self, cx: &mut Context<Self>) -> Div {
        let state = self.state.read(cx);
        let live = state.daemon_state.facade().is_some();
        let stale = !state.tasks.fresh && !state.tasks.records.is_empty();
        let cache_error = state.tasks.error.clone();
        let project_error = state.project_load_error.clone();
        div().v_flex().gap(px(6.0)).px(px(28.0)).flex_shrink_0()
            .when(!live, |notices| notices.child(ui::meta("Daemon offline · showing last known tasks. Start the daemon from the sidebar to begin work.")))
            .when(stale, |notices| notices.child(ui::meta("Task information may be out of date.")))
            .when_some(project_error, |notices, error| notices.child(div().text_color(theme::error()).child(error)))
            .when_some(cache_error, |notices, error| notices.child(ui::meta(error)))
            .when_some(self.task_error.clone(), |notices, error| notices.child(ui::meta(error)))
            .when(self.task_busy, |notices| notices.child(ui::meta("Task operation pending…")))
            .when(self.selected_item.is_some_and(|item| self.task_rejections.contains(&item)) && !self.task_busy, |notices| notices.child(
                gpui_kit::component::button::Button::new("discard-rejected-task-operation")
                    .label("Dismiss rejected operation; keep draft")
                    .on_click(cx.listener(|this, _, _, cx| this.dismiss_rejected_task_operation(cx)))))
            .when(self.selected_item.is_some_and(|item| self.task_submissions.contains_key(&item)) && !self.task_busy, |notices| notices.child(
                gpui_kit::component::button::Button::new("retry-task-operation").label("Retry task operation")
                    .on_click(cx.listener(|this, _, _, cx| this.retry_task(cx)))))
    }
}

impl Render for FleetScreen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.task_last_render = Some(std::time::Instant::now());
        self.refresh_tasks_if_changed(cx);
        let durable: Vec<_> = self
            .state
            .read(cx)
            .tasks
            .records
            .clone()
            .into_iter()
            .filter(|item| self.includes_durable(self.filter, item, cx))
            .collect();
        let tasks = self.tasks(cx);
        let visible: Vec<_> = tasks
            .iter()
            .filter(|task| self.filter.includes(task))
            .collect();
        if self
            .selected
            .is_some_and(|run| !visible.iter().any(|task| task.run.run_id == run))
        {
            self.selected = None;
        }
        let selected = visible
            .iter()
            .find(|task| Some(task.run.run_id) == self.selected)
            .copied();
        let has_selection = self.selected_item.is_some() || selected.is_some();
        let compact = window.viewport_size().width < px(1000.0);
        let workspace = if let Some(item) = self.selected_item {
            self.render_durable_workspace(item, compact, window, cx)
        } else if let Some(task) = selected {
            self.render_legacy_workspace(
                task,
                compact,
                if window.viewport_size().width >= px(1280.0) {
                    px(400.0)
                } else {
                    px(320.0)
                },
                cx,
            )
        } else {
            self.render_task_list(&tasks, &visible, &durable, cx)
        };
        let creation_form = if self.creating_task {
            Some(
                div()
                    .id("task-create-form")
                    .test_support()
                    .debug_selector(|| "task-create-form".into())
                    .max_w(px(780.0))
                    .w_full()
                    .child(self.render_task_creation(window, cx)),
            )
        } else {
            None
        };
        div()
            .v_flex()
            .size_full()
            .min_w(px(0.0))
            .bg(theme::background())
            .text_color(theme::text_primary())
            .child(
                div()
                    .h_flex()
                    .items_center()
                    .flex_shrink_0()
                    .gap(px(12.0))
                    .px(px(28.0))
                    .py(px(18.0))
                    .when(has_selection, |header| {
                        header.child(
                            gpui_kit::component::button::Button::new("fleet-back-to-list")
                                .ghost()
                                .label("‹  Tasks")
                                .accessibility_id("fleet-back-to-list")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.show_task_list(cx);
                                })),
                        )
                    })
                    .when(!has_selection, |header| {
                        header.child(
                            div()
                                .text_size(px(24.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child("Tasks"),
                        )
                    })
                    .child(div().flex_1())
                    .child(
                        gpui_kit::component::button::Button::new("refresh-durable-tasks")
                            .ghost()
                            .label("Refresh")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.refresh_durable_tasks(cx);
                                if let Some(item) = this.selected_item {
                                    this.reload_item(item, cx);
                                }
                            })),
                    )
                    .when(!has_selection, |header| {
                        header
                            .child(
                                gpui_kit::component::button::Button::new("new-durable-task")
                                    .label("New task")
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.start_new_task(cx)),
                                    ),
                            )
                            .child(
                                gpui_kit::component::button::Button::new("fleet-new-task")
                                    .ghost()
                                    .label("Plan application")
                                    .accessibility_id("fleet-new-task")
                                    .on_click(
                                        cx.listener(|_, _, _, cx| cx.emit(FleetAction::NewTask)),
                                    ),
                            )
                    }),
            )
            .child(self.render_task_notices(cx))
            .children(creation_form)
            .child(workspace)
            .when(!has_selection && !self.creating_task, |screen| {
                screen.child(self.render_command_bar(window, cx))
            })
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

    #[cfg(unix)]
    #[gpui_kit::test]
    fn durable_task_without_runs_arrives_through_real_facade(cx: &mut TestAppContext) {
        use surge_core::id::{WorkItemId, WorkItemProjectId};
        use surge_core::work_item::{
            WorkItemPage, WorkItemRecord, WorkItemResult, WorkItemWorkspace,
        };
        use surge_orchestrator::engine::ipc::{DaemonRequest, DaemonResponse};
        let home = tempfile::tempdir().unwrap();
        let repository = home.path().join("repository.git");
        let item = WorkItemRecord {
            id: WorkItemId::new(),
            project: WorkItemProjectId::new(),
            title: "Retained zero-run task".into(),
            accepted_revision: 1,
            version: 1,
            archived_at_ms: None,
            active_run: None,
            generation: 0,
            workspace: WorkItemWorkspace {
                repository: repository.clone(),
                checkout: home.path().join("project"),
                path: home.path().join("task-workspace"),
                ownership: "task-test".into(),
                branch: "task/test".into(),
                base_commit: "a".repeat(40),
            },
        };
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let records = runtime.block_on(async {
            let socket = home.path().join("task.sock");
            let listener = tokio::net::UnixListener::bind(&socket).unwrap();
            let served = item.clone();
            let server = tokio::spawn(async move {
                let (stream, _) = listener.accept().await.unwrap();
                let (read, mut write) = stream.into_split();
                let mut reader = tokio::io::BufReader::new(read);
                let Some(DaemonRequest::WorkItem {
                    request_id,
                    command,
                }) = surge_orchestrator::engine::ipc::read_request_frame(&mut reader)
                    .await
                    .unwrap()
                else {
                    panic!("expected task request");
                };
                assert!(matches!(
                    *command,
                    surge_core::work_item::WorkItemCommand::List { after: None, .. }
                ));
                surge_orchestrator::engine::ipc::write_frame(
                    &mut write,
                    &DaemonResponse::WorkItemOk {
                        request_id,
                        result: Box::new(WorkItemResult::Items(WorkItemPage {
                            entries: vec![served],
                            next_cursor: None,
                        })),
                    },
                )
                .await
                .unwrap();
            });
            let facade =
                surge_orchestrator::engine::daemon_facade::DaemonEngineFacade::connect(socket)
                    .await
                    .unwrap();
            let records = crate::work_items::list_project_tasks(&facade, &repository, None)
                .await
                .unwrap();
            server.await.unwrap();
            records
        });
        cx.update(gpui_kit::init);
        let state = cx.new(|_| {
            let mut state = AppState::new();
            let scope = state.tasks.begin(repository);
            state.tasks.apply_page(&scope, records, false);
            state
        });
        let (handle, view) = cx.update(|cx| {
            gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| {
                cx.new(|cx| FleetScreen::new(state.clone(), cx))
            })
            .unwrap()
        });
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(
                window
                    .find(SharedString::from(format!("work-item-{}", item.id)))
                    .label(),
                Some(item.title.as_str()),
                "a durable task must exist before its first run"
            );
            assert_eq!(
                window.find("filter-all").label(),
                Some("All  1"),
                "the task count includes the real zero-run task"
            );
        })
        .unwrap();

        // Fixed independent count: two known attempts belong to ONE durable task;
        // one unrelated legacy run remains a second visible task.
        let first = surge_core::RunId::new();
        let second = surge_core::RunId::new();
        let unrelated = surge_core::RunId::new();
        state.update(cx, |state, _| {
            use surge_core::work_item::{
                AcceptedRevisionRelation, WorkItemAttempt, WorkItemAttemptState, WorkItemBinding,
                WorkItemRequirements,
            };
            let requirements =
                WorkItemRequirements::new("Retained".into(), vec!["Checked".into()]).unwrap();
            let attempts = [first, second]
                .into_iter()
                .enumerate()
                .map(|(index, run)| WorkItemAttempt {
                    item: item.id,
                    run,
                    ordinal: index as u64 + 1,
                    binding: WorkItemBinding {
                        item: item.id,
                        revision: 1,
                        requirements_hash: requirements.hash().unwrap(),
                        generation: index as u64 + 1,
                    },
                    accepted_revision_relation: AcceptedRevisionRelation::MatchesCurrent,
                    graph: Box::new(
                        toml::from_str(include_str!(
                            "../../../../examples/flow_terminal_only.toml"
                        ))
                        .unwrap(),
                    ),
                    config: "{}".into(),
                    state: WorkItemAttemptState::Completed,
                    diagnostic: None,
                    usage_seq: 0,
                    input_tokens: 0,
                    output_tokens: 0,
                    known_cost_usd: 0.0,
                    usage_unknown: true,
                })
                .collect();
            state.tasks.histories.insert(
                item.id,
                crate::work_items::TaskHistory {
                    attempts: WorkItemPage {
                        entries: attempts,
                        next_cursor: None,
                    },
                    discussion: WorkItemPage {
                        entries: vec![],
                        next_cursor: None,
                    },
                    revisions: WorkItemPage {
                        entries: vec![],
                        next_cursor: None,
                    },
                },
            );
            for run_id in [first, second, unrelated] {
                state.runs.push(crate::app_state::UiRun {
                    run_id,
                    status: RunStatus::Completed,
                    started_at: chrono::Utc::now(),
                    last_event_seq: None,
                    ended_at: None,
                });
            }
        });
        cx.update_window(handle, |_, window, cx| {
            assert_eq!(
                view.read(cx).tasks(cx).len(),
                1,
                "only the unrelated legacy run is separate"
            );
            window.render_frame(cx);
            assert_eq!(window.find("filter-all").label(), Some("All  2"));
            assert_eq!(window.find("filter-finished").label(), Some("Finished  2"));
        })
        .unwrap();
    }

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
    fn task_list_navigation_preserves_new_task_draft(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let state = cx.new(|_| AppState::new());
        let view = cx.new(|cx| FleetScreen::new(state, cx));
        let (_, window) = cx
            .add_window_view(|window, cx| gpui_kit::component::Root::new(view.clone(), window, cx));
        window.update(|window, cx| {
            view.update(cx, |fleet, cx| {
                fleet.start_new_task(cx);
                let _ = fleet.render_task_creation(window, cx);
                let title = fleet.new_task_draft.as_ref().unwrap().title.clone();
                title.update(cx, |input, cx| {
                    input.set_value("Retained task draft", window, cx)
                });
                fleet.selected = Some(surge_core::RunId::new());
                fleet.selected_item = Some(surge_core::id::WorkItemId::new());
                fleet.show_task_list(cx);
                assert!(!fleet.creating_task);
                assert!(fleet.selected.is_none());
                assert!(fleet.selected_item.is_none());
                fleet.start_new_task(cx);
                let _ = fleet.render_task_creation(window, cx);
                let retained = &fleet.new_task_draft.as_ref().unwrap().title;
                assert_eq!(retained.entity_id(), title.entity_id());
                assert_eq!(retained.read(cx).value().as_ref(), "Retained task draft");
            });
        });
    }

    #[gpui_kit::test]
    fn selected_task_opens_conversation_and_back_restores_list(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let run_id = surge_core::RunId::new();
        let request = "Build a small browser pomodoro timer: plain HTML/CSS/JS, no dependencies, start/pause/reset, 25/5 minute cycles, keyboard shortcuts, accessible status text, and a Node test for the timer logic.";
        let state = cx.new(|_| {
            let mut state = AppState::new();
            state.runs.push(crate::app_state::UiRun {
                run_id,
                status: RunStatus::Active,
                started_at: chrono::Utc::now(),
                last_event_seq: None,
                ended_at: None,
            });
            state.run_streams.entry(run_id).or_default().prompt = Some(request.into());
            state
        });
        let view = cx.new(|cx| FleetScreen::new(state, cx));
        let (_, window) = cx
            .add_window_view(|window, cx| gpui_kit::component::Root::new(view.clone(), window, cx));
        window.update(|window, cx| {
            view.update(cx, |fleet, cx| fleet.start_new_task(cx));
            window.render_frame(cx);
            window.click(SharedString::from(format!("task-{run_id}")), cx);
            window.render_frame(cx);
            assert_eq!(view.read(cx).tasks(cx)[0].request.as_deref(), Some(request));
            assert!(
                !view.read(cx).creating_task,
                "opening a real row exits creation"
            );
        });
        assert!(window.debug_bounds("task-conversation").is_some());
        let title = window.debug_bounds("task-conversation-title").unwrap();
        assert!(title.size.height > gpui_kit::px(0.0));
        assert!(
            title.size.height <= gpui_kit::px(90.0),
            "a long request leaves room for the conversation below its title"
        );
        assert!(
            title.origin.y >= gpui_kit::px(0.0),
            "the task title remains visible at the top of the thread"
        );
        assert!(window.debug_bounds("task-create-form").is_none());
        assert!(window.debug_bounds("fleet-prompt-region").is_none());
        window.update(|window, cx| {
            window.click("fleet-back-to-list", cx);
            window.render_frame(cx);
            assert!(view.read(cx).selected.is_none());
        });
        assert!(window.debug_bounds("task-conversation").is_none());
        assert!(window.debug_bounds("fleet-prompt-region").is_some());
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
        window.update(|window, cx| {
            window.click("fleet-back-to-list", cx);
            window.render_frame(cx);
        });
        let decisions = window.debug_bounds("filter-decisions").unwrap();
        window.simulate_click(decisions.center(), gpui_kit::Modifiers::default());
        window.update(|_, cx| {
            assert_eq!(view.read(cx).filter, super::TaskFilter::NeedsDecision);
            assert!(view.read(cx).selected.is_none());
        });
        assert!(window.debug_bounds("fleet-task-row").is_some());
        window.update(|window, cx| {
            window.click(SharedString::from(format!("task-{stopped_id}")), cx);
            assert_eq!(view.read(cx).selected, Some(stopped_id));
            window.render_frame(cx);
            window.click("fleet-back-to-list", cx);
            window.render_frame(cx);
            assert_eq!(
                view.read(cx).filter,
                super::TaskFilter::NeedsDecision,
                "returning from a filtered row retains the chosen filter"
            );
        });
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
        window.update(|window, cx| {
            window.click("fleet-back-to-list", cx);
            window.render_frame(cx);
            assert_eq!(view.read(cx).filter, super::TaskFilter::Finished);
        });
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
        let expected_run = cx.update(|cx| view.read(cx).tasks(cx)[0].run.run_id);
        let inbox_actions = Rc::new(Cell::new(0));
        let captured = inbox_actions.clone();
        cx.update(|cx| {
            cx.subscribe(&view, move |_, event: &FleetAction, _| {
                assert!(
                    matches!(event, FleetAction::OpenGate(id) if id == &expected_run.to_string())
                );
                captured.set(captured.get() + 1);
            })
            .detach();
        });
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click(SharedString::from(format!("task-{expected_run}")), cx);
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
