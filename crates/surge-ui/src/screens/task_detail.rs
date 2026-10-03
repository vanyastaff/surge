//! Durable task detail and daemon-owned task commands for the Tasks surface.

use super::{FleetAction, FleetScreen, TaskDraft};
use crate::{theme, ui};
use gpui_kit::component::button::ButtonVariants as _;
use gpui_kit::component::input::{Textarea, TextareaState};
use gpui_kit::component::{Disableable, StyledExt};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

impl FleetScreen {
    pub(super) fn refresh_tasks_if_changed(&mut self, cx: &mut Context<Self>) {
        let state = self.state.read(cx);
        let identity = Some((
            state
                .daemon_state
                .facade()
                .map_or(0, |facade| std::sync::Arc::as_ptr(&facade) as usize),
            state.project_path.clone(),
        ));
        if self.task_refresh_identity == identity {
            return;
        }
        let project_changed = self
            .task_refresh_identity
            .as_ref()
            .is_some_and(|old| Some(&old.1) != identity.as_ref().map(|new| &new.1));
        self.task_refresh_identity = identity;
        if project_changed {
            self.selected_item = None;
            self.state
                .update(cx, |state, _| state.tasks.clear_project());
        }
        if self
            .task_refresh_identity
            .as_ref()
            .is_some_and(|identity| identity.0 == 0)
        {
            self.state.update(cx, |state, _| state.tasks.disconnected());
        }
        self.task_error = None;
        self.refresh_durable_tasks(cx);
    }

    pub(super) fn render_durable_row(
        &self,
        item: &surge_core::work_item::WorkItemRecord,
        cx: &mut Context<Self>,
    ) -> Div {
        let id = item.id;
        let label = self.durable_status(item, cx);
        div()
            .v_flex()
            .gap(px(8.0))
            .p(px(16.0))
            .border_b_1()
            .border_color(theme::hairline())
            .child(
                gpui_kit::component::button::Button::new(SharedString::from(format!(
                    "work-item-{id}"
                )))
                .ghost()
                .label(item.title.clone())
                .accessibility_id(format!("work-item-{id}"))
                .on_click(cx.listener(move |this, _, _, cx| this.select_item(id, cx))),
            )
            .child(ui::meta(format!(
                "{label} · revision {} · {}",
                item.accepted_revision, item.id
            )))
    }

    pub(super) fn render_durable_detail(
        &mut self,
        item: surge_core::id::WorkItemId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let state = self.state.read(cx);
        let detail = state.tasks.details.get(&item).cloned();
        let history = state.tasks.histories.get(&item).cloned();
        let mut panel = div()
            .id("durable-task-detail")
            .v_flex()
            .gap(px(12.0))
            .w(px(380.0))
            .p(px(20.0))
            .overflow_y_scroll()
            .border_l_1()
            .border_color(theme::hairline());
        let Some(detail) = detail else {
            return panel.child(ui::meta("Select Refresh if task details are unavailable."));
        };
        panel = self.render_task_summary(panel, &detail, cx);
        let recorded_run = detail.item.active_run.or_else(|| {
            history.as_ref().and_then(|history| {
                history
                    .attempts
                    .entries
                    .iter()
                    .max_by_key(|attempt| attempt.ordinal)
                    .map(|attempt| attempt.run)
            })
        });
        panel = self.render_task_progress(panel, recorded_run, cx);
        panel = self.render_task_history(panel, &detail, history, cx);
        self.render_task_controls(panel, &detail, window, cx)
    }

