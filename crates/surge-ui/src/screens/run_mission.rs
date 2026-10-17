//! Overview tab: the mission level by level, each with its own flow.
//!
//! Plan (your approvals) → milestones (their own steps around the task
//! loop) → tasks → recorded subtasks (each with its own steps) → final steps → result.
//! Folded by [`crate::mission::fold`] from the recorded events; reloaded
//! when the run's event sequence moves.

use std::collections::BTreeSet;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::Button;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{Icon, IconName, StyledExt};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use surge_core::roadmap::RoadmapStatus;
use surge_core::roadmap::{MilestoneId, RoadmapTaskId};
use surge_core::{BootstrapStage, RunId};

use crate::flow_diagram::Lane;
use crate::mission::{self, MilestoneView, Mission, PlanState, RunEnd, Step, StepState, TaskView};
use crate::theme::{self, Semantic};
use crate::ui;

#[path = "run_mission/history.rs"]
mod history;
pub(super) use history::MissionRequest;
pub(super) use history::operation_message;
use history::{ImplementationHistory, LoadOutcome};
use surge_core::bootstrap_operation::{BootstrapOperationStatus, BootstrapState};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct MissionRefresh {
    operation: Option<(RunId, u64)>,
    streams: Vec<(RunId, Option<u64>, Option<u64>)>,
    observations: Vec<(RunId, bool)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LoadingState {
    Reading,
    Ready,
    Changed,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct StepOccurrence {
    scope: Vec<usize>,
    index: usize,
}

#[cfg(test)]
type ControlledLoads =
    std::rc::Rc<std::cell::RefCell<Vec<tokio::sync::oneshot::Sender<Result<LoadOutcome, String>>>>>;

#[cfg(test)]
struct PendingLoad(std::rc::Rc<std::cell::Cell<usize>>);

#[cfg(test)]
impl Drop for PendingLoad {
    fn drop(&mut self) {
        self.0.set(self.0.get() - 1);
    }
}

pub(super) struct MissionPanel {
    #[cfg(test)]
    active_loads: std::rc::Rc<std::cell::Cell<usize>>,
    #[cfg(test)]
    controlled_loads: Option<ControlledLoads>,
    home: Option<std::path::PathBuf>,
    request: MissionRequest,
    mission: Option<Mission>,
    implementation: Option<ImplementationHistory>,
    operation: Option<BootstrapOperationStatus>,
    loading: LoadingState,
    error: Option<String>,
    generation: u64,
    /// Operation revision and both run observations the last request reflects.
    loaded_refresh: Option<MissionRefresh>,
    scope: Vec<usize>,
    requested_task: Option<(MilestoneId, RoadmapTaskId)>,
    expanded_steps: BTreeSet<StepOccurrence>,
}

impl MissionPanel {
    pub(super) fn new(
        home: Option<std::path::PathBuf>,
        request: MissionRequest,
        requested_task: Option<(MilestoneId, RoadmapTaskId)>,
        #[cfg(test)] active_loads: std::rc::Rc<std::cell::Cell<usize>>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut panel = Self {
            #[cfg(test)]
            controlled_loads: None,
            #[cfg(test)]
            active_loads,
            home,
            request,
            mission: None,
            implementation: None,
            operation: None,
            loading: LoadingState::Reading,
            error: None,
            generation: 0,
            loaded_refresh: None,
            scope: Vec::new(),
            requested_task,
            expanded_steps: Default::default(),
        };
        panel.reload(cx);
        panel
    }

    pub(super) fn request(&self) -> &MissionRequest {
        &self.request
    }

    /// Read-only exact ownership learned by the existing cold history lookup.
    pub(super) fn operation_for(&self, run: RunId) -> Option<&BootstrapOperationStatus> {
        self.operation
            .as_ref()
            .filter(|status| [status.planning_run, status.implementation_run].contains(&run))
    }

    /// Both reserved streams and operation transitions can change this composition.
    pub(super) fn refresh_key(&self, state: &crate::app_state::AppState) -> MissionRefresh {
        let operation = state
            .bootstrap_operations
            .values()
            .find(|status| {
                [status.planning_run, status.implementation_run].contains(&self.request.selected)
            })
            .or(self.operation.as_ref());
        let ids = operation.map_or_else(
            || vec![self.request.selected],
            |status| vec![status.planning_run, status.implementation_run],
        );
        let observations = ids
            .iter()
            .map(|run| {
                (
                    *run,
                    state
                        .run_streams
                        .get(run)
                        .is_some_and(|stream| stream.observation_available_for(*run)),
                )
            })
            .collect();
        MissionRefresh {
            observations,
            operation: operation.map(|status| (status.operation_id, status.revision)),
            streams: ids
                .into_iter()
                .map(|run| {
                    (
                        run,
                        state
                            .runs
                            .iter()
                            .find(|item| item.run_id == run)
                            .and_then(|item| item.last_event_seq),
                        state.run_streams.get(&run).map(|stream| stream.last_seq),
                    )
                })
                .collect(),
        }
    }

    pub(super) fn sync(&mut self, refresh: MissionRefresh, cx: &mut Context<Self>) {
        let history_changed = self.loaded_refresh.as_ref().is_none_or(|loaded| {
            loaded.operation != refresh.operation || loaded.streams != refresh.streams
        });
        let observation_changed = self
            .loaded_refresh
            .as_ref()
            .is_none_or(|loaded| loaded.observations != refresh.observations);
        self.loaded_refresh = Some(refresh);
        if history_changed {
            self.reload(cx);
        } else if observation_changed {
            cx.notify();
        }
    }

    fn current_observation(&self) -> bool {
        let run = self
            .operation
            .as_ref()
            .map_or(self.request.selected, |status| {
                if matches!(self.implementation, Some(ImplementationHistory::Recorded)) {
                    status.implementation_run
                } else {
                    status.planning_run
                }
            });
        let Some(mission) = &self.mission else {
            return false;
        };
        self.observes_run(run, &mission.recovery, mission.end.is_some())
    }

    fn observes_run(
        &self,
        run: RunId,
        recovery: &surge_core::recovery_observation::RecoveryObservation,
        terminal: bool,
    ) -> bool {
        if terminal || self.loading != LoadingState::Ready || self.error.is_some() {
            return false;
        }
        let Some(refresh) = &self.loaded_refresh else {
            return false;
        };
        let surge_core::recovery_observation::RecoveryObservation::CompletePrefix {
            through, ..
        } = *recovery
        else {
            return false;
        };
        refresh.observations.contains(&(run, true))
            && refresh.streams.iter().any(|(id, registry, stream)| {
                *id == run && *stream == Some(through) && registry.is_none_or(|seq| seq <= through)
            })
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        self.reload_once(false, cx);
    }

    fn reload_once(&mut self, retried: bool, cx: &mut Context<Self>) {
        self.generation += 1;
        self.loading = LoadingState::Reading;
        let generation = self.generation;
        let request = self.request.clone();
        let home = self.home.clone();
        #[cfg(test)]
        let controlled = self.controlled_loads.clone();
        #[cfg(test)]
        let pending = {
            self.active_loads.set(self.active_loads.get() + 1);
            PendingLoad(self.active_loads.clone())
        };
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            #[cfg(test)]
            let _pending = pending;
            #[cfg(not(test))]
            let result = history::load(home, request).await;
            #[cfg(test)]
            let result = if let Some(controlled) = controlled {
                let (sender, receiver) = tokio::sync::oneshot::channel();
                controlled.borrow_mut().push(sender);
                receiver
                    .await
                    .map_err(|error| error.to_string())
                    .and_then(std::convert::identity)
            } else {
                history::load(home, request).await
            };
            cx.update(|cx| {
                let _ = this.update(cx, |panel, cx| {
                    if panel.generation != generation {
                        return;
                    }
                    match result {
                        Ok(LoadOutcome::Loaded(snapshot)) => {
                            let m = snapshot.mission;
                            panel.implementation = Some(snapshot.implementation);
                            panel.operation = snapshot.operation;
                            panel.loading = LoadingState::Ready;
                            if let Some((milestone, task)) = &panel.requested_task
                                && let Some(path) = task_scope(&m, milestone, task)
                            {
                                panel.scope = path;
                                panel.requested_task = None;
                            }
                            panel.mission = Some(m);
                            panel.error = None;
                        },
                        Ok(LoadOutcome::Stale) if !retried => panel.reload_once(true, cx),
                        Ok(LoadOutcome::Stale) => panel.loading = LoadingState::Changed,
                        Err(e) => {
                            panel.error = Some(e);
                            panel.loading = LoadingState::Ready;
                        },
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }
}

fn lane_color(lane: Lane) -> Hsla {
    match lane {
        Lane::Plan => Semantic::Plan.color(),
        Lane::Approve => Semantic::You.color(),
        Lane::Build => Semantic::Agent.color(),
        Lane::Verify => Semantic::Verified.color(),
        Lane::Result => Semantic::External.color(),
    }
}

/// Final colors actually consumed by the capsule border, tint and live cue.
struct StepChipStyle {
    border: Hsla,
    background: Hsla,
    activity: Option<Hsla>,
    icon: Option<IconName>,
    outcome: Hsla,
}

fn step_chip_style(
    step: &Step,
    available: bool,
    attribution: Option<mission::PlanningAttribution>,
) -> StepChipStyle {
    let unavailable = attribution == Some(mission::PlanningAttribution::Unavailable);
    let role = if unavailable {
        Semantic::External.color()
    } else {
        lane_color(step.lane)
    };
    let (icon, tone): (Option<IconName>, Hsla) = match &step.state {
        StepState::Done(_) => (Some(IconName::Check), theme::success()),
        StepState::Failed(_) => (Some(IconName::Close), theme::error()),
        StepState::WaitingOnYou | StepState::RecoveryRequired => (None, theme::warning()),
        StepState::Working => (None, theme::accent()),
        StepState::Unknown | StepState::Unfinished => (Some(IconName::Minus), theme::slate()),
    };
    let live = available && matches!(step.state, StepState::Working | StepState::WaitingOnYou);
    let activity = if unavailable {
        Semantic::External.color()
    } else {
        tone
    };
    StepChipStyle {
        border: if live {
            theme::stroke(activity)
        } else {
            theme::stroke(role).opacity(0.5)
        },
        background: if live {
            theme::tint(activity)
        } else {
            theme::tint(role)
        },
        activity: live.then_some(activity),
        icon,
        outcome: tone,
    }
}

/// A step as a small capsule: its role color, its label, and how it went.
fn step_chip(
    id: String,
    step: &Step,
    available: bool,
    attribution: Option<mission::PlanningAttribution>,
) -> impl IntoElement {
    let style = step_chip_style(step, available, attribution);
    let tip = match &step.state {
        StepState::Done(outcome) => step
            .summary
            .clone()
            .unwrap_or_else(|| format!("finished · {outcome}")),
        StepState::Failed(reason) => format!("failed · {reason}"),
        StepState::WaitingOnYou => "waiting for your answer".into(),
        StepState::RecoveryRequired => "paused · recovery requires review".into(),
        StepState::Working => if available {
            "working now"
        } else {
            "last recorded progress"
        }
        .into(),
        StepState::Unknown => "continuation unknown after a journal gap".into(),
        StepState::Unfinished => "the run ended before this step reported".into(),
    };
    let tip = if step.attempts > 1 {
        format!("attempt {} · {tip}", step.attempts)
    } else {
        tip
    };
    div()
        .id(SharedString::from(id))
        .test_support()
        .role(Role::Group)
        .aria_label(format!("{}: {tip}", step.label))
        .flex_none()
        .h_flex()
        .gap(px(5.0))
        .items_center()
        .h(px(22.0))
        .px(px(8.0))
        .rounded(px(999.0))
        .border_1()
        .border_color(style.border)
        .bg(style.background)
        .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
        .children(style.activity.map(ui::live_dot))
        .child(
            div()
                .text_size(px(10.5))
                .text_color(theme::text_primary())
                .child(step.label.clone()),
        )
        .children(
            style
                .icon
                .map(|i| Icon::new(i).size(px(10.0)).text_color(style.outcome)),
        )
}

fn step_row(prefix: &str, steps: &[Step], available: bool) -> Div {
    let mut row = div().h_flex().flex_wrap().gap(px(4.0)).items_center();
    for (i, step) in steps.iter().enumerate() {
        if i > 0 {
            row = row.child(div().w(px(6.0)).h(px(1.0)).bg(theme::graph_line()));
        }
        row = row.child(step_chip(format!("{prefix}-{i}"), step, available, None));
    }
    row
}

fn plan_label(stage: BootstrapStage) -> &'static str {
    match stage {
        BootstrapStage::Description => "Description",
        BootstrapStage::Roadmap => "Roadmap",
        BootstrapStage::Flow => "Flow",
    }
}

impl MissionPanel {
    fn render_plan(&self, mission: &Mission) -> Div {
        let mut row = div().h_flex().gap(px(6.0)).items_center();
        for (i, step) in mission.plan.iter().enumerate() {
            if i > 0 {
                row = row.child(
                    Icon::new(Lucide::ArrowRight)
                        .size(px(11.0))
                        .text_color(theme::graph_line()),
                );
            }
            let (note, tone, icon) = match step.state {
                PlanState::NotStarted => ("No review recorded".to_string(), theme::slate(), None),
                PlanState::Drafting { edits } => (
                    if edits > 0 {
                        format!("redrafting · {edits} edit")
                    } else {
                        "drafting".into()
                    },
                    theme::accent(),
                    None,
                ),
                PlanState::AwaitingYou { .. } => ("waiting for you".into(), theme::warning(), None),
                PlanState::Approved { edits } => (
                    if edits > 0 {
                        format!(
                            "approved after {edits} edit{}",
                            if edits == 1 { "" } else { "s" }
                        )
                    } else {
                        "approved by you".into()
                    },
                    theme::success(),
                    Some(IconName::Check),
                ),
                PlanState::Rejected => ("rejected".into(), theme::error(), Some(IconName::Close)),
            };
            row = row.child(
                ui::node_card(Semantic::Plan.color())
                    .id(SharedString::from(format!("planning-review-{i}")))
                    .test_support()
                    .role(Role::Group)
                    .aria_label(format!("{}: {note}", plan_label(step.stage)))
                    .flex_1()
                    .min_w(px(0.0))
                    .v_flex()
                    .gap(px(3.0))
                    .px(px(12.0))
                    .py(px(8.0))
                    .child(
                        div()
                            .h_flex()
                            .gap(px(6.0))
                            .items_center()
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(theme::text_primary())
                                    .child(plan_label(step.stage)),
                            )
                            .children(icon.map(|i| Icon::new(i).size(px(11.0)).text_color(tone))),
                    )
                    .child(div().text_size(px(10.5)).text_color(tone).child(note)),
            );
        }
        div()
            .v_flex()
            .gap(px(8.0))
            .child(ui::section_label("Plan"))
            .child(row)
    }

    fn render_planning(&self, planning: &mission::PlanningExecution) -> Div {
        let available = self.observes_run(planning.run, &planning.recovery, planning.end.is_some());
        let coverage = if planning.recovery.is_complete() {
            "complete loaded prefix"
        } else {
            "incomplete history"
        };
        let facts = format!(
            "Planning run {} · {coverage} · through event {}",
            planning.run,
            planning
                .through
                .map_or_else(|| "unknown".into(), |seq| seq.to_string())
        );
        let mut group = div()
            .v_flex()
            .gap(px(8.0))
            .child(ui::section_label("Recorded planning steps"))
            .child(
                div()
                    .id("planning-history-facts")
                    .test_support()
                    .role(Role::Group)
                    .aria_label(facts.clone())
                    .child(ui::meta(facts)),
            );
        if let Some(end) = &planning.end {
            let text = match end {
                RunEnd::Completed => "Planning run: Completed".into(),
                RunEnd::Failed(error) => format!("Planning run: Failed · {error}"),
                RunEnd::Aborted(reason) => format!("Planning run: Stopped · {reason}"),
            };
            group = group.child(
                div()
                    .id("planning-terminal")
                    .test_support()
                    .role(Role::Label)
                    .aria_label(text.clone())
                    .child(ui::meta(text)),
            );
        }
        for caveat in &planning.caveats {
            group = group.child(ui::meta(caveat.clone()));
        }
        if let Some(problem) = mission::recovery_problem(&planning.recovery, false) {
            group = group.child(
                div()
                    .id("planning-recovery")
                    .test_support()
                    .role(Role::Group)
                    .aria_label(problem.clone())
                    .child(ui::meta(problem)),
            );
        }
        for occurrence in &planning.steps {
            let id = format!("planning-step-{}", occurrence.entered);
            let mut row = div().v_flex().gap(px(4.0)).child(step_chip(
                id.clone(),
                &occurrence.step,
                available,
                Some(occurrence.attribution),
            ));
            if let Some(summary) = &occurrence.step.summary {
                row = row.child(ui::meta(summary.clone()));
            }
            for (index, participant) in occurrence.sessions.iter().enumerate() {
                row = row.child(process_evidence(
                    format!("{id}-session-{index}"),
                    format!(
                        "Performed by {} · Runtime: {} · Session: {}",
                        participant.profile,
                        participant.runtime.as_deref().unwrap_or("not recorded"),
                        participant.session
                    ),
                ));
            }
            group = group.child(row);
        }
        group
    }

    fn render_subtask(&self, task: &TaskView, path: &str) -> AnyElement {
        let (state, color) = match &task.iteration {
            mission::IterationState::Planned => ("Planned".to_string(), theme::text_muted()),
            mission::IterationState::Discovered => {
                ("Discovered · not dispatched".into(), theme::text_muted())
            },
            mission::IterationState::Queued => ("Queued after split".into(), theme::text_muted()),
            mission::IterationState::Replaced => {
                ("Split into smaller tasks".into(), theme::text_muted())
            },
            mission::IterationState::Running => (
                if self.current_observation() {
                    "Running"
                } else {
                    "Last recorded in progress"
                }
                .to_string(),
                theme::accent(),
            ),
            mission::IterationState::Finished(outcome) => {
                (format!("Process finished · {outcome}"), theme::text_muted())
            },
            mission::IterationState::Unknown => {
                ("Continuation unknown".to_string(), theme::warning())
            },
            mission::IterationState::Unfinished => {
                ("No completion recorded".to_string(), theme::warning())
            },
        };
        let ledger = match task.status {
            RoadmapStatus::Pending => "Task status: pending",
            RoadmapStatus::Running => "Task status: running",
            RoadmapStatus::ReadyForVerification => "Task status: ready for verification",
            RoadmapStatus::Completed => "Task status: completed",
            RoadmapStatus::Failed => "Task status: failed",
            RoadmapStatus::FailedVerification => "Task status: verification failed",
            RoadmapStatus::Paused => "Task status: paused",
            RoadmapStatus::Skipped => "Task status: skipped",
        };
        div()
            .id(SharedString::from(path.to_string()))
            .test_support()
            .role(Role::Group)
            .aria_label(format!("{}: {state}", task.title))
            .v_flex()
            .gap(px(8.0))
            .p(px(12.0))
            .rounded(px(ui::R_CONTROL))
            .bg(theme::panel())
            .border_l_2()
            .border_color(color)
            .child(
                div()
                    .text_size(px(14.0))
                    .text_color(theme::text_primary())
                    .child(task.title.clone()),
            )
            .child(ui::meta(state))
            .child(ui::meta(ledger))
            .when(task.verified, |item| {
                item.child(ui::meta("Verification recorded"))
            })
            .when(!task.steps.is_empty(), |item| {
                item.child(step_row(path, &task.steps, self.current_observation()))
            })
            .children(
                task.subtasks
                    .iter()
                    .enumerate()
                    .map(|(index, child)| self.render_subtask(child, &format!("{path}-s{index}"))),
            )
            .into_any_element()
    }

    fn render_task(&self, m: usize, t: usize, task: &TaskView) -> AnyElement {
        let (label, role) = match task.status {
            RoadmapStatus::Completed if task.verified => ("verified", Semantic::Verified),
            RoadmapStatus::Completed => ("done · unverified", Semantic::External),
            RoadmapStatus::Running | RoadmapStatus::ReadyForVerification => (
                if self.current_observation() {
                    "in progress"
                } else {
                    "last recorded in progress"
                },
                Semantic::Agent,
            ),
            RoadmapStatus::Failed | RoadmapStatus::FailedVerification => {
                ("failed", Semantic::Failure)
            },
            RoadmapStatus::Paused => ("paused", Semantic::You),
            RoadmapStatus::Skipped => ("skipped", Semantic::External),
            RoadmapStatus::Pending => ("planned", Semantic::External),
        };
        let (label, role) = match task.iteration {
            mission::IterationState::Unknown => ("continuation unknown", Semantic::External),
            mission::IterationState::Discovered => {
                ("discovered · not dispatched", Semantic::External)
            },
            mission::IterationState::Queued => ("queued after split", Semantic::External),
            mission::IterationState::Replaced => ("split into smaller tasks", Semantic::External),
            mission::IterationState::Running if task.status == RoadmapStatus::Pending => (
                if self.current_observation() {
                    "in progress"
                } else {
                    "last recorded in progress"
                },
                Semantic::Agent,
            ),
            mission::IterationState::Finished(_) if task.status == RoadmapStatus::Pending => {
                ("process finished · unverified", Semantic::External)
            },
            mission::IterationState::Unfinished if task.status == RoadmapStatus::Pending => {
                ("no completion recorded", Semantic::External)
            },
            _ => (label, role),
        };
        let color = role.color();
        let live = self.current_observation()
            && task
                .steps
                .iter()
                .any(|s| matches!(s.state, StepState::Working | StepState::WaitingOnYou));
        div()
            .id(SharedString::from(format!("task-state-{m}-{t}")))
            .test_support()
            .role(Role::Group)
            .aria_label(format!("{}: {label}", task.title))
            .v_flex()
            .gap(px(6.0))
            .px(px(12.0))
            .py(px(9.0))
            .rounded(px(ui::R_CONTROL))
            .when(live, |el| el.bg(theme::tint(theme::accent())))
            .child(
                div()
                    .h_flex()
                    .gap(px(10.0))
                    .items_center()
                    .child(if live {
                        ui::live_dot(color)
                    } else {
                        ui::status_dot(color)
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .text_size(px(12.0))
                            .text_color(if task.status == RoadmapStatus::Pending {
                                theme::text_muted()
                            } else {
                                theme::text_primary()
                            })
                            .truncate()
                            .child(task.title.clone()),
                    )
                    .when_some(task.discovered_from.clone(), |el, from| {
                        el.child(
                            div()
                                .text_size(px(10.0))
                                .text_color(theme::text_dim())
                                .child(format!("found in {from}")),
                        )
                    })
                    .child(if task.verified {
                        div()
                            .h_flex()
                            .gap(px(4.0))
                            .items_center()
                            .text_size(px(10.0))
                            .text_color(theme::success())
                            .child(Icon::new(Lucide::ShieldCheck).size(px(12.0)))
                            .child("verified")
                    } else {
                        ui::pill(label, color, theme::tint(color))
                    }),
            )
            .when(!task.steps.is_empty(), |el| {
                el.child(div().pl(px(17.0)).child(step_row(
                    &format!("t{m}-{t}"),
                    &task.steps,
                    self.current_observation(),
                )))
            })
            .when(!task.subtasks.is_empty(), |el| {
                el.child(ui::meta("Subtasks"))
                    .children(task.subtasks.iter().enumerate().map(|(index, child)| {
                        self.render_subtask(child, &format!("t{m}-{t}-s{index}"))
                    }))
            })
            .into_any_element()
    }

    fn render_milestone(&self, index: usize, m: &MilestoneView) -> Div {
        let total = m.tasks.len();
        let done = m
            .tasks
            .iter()
            .filter(|t| matches!(t.status, RoadmapStatus::Completed | RoadmapStatus::Skipped))
            .count();
        let verified = m.tasks.iter().filter(|t| t.verified).count();
        let (label, role) = match (&m.finished, m.started) {
            (Some(outcome), _) if outcome == "completed" || outcome == "pass" => {
                ("done", Semantic::Verified)
            },
            (Some(_), _) => ("ended", Semantic::Failure),
            (None, true) => (
                if self.current_observation() {
                    "in progress"
                } else {
                    "last recorded in progress"
                },
                Semantic::Agent,
            ),
            (None, false) => ("planned", Semantic::External),
        };
        let color = role.color();
        ui::panel()
            .v_flex()
            .overflow_hidden()
            .child(
                div()
                    .h_flex()
                    .gap(px(12.0))
                    .items_center()
                    .px(px(14.0))
                    .py(px(12.0))
                    .child(
                        div()
                            .size(px(24.0))
                            .flex_none()
                            .rounded_full()
                            .border_1()
                            .border_color(theme::stroke(color))
                            .bg(theme::tint(color))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(10.5))
                            .font_weight(FontWeight::BOLD)
                            .text_color(color)
                            .child((index + 1).to_string()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .v_flex()
                            .gap(px(2.0))
                            .child(
                                div()
                                    .h_flex()
                                    .gap(px(8.0))
                                    .items_center()
                                    .child(
                                        div()
                                            .text_size(px(13.5))
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(theme::text_primary())
                                            .truncate()
                                            .child(m.title.clone()),
                                    )
                                    .child(ui::pill(label, color, theme::tint(color))),
                            )
                            .child(
                                div()
                                    .text_size(px(10.5))
                                    .text_color(theme::text_dim())
                                    .child(format!("{done}/{total} tasks · {verified} verified")),
                            ),
                    ),
            )
            .when(!m.steps.is_empty(), |el| {
                el.child(
                    div()
                        .h_flex()
                        .gap(px(10.0))
                        .items_center()
                        .px(px(14.0))
                        .pb(px(10.0))
                        .child(ui::section_label("Milestone steps"))
                        .child(step_row(
                            &format!("m{index}"),
                            &m.steps,
                            self.current_observation(),
                        )),
                )
            })
            .child(
                div()
                    .v_flex()
                    .gap(px(1.0))
                    .p(px(4.0))
                    .border_t_1()
                    .border_color(theme::hairline())
                    .children(
                        m.tasks
                            .iter()
                            .enumerate()
                            .map(|(t, task)| self.render_task(index, t, task)),
                    )
                    .when(m.tasks.is_empty(), |el| {
                        el.child(
                            div()
                                .px(px(12.0))
                                .py(px(8.0))
                                .text_size(px(11.0))
                                .text_color(theme::text_dim())
                                .child("No tasks recorded for this milestone."),
                        )
                    }),
            )
    }

    fn render_mission(&self, mission: &Mission) -> Div {
        let mut body = div().v_flex().gap(px(16.0));
        for caveat in &mission.caveats {
            body = body.child(
                div()
                    .text_sm()
                    .text_color(theme::warning())
                    .child(caveat.clone()),
            );
        }
        if mission.planning.is_some()
            || mission
                .plan
                .iter()
                .any(|p| p.state != PlanState::NotStarted)
        {
            body = body.child(self.render_plan(mission));
        }
        if let Some(planning) = &mission.planning {
            body = body.child(self.render_planning(planning));
        }
        if let Some(now) = &mission.now {
            let activity = if self.current_observation() {
                format!("Working on  {now}")
            } else {
                format!("Last recorded activity  {now}")
            };
            body = body.child(
                ui::node_card(theme::accent())
                    .id("mission-current-activity")
                    .test_support()
                    .role(Role::Group)
                    .aria_label(activity.clone())
                    .h_flex()
                    .gap(px(10.0))
                    .items_center()
                    .px(px(14.0))
                    .py(px(10.0))
                    .when(self.current_observation(), |el| {
                        el.child(ui::live_dot(theme::accent()))
                    })
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(theme::text_primary())
                            .child(activity),
                    ),
            );
        }
        if let Some(end) = &mission.end {
            let (headline, detail, role) = match end {
                RunEnd::Completed => (
                    "Finished — the flow reached its end.".to_string(),
                    None,
                    Semantic::Verified,
                ),
                RunEnd::Failed(e) => (
                    mission::failure_headline(e).to_string(),
                    Some(e.clone()),
                    Semantic::Failure,
                ),
                RunEnd::Aborted(r) => (
                    "Stopped by request".to_string(),
                    Some(r.clone()),
                    Semantic::External,
                ),
            };
            let color = role.color();
            body = body.child(
                ui::node_card(color)
                    .v_flex()
                    .gap(px(4.0))
                    .px(px(14.0))
                    .py(px(10.0))
                    .child(
                        div()
                            .text_size(px(12.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::text_primary())
                            .child(headline),
                    )
                    .children(detail.map(|d| {
                        div()
                            .text_size(px(10.5))
                            .line_height(px(15.0))
                            .text_color(theme::text_muted())
                            .child(d)
                    })),
            );
        }
        if !mission.milestones.is_empty() {
            body = body
                .child(ui::section_label(format!(
                    "Milestones · {}",
                    mission.milestones.len()
                )))
                .children(
                    mission
                        .milestones
                        .iter()
                        .enumerate()
                        .map(|(i, m)| self.render_milestone(i, m)),
                );
        }
        if !mission.steps.is_empty() {
            body = body.child(
                div()
                    .v_flex()
                    .gap(px(8.0))
                    .child(ui::section_label("Whole-project steps"))
                    .child(step_row("run", &mission.steps, self.current_observation())),
            );
        }
        if mission.milestones.is_empty() && mission.steps.is_empty() && mission.end.is_none() {
            body = body.child(ui::empty_state(
                "◎",
                if mission.planning.is_some() {
                    "Implementation has no recorded work yet"
                } else {
                    "Nothing has run yet"
                },
                "Milestones, tasks and their steps appear here as the build moves.",
            ));
        }
        body
    }
}

/// Indices are qualified by every parent; repeated task IDs in other milestones
/// cannot select the wrong occurrence.
fn task_scope(
    mission: &Mission,
    milestone: &MilestoneId,
    task: &RoadmapTaskId,
) -> Option<Vec<usize>> {
    let m = mission
        .milestones
        .iter()
        .position(|item| &item.id == milestone)?;
    let t = mission.milestones[m]
        .tasks
        .iter()
        .position(|item| &item.id == task)?;
    Some(vec![m, t])
}

fn scoped_task<'a>(mission: &'a Mission, path: &[usize]) -> Option<&'a TaskView> {
    let mut task = mission
        .milestones
        .get(*path.first()?)?
        .tasks
        .get(*path.get(1)?)?;
    for index in &path[2..] {
        task = task.subtasks.get(*index)?;
    }
    Some(task)
}

fn process_evidence(id: String, text: String) -> impl IntoElement + use<> {
    div()
        .id(SharedString::from(id))
        .test_support()
        .role(Role::Group)
        .aria_label(text.clone())
        .text_size(px(13.0))
        .text_color(theme::text_muted())
        .child(text)
}

fn participant_text(step: &Step) -> String {
    if step.lane == Lane::Approve {
        return "Performed by you".into();
    }
    match &step.participant {
        Some(participant) => {
            let mut text = format!("Performed by {}", participant.profile);
            if let Some(runtime) = &participant.runtime {
                text.push_str(&format!(" · {runtime}"));
            }
            text
        },
        None => "Participant not recorded".into(),
    }
}

fn session_text(step: &Step) -> String {
    match &step.participant {
        Some(participant) => format!(
            "Runtime: {} · Session: {}",
            participant.runtime.as_deref().unwrap_or("not recorded"),
            participant.session
        ),
        None if step.lane == Lane::Approve => "Human decision · no agent session".into(),
        None => "Agent session not recorded".into(),
    }
}

fn work_details(index: usize, step: &Step) -> Div {
    div()
        .v_flex()
        .gap(px(8.0))
        .child(process_evidence(
            format!("process-step-{index}-session"),
            session_text(step),
        ))
        .child(process_evidence(
            format!("process-step-{index}-inputs"),
            inputs_text(step),
        ))
        .child(process_evidence(
            format!("process-step-{index}-skills"),
            skills_text(step),
        ))
        .child(ui::meta(format!(
            "Recorded step: {} · attempt {}",
            step.node, step.attempts
        )))
}

fn inputs_text(step: &Step) -> String {
    match &step.inputs {
        Some(inputs) if inputs.is_empty() => "Resolved inputs: none".into(),
        Some(inputs) => format!("Resolved inputs: {}", inputs.join(", ")),
        None => "Resolved inputs not recorded".into(),
    }
}

fn skills_text(step: &Step) -> String {
    if step.skills.is_empty() {
        "Bound skills not recorded".into()
    } else {
        format!("Bound skills: {}", step.skills.join(", "))
    }
}

impl MissionPanel {
    fn process_steps(&self, steps: &[Step], cx: &mut Context<Self>) -> Div {
        let mut body = div().v_flex().gap(px(10.0));
        if steps.is_empty() {
            return body.child(ui::meta("No steps have been recorded at this level yet."));
        }
        for (index, step) in steps.iter().enumerate() {
            let (status, color) = match &step.state {
                StepState::Done(_) => ("Finished", theme::success()),
                StepState::Failed(_) => ("Failed", theme::error()),
                StepState::WaitingOnYou => ("Needs your decision", theme::warning()),
                StepState::Working => (
                    if self.current_observation() {
                        "Working"
                    } else {
                        "Last recorded progress"
                    },
                    theme::accent(),
                ),
                StepState::RecoveryRequired => ("Recovery requires review", theme::warning()),
                StepState::Unknown => ("Continuation unknown", theme::text_muted()),
                StepState::Unfinished => ("No completion recorded", theme::text_muted()),
            };
            let detail = match &step.state {
                StepState::Failed(reason) => Some(reason.clone()),
                _ => step.summary.clone(),
            };
            let occurrence = StepOccurrence {
                scope: self.scope.clone(),
                index,
            };
            let expanded = self.expanded_steps.contains(&occurrence);
            let button_id = format!("work-details-{:?}-{index}", self.scope);
            body = body.child(
                ui::panel()
                    .v_flex()
                    .gap(px(8.0))
                    .p(px(16.0))
                    .child(
                        div()
                            .h_flex()
                            .gap(px(12.0))
                            .items_center()
                            .child(ui::meta(format!("{}", index + 1)))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .text_size(px(16.0))
                                    .child(step.label.clone()),
                            )
                            .child(ui::pill(status, color, theme::tint(color))),
                    )
                    .child(process_evidence(
                        format!("process-step-{index}-participant"),
                        participant_text(step),
                    ))
                    .children(detail.map(|text| div().text_size(px(14.0)).child(text)))
                    .child(
                        Button::new(SharedString::from(button_id.clone()))
                            .accessibility_id(SharedString::from(button_id))
                            .label(if expanded {
                                "Hide work details"
                            } else {
                                "Work details"
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !this.expanded_steps.remove(&occurrence) {
                                    this.expanded_steps.insert(occurrence.clone());
                                }
                                cx.notify();
                            })),
                    )
                    .when(expanded, |card| card.child(work_details(index, step))),
            );
        }
        body
    }
}

impl MissionPanel {
    fn scope_button(&self, label: String, path: Vec<usize>, cx: &mut Context<Self>) -> Button {
        Button::new(SharedString::from(format!("process-scope-{path:?}")))
            .label(label)
            .on_click(cx.listener(move |this, _, _, cx| {
                this.scope = path.clone();
                this.requested_task = None;
                cx.notify();
            }))
    }

    fn render_explorer(&self, mission: &Mission, cx: &mut Context<Self>) -> Div {
        let mut body = div().v_flex().gap(px(16.0));
        let mut crumbs = div()
            .h_flex()
            .flex_wrap()
            .gap(px(8.0))
            .child(self.scope_button("Mission".into(), Vec::new(), cx));
        if let Some(&m) = self.scope.first() {
            if let Some(milestone) = mission.milestones.get(m) {
                crumbs = crumbs.child(self.scope_button(milestone.title.clone(), vec![m], cx));
            }
            for depth in 2..=self.scope.len() {
                if let Some(task) = scoped_task(mission, &self.scope[..depth]) {
                    crumbs = crumbs.child(self.scope_button(
                        task.title.clone(),
                        self.scope[..depth].to_vec(),
                        cx,
                    ));
                }
            }
        }
        body = body.child(crumbs);
        if let Some((_, task)) = &self.requested_task {
            return body
                .child(ui::meta(format!(
                    "This task ({task}) has no recorded execution in this mission yet."
                )))
                .child(self.render_mission(mission));
        }
        if self.scope.is_empty() {
            let links = mission
                .milestones
                .iter()
                .enumerate()
                .map(|(index, milestone)| {
                    self.scope_button(
                        format!("Open milestone: {}", milestone.title),
                        vec![index],
                        cx,
                    )
                })
                .collect::<Vec<_>>();
            return body
                .child(div().h_flex().flex_wrap().gap(px(8.0)).children(links))
                .child(self.render_mission(mission));
        }
        let Some(milestone) = mission.milestones.get(self.scope[0]) else {
            return body.child(ui::meta("This recorded occurrence is no longer available."));
        };
        let (title, steps, children) = if self.scope.len() == 1 {
            (&milestone.title, &milestone.steps, &milestone.tasks)
        } else if let Some(task) = scoped_task(mission, &self.scope) {
            (&task.title, &task.steps, &task.subtasks)
        } else {
            return body.child(ui::meta("This recorded occurrence is no longer available."));
        };
        body = body
            .child(
                div()
                    .text_size(px(22.0))
                    .font_weight(FontWeight::BOLD)
                    .child(title.clone()),
            )
            .child(ui::meta(
                "Recorded process · states and reports from this execution",
            ))
            .child(self.process_steps(steps, cx));
        if !children.is_empty() {
            let links = children
                .iter()
                .enumerate()
                .map(|(index, task)| {
                    let mut path = self.scope.clone();
                    path.push(index);
                    self.scope_button(task.title.clone(), path, cx)
                })
                .collect::<Vec<_>>();
            body = body
                .child(ui::section_label(if self.scope.len() == 1 {
                    "Tasks"
                } else {
                    "Subtasks"
                }))
                .child(div().v_flex().gap(px(8.0)).children(links));
        }
        body
    }
}

impl Render for MissionPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content: AnyElement = match (&self.mission, &self.error) {
            (Some(mission), None) => self.render_explorer(mission, cx).into_any_element(),
            (_, Some(error)) => {
                ui::empty_state("◌", "Could not read this run", error.clone()).into_any_element()
            },
            (None, None) => div()
                .text_size(px(12.0))
                .text_color(theme::text_muted())
                .child("Reading the run…")
                .into_any_element(),
        };
        let mut body = div().v_flex().gap(px(12.0));
        if let Some(message) = self.mission.as_ref().and_then(|mission| {
            let unknown = self.operation.as_ref().is_some_and(|status| matches!(
                status.state,
                BootstrapState::NeedsAttention {
                    reason: surge_core::bootstrap_operation::BootstrapAttentionReason::StorageUnconfirmed,
                    ..
                }
            ));
            mission::recovery_problem(&mission.recovery, unknown)
        }) {
            body = body.child(div().id("mission-recovery").test_support()
                .role(Role::Group).aria_label(message.clone())
                .text_size(px(12.0)).text_color(theme::warning()).child(message));
        }
        if let Some(status) = &self.operation {
            let message = history::operation_message(status);
            body = body.child(
                div()
                    .id("mission-operation")
                    .test_support()
                    .role(Role::Group)
                    .aria_label(message.clone())
                    .text_size(px(12.0))
                    .text_color(theme::text_muted())
                    .child(message),
            );
        }
        let message = match &self.implementation {
            Some(ImplementationHistory::PlanningOnly) => {
                Some("No implementation launch is recorded.".into())
            },
            Some(ImplementationHistory::AwaitingStartup) => {
                Some(match self.operation.as_ref().map(|status| &status.state) {
                    Some(BootstrapState::Cancelling { .. }) => {
                        "Implementation start not recorded; cancelling.".into()
                    },
                    Some(BootstrapState::NeedsAttention { .. }) => {
                        "Implementation start not recorded; operator attention is required.".into()
                    },
                    _ => "Implementation awaiting startup.".into(),
                })
            },
            Some(ImplementationHistory::Unavailable(issue)) => Some(issue.message()),
            _ => None,
        };
        if let Some(message) = message {
            body = body.child(
                div()
                    .id("mission-implementation-history")
                    .test_support()
                    .role(Role::Group)
                    .aria_label(message.clone())
                    .text_size(px(12.0))
                    .text_color(theme::text_muted())
                    .child(message),
            );
        }
        if self.loading == LoadingState::Changed {
            body = body.child("Run changed while reading; waiting for the next update.");
        }
        div()
            .id("run-mission")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px(px(20.0))
            .py(px(16.0))
            .child(div().max_w(px(980.0)).child(body.child(content)))
    }
}

