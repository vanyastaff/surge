//! Explicit accepted-requirement creation through the existing task owner.

use super::{FleetScreen, NewTaskDraft};
use crate::ui;
use gpui_kit::component::button::ButtonVariants as _;
use gpui_kit::component::input::{Input, InputState, Textarea, TextareaState};
use gpui_kit::component::{Disableable, StyledExt};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

impl FleetScreen {
    pub(super) fn render_task_creation(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let draft = self.new_task_draft.get_or_insert_with(|| NewTaskDraft {
            title: cx.new(|cx| InputState::new(window, cx).placeholder("Task title")),
            requirements: cx.new(|cx| {
                TextareaState::new(window, cx)
                    .rows(4)
                    .placeholder("Accepted requirements")
            }),
            criteria: cx.new(|cx| {
                TextareaState::new(window, cx)
                    .rows(3)
                    .placeholder("One acceptance criterion per line")
            }),
        });
        div()
            .v_flex()
            .gap(px(8.0))
            .p(px(20.0))
            .child(ui::meta("Task title"))
            .child(Input::new(&draft.title).accessibility_id("new-task-title"))
            .child(ui::meta("Accepted requirements"))
            .child(
                Textarea::new(&draft.requirements)
                    .h(px(100.0))
                    .accessibility_id("new-task-requirements"),
            )
            .child(ui::meta("Acceptance criteria · one per line"))
            .child(
                Textarea::new(&draft.criteria)
                    .h(px(85.0))
                    .accessibility_id("new-task-criteria"),
            )
            .when_some(self.creation_error.clone(), |panel, error| {
                panel.child(ui::meta(error))
            })
            .when_some(
                self.creation_project
                    .clone()
                    .filter(|_| self.creation_submission.is_some()),
                |panel, project| {
                    panel.child(ui::meta(format!(
                        "Pending creation belongs to {}",
                        project.display()
                    )))
                },
            )
            .child(
                gpui_kit::component::button::Button::new("create-durable-task")
                    .primary()
                    .label(if self.creation_submission.is_some() {
                        "Retry original creation"
                    } else {
                        "Create task"
                    })
                    .disabled(self.creation_busy)
                    .on_click(cx.listener(|this, _, _, cx| this.submit_task_creation(cx))),
            )
            .child(
                gpui_kit::component::button::Button::new("close-task-creation")
                    .label(if self.creation_submission.is_some() {
                        "Close form · keep pending creation"
                    } else {
                        "Cancel"
                    })
                    .disabled(self.creation_busy)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.creating_task = false;
                        cx.notify();
                    })),
            )
    }

    pub(super) fn submit_task_creation(&mut self, cx: &mut Context<Self>) {
        use surge_core::work_item::{WorkItemCommand, WorkItemRequirements, WorkItemResult};
        if self.creation_busy {
            return;
        }
        if self.creation_submission.is_none() {
            let Some(draft) = &self.new_task_draft else {
                return;
            };
            let Some(project) = self.state.read(cx).project_path.clone() else {
                self.creation_error = Some("Open a Git project before creating a task.".into());
                cx.notify();
                return;
            };
            let title = draft.title.read(cx).value().to_string();
            if title.trim().is_empty() || title.len() > 1024 {
                self.creation_error =
                    Some("Enter a task title that is not empty or oversized.".into());
                cx.notify();
                return;
            }
            let criteria = draft
                .criteria
                .read(cx)
                .value()
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(str::to_owned)
                .collect();
            let requirements = match WorkItemRequirements::new(
                draft.requirements.read(cx).value().to_string(),
                criteria,
            ) {
                Ok(requirements) => requirements,
                Err(error) => {
                    self.creation_error = Some(error);
                    cx.notify();
                    return;
                },
            };
            self.creation_submission = Some(crate::work_items::TaskSubmission::new(
                WorkItemCommand::Create {
                    operation_id: surge_core::id::WorkItemOperationId::new(),
                    project: project.clone(),
                    title,
                    requirements,
                },
            ));
            self.creation_project = Some(project);
        }
        let Some(facade) = self.state.read(cx).daemon_state.facade() else {
            self.creation_error = Some(
                "Draft retained. Retry its original creation when the daemon is connected.".into(),
            );
            cx.notify();
            return;
        };
        let Some(submission) = self.creation_submission.clone() else {
            return;
        };
        let project = self.creation_project.clone();
        let accepted_command = submission.retry();
        self.creation_busy = true;
        self.creation_error = None;
        cx.spawn(async move |this, cx| {
            let result = facade.work_item(submission.retry()).await;
            let _ = this.update(cx, |this, cx| {
                this.creation_busy = false;
                match result {
                    Ok(WorkItemResult::Detail(detail)) => {
                        if let (
                            Some(draft),
                            WorkItemCommand::Create {
                                title,
                                requirements,
                                ..
                            },
                        ) = (&this.new_task_draft, &accepted_command)
                        {
                            let criteria: Vec<_> = draft
                                .criteria
                                .read(cx)
                                .value()
                                .lines()
                                .map(str::trim)
                                .filter(|line| !line.is_empty())
                                .map(str::to_owned)
                                .collect();
                            if draft.title.read(cx).value().as_ref() == title
                                && draft.requirements.read(cx).value().as_ref()
                                    == requirements.text()
                                && criteria == requirements.criteria()
                            {
                                this.new_task_draft = None;
                            }
                        }
                        this.creation_submission = None;
                        this.creation_project = None;
                        if this.state.read(cx).project_path == project {
                            this.creating_task = false;
                            let item = detail.item.id;
                            this.state.update(cx, |state, cx| {
                                if !state.tasks.records.iter().any(|record| record.id == item) {
                                    state.tasks.records.push(detail.item.clone());
                                }
                                state.tasks.details.insert(item, *detail);
                                cx.notify();
                            });
                            this.refresh_durable_tasks(cx);
                            this.select_item(item, cx);
                        }
                    },
                    Ok(_) => {
                        this.creation_error = Some(
                            "Unexpected task creation response; retry the original operation."
                                .into(),
                        )
                    },
                    Err(error) => this.creation_error = Some(error.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}
