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
use surge_core::roadmap::{MilestoneId, RoadmapTaskId};
use surge_core::roadmap::{RoadmapArtifact, RoadmapStatus};
use surge_core::{BootstrapStage, EventPayload, RunId};

use crate::flow_diagram::Lane;
use crate::mission::{self, MilestoneView, Mission, PlanState, RunEnd, Step, StepState, TaskView};
use crate::theme::{self, Semantic};
use crate::ui;

/// The runs a mission is made of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct MissionRuns {
    pub planning: Option<RunId>,
    pub implementation: RunId,
}

async fn events_of(root: std::path::PathBuf, run: RunId) -> Vec<EventPayload> {
    surge_persistence::runs::Storage::inspect_existing_run_events(root, run)
        .await
        .map(|events| events.into_iter().map(|e| e.payload.payload).collect())
        .unwrap_or_default()
}

async fn load(runs: MissionRuns) -> Result<Mission, String> {
    // Storage reads run on the app's tokio runtime (entered in `main`);
    // without one (headless tests) say so instead of panicking.
    tokio::runtime::Handle::try_current()
        .map_err(|_| "Run storage is not available".to_string())?;
    let home = surge_core::home::surge_home_dir().ok_or("Surge home unavailable")?;
    let root = home.join("runs");
    let planning = match runs.planning {
        Some(run) => events_of(root.clone(), run).await,
        None => Vec::new(),
    };
    let implementation = if Some(runs.implementation) == runs.planning {
        Vec::new()
    } else {
        events_of(root, runs.implementation).await
    };
    let roadmap: Option<RoadmapArtifact> = runs
        .planning
        .and_then(|p| crate::roadmap_source::read_run_artifact(&home, p, "roadmap_toml"))
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .and_then(|text| toml::from_str(&text).ok());
    Ok(mission::fold(&planning, &implementation, roadmap.as_ref()))
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct StepOccurrence {
    scope: Vec<usize>,
    index: usize,
}

pub(super) struct MissionPanel {
    runs: MissionRuns,
    mission: Option<Mission>,
    error: Option<String>,
    generation: u64,
    /// Event count the current fold reflects.
    loaded_seq: Option<String>,
    scope: Vec<usize>,
    requested_task: Option<(MilestoneId, RoadmapTaskId)>,
    expanded_steps: BTreeSet<StepOccurrence>,
}

impl MissionPanel {
    pub(super) fn new(
        runs: MissionRuns,
        requested_task: Option<(MilestoneId, RoadmapTaskId)>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut panel = Self {
            runs,
            mission: None,
            error: None,
            generation: 0,
            loaded_seq: None,
            scope: Vec::new(),
            requested_task,
            expanded_steps: Default::default(),
        };
        panel.reload(cx);
        panel
    }

    pub(super) fn runs(&self) -> MissionRuns {
        self.runs
    }

    /// Reload when the run has moved since the last fold.
    pub(super) fn sync(&mut self, seq: &str, cx: &mut Context<Self>) {
        if self.loaded_seq.as_deref() != Some(seq) {
            self.loaded_seq = Some(seq.to_string());
            self.reload(cx);
        }
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        let runs = self.runs;
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let result = load(runs).await;
            cx.update(|cx| {
                let _ = this.update(cx, |panel, cx| {
                    if panel.generation != generation {
                        return;
                    }
                    match result {
                        Ok(m) => {
                            if let Some((milestone, task)) = &panel.requested_task
                                && let Some(path) = task_scope(&m, milestone, task)
                            {
                                panel.scope = path;
                                panel.requested_task = None;
                            }
                            panel.mission = Some(m);
                            panel.error = None;
                        },
                        Err(e) => panel.error = Some(e),
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

/// A step as a small capsule: its role color, its label, and how it went.
fn step_chip(id: String, step: &Step) -> Stateful<Div> {
    let role = lane_color(step.lane);
    let (icon, tone): (Option<IconName>, Hsla) = match &step.state {
        StepState::Done(_) => (Some(IconName::Check), theme::success()),
        StepState::Failed(_) => (Some(IconName::Close), theme::error()),
        StepState::WaitingOnYou => (None, theme::warning()),
        StepState::Working => (None, theme::accent()),
        StepState::Unfinished => (Some(IconName::Minus), theme::slate()),
    };
    let tip = match &step.state {
        StepState::Done(outcome) => step
            .summary
            .clone()
            .unwrap_or_else(|| format!("finished · {outcome}")),
        StepState::Failed(reason) => format!("failed · {reason}"),
        StepState::WaitingOnYou => "waiting for your answer".into(),
        StepState::Working => "working now".into(),
        StepState::Unfinished => "the run ended before this step reported".into(),
    };
    let tip = if step.attempts > 1 {
        format!("attempt {} · {tip}", step.attempts)
    } else {
        tip
    };
    let live = matches!(step.state, StepState::Working | StepState::WaitingOnYou);
    div()
        .id(SharedString::from(id))
        .flex_none()
        .h_flex()
        .gap(px(5.0))
        .items_center()
        .h(px(22.0))
        .px(px(8.0))
        .rounded(px(999.0))
        .border_1()
        .border_color(if live {
            theme::stroke(tone)
        } else {
            theme::stroke(role).opacity(0.5)
        })
        .bg(if live {
            theme::tint(tone)
        } else {
            theme::tint(role)
        })
        .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
        .when(live, |el| el.child(ui::live_dot(tone)))
        .child(
            div()
                .text_size(px(10.5))
                .text_color(theme::text_primary())
                .child(step.label.clone()),
        )
        .children(icon.map(|i| Icon::new(i).size(px(10.0)).text_color(tone)))
}

fn step_row(prefix: &str, steps: &[Step]) -> Div {
    let mut row = div().h_flex().flex_wrap().gap(px(4.0)).items_center();
    for (i, step) in steps.iter().enumerate() {
        if i > 0 {
            row = row.child(div().w(px(6.0)).h(px(1.0)).bg(theme::graph_line()));
        }
        row = row.child(step_chip(format!("{prefix}-{i}"), step));
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
                PlanState::NotStarted => ("not started".to_string(), theme::slate(), None),
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

    fn render_subtask(task: &TaskView, path: &str) -> AnyElement {
        let (state, color) = match &task.iteration {
            mission::IterationState::Planned => ("Planned".to_string(), theme::text_muted()),
            mission::IterationState::Running => ("Running".to_string(), theme::accent()),
            mission::IterationState::Finished(outcome) => {
                (format!("Process finished · {outcome}"), theme::text_muted())
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
                item.child(step_row(path, &task.steps))
            })
            .children(
                task.subtasks
                    .iter()
                    .enumerate()
                    .map(|(index, child)| Self::render_subtask(child, &format!("{path}-s{index}"))),
            )
            .into_any_element()
    }

    fn render_task(&self, m: usize, t: usize, task: &TaskView) -> Div {
        let (label, role) = match task.status {
            RoadmapStatus::Completed if task.verified => ("verified", Semantic::Verified),
            RoadmapStatus::Completed => ("done · unverified", Semantic::External),
            RoadmapStatus::Running | RoadmapStatus::ReadyForVerification => {
                ("in progress", Semantic::Agent)
            },
            RoadmapStatus::Failed | RoadmapStatus::FailedVerification => {
                ("failed", Semantic::Failure)
            },
            RoadmapStatus::Paused => ("paused", Semantic::You),
            RoadmapStatus::Skipped => ("skipped", Semantic::External),
            RoadmapStatus::Pending => ("planned", Semantic::External),
        };
        let color = role.color();
        let live = task
            .steps
            .iter()
            .any(|s| matches!(s.state, StepState::Working | StepState::WaitingOnYou));
        div()
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
                el.child(
                    div()
                        .pl(px(17.0))
                        .child(step_row(&format!("t{m}-{t}"), &task.steps)),
                )
            })
            .when(!task.subtasks.is_empty(), |el| {
                el.child(ui::meta("Subtasks"))
                    .children(task.subtasks.iter().enumerate().map(|(index, child)| {
                        Self::render_subtask(child, &format!("t{m}-{t}-s{index}"))
                    }))
            })
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
            (None, true) => ("in progress", Semantic::Agent),
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
                        .child(step_row(&format!("m{index}"), &m.steps)),
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
        if mission
            .plan
            .iter()
            .any(|p| p.state != PlanState::NotStarted)
        {
            body = body.child(self.render_plan(mission));
        }
        if let Some(now) = &mission.now {
            body = body.child(
                ui::node_card(theme::accent())
                    .h_flex()
                    .gap(px(10.0))
                    .items_center()
                    .px(px(14.0))
                    .py(px(10.0))
                    .child(ui::live_dot(theme::accent()))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(theme::text_primary())
                            .child(format!("Working on  {now}")),
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
                    .child(step_row("run", &mission.steps)),
            );
        }
        if mission.milestones.is_empty() && mission.steps.is_empty() && mission.end.is_none() {
            body = body.child(ui::empty_state(
                "◎",
                "Nothing has run yet",
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
                StepState::Working => ("Working", theme::accent()),
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
            (Some(mission), _) => self.render_explorer(mission, cx).into_any_element(),
            (None, Some(error)) => {
                ui::empty_state("◌", "Could not read this run", error.clone()).into_any_element()
            },
            (None, None) => div()
                .text_size(px(12.0))
                .text_color(theme::text_muted())
                .child("Reading the run…")
                .into_any_element(),
        };
        div()
            .id("run-mission")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px(px(20.0))
            .py(px(16.0))
            .child(div().max_w(px(980.0)).child(content))
    }
}

#[cfg(test)]
mod hierarchy_tests {
    use super::{MissionPanel, MissionRuns};
    use gpui_kit::test::TestWindowExt;
    use gpui_kit::{AppContext as _, TestAppContext};
    use surge_core::keys::{NodeKey, SubgraphKey};
    use surge_core::loop_config::IterableSource;
    use surge_core::{ContentHash, EventPayload, RunId};

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
        let mut mission = crate::mission::fold(&[], &events, None);
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
            runs: MissionRuns {
                planning: None,
                implementation: RunId::new(),
            },
            mission: Some(mission),
            error: None,
            generation: 0,
            loaded_seq: None,
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
                Some(format!("{title}: Running").as_str())
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
                node: key("impl_task"),
                session,
                agent: "implementer@1.0".into(),
                agent_id: Some("claude".into()),
            },
            EventPayload::StageInputsResolved {
                node: key("impl_task"),
                bindings: [(
                    "architecture".into(),
                    ContentHash::compute(b"secret content must not display"),
                )]
                .into(),
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
                node: key("impl_task"),
                session: surge_core::SessionId::new(),
                agent: "reviewer@1.0".into(),
                agent_id: None,
            },
        ]);
        let panel = cx.new(|_| MissionPanel {
            runs: MissionRuns {
                planning: None,
                implementation: RunId::new(),
            },
            mission: Some(crate::mission::fold(&[], &events, None)),
            error: None,
            generation: 0,
            loaded_seq: None,
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
        let old = crate::mission::fold(
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
        let human = crate::mission::fold(
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
                    bindings: Default::default(),
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