#[cfg(test)]
mod hierarchy_tests {
    use super::{LoadingState, MissionPanel, MissionRequest};
    use gpui_kit::test::TestWindowExt;
    use gpui_kit::{AppContext as _, TestAppContext};
    use surge_core::keys::{NodeKey, SubgraphKey};
    use surge_core::loop_config::IterableSource;
    use surge_core::{ContentHash, EventPayload, RunId};

    #[gpui_kit::test]
    fn recovery_observability_offline_mission_does_not_claim_current_worker(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let mission = crate::mission::Mission {
            now: Some("Recorded stage".into()),
            ..Default::default()
        };
        let panel = cx.new(|_| MissionPanel {
            controlled_loads: None,
            active_loads: Default::default(),
            home: None,
            request: MissionRequest {
                selected: RunId::new(),
                project: None,
            },
            mission: Some(mission),
            implementation: None,
            operation: None,
            loading: LoadingState::Ready,
            error: None,
            generation: 0,
            loaded_refresh: None,
            scope: Vec::new(),
            requested_task: None,
            expanded_steps: Default::default(),
        });
        let (_, window) =
            cx.add_window_view(|window, cx| gpui_kit::component::Root::new(panel, window, cx));
        window.update(|window, cx| {
            window.render_frame(cx);
            let label = window
                .find("mission-current-activity")
                .label()
                .unwrap()
                .to_owned();
            assert_eq!(label, "Last recorded activity  Recorded stage");
        });
    }