    fn render_task_summary(
        &self,
        mut panel: Stateful<Div>,
        detail: &surge_core::work_item::WorkItemDetail,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let record = &detail.item;
        let control_label = self.durable_status(record, cx);
        panel = panel
            .child(
                div()
                    .text_size(px(22.0))
                    .font_weight(FontWeight::BOLD)
                    .child(record.title.clone()),
            )
            .child(ui::meta(control_label))
            .child(ui::meta(format!(
                "Accepted revision {}",
                detail.revision.revision
            )))
            .child(div().child(detail.revision.requirements.text().to_owned()))
            .children(
                detail
                    .revision
                    .requirements
                    .criteria()
                    .iter()
                    .map(|criterion| div().child(format!("• {criterion}"))),
            )
            .child(ui::meta(format!("Branch {}", record.workspace.branch)))
            .child(ui::meta(record.workspace.path.display().to_string()))
            .child(
                gpui_kit::component::button::Button::new("durable-task-open-workspace")
                    .label("Open retained workspace")
                    .on_click(cx.listener({
                        let path = record.workspace.path.clone();
                        move |_, _, _, cx| cx.reveal_path(&path)
                    })),
            )
            .when_some(detail.pr.clone(), |panel, pr| {
                panel.child(ui::meta(format!("PR {}", pr.url))).child(
                    gpui_kit::component::button::Button::new("durable-task-open-pr")
                        .label("Open pull request")
                        .on_click(cx.listener(move |_, _, _, cx| cx.open_url(&pr.url))),
                )
            })
            .child(ui::meta(format!(
                "{} attempts · {} input / {} output tokens · {} runs with unknown usage",
                detail.usage.runs,
                detail.usage.input_tokens,
                detail.usage.output_tokens,
                detail.usage.unknown_runs
            )))
            .child(
                ui::meta(format!(
                    "Known recorded cost: ${:.4}{}",
                    detail.usage.known_cost_usd,
                    if detail.usage.unknown_runs > 0 {
                        format!(
                            " · total incomplete ({} runs with unknown usage)",
                            detail.usage.unknown_runs
                        )
                    } else {
                        " · recorded amounts only".into()
                    }
                ))
                .id("durable-task-recorded-cost")
                .test_support()
                .debug_selector(|| "durable-task-recorded-cost".into())
                .aria_label(format!(
                    "Known recorded cost: ${:.4}; {} runs with unknown usage",
                    detail.usage.known_cost_usd, detail.usage.unknown_runs
                )),
            );
        panel
    }

