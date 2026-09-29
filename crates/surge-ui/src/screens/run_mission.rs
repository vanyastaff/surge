//! Overview tab: the mission level by level, each with its own flow.
//!
//! Plan (your approvals) → milestones (their own steps around the task
//! loop) → tasks (their own steps, retries visible) → final steps → result.
//! Folded by [`crate::mission::fold`] from the recorded events; reloaded
//! when the run's event sequence moves.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{Icon, IconName, StyledExt};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
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

pub(super) struct MissionPanel {
    runs: MissionRuns,
    mission: Option<Mission>,
    error: Option<String>,
    generation: u64,
    /// Event count the current fold reflects.
    loaded_seq: Option<String>,
}

impl MissionPanel {
    pub(super) fn new(runs: MissionRuns, cx: &mut Context<Self>) -> Self {
        let mut panel = Self {
            runs,
            mission: None,
            error: None,
            generation: 0,
            loaded_seq: None,
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

impl Render for MissionPanel {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let content: AnyElement = match (&self.mission, &self.error) {
            (Some(mission), _) => self.render_mission(mission).into_any_element(),
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