    #[gpui_kit::test]
    fn recovery_observability_availability_changes_render_without_history_reload(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let run = RunId::new();
        let other = RunId::new();
        let mission = crate::mission::Mission {
            now: Some("Recorded stage".into()),
            recovery: surge_core::recovery_observation::RecoveryObservation::CompletePrefix {
                through: 7,
                latest: None,
            },
            ..Default::default()
        };
        let refresh = |live| super::MissionRefresh {
            operation: None,
            streams: vec![(run, Some(7), Some(7)), (other, Some(7), Some(7))],
            observations: vec![(run, live), (other, true)],
        };
        let panel = cx.new(|_| MissionPanel {
            controlled_loads: None,
            active_loads: Default::default(),
            home: None,
            request: MissionRequest {
                selected: run,
                project: None,
            },
            mission: Some(mission),
            implementation: Some(super::ImplementationHistory::Recorded),
            operation: None,
            loading: LoadingState::Ready,
            error: None,
            generation: 5,
            loaded_refresh: Some(refresh(false)),
            scope: Vec::new(),
            requested_task: None,
            expanded_steps: Default::default(),
        });
        let (_, window) = cx.add_window_view(|window, cx| {
            gpui_kit::component::Root::new(panel.clone(), window, cx)
        });
        for live in [false, true, false] {
            window.update(|window, cx| {
                panel.update(cx, |panel, cx| {
                    panel.sync(refresh(live), cx);
                    assert_eq!(
                        panel.generation, 5,
                        "availability cannot reload journal history"
                    );
                    assert_eq!(panel.current_observation(), live);
                });
                window.render_frame(cx);
                let label = window
                    .find("mission-current-activity")
                    .label()
                    .unwrap()
                    .to_owned();
                assert!(label.starts_with(if live {
                    "Working on"
                } else {
                    "Last recorded activity"
                }));
            });
        }
        window.update(|_, cx| {
            panel.update(cx, |panel, _| {
                panel.loading = LoadingState::Reading;
                assert!(!panel.current_observation());
                panel.loading = LoadingState::Ready;
                let mut advanced = refresh(true);
                advanced.streams[0].2 = Some(8);
                panel.loaded_refresh = Some(advanced);
                assert!(
                    !panel.current_observation(),
                    "old stage cannot be current after a newer watermark"
                );
                panel.loaded_refresh = Some(refresh(false));
                panel.operation = Some(surge_core::bootstrap_operation::BootstrapOperationStatus {
                    operation_id: run,
                    planning_run: run,
                    implementation_run: other,
                    state: surge_core::bootstrap_operation::BootstrapState::Pending {
                        phase: surge_core::bootstrap_operation::BootstrapPhase::Implementing,
                    },
                    result: None,
                    revision: 1,
                    cancel_requested: false,
                    queue_sequence: 1,
                    payload_version: 1,
                });
                panel.implementation = Some(super::ImplementationHistory::PlanningOnly);
                assert!(
                    !panel.current_observation(),
                    "live child is not live planning history"
                );
                panel.implementation = Some(super::ImplementationHistory::Recorded);
                assert!(
                    panel.current_observation(),
                    "recorded child uses its exact attributed observation"
                );
                panel.mission.as_mut().unwrap().recovery =
                    surge_core::recovery_observation::RecoveryObservation::PartialPrefix {
                        through: Some(7),
                        latest: None,
                    };
                assert!(
                    !panel.current_observation(),
                    "partial history cannot show a current worker"
                );
            })
        });
    }