    fn render_task_progress(
        &self,
        mut panel: Stateful<Div>,
        run: Option<surge_core::RunId>,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let state = self.state.read(cx);
        let Some(run) = run else {
            return panel;
        };
        let Some(stream) = state.run_streams.get(&run) else {
            return panel;
        };
        panel = panel.child(
            div()
                .font_weight(FontWeight::BOLD)
                .child("Recorded stage progress"),
        );
        for stage in stream.stages.iter().rev().take(6) {
            panel = panel
                .child(ui::meta(format!(
                    "{} · attempt {} · {:?}",
                    stage.node, stage.attempt, stage.phase
                )))
                .child(ui::meta(stage.detail.clone()));
        }
        let last_page = stream.sessions.len().saturating_sub(1) / ui::SESSIONS_PER_PAGE;
        let page = self
            .session_pages
            .get(&run)
            .copied()
            .unwrap_or(0)
            .min(last_page);
        panel = panel.child(ui::recorded_session_page(stream, page));
        panel = panel.child(
            div()
                .h_flex()
                .gap(px(8.0))
                .child(
                    gpui_kit::component::button::Button::new("task-newer-sessions")
                        .label("Newer sessions")
                        .disabled(page == 0)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.session_pages.insert(run, page.saturating_sub(1));
                            cx.notify();
                        })),
                )
                .child(
                    gpui_kit::component::button::Button::new("task-older-sessions")
                        .label("Older sessions")
                        .disabled(page >= last_page)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.session_pages.insert(run, page + 1);
                            cx.notify();
                        })),
                ),
        );
        panel
    }

    fn render_task_controls(
        &mut self,
        mut panel: Stateful<Div>,
        detail: &surge_core::work_item::WorkItemDetail,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        use surge_core::execution_recovery::ExecutionControlState as Control;
        use surge_core::id::WorkItemOperationId;
        use surge_core::work_item::WorkItemCommand;
        let record = &detail.item;
        let item = record.id;
        let version = record.version;
        let confirmed = self.task_detail_confirmed(item, cx);
        if let Some(run) = record.active_run.filter(|run| {
            self.state
                .read(cx)
                .pending_decisions()
                .iter()
                .any(|(id, _)| id == run)
        }) {
            let state = self.state.read(cx);
            let confirmed = state.tasks.fresh
                && state.daemon_state.facade().is_some()
                && state
                    .run_streams
                    .get(&run)
                    .is_some_and(|stream| stream.live);
            panel = panel.child(
                gpui_kit::component::button::Button::new("durable-task-review-decision")
                    .primary()
                    .label(if confirmed {
                        "Review decision"
                    } else {
                        "Review recorded decision · unconfirmed"
                    })
                    .accessibility_id("durable-task-review-decision")
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(FleetAction::OpenGate(run.to_string()));
                    })),
            );
        }
        if record.archived_at_ms.is_none() && record.active_run.is_some() {
            let paused = detail.control.as_ref().is_some_and(|control| {
                matches!(control.state, Control::Suspended | Control::Attention)
            });
            panel = panel.child(
                gpui_kit::component::button::Button::new("durable-task-control")
                    .label(if paused {
                        "Continue saved session"
                    } else {
                        "Suspend"
                    })
                    .disabled(self.task_busy || !confirmed)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let operation_id = WorkItemOperationId::new();
                        let command = if paused {
                            WorkItemCommand::Continue {
                                operation_id,
                                item,
                                expected_version: version,
                                new_session: false,
                            }
                        } else {
                            WorkItemCommand::Suspend {
                                operation_id,
                                item,
                                expected_version: version,
                            }
                        };
                        this.submit_task(command, cx);
                    })),
            );
            panel = panel.child(ui::meta("Editing and archiving active work requires the task interruption and preservation workflow."));
        } else if record.archived_at_ms.is_none() {
            panel = panel.child(
                gpui_kit::component::button::Button::new("durable-task-archive")
                    .label("Archive task")
                    .disabled(self.task_busy || !confirmed)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.submit_task(
                            WorkItemCommand::Archive {
                                operation_id: WorkItemOperationId::new(),
                                item,
                                expected_version: version,
                            },
                            cx,
                        )
                    })),
            );
        }
        if record.archived_at_ms.is_none() {
            panel = panel.child(self.render_task_draft(detail, window, cx));
        }
        panel
    }

    fn render_task_history(
        &mut self,
        mut panel: Stateful<Div>,
        detail: &surge_core::work_item::WorkItemDetail,
        history: Option<crate::work_items::TaskHistory>,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        use surge_core::id::WorkItemOperationId;
        use surge_core::work_item::WorkItemCommand;
        let record = &detail.item;
        let item = record.id;
        let version = record.version;
        if let Some(history) = history {
            panel = self.render_task_attempts(panel, &history.attempts.entries, cx);
            panel = panel.child(div().font_weight(FontWeight::BOLD).child("Discussion"));
            for message in history.discussion.entries {
                let proposal_id = message.sequence;
                panel = panel
                    .child(div().child(format!("{}: {}", message.actor, message.body)))
                    .when_some(message.proposal, |panel, proposal| {
                        let mut panel = panel
                            .child(
                                div().child(format!("Proposed requirements: {}", proposal.text())),
                            )
                            .children(
                                proposal
                                    .criteria()
                                    .iter()
                                    .map(|criterion| ui::meta(format!("• {criterion}"))),
                            );
                        if record.active_run.is_none() && record.archived_at_ms.is_none() {
                            let revision = record.accepted_revision;
                            panel = panel.child(
                                gpui_kit::component::button::Button::new(SharedString::from(
                                    format!("accept-proposal-{proposal_id}"),
                                ))
                                .label("Accept proposal")
                                .disabled(self.task_busy)
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.submit_task(
                                            WorkItemCommand::AcceptProposal {
                                                operation_id: WorkItemOperationId::new(),
                                                item,
                                                expected_version: version,
                                                expected_revision: revision,
                                                proposal: proposal_id,
                                            },
                                            cx,
                                        )
                                    },
                                )),
                            );
                        }
                        panel
                    });
            }
            panel = panel
                .child(
                    div()
                        .font_weight(FontWeight::BOLD)
                        .child("Accepted revisions"),
                )
                .children(history.revisions.entries.iter().map(|revision| {
                    ui::meta(format!(
                        "Revision {} · {}",
                        revision.revision, revision.actor
                    ))
                }));
            for (kind, label, present) in [
                (
                    crate::work_items::TaskHistoryKind::Attempts,
                    "Load more attempts",
                    history.attempts.next_cursor.is_some(),
                ),
                (
                    crate::work_items::TaskHistoryKind::Discussion,
                    "Load more discussion",
                    history.discussion.next_cursor.is_some(),
                ),
                (
                    crate::work_items::TaskHistoryKind::Revisions,
                    "Load more revisions",
                    history.revisions.next_cursor.is_some(),
                ),
            ] {
                if present {
                    panel = panel.child(
                        gpui_kit::component::button::Button::new(SharedString::from(label))
                            .label(label)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.load_more_history(item, kind, cx)
                            })),
                    );
                }
            }
        }
        panel
    }

    fn render_task_attempts(
        &self,
        mut panel: Stateful<Div>,
        attempts: &[surge_core::work_item::WorkItemAttempt],
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        panel = panel.child(div().font_weight(FontWeight::BOLD).child("Attempts"));
        for attempt in attempts {
            let run = attempt.run;
            panel = panel
                .child(
                    gpui_kit::component::button::Button::new(SharedString::from(format!(
                        "attempt-{run}"
                    )))
                    .label(format!(
                        "Attempt {} · {:?} · {:?}",
                        attempt.ordinal, attempt.state, attempt.accepted_revision_relation
                    ))
                    .on_click(
                        cx.listener(move |_, _, _, cx| cx.emit(FleetAction::OpenRun(Some(run)))),
                    ),
                )
                .child(ui::meta(format!(
                    "Revision {} · {} input / {} output tokens{}",
                    attempt.binding.revision,
                    attempt.input_tokens,
                    attempt.output_tokens,
                    if attempt.usage_unknown {
                        " · usage incomplete"
                    } else {
                        ""
                    }
                )))
                .when_some(attempt.diagnostic.clone(), |panel, diagnostic| {
                    panel.child(ui::meta(diagnostic))
                });
        }
        panel
    }

    pub(super) fn durable_status(
        &self,
        record: &surge_core::work_item::WorkItemRecord,
        cx: &Context<Self>,
    ) -> &'static str {
        let state = self.state.read(cx);
        let record = state
            .tasks
            .records
            .iter()
            .find(|current| current.id == record.id)
            .unwrap_or(record);
        let label = crate::work_items::task_status(
            state.tasks.details.get(&record.id),
            state.tasks.histories.get(&record.id),
            record,
        );
        if matches!(label, "Running" | "Status unavailable") {
            if !state.tasks.fresh || state.daemon_state.facade().is_none() {
                return "Run state is unconfirmed";
            }
            return record
                .active_run
                .and_then(|run| state.run_streams.get(&run))
                .map_or("Run state is unconfirmed", |stream| {
                    let display = stream.display();
                    if display == surge_core::run_display::RunDisplayState::Working && !stream.live
                    {
                        "Run state is unconfirmed"
                    } else {
                        display.label()
                    }
                });
        }
        label
    }

    pub(super) fn task_detail_confirmed(
        &self,
        item: surge_core::id::WorkItemId,
        cx: &Context<Self>,
    ) -> bool {
        let state = self.state.read(cx);
        state.tasks.fresh
            && state.daemon_state.facade().is_some()
            && state
                .tasks
                .records
                .iter()
                .find(|record| record.id == item)
                .zip(state.tasks.details.get(&item))
                .is_some_and(|(record, detail)| crate::work_items::detail_matches(record, detail))
    }

    pub(super) fn load_more_history(
        &mut self,
        item: surge_core::id::WorkItemId,
        kind: crate::work_items::TaskHistoryKind,
        cx: &mut Context<Self>,
    ) {
        use crate::work_items::TaskHistoryKind as Kind;
        use surge_core::work_item::{WorkItemCommand, WorkItemResult};
        let state = self.state.read(cx);
        let Some(facade) = state.daemon_state.facade() else {
            return;
        };
        let Some(after) = state
            .tasks
            .histories
            .get(&item)
            .and_then(|history| history.next(kind))
        else {
            return;
        };
        let scope = state.tasks.scope.clone();
        let observed_cursor = after.clone();
        let command = match kind {
            Kind::Attempts => WorkItemCommand::Attempts {
                item,
                after: Some(after),
                limit: 100,
            },
            Kind::Discussion => WorkItemCommand::Discussion {
                item,
                after: Some(after),
                limit: 100,
            },
            Kind::Revisions => WorkItemCommand::Revisions {
                item,
                after: Some(after),
                limit: 100,
            },
        };
        cx.spawn(async move |this, cx| {
            let result = facade.work_item(command).await;
            let _ = this.update(cx, |this, cx| {
                if this.selected_item != Some(item) || this.state.read(cx).tasks.scope != scope {
                    return;
                }
                match result {
                    Ok(result) => this.state.update(cx, |state, cx| {
                        let Some(history) = state.tasks.histories.get_mut(&item) else {
                            return;
                        };
                        if history.next(kind).as_deref() != Some(observed_cursor.as_str()) {
                            return;
                        }
                        match (kind, result) {
                            (Kind::Attempts, WorkItemResult::Attempts(page)) => {
                                history.attempts.entries.extend(page.entries);
                                history.attempts.next_cursor = page.next_cursor;
                            },
                            (Kind::Discussion, WorkItemResult::Discussion(page)) => {
                                history.discussion.entries.extend(page.entries);
                                history.discussion.next_cursor = page.next_cursor;
                            },
                            (Kind::Revisions, WorkItemResult::Revisions(page)) => {
                                history.revisions.entries.extend(page.entries);
                                history.revisions.next_cursor = page.next_cursor;
                            },
                            _ => {
                                state.tasks.error = Some("Unexpected task history response".into());
                            },
                        }
                        cx.notify();
                    }),
                    Err(error) => {
                        this.task_error = Some(error.to_string());
                        cx.notify();
                    },
                }
            });
        })
        .detach();
    }

    pub(super) fn ensure_task_draft(
        &mut self,
        detail: &surge_core::work_item::WorkItemDetail,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let item = detail.item.id;
        self.task_drafts.entry(item).or_insert_with(|| {
            let comment = cx.new(|cx| {
                TextareaState::new(window, cx)
                    .rows(3)
                    .placeholder("Discuss this task…")
            });
            let requirements = cx.new(|cx| TextareaState::new(window, cx).rows(5));
            requirements.update(cx, |input, cx| {
                input.set_value(detail.revision.requirements.text(), window, cx)
            });
            let criteria = cx.new(|cx| {
                TextareaState::new(window, cx)
                    .rows(4)
                    .placeholder("One acceptance criterion per line")
            });
            criteria.update(cx, |input, cx| {
                input.set_value(
                    detail.revision.requirements.criteria().join("\n"),
                    window,
                    cx,
                )
            });
            let flow = cx.new(|cx| {
                TextareaState::new(window, cx)
                    .rows(6)
                    .placeholder("Paste the reviewed workflow TOML to start this task")
            });
            TaskDraft {
                comment,
                requirements,
                criteria,
                flow,
                version: detail.item.version,
                revision: detail.revision.revision,
                accepted_hash: detail.revision.hash,
            }
        });
    }

    pub(super) fn render_task_draft(
        &mut self,
        detail: &surge_core::work_item::WorkItemDetail,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        if let Some(submitted) = self.task_clear_comment.remove(&detail.item.id)
            && let Some(draft) = self.task_drafts.get(&detail.item.id)
            && draft.comment.read(cx).value().as_ref() == submitted
        {
            draft
                .comment
                .update(cx, |input, cx| input.set_value("", window, cx));
        }
        let item = detail.item.id;
        self.ensure_task_draft(detail, window, cx);
        let draft = &self.task_drafts[&item];
        let comment = draft.comment.clone();
        let requirements = draft.requirements.clone();
        let criteria = draft.criteria.clone();
        let version = draft.version;
        let revision = draft.revision;
        let flow = draft.flow.clone();
        let can_edit = detail.item.active_run.is_none();
        div()
            .v_flex()
            .gap(px(8.0))
            .child(
                div()
                    .font_weight(FontWeight::BOLD)
                    .child("Discuss or propose changes"),
            )
            .when_some(self.task_feedback.get(&item).cloned(), |panel, feedback| {
                panel.child(ui::meta(feedback).id("task-accepted-feedback"))
            })
            .child(ui::meta("Discussion"))
            .child(
                Textarea::new(&comment)
                    .h(px(90.0))
                    .accessibility_id("task-discussion-draft"),
            )
            .child(ui::meta("Requirements draft"))
            .child(
                Textarea::new(&requirements)
                    .h(px(130.0))
                    .accessibility_id("task-requirements-draft"),
            )
            .child(ui::meta("Acceptance criteria · one per line"))
            .child(
                Textarea::new(&criteria)
                    .h(px(110.0))
                    .accessibility_id("task-criteria-draft"),
            )
            .child(ui::meta(format!("Draft based on revision {revision}")))
            .child(
                gpui_kit::component::button::Button::new("task-reload-requirements")
                    .label("Replace draft with accepted requirements")
                    .disabled(self.task_busy || self.task_submissions.contains_key(&item))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.reload_task_draft(item, window, cx)
                    })),
            )
            .child(
                gpui_kit::component::button::Button::new("task-use-current-version")
                    .label("Use current task version for this draft")
                    .disabled(self.task_busy || self.task_submissions.contains_key(&item))
                    .on_click(cx.listener(move |this, _, _, cx| this.rebase_task_draft(item, cx))),
            )
            .child(
                gpui_kit::component::button::Button::new("task-post-discussion")
                    .label("Post discussion")
                    .disabled(self.task_busy)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.submit_task_draft(item, false, false, cx)
                    })),
            )
            .child(
                gpui_kit::component::button::Button::new("task-propose-edit")
                    .label("Propose requirements")
                    .disabled(self.task_busy)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.submit_task_draft(item, true, false, cx)
                    })),
            )
            .when(can_edit, |panel| {
                panel.child(
                    gpui_kit::component::button::Button::new("task-save-requirements")
                        .label("Accept edited requirements")
                        .disabled(self.task_busy)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.submit_task_draft(item, true, true, cx)
                        })),
                )
            })
            .when(can_edit, |panel| {
                panel
                    .child(
                        Textarea::new(&flow)
                            .h(px(150.0))
                            .accessibility_id("task-workflow-draft"),
                    )
                    .child(
                        gpui_kit::component::button::Button::new("task-start-workflow")
                            .label("Start reviewed workflow")
                            .disabled(self.task_busy)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.start_task_workflow(item, cx)
                            })),
                    )
            })
            .child(ui::meta(format!("Observed task version {version}")))
    }

    pub(super) fn reload_task_draft(
        &mut self,
        item: surge_core::id::WorkItemId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.task_busy || self.task_submissions.contains_key(&item) {
            return;
        }
        let Some(detail) = self.state.read(cx).tasks.details.get(&item).cloned() else {
            return;
        };
        let Some(draft) = self.task_drafts.get_mut(&item) else {
            return;
        };
        draft.requirements.update(cx, |input, cx| {
            input.set_value(detail.revision.requirements.text(), window, cx)
        });
        draft.criteria.update(cx, |input, cx| {
            input.set_value(
                detail.revision.requirements.criteria().join("\n"),
                window,
                cx,
            )
        });
        draft.version = detail.item.version;
        draft.revision = detail.revision.revision;
        draft.accepted_hash = detail.revision.hash;
        cx.notify();
    }

    pub(super) fn rebase_task_draft(
        &mut self,
        item: surge_core::id::WorkItemId,
        cx: &mut Context<Self>,
    ) {
        if self.task_busy || self.task_submissions.contains_key(&item) {
            return;
        }
        let Some(detail) = self.state.read(cx).tasks.details.get(&item).cloned() else {
            return;
        };
        let Some(draft) = self.task_drafts.get_mut(&item) else {
            return;
        };
        draft.version = detail.item.version;
        draft.revision = detail.revision.revision;
        draft.accepted_hash = detail.revision.hash;
        cx.notify();
    }

    pub(super) fn start_task_workflow(
        &mut self,
        item: surge_core::id::WorkItemId,
        cx: &mut Context<Self>,
    ) {
        let Some(draft) = self.task_drafts.get(&item) else {
            return;
        };
        match crate::work_items::reviewed_start_command(
            item,
            draft.version,
            draft.flow.read(cx).value().as_ref(),
        ) {
            Ok(command) => self.submit_task(command, cx),
            Err(error) => {
                self.task_error = Some(error);
                cx.notify();
            },
        }
    }

    pub(super) fn submit_task_draft(
        &mut self,
        item: surge_core::id::WorkItemId,
        proposal: bool,
        edit: bool,
        cx: &mut Context<Self>,
    ) {
        use surge_core::work_item::{WorkItemCommand, WorkItemRequirements};
        if self.selected_item != Some(item) {
            return;
        }
        let Some(draft) = self.task_drafts.get(&item) else {
            return;
        };
        let body = draft.comment.read(cx).value().to_string();
        let requirements = if proposal {
            let criteria = draft
                .criteria
                .read(cx)
                .value()
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(str::to_owned)
                .collect();
            match WorkItemRequirements::new(
                draft.requirements.read(cx).value().to_string(),
                criteria,
            ) {
                Ok(requirements) => Some(requirements),
                Err(error) => {
                    self.task_error = Some(error);
                    cx.notify();
                    return;
                },
            }
        } else {
            None
        };
        let operation_id = surge_core::id::WorkItemOperationId::new();
        let command = if edit {
            let Some(requirements) = requirements else {
                return;
            };
            WorkItemCommand::Edit {
                operation_id,
                item,
                expected_version: draft.version,
                expected_revision: draft.revision,
                requirements,
            }
        } else {
            if body.trim().is_empty() {
                self.task_error = Some("Write a discussion message before posting.".into());
                cx.notify();
                return;
            }
            WorkItemCommand::Discuss {
                operation_id,
                item,
                expected_version: draft.version,
                body,
                proposal: requirements,
            }
        };
        self.submit_task(command, cx);
    }

    pub(super) fn refresh_durable_tasks(&mut self, cx: &mut Context<Self>) {
        self.load_durable_tasks(false, cx);
    }

    pub(super) fn load_durable_tasks(&mut self, append: bool, cx: &mut Context<Self>) {
        let state = self.state.read(cx);
        let Some(facade) = state.daemon_state.facade() else {
            return;
        };
        let Some(project) = state.project_path.as_ref() else {
            return;
        };
        let Some(repository) = crate::project::git_common_dir(project) else {
            return;
        };
        let after = if append {
            state.tasks.next_cursor.clone()
        } else {
            None
        };
        let scope = self.state.update(cx, |state, cx| {
            let scope = state.tasks.begin(repository.clone());
            cx.notify();
            scope
        });
        cx.spawn(async move |this, cx| {
            let result = crate::work_items::list_project_tasks(&facade, &repository, after).await;
            let _ = this.update(cx, |this, cx| {
                this.state.update(cx, |state, cx| {
                    match result {
                        Ok(page) => state.tasks.apply_page(&scope, page, append),
                        Err(error) => state.tasks.fail(&scope, error),
                    }
                    cx.notify();
                });
                let items = this
                    .state
                    .read(cx)
                    .tasks
                    .records
                    .iter()
                    .take(40)
                    .map(|record| record.id)
                    .collect();
                this.refresh_task_details(facade.clone(), scope.clone(), items, cx);
            });
        })
        .detach();
    }

    pub(super) fn refresh_task_details(
        &mut self,
        facade: std::sync::Arc<surge_orchestrator::engine::daemon_facade::DaemonEngineFacade>,
        scope: crate::work_items::TaskRequestScope,
        items: Vec<surge_core::id::WorkItemId>,
        cx: &mut Context<Self>,
    ) {
        let items: Vec<_> = self.state.update(cx, |state, _| {
            items
                .into_iter()
                .map(|item| (item, state.tasks.begin_detail(item)))
                .collect()
        });
        cx.spawn(async move |this, cx| {
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(8);
            for (item, request) in items {
                let result =
                    tokio::time::timeout_at(deadline, crate::work_items::load_task(&facade, item))
                        .await;
                let Ok(result) = result else {
                    break;
                };
                let alive = this.update(cx, |this, cx| {
                    this.state.update(cx, |state, cx| {
                        if !state.tasks.accepts(&scope) {
                            return;
                        }
                        if let Ok((detail, history)) = result {
                            if detail.item.workspace.repository != scope.repository {
                                return;
                            }
                            state.tasks.apply_detail(request, detail, history);
                            cx.notify();
                        }
                    });
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    pub(super) fn select_item(&mut self, item: surge_core::id::WorkItemId, cx: &mut Context<Self>) {
        self.selected_item = Some(item);
        self.task_error = None;
        self.reload_item(item, cx);
    }

    pub(super) fn reload_item(&mut self, item: surge_core::id::WorkItemId, cx: &mut Context<Self>) {
        let state = self.state.read(cx);
        let Some(facade) = state.daemon_state.facade() else {
            cx.notify();
            return;
        };
        let scope = state.tasks.scope.clone();
        let request = self
            .state
            .update(cx, |state, _| state.tasks.begin_detail(item));
        cx.spawn(async move |this, cx| {
            let result = crate::work_items::load_task(&facade, item).await;
            let _ = this.update(cx, |this, cx| {
                if this.selected_item != Some(item) || this.state.read(cx).tasks.scope != scope {
                    return;
                }
                match result {
                    Ok((detail, history)) => this.state.update(cx, |state, cx| {
                        state.tasks.apply_detail(request, detail, history);
                        cx.notify();
                    }),
                    Err(error) => {
                        this.task_error = Some(error);
                        cx.notify();
                    },
                }
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn submit_task(
        &mut self,
        command: surge_core::work_item::WorkItemCommand,
        cx: &mut Context<Self>,
    ) {
        if self.task_busy {
            return;
        }
        let Some(item) = self.selected_item else {
            return;
        };
        let target = crate::work_items::mutation_target(&command);
        let current_version = self
            .state
            .read(cx)
            .tasks
            .records
            .iter()
            .find(|record| record.id == item)
            .map(|record| record.version);
        if target.is_none_or(|(target, version)| target != item || Some(version) != current_version)
        {
            self.task_error = Some("This operation belongs to an older task or version. Review the current task version; your draft is retained.".into());
            cx.notify();
            return;
        }
        if !self.task_detail_confirmed(item, cx) {
            self.task_error = Some("Refresh this task before submitting a new operation; its current state is unconfirmed.".into());
            cx.notify();
            return;
        }
        if self.task_submissions.contains_key(&item) {
            self.task_error =
                Some("Resolve the pending task operation before submitting another.".into());
            cx.notify();
            return;
        }
        self.task_submissions
            .insert(item, crate::work_items::TaskSubmission::new(command));
        self.retry_task(cx);
    }

    pub(super) fn dismiss_rejected_task_operation(&mut self, cx: &mut Context<Self>) {
        let Some(item) = self.selected_item else {
            return;
        };
        if self.task_busy || !self.task_rejections.remove(&item) {
            return;
        }
        self.task_submissions.remove(&item);
        self.task_error = None;
        self.reload_item(item, cx);
        cx.notify();
    }

    pub(super) fn retry_task(&mut self, cx: &mut Context<Self>) {
        if self.task_busy {
            return;
        }
        let Some(item) = self.selected_item else {
            return;
        };
        let Some(submission) = self.task_submissions.get(&item).cloned() else {
            return;
        };
        let Some(facade) = self.state.read(cx).daemon_state.facade() else {
            self.task_error = Some("Connect to the daemon to retry this task operation".into());
            cx.notify();
            return;
        };
        let scope = self.state.read(cx).tasks.scope.clone();
        // A fresh dispatch can be admitted even after an earlier definitive refusal.
        // Its uncertain reply must never inherit permission to dismiss the operation.
        self.task_rejections.remove(&item);
        self.task_busy = true;
        self.task_error = None;
        cx.spawn(async move |this, cx| {
            let result = facade.work_item(submission.retry()).await;
            let _ = this.update(cx, |this, cx| {
                this.task_busy = false;
                if result.is_ok() {
                    if let Ok(result) = &result {
                        this.acknowledge_task(item, &submission.retry(), result, cx);
                    }
                    this.task_submissions.remove(&item);
                    this.task_rejections.remove(&item);
                }
                if matches!(
                    &result,
                    Err(surge_orchestrator::engine::error::EngineError::WorkItemRejected(_))
                ) {
                    this.task_rejections.insert(item);
                }
                if this.selected_item != Some(item) || this.state.read(cx).tasks.scope != scope {
                    cx.notify();
                    return;
                }
                match result {
                    Ok(_) => this.refresh_durable_tasks(cx),
                    Err(error) => this.task_error = Some(error.to_string()),
                }
                this.reload_item(item, cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn acknowledge_task(
        &mut self,
        item: surge_core::id::WorkItemId,
        command: &surge_core::work_item::WorkItemCommand,
        result: &surge_core::work_item::WorkItemResult,
        cx: &Context<Self>,
    ) {
        use surge_core::work_item::{WorkItemCommand, WorkItemResult};
        let feedback = match command {
            WorkItemCommand::Discuss {
                body,
                expected_version,
                ..
            } => {
                self.task_clear_comment.insert(item, body.clone());
                if let WorkItemResult::Detail(detail) = result
                    && let Some(draft) = self.task_drafts.get_mut(&item)
                    && draft.version == *expected_version
                    && draft.revision == detail.revision.revision
                    && draft.accepted_hash == detail.revision.hash
                {
                    draft.version = detail.item.version;
                }
                "Discussion accepted".to_owned()
            },
            WorkItemCommand::Edit {
                expected_version,
                expected_revision,
                requirements,
                ..
            } => {
                if let WorkItemResult::Detail(detail) = result {
                    if &detail.revision.requirements == requirements {
                        if let Some(draft) = self.task_drafts.get_mut(&item) {
                            let text = draft.requirements.read(cx).value();
                            let criteria = draft.criteria.read(cx).value();
                            let criteria: Vec<_> = criteria
                                .lines()
                                .map(str::trim)
                                .filter(|line| !line.is_empty())
                                .collect();
                            if draft.version == *expected_version
                                && draft.revision == *expected_revision
                                && text.as_ref() == requirements.text()
                                && criteria
                                    == requirements
                                        .criteria()
                                        .iter()
                                        .map(String::as_str)
                                        .collect::<Vec<_>>()
                            {
                                draft.version = detail.item.version;
                                draft.revision = detail.revision.revision;
                                draft.accepted_hash = detail.revision.hash;
                            }
                        }
                        format!(
                            "Requirements accepted · revision {}",
                            detail.revision.revision
                        )
                    } else {
                        "Task operation accepted; refresh its requirements".into()
                    }
                } else {
                    "Task operation accepted".into()
                }
            },
            _ => "Task operation accepted".into(),
        };
        self.task_feedback.insert(item, feedback);
    }
}
