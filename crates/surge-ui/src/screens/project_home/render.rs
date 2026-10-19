use super::*;
use crate::mission::{IterationState, Mission, PlanState, Step, StepState, TaskView};
use crate::project_lifecycle::{HomeOperation, OperationKey};
use crate::{theme, ui};
use gpui_kit::component::{
    StyledExt,
    button::{Button, ButtonVariants},
};
use gpui_kit::prelude::FluentBuilder;

impl Render for ProjectHomeScreen {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.last_render = Some(std::time::Instant::now());
        let state = self.state.read(cx);
        let mut body = self.render_header(cx);
        body = body
            .child(self.render_decisions(cx))
            .child(self.render_current(cx));
        for input in state
            .project_history
            .planning_inputs
            .iter()
            .enumerate()
            .filter(|(_, input)| input.source.availability != Availability::Absent)
        {
            let (index, input) = input;
            body = body
                .child(
                    section_title("Proposed project plan")
                        .id(SharedString::from(format!("home-planning-input-{index}")))
                        .role(Role::Region)
                        .aria_label("Proposed project plan")
                        .test_support()
                        .debug_selector(move || format!("home-planning-input-{index}")),
                )
                .child(ui::meta(input.path.display().to_string()))
                .child(ui::meta(input.source.label()));
            if let Some(roadmap) = &input.roadmap {
                body = body.child(ui::meta(format!("{} authored milestones · proposed scope only", roadmap.milestones.len())))
                    .child(ui::meta("File statuses do not establish accepted scope, execution, or verification. Open Project plan to inspect this input."));
            }
        }
        body = body.child(section_title("Stages and requests"));
        if self.model.operations.is_empty() {
            body = body.child(ui::meta(
                "Plan an application, or create a focused task. Refresh to inspect saved work.",
            ));
        }
        for operation in &self.model.operations {
            body = body.child(self.render_operation(operation, cx));
        }
        if state.tasks.next_cursor.is_some() {
            body = body
                .child(ui::meta(
                    "Accepted task list has more pages; loaded items are not the complete project.",
                ))
                .child(
                    Button::new("home-more-accepted-tasks")
                        .ghost()
                        .label("Load more accepted tasks")
                        .on_click(cx.listener(|this, _, _, cx| {
                            crate::work_items::refresh_project_tasks(&this.state, true, cx)
                        })),
                );
        }
        if state.project_history.next_cursor.is_some() {
            body = body.child(
                Button::new("home-older-history")
                    .ghost()
                    .label("Load more project history")
                    .accessibility_id("home-older-history")
                    .on_click(cx.listener(|this, _, _, cx| this.reload(true, cx))),
            );
        }
        body = body.child(
            Button::new("home-history-details")
                .ghost()
                .label(if self.provenance_open {
                    "Hide history details"
                } else {
                    "Inspect history details"
                })
                .on_click(cx.listener(|this, _, _, cx| {
                    this.provenance_open = !this.provenance_open;
                    cx.notify();
                })),
        );
        if self.provenance_open {
            body=body.child(div().id("home-history-provenance").role(Role::Region).aria_label("History inspection details").test_support().debug_selector(||"home-history-provenance".into()).v_flex().gap(px(6.0))
                .child(ui::meta(format!("Loaded history: {} task occurrences · {} verified records · {} runs with unknown ownership excluded",self.model.recorded_tasks,self.model.verified,self.model.unknown_runs)))
                .child(ui::meta("Registry pages, accepted task records and run journals are separate observations. Counts describe loaded history, not completion of the project."))
                .child(ui::meta("Membership uses recorded repository and Git identities. Reusing the same folder for a different repository incarnation cannot be independently confirmed from paths alone.")));
        }
        div()
            .id("project-home-scroll")
            .size_full()
            .overflow_y_scroll()
            .child(body)
    }
}
impl ProjectHomeScreen {
    fn render_header(&self, cx: &Context<Self>) -> Stateful<Div> {
        let state = self.state.read(cx);
        let title = if state.project_name.is_empty() {
            "Project home".into()
        } else {
            state.project_name.clone()
        };
        let source = match &state.project_history.source.availability {
            Availability::Unavailable(error) => format!("Saved history could not be read: {error}"),
            Availability::Absent => "No saved runs found yet.".into(),
            _ => String::new(),
        };
        let task_source = if let Some(error) = &state.tasks.error {
            format!("Tasks unavailable: {error}")
        } else if state.tasks.loading {
            "Refreshing accepted tasks".into()
        } else if state.tasks.fresh {
            "Accepted tasks · last observed".into()
        } else {
            "Accepted task history has not been confirmed".into()
        };
        let mut body = div()
            .id("project-home")
            .role(Role::Region)
            .aria_label("Project home")
            .v_flex()
            .gap(px(20.0))
            .p(px(24.0));
        body = body
            .child(ui::page_header(
                title,
                Some("What you asked for, what is happening, and what comes next.".into()),
                div(),
            ))
            .child(
                div()
                    .h_flex()
                    .gap(px(8.0))
                    .child(
                        Button::new("home-add-idea")
                            .primary()
                            .label("Plan application")
                            .accessibility_id("home-add-idea")
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(HomeAction::PlanApplication))),
                    )
                    .child(Button::new("home-create-task").ghost().label("Create task").on_click(cx.listener(|_,_,_,cx|cx.emit(HomeAction::CreateTask))))
                    .child(
                        Button::new("home-refresh")
                            .ghost()
                            .label("Refresh")
                            .on_click(cx.listener(|this, _, _, cx| {
                                crate::work_items::refresh_project_tasks(&this.state, false, cx);
                                this.reload(false, cx);
                            })),
                    )
                    .child(
                        Button::new("home-open-plan")
                            .ghost()
                            .label("Project plan")
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(HomeAction::OpenPlan))),
                    ),
            )
            .child(ui::meta(
                "Your working copy stays separate; agent work uses worktrees.",
            ))
            .when(!state.tasks.records.is_empty() || state.tasks.error.is_some(),|body|body.child(ui::meta(task_source)))
            .when(!source.is_empty(),|body|body.child(ui::meta(source)))
            .when(state.daemon_state.facade().is_none(),|body|body.child(ui::meta("Start the daemon from the sidebar when you are ready to run work. Planning opens a form first.")));
        body
    }
    fn render_decisions(&self, cx: &Context<Self>) -> Div {
        let mut decisions = div()
            .v_flex()
            .gap(px(8.0))
            .child(section_title("Decisions · last observed"));
        if self.model.decisions.is_empty() {
            if self.model.operations.is_empty() {
                return div();
            }
            let message = if self.model.decisions_uncertain {
                "Pending decisions are unconfirmed. Some history is incomplete or unavailable; inspect a recorded run for earlier requests."
            } else {
                "No pending decisions in the observed work."
            };
            decisions = decisions.child(
                div()
                    .id("home-decision-status")
                    .role(Role::Region)
                    .aria_label(message)
                    .test_support()
                    .debug_selector(|| "home-decision-status".into())
                    .child(ui::meta(message)),
            );
        }
        for decision in &self.model.decisions {
            let run = decision.id.run;
            let seq = decision.id.request_seq;
            decisions = decisions.child(
                Button::new(SharedString::from(format!("home-decision-{run}-{seq}")))
                    .ghost()
                    .label(decision.prompt.clone())
                    .accessibility_id(format!("home-decision-{run}-{seq}"))
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(HomeAction::OpenDecision {
                            run,
                            request_seq: seq,
                        })
                    })),
            );
        }
        decisions
    }
    fn render_current(&self, cx: &Context<Self>) -> Div {
        if self.model.current.is_empty() && self.model.unknown.is_empty() {
            return div();
        }
        let mut body = div()
            .v_flex()
            .gap(px(8.0))
            .child(section_title("Current work · last observed"));
        if self.model.current.is_empty() {
            body=body.child(ui::meta("No open stage is confirmed in the loaded history. Refresh or inspect unavailable runs before concluding work has stopped."));
        }
        for work in &self.model.current {
            let run = work.run;
            let target = crate::screens::runs::RecordedTarget::Stage {
                entered: work.entered,
            };
            body =
                body.child(
                    Button::new(SharedString::from(format!(
                        "home-current-stage-{run}-{}",
                        work.entered
                    )))
                    .ghost()
                    .label("Inspect current stage")
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(HomeAction::OpenRecorded { run, target })
                    })),
                );
            body = body
                .child(paragraph(work.title.clone()))
                .child(ui::meta(format!(
                    "Stage entry {} · profile {} · runtime {} · session {}",
                    work.entered,
                    work.profile.as_deref().unwrap_or("unrecorded"),
                    work.runtime.as_deref().unwrap_or("unrecorded"),
                    work.session
                        .map_or_else(|| "unrecorded".into(), |session| session.to_string())
                )))
                .child(
                    Button::new(SharedString::from(format!("home-current-{run}")))
                        .ghost()
                        .label("Inspect this run")
                        .on_click(
                            cx.listener(move |_, _, _, cx| cx.emit(HomeAction::OpenRun(run))),
                        ),
                );
        }
        if !self.model.unknown.is_empty() {
            body = body.child(section_title("History needs attention"));
        }
        for (run, reason) in &self.model.unknown {
            let run = *run;
            body = body.child(ui::meta(reason.clone())).child(
                Button::new(SharedString::from(format!("home-unknown-{run}")))
                    .ghost()
                    .label(format!("Inspect unassigned run {run}"))
                    .on_click(cx.listener(move |_, _, _, cx| cx.emit(HomeAction::OpenRun(run)))),
            );
        }
        body
    }
    fn render_operation(&self, operation: &HomeOperation, cx: &Context<Self>) -> Div {
        let key = operation.key;
        let expanded = self.expanded.contains(&key);
        let mut card = div()
            .v_flex()
            .gap(px(8.0))
            .p(px(16.0))
            .border_1()
            .border_color(theme::hairline())
            .rounded(px(ui::R_CONTROL))
            .child(
                Button::new(SharedString::from(format!("home-operation-{key:?}")))
                    .ghost()
                    .label(operation.title.clone())
                    .accessibility_id(format!("home-operation-{key:?}"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.expanded.insert(key) {
                            this.expanded.remove(&key);
                        } else if let OperationKey::Task { item, .. } = key {
                            crate::work_items::request_task_detail(&this.state, item, cx);
                        }
                        cx.notify();
                    })),
            )
            .child(ui::meta(operation.status.clone()));
        if let Some(accepted) = &operation.accepted {
            card = card.child(ui::meta(format!("Accepted revision {}", accepted.revision)));
        }
        if let Some(asked) = &operation.asked {
            let excerpt: String = asked.chars().take(180).collect();
            card = card.child(paragraph(excerpt));
        }
        if let OperationKey::Task { item, .. } = key {
            card = card.child(
                Button::new(SharedString::from(format!("home-task-{item}")))
                    .ghost()
                    .label("Open task discussion and workflow")
                    .accessibility_id(format!("home-task-{item}"))
                    .on_click(cx.listener(move |_, _, _, cx| cx.emit(HomeAction::OpenTask(item)))),
            );
        }
        if !expanded {
            return card;
        }
        card = self.render_accepted(card, operation, cx);
        card = self.render_more_history(card, operation, cx);
        for run in &operation.runs {
            card = card.child(self.render_run(*run, cx));
        }
        card
    }
    fn render_accepted(&self, mut card: Div, operation: &HomeOperation, cx: &Context<Self>) -> Div {
        let key = operation.key;
        card = card
            .child(section_title("What you asked for"))
            .child(paragraph(operation.asked.clone().unwrap_or_else(|| {
                "Accepted request is unavailable; the title alone is not accepted scope.".into()
            })));
        if let Some(accepted) = &operation.accepted {
            card = card.child(ui::meta(format!(
                "Accepted revision {} · {}",
                accepted.revision, accepted.actor
            )));
            if let surge_core::work_item::AcceptedWorkItemOrigin::Requirements(requirements) =
                &accepted.origin
            {
                for criterion in requirements.criteria() {
                    card = card.child(paragraph(format!("• {criterion}")));
                }
            } else {
                card = card.child(ui::meta(
                    "Accepted ordinary workflow; its prompt is not synthetic task criteria.",
                ));
            }
        }
        for revision in &operation.revisions {
            card = card
                .child(ui::meta(format!(
                    "Revision {} · {}",
                    revision.revision, revision.actor
                )))
                .child(paragraph(lifecycle::origin_text(&revision.origin)));
        }
        if let OperationKey::Bootstrap(operation) = key
            && let Some(legacy) = self
                .state
                .read(cx)
                .project_history
                .operations
                .get(&operation)
        {
            card = card.child(ui::meta(legacy.roadmap_source.label()));
            if let Some(child) = &legacy.record.child {
                for reference in child.artifacts() {
                    card = card.child(ui::meta(format!(
                        "Accepted {} · {}",
                        reference.name, reference.hash
                    )));
                }
            }
        }
        card
    }
    fn render_more_history(
        &self,
        mut card: Div,
        operation: &HomeOperation,
        cx: &Context<Self>,
    ) -> Div {
        let key = operation.key;
        if operation.history_more
            && let OperationKey::Task { item, .. } = key
        {
            for (kind, label) in [
                (
                    crate::work_items::TaskHistoryKind::Attempts,
                    "Load more attempts",
                ),
                (
                    crate::work_items::TaskHistoryKind::Discussion,
                    "Load more discussion",
                ),
                (
                    crate::work_items::TaskHistoryKind::Revisions,
                    "Load more accepted revisions",
                ),
            ] {
                if self
                    .state
                    .read(cx)
                    .tasks
                    .histories
                    .get(&item)
                    .and_then(|history| history.next(kind))
                    .is_some()
                {
                    card = card.child(
                        Button::new(SharedString::from(format!("home-more-{item}-{kind:?}")))
                            .ghost()
                            .label(label)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                crate::work_items::request_task_history(&this.state, item, kind, cx)
                            })),
                    );
                }
            }
        }
        card
    }
    fn render_run(&self, run: RunId, cx: &Context<Self>) -> Div {
        let state = self.state.read(cx);
        let observed = state.project_history.runs.get(&run);
        let mut body = div()
            .v_flex()
            .gap(px(6.0))
            .p(px(10.0))
            .child(ui::meta(format!("Run {run}")))
            .child(
                Button::new(SharedString::from(format!("home-run-{run}")))
                    .ghost()
                    .label("Inspect run, results and agent activity")
                    .accessibility_id(format!("home-run-{run}"))
                    .on_click(cx.listener(move |_, _, _, cx| cx.emit(HomeAction::OpenRun(run)))),
            );
        if let Some(summary) = state
            .project_history
            .summaries
            .iter()
            .find(|summary| summary.id == run)
        {
            body = body.child(ui::meta(format!(
                "Saved registry status: {}",
                summary.status.as_str()
            )));
        }
        let Some(observed) = observed else {
            return body.child(ui::meta("Run history not loaded yet"));
        };
        body = body.child(ui::meta(observed.source.label()));
        body = body.child(ui::meta(match &observed.ownership {
            RunOwnership::Modern {
                project, binding, ..
            } => format!(
                "Project {project} · task revision {} · attempt generation {}",
                binding.revision, binding.generation
            ),
            RunOwnership::Bootstrap { planning, .. } => {
                if *planning {
                    "Planning run".into()
                } else {
                    "Implementation run".into()
                }
            },
            RunOwnership::CandidateProject => {
                "Saved project match; startup history has not been checked yet".into()
            },
            RunOwnership::RecordedProject => "Recorded project workflow".into(),
            _ => "Ownership unconfirmed; excluded from project progress".into(),
        }));
        if let Some(mission) = &observed.mission {
            body = body.child(render_mission(mission));
        }
        if let Some(execution) = &observed.execution {
            for stage in &execution.stages {
                let target = crate::screens::runs::RecordedTarget::Stage {
                    entered: stage.entered,
                };
                body = body.child(
                    Button::new(SharedString::from(format!(
                        "home-stage-{run}-{}",
                        stage.entered
                    )))
                    .ghost()
                    .label("Inspect recorded stage")
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(HomeAction::OpenRecorded { run, target })
                    })),
                );
                for session in &stage.sessions {
                    body = body.child(ui::meta(format!(
                        "Recorded agent: {} via {} · stage entry {} · session {}",
                        session.profile,
                        session.runtime.as_deref().unwrap_or("unknown runtime"),
                        stage.entered,
                        session.connection
                    )));
                    let target = crate::screens::runs::RecordedTarget::Session {
                        entered: stage.entered,
                        opened: session.opened,
                        connection: session.connection,
                    };
                    body = body.child(
                        Button::new(SharedString::from(format!(
                            "home-session-{run}-{}",
                            session.opened
                        )))
                        .ghost()
                        .label("Inspect recorded session")
                        .on_click(cx.listener(move |_, _, _, cx| {
                            cx.emit(HomeAction::OpenRecorded { run, target })
                        })),
                    );
                }
            }
        }
        if observed.source.coverage == Coverage::Partial {
            let owner = observed.requested.clone();
            body = body.child(
                Button::new(SharedString::from(format!("home-more-run-{run}")))
                    .ghost()
                    .label("Load more run history")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(scope) = this.state.read(cx).project_history.scope.clone() {
                            this.load_one(scope, run, owner.clone(), true, cx);
                        }
                    })),
            );
        }
        body
    }
}
fn section_title(title: &str) -> Div {
    div()
        .text_size(px(16.0))
        .text_color(theme::text_primary())
        .child(title.to_owned())
}
fn render_mission(mission: &Mission) -> Div {
    let mut body = div().v_flex().gap(px(6.0));
    for step in &mission.plan {
        let name = match step.stage {
            surge_core::BootstrapStage::Description => "Description",
            surge_core::BootstrapStage::Roadmap => "Roadmap",
            surge_core::BootstrapStage::Flow => "Workflow",
        };
        let state = match step.state {
            PlanState::NotStarted => "planned",
            PlanState::Drafting { .. } => "drafting",
            PlanState::AwaitingYou { .. } => "needs your approval",
            PlanState::Approved { .. } => "approved",
            PlanState::Rejected => "rejected",
        };
        body = body.child(paragraph(format!("{name} · {state}")));
    }
    for step in &mission.steps {
        body = body.child(step_line(step));
    }
    for milestone in &mission.milestones {
        body = body.child(section_title(&milestone.title));
        for step in &milestone.steps {
            body = body.child(step_line(step));
        }
        for task in &milestone.tasks {
            body = body.child(task_lines(task));
        }
    }
    for caveat in &mission.caveats {
        body = body.child(ui::meta(caveat.clone()));
    }
    body
}
fn step_line(step: &Step) -> Div {
    let state = match &step.state {
        StepState::Working => "last observed in progress".into(),
        StepState::WaitingOnYou => "decision requested".into(),
        StepState::Done(outcome) => format!("finished: {outcome}"),
        StepState::Failed(error) => format!("failed: {error}"),
        StepState::Unknown => "continuation unknown".into(),
        StepState::Unfinished => "no completion recorded".into(),
    };
    paragraph(format!(
        "{} · {state} · attempt {}",
        step.label, step.attempts
    ))
}
fn task_lines(task: &TaskView) -> Div {
    let state = match &task.iteration {
        IterationState::Planned => "planned".into(),
        IterationState::Discovered => "discovered; not dispatched in this prefix".into(),
        IterationState::Queued => "queued after split".into(),
        IterationState::Replaced => "split into smaller tasks".into(),
        IterationState::Running => "last observed in progress".into(),
        IterationState::Finished(outcome) => format!("iteration finished: {outcome}"),
        IterationState::Unknown => "continuation unknown".into(),
        IterationState::Unfinished => "no completion recorded".into(),
    };
    let mut body = div()
        .v_flex()
        .gap(px(4.0))
        .pl(px(12.0))
        .child(paragraph(format!(
            "{} · {state} · {}",
            task.title,
            if task.verified {
                "verified at the recorded revision"
            } else {
                "verification unconfirmed"
            }
        )));
    for step in &task.steps {
        body = body.child(step_line(step));
    }
    for child in &task.subtasks {
        body = body.child(task_lines(child));
    }
    body
}

fn paragraph(text: impl Into<SharedString>) -> Div {
    let text: SharedString = text.into();
    div()
        .text_size(px(13.0))
        .text_color(theme::text_primary())
        .child(text)
}