    #[gpui_kit::test]
    fn recovery_observability_native_problem_keeps_old_reason_unknown(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let run = RunId::new();
        let mission = crate::mission::fixture_fold(
            &[EventPayload::RunRecoveryRequired {
                control_generation: 0,
                diagnostic: "gate occurrence mismatch <literal> [inspect](https://example.test)"
                    .into(),
            }],
            &[],
            None,
        );
        let panel = cx.new(|_| MissionPanel {
            controlled_loads: None, active_loads: Default::default(), home: None,
            request: MissionRequest { selected: run, project: None }, mission: Some(mission),
            implementation: Some(super::ImplementationHistory::PlanningOnly),
            operation: Some(surge_core::bootstrap_operation::BootstrapOperationStatus {
                operation_id: run, planning_run: run, implementation_run: RunId::new(),
                state: surge_core::bootstrap_operation::BootstrapState::NeedsAttention {
                    phase: surge_core::bootstrap_operation::BootstrapPhase::Planning, run_id: Some(run),
                    reason: surge_core::bootstrap_operation::BootstrapAttentionReason::StorageUnconfirmed,
                }, result: None, revision: 1, cancel_requested: false, queue_sequence: 1, payload_version: 1,
            }), loading: LoadingState::Ready, error: None, generation: 0, loaded_refresh: None,
            scope: Vec::new(), requested_task: None, expanded_steps: Default::default(),
        });
        let (_, window) =
            cx.add_window_view(|window, cx| gpui_kit::component::Root::new(panel, window, cx));
        window.update(|window, cx| {
            window.render_frame(cx);
            let label = window.find("mission-recovery").label().unwrap().to_owned();
            assert!(label.contains("confirmation unconfirmed"));
            assert!(
                label
                    .contains("gate occurrence mismatch <literal> [inspect](https://example.test)")
            );
            assert!(!label.contains("free disk"));
        });
    }

    #[gpui_kit::test]
    fn started_pending_task_displays_progress_without_a_ledger_promotion(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let events = [
            EventPayload::LoopIterationStarted {
                loop_id: "task_loop".parse().unwrap(),
                item: toml::from_str("id='login'\ntitle='Login'").unwrap(),
                index: 0,
            },
            EventPayload::StageEntered {
                node: "implement".parse().unwrap(),
                attempt: 1,
            },
        ];
        let mission = crate::mission::fixture_fold(&[], &events, None);
        assert_eq!(
            mission.milestones[0].tasks[0].status,
            surge_core::roadmap::RoadmapStatus::Pending
        );
        assert!(!mission.milestones[0].tasks[0].verified);
        let panel = cx.new(|_| MissionPanel {
            controlled_loads: None,
            active_loads: Default::default(),
            home: None,
            request: MissionRequest {
                selected: RunId::new(),
                project: None,
            },
            mission: Some(mission),
            implementation: None,
            operation: None,
            loading: LoadingState::Ready,
            error: None,
            generation: 0,
            loaded_refresh: None,
            scope: Vec::new(),
            requested_task: None,
            expanded_steps: Default::default(),
        });
        let (_, window) =
            cx.add_window_view(|window, cx| gpui_kit::component::Root::new(panel, window, cx));
        window.update(|window, cx| {
            window.render_frame(cx);
            assert_eq!(
                window.find("task-state-0-0").label(),
                Some("Login: last recorded in progress")
            );
        });
    }

    #[gpui_kit::test]
    fn selected_mission_renders_full_subtask_title_and_recorded_state(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let key = |text: &str| text.parse::<NodeKey>().unwrap();
        let mut graph: surge_core::graph::Graph =
            toml::from_str(include_str!("../../testdata/nested_loops_flow.toml")).unwrap();
        let body: SubgraphKey = "task_body".parse().unwrap();
        let child_body: SubgraphKey = "subtask_body".parse().unwrap();
        graph
            .subgraphs
            .insert(child_body.clone(), graph.subgraphs[&body].clone());
        let mut loop_node = graph.nodes[&key("milestone_loop")].clone();
        loop_node.id = key("subtask_loop");
        let surge_core::node::NodeConfig::Loop(config) = &mut loop_node.config else {
            panic!("fixture outer node is a loop");
        };
        config.body = child_body;
        config.iteration_var_name = "subtask".into();
        config.iterates_over = IterableSource::Artifact {
            node: key("spec_task"),
            name: "spec".into(),
            jsonpath: "spec.subtasks".into(),
        };
        graph
            .subgraphs
            .get_mut(&body)
            .unwrap()
            .nodes
            .insert(loop_node.id.clone(), loop_node);
        let title = "Create the complete keyboard interaction for the countdown timer";
        let events = [
            EventPayload::PipelineMaterialized {
                graph: Box::new(graph),
                graph_hash: ContentHash::compute(b"subtask-ui"),
            },
            EventPayload::LoopIterationStarted {
                loop_id: key("milestone_loop"),
                item: toml::from_str("id = 'm1'\ntitle = 'Timer'\ntasks = []").unwrap(),
                index: 0,
            },
            EventPayload::LoopIterationStarted {
                loop_id: key("task_loop"),
                item: toml::from_str("id = 't1'\ntitle = 'Build countdown'").unwrap(),
                index: 0,
            },
            EventPayload::LoopIterationStarted {
                loop_id: key("subtask_loop"),
                item: toml::from_str(&format!("id = 'leaf'\ntitle = '{title}'")).unwrap(),
                index: 0,
            },
            EventPayload::StageEntered {
                node: key("impl_task"),
                attempt: 1,
            },
        ];
        let mut mission = crate::mission::fixture_fold(&[], &events, None);
        let mut second = mission.milestones[0].clone();
        second.id = "m2".into();
        second.tasks[0].subtasks[0].title = "A different occurrence".into();
        mission.milestones.push(second);
        let path = super::task_scope(&mission, &"m2".into(), &"t1".into()).unwrap();
        assert_eq!(path, vec![1, 0]);
        assert_eq!(
            super::scoped_task(&mission, &[1, 0, 0]).unwrap().title,
            "A different occurrence"
        );
        assert_eq!(
            super::scoped_task(&mission, &[0, 0, 0]).unwrap().title,
            title
        );
        assert!(super::task_scope(&mission, &"missing".into(), &"t1".into()).is_none());
        let panel = cx.new(|_| MissionPanel {
            controlled_loads: None,
            active_loads: Default::default(),
            home: None,
            request: MissionRequest {
                selected: RunId::new(),
                project: None,
            },
            mission: Some(mission),
            implementation: None,
            operation: None,
            loading: LoadingState::Ready,
            error: None,
            generation: 0,
            loaded_refresh: None,
            scope: Vec::new(),
            requested_task: None,
            expanded_steps: Default::default(),
        });
        let (_, window) =
            cx.add_window_view(|window, cx| gpui_kit::component::Root::new(panel, window, cx));
        window.update(|window, cx| {
            window.render_frame(cx);
            assert_eq!(
                window.find("t0-0-s0").label(),
                Some(format!("{title}: Last recorded in progress").as_str())
            );
        });
    }
    #[gpui_kit::test]
    fn task_process_displays_recorded_participant_and_context(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let key = |text: &str| text.parse::<NodeKey>().unwrap();
        let session = surge_core::SessionId::new();
        let events = [
            EventPayload::PipelineMaterialized {
                graph: Box::new(
                    toml::from_str(include_str!("../../testdata/nested_loops_flow.toml")).unwrap(),
                ),
                graph_hash: ContentHash::compute(b"participant-ui"),
            },
            EventPayload::LoopIterationStarted {
                loop_id: key("milestone_loop"),
                item: toml::from_str("id = 'm1'\ntitle = 'Timer'\ntasks = []").unwrap(),
                index: 0,
            },
            EventPayload::LoopIterationStarted {
                loop_id: key("task_loop"),
                item: toml::from_str("id = 't1'\ntitle = 'Build countdown'").unwrap(),
                index: 0,
            },
            EventPayload::StageEntered {
                node: key("impl_task"),
                attempt: 1,
            },
            EventPayload::SessionOpened {
                opened: None,
                handoff: None,
                node: key("impl_task"),
                session,
                agent: "implementer@1.0".into(),
                agent_id: Some("claude".into()),
            },
            EventPayload::StageInputsResolved {
                node: key("impl_task"),
                attempt: 1,
                bindings: [(
                    "architecture".into(),
                    ContentHash::compute(b"secret content must not display"),
                )]
                .into(),
                memory_receipt: None,
            },
            EventPayload::SkillBound {
                node: key("impl_task"),
                name: "rust-builder".into(),
                provider: surge_core::skill::SkillProvider::ProjectDir,
                hash: ContentHash::compute(b"skill"),
                gate_enabled: true,
            },
        ];
        let mut events = events.to_vec();
        events.extend([
            EventPayload::StageCompleted {
                node: key("impl_task"),
                outcome: "pass".parse().unwrap(),
            },
            EventPayload::LoopIterationCompleted {
                loop_id: key("task_loop"),
                index: 0,
                outcome: "completed".parse().unwrap(),
            },
            EventPayload::LoopIterationStarted {
                loop_id: key("task_loop"),
                item: toml::from_str("id = 't2'\ntitle = 'Second task'").unwrap(),
                index: 1,
            },
            EventPayload::StageEntered {
                node: key("impl_task"),
                attempt: 1,
            },
            EventPayload::SessionOpened {
                opened: None,
                handoff: None,
                node: key("impl_task"),
                session: surge_core::SessionId::new(),
                agent: "reviewer@1.0".into(),
                agent_id: None,
            },
        ]);
        let panel = cx.new(|_| MissionPanel {
            controlled_loads: None,
            active_loads: Default::default(),
            home: None,
            request: MissionRequest {
                selected: RunId::new(),
                project: None,
            },
            mission: Some(crate::mission::fixture_fold(&[], &events, None)),
            implementation: None,
            operation: None,
            loading: LoadingState::Ready,
            error: None,
            generation: 0,
            loaded_refresh: None,
            scope: vec![0, 0],
            requested_task: None,
            expanded_steps: Default::default(),
        });
        let (_, window) =
            cx.add_window_view(|window, cx| gpui_kit::component::Root::new(panel, window, cx));
        window.update(|window, cx| {
            window.render_frame(cx);
            assert_eq!(
                window.find("process-step-0-participant").label(),
                Some("Performed by implementer@1.0 · claude")
            );
            window.click("work-details-[0, 0]-0", cx);
            window.render_frame(cx);
            assert_eq!(
                window.find("process-step-0-session").label(),
                Some(format!("Runtime: claude · Session: {session}").as_str())
            );
            assert_eq!(
                window.find("process-step-0-inputs").label(),
                Some("Resolved inputs: architecture")
            );
            assert_eq!(
                window.find("process-step-0-skills").label(),
                Some("Bound skills: rust-builder")
            );
            window.click("work-details-[0, 0]-0", cx);
            window.render_frame(cx);
            assert_eq!(
                window.find("work-details-[0, 0]-0").label(),
                Some("Work details")
            );
            window.click("work-details-[0, 0]-0", cx);
            window.render_frame(cx);
            window.click("process-scope-[0]", cx);
            window.render_frame(cx);
            window.click("process-scope-[0, 1]", cx);
            window.render_frame(cx);
            assert_eq!(
                window.find("process-step-0-participant").label(),
                Some("Performed by reviewer@1.0")
            );
            assert_eq!(
                window.find("work-details-[0, 1]-0").label(),
                Some("Work details")
            );
            window.click("work-details-[0, 1]-0", cx);
            window.render_frame(cx);
            assert!(
                window
                    .find("process-step-0-session")
                    .label()
                    .unwrap()
                    .starts_with("Runtime: not recorded · Session: ")
            );
            assert_eq!(
                window.find("process-step-0-inputs").label(),
                Some("Resolved inputs not recorded")
            );
            assert_eq!(
                window.find("process-step-0-skills").label(),
                Some("Bound skills not recorded")
            );
        });
    }
    #[test]
    fn old_steps_and_human_gates_do_not_invent_agent_evidence() {
        let key = |text: &str| text.parse::<NodeKey>().unwrap();
        let old = crate::mission::fixture_fold(
            &[],
            &[EventPayload::StageEntered {
                node: key("legacy_task"),
                attempt: 1,
            }],
            None,
        );
        assert_eq!(
            super::participant_text(&old.steps[0]),
            "Participant not recorded"
        );
        assert_eq!(
            super::session_text(&old.steps[0]),
            "Agent session not recorded"
        );
        assert_eq!(
            super::inputs_text(&old.steps[0]),
            "Resolved inputs not recorded"
        );
        assert_eq!(
            super::skills_text(&old.steps[0]),
            "Bound skills not recorded"
        );
        let mut graph: surge_core::graph::Graph =
            toml::from_str(include_str!("../../testdata/nested_loops_flow.toml")).unwrap();
        graph.nodes.get_mut(&key("final_spec")).unwrap().config = surge_core::node::NodeConfig::HumanGate(
            toml::from_str("delivery_channels = []\noptions = []\nsummary = { title = 'Approve', body = 'Plan' }").unwrap());
        let human = crate::mission::fixture_fold(
            &[],
            &[
                EventPayload::PipelineMaterialized {
                    graph: Box::new(graph),
                    graph_hash: ContentHash::compute(b"human"),
                },
                EventPayload::StageEntered {
                    node: key("final_spec"),
                    attempt: 1,
                },
                EventPayload::StageInputsResolved {
                    node: key("final_spec"),
                    attempt: 1,
                    bindings: Default::default(),
                    memory_receipt: None,
                },
            ],
            None,
        );
        assert_eq!(super::participant_text(&human.steps[0]), "Performed by you");
        assert_eq!(
            super::session_text(&human.steps[0]),
            "Human decision · no agent session"
        );
        assert_eq!(super::inputs_text(&human.steps[0]), "Resolved inputs: none");
    }
}

#[cfg(test)]
#[path = "run_mission/results_history_tests.rs"]
mod results_history_tests;
