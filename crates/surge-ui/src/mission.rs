//! A mission: one idea, planned and built — folded from the event logs.
//!
//! Surge works in levels, each with its own flow (see `docs/workflow.md`):
//! bootstrap (description → roadmap → flow, each approved by you), then the
//! roadmap's milestones — each running its own steps around an inner task
//! loop — and inside each milestone the tasks, each running its own steps
//! (spec → build → check …, with retries). Final steps run after the loops.
//!
//! [`fold`] turns the planning run's and the implementation run's events,
//! plus the run graph and the approved roadmap, into that hierarchy. It is
//! pure: every state shown comes from a recorded event or the plan itself.

use std::collections::{BTreeMap, HashMap};

use surge_core::graph::Graph;
use surge_core::node::{Node, NodeConfig};
use surge_core::roadmap::{RoadmapArtifact, RoadmapStatus};
use surge_core::{BootstrapDecision, BootstrapStage, EventPayload};

use crate::flow_diagram::{Lane, lane_of};

/// Where a step stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StepState {
    Working,
    WaitingOnYou,
    Done(String),
    Failed(String),
    /// Entered, but the run ended with no exit recorded.
    Unfinished,
}

/// One step of a level's own flow (a graph node as it ran).
#[derive(Clone, Debug, PartialEq)]
pub struct Step {
    pub node: String,
    pub label: String,
    pub lane: Lane,
    pub state: StepState,
    pub attempts: u32,
    /// The agent's own one-line report, when it gave one.
    pub summary: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TaskView {
    pub id: String,
    pub title: String,
    pub status: RoadmapStatus,
    pub verified: bool,
    pub steps: Vec<Step>,
    /// Found while doing another task.
    pub discovered_from: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MilestoneView {
    pub id: String,
    pub title: String,
    /// Steps the milestone runs itself (around its task loop).
    pub steps: Vec<Step>,
    pub tasks: Vec<TaskView>,
    pub started: bool,
    pub finished: Option<String>,
}

/// Bootstrap planning stage.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlanState {
    NotStarted,
    Drafting { edits: u32 },
    AwaitingYou { edits: u32 },
    Approved { edits: u32 },
    Rejected,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanStep {
    pub stage: BootstrapStage,
    pub state: PlanState,
}

/// How the implementation run ended, if it did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunEnd {
    Completed,
    Failed(String),
    Aborted(String),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mission {
    pub plan: Vec<PlanStep>,
    /// Steps outside any loop (before and after the milestones).
    pub steps: Vec<Step>,
    pub milestones: Vec<MilestoneView>,
    pub end: Option<RunEnd>,
    /// The step running right now, as "milestone / task / step" titles.
    pub now: Option<String>,
}

/// Loop scope while folding.
#[derive(Clone, Debug)]
enum Scope {
    Milestone {
        loop_id: String,
        index: usize,
    },
    Task {
        loop_id: String,
        milestone: usize,
        index: usize,
    },
}

/// Every node in the graph, subgraphs included.
fn all_nodes(graph: &Graph) -> HashMap<String, &Node> {
    let mut nodes: HashMap<String, &Node> = graph
        .nodes
        .iter()
        .map(|(k, n)| (k.as_str().to_string(), n))
        .collect();
    for sub in graph.subgraphs.values() {
        nodes.extend(sub.nodes.iter().map(|(k, n)| (k.as_str().to_string(), n)));
    }
    nodes
}

/// A node that is work someone does (agent or you) — not wiring
/// (loops, terminals, branches, notifications).
fn is_work(node: &Node) -> bool {
    matches!(node.config, NodeConfig::Agent(_) | NodeConfig::HumanGate(_))
}

/// "spec-author@1.0" → "Spec", "implementer@2.0" → "Build", …
fn step_label(node_id: &str, node: Option<&Node>) -> String {
    match node.map(|n| &n.config) {
        Some(NodeConfig::HumanGate(_)) => "Your approval".into(),
        Some(NodeConfig::Agent(agent)) => {
            let profile = agent.profile.to_string();
            let role = profile.split('@').next().unwrap_or(&profile).to_lowercase();
            match role.as_str() {
                r if r.contains("spec") => "Spec".into(),
                r if r.contains("implement") => "Build".into(),
                r if r.contains("verif") => "Check".into(),
                r if r.contains("review") => "Review".into(),
                r if r.contains("reproduce") => "Reproduce".into(),
                r if r.contains("plan") => "Plan".into(),
                r => humanize(r),
            }
        },
        _ => humanize(node_id),
    }
}

fn humanize(id: &str) -> String {
    let spaced = id.replace(['_', '-'], " ");
    let mut chars = spaced.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

fn item_str(item: &toml::Value, key: &str) -> Option<String> {
    item.get(key)
        .and_then(toml::Value::as_str)
        .map(str::to_string)
}

/// Seed milestones and tasks from the approved roadmap so work not yet
/// started is visible as planned.
fn seed(roadmap: Option<&RoadmapArtifact>) -> Vec<MilestoneView> {
    roadmap
        .map(|r| {
            r.milestones
                .iter()
                .map(|m| MilestoneView {
                    id: m.id.clone(),
                    title: m.title.clone(),
                    steps: Vec::new(),
                    tasks: m
                        .tasks
                        .iter()
                        .map(|t| TaskView {
                            id: t.id.clone(),
                            title: t.title.clone(),
                            status: RoadmapStatus::Pending,
                            verified: false,
                            steps: Vec::new(),
                            discovered_from: t.discovered_from.clone(),
                        })
                        .collect(),
                    started: false,
                    finished: None,
                })
                .collect()
        })
        .unwrap_or_default()
}

fn fold_plan(planning: &[EventPayload]) -> Vec<PlanStep> {
    let mut plan: Vec<PlanStep> = [
        BootstrapStage::Description,
        BootstrapStage::Roadmap,
        BootstrapStage::Flow,
    ]
    .into_iter()
    .map(|stage| PlanStep {
        stage,
        state: PlanState::NotStarted,
    })
    .collect();
    let edits_of = |s: &PlanState| match s {
        PlanState::Drafting { edits }
        | PlanState::AwaitingYou { edits }
        | PlanState::Approved { edits } => *edits,
        _ => 0,
    };
    for event in planning {
        let (stage, next): (BootstrapStage, fn(u32) -> PlanState) = match event {
            EventPayload::BootstrapStageStarted { stage } => {
                (*stage, |e| PlanState::Drafting { edits: e })
            },
            EventPayload::BootstrapApprovalRequested { stage, .. } => {
                (*stage, |e| PlanState::AwaitingYou { edits: e })
            },
            EventPayload::BootstrapApprovalDecided {
                stage, decision, ..
            } => match decision {
                BootstrapDecision::Approve => (*stage, |e| PlanState::Approved { edits: e }),
                BootstrapDecision::Reject => (*stage, |_| PlanState::Rejected),
                BootstrapDecision::Edit => (*stage, |e| PlanState::Drafting { edits: e + 1 }),
            },
            _ => continue,
        };
        if let Some(step) = plan.iter_mut().find(|p| p.stage == stage) {
            step.state = next(edits_of(&step.state));
        }
    }
    plan
}

/// Fold both runs into a mission.
pub fn fold(
    planning: &[EventPayload],
    implementation: &[EventPayload],
    roadmap: Option<&RoadmapArtifact>,
) -> Mission {
    // A run started before bootstrap operations were recorded carries its
    // planning stages in its own log.
    let plan_events = if planning.is_empty() {
        implementation
    } else {
        planning
    };
    let mut mission = Mission {
        plan: fold_plan(plan_events),
        milestones: seed(roadmap),
        ..Mission::default()
    };

    // The graph: first materialized, then any accepted revision.
    let mut graph: Option<Graph> = None;
    let mut scopes: Vec<Scope> = Vec::new();
    // node → where its latest attempt lives, for completion routing.
    let mut placed: BTreeMap<String, Option<Scope>> = BTreeMap::new();

    for event in implementation {
        match event {
            EventPayload::PipelineMaterialized { graph: g, .. } => {
                if graph.is_none() {
                    graph = Some((**g).clone());
                }
            },
            EventPayload::GraphRevisionAccepted { graph: g, .. } => graph = Some((**g).clone()),
            EventPayload::LoopIterationStarted {
                loop_id,
                item,
                index,
            } => {
                let loop_id = loop_id.as_str().to_string();
                // Leaving a previous iteration of the same loop.
                while scopes.last().is_some_and(|s| match s {
                    Scope::Milestone { loop_id: l, .. } | Scope::Task { loop_id: l, .. } => {
                        *l == loop_id
                    },
                }) {
                    scopes.pop();
                }
                let id = item_str(item, "id").unwrap_or_else(|| format!("#{}", index + 1));
                let title = item_str(item, "title").unwrap_or_else(|| id.clone());
                let is_milestone = item.get("tasks").is_some_and(toml::Value::is_array);
                if is_milestone {
                    let index = match mission.milestones.iter().position(|m| m.id == id) {
                        Some(i) => i,
                        None => {
                            mission.milestones.push(MilestoneView {
                                id: id.clone(),
                                title,
                                steps: Vec::new(),
                                tasks: Vec::new(),
                                started: false,
                                finished: None,
                            });
                            mission.milestones.len() - 1
                        },
                    };
                    mission.milestones[index].started = true;
                    scopes.push(Scope::Milestone { loop_id, index });
                } else {
                    // A task: inside the current milestone, or a single
                    // implicit one when the flow has only a task loop.
                    let milestone = match scopes.iter().rev().find_map(|s| match s {
                        Scope::Milestone { index, .. } => Some(*index),
                        _ => None,
                    }) {
                        Some(i) => i,
                        None => {
                            if mission.milestones.is_empty() {
                                mission.milestones.push(MilestoneView {
                                    id: "tasks".into(),
                                    title: "Tasks".into(),
                                    steps: Vec::new(),
                                    tasks: Vec::new(),
                                    started: true,
                                    finished: None,
                                });
                            }
                            0
                        },
                    };
                    let m = &mut mission.milestones[milestone];
                    m.started = true;
                    let index = match m.tasks.iter().position(|t| t.id == id) {
                        Some(i) => i,
                        None => {
                            m.tasks.push(TaskView {
                                id: id.clone(),
                                title,
                                status: RoadmapStatus::Pending,
                                verified: false,
                                steps: Vec::new(),
                                discovered_from: None,
                            });
                            m.tasks.len() - 1
                        },
                    };
                    if m.tasks[index].status == RoadmapStatus::Pending {
                        m.tasks[index].status = RoadmapStatus::Running;
                    }
                    scopes.push(Scope::Task {
                        loop_id,
                        milestone,
                        index,
                    });
                }
            },
            EventPayload::LoopIterationCompleted {
                loop_id, outcome, ..
            } => {
                let loop_id = loop_id.as_str();
                if let Some(pos) = scopes.iter().rposition(|s| match s {
                    Scope::Milestone { loop_id: l, .. } | Scope::Task { loop_id: l, .. } => {
                        l == loop_id
                    },
                }) {
                    if let Scope::Milestone { index, .. } = &scopes[pos] {
                        mission.milestones[*index].finished = Some(outcome.as_str().to_string());
                    }
                    scopes.truncate(pos);
                }
            },
            EventPayload::LoopCompleted { loop_id, .. } => {
                let loop_id = loop_id.as_str();
                if let Some(pos) = scopes.iter().position(|s| match s {
                    Scope::Milestone { loop_id: l, .. } | Scope::Task { loop_id: l, .. } => {
                        l == loop_id
                    },
                }) {
                    scopes.truncate(pos);
                }
            },
            EventPayload::StageEntered { node, attempt } => {
                let id = node.as_str().to_string();
                let nodes = graph.as_ref().map(all_nodes).unwrap_or_default();
                let found = nodes.get(&id).copied();
                if found.is_some_and(|n| !is_work(n)) {
                    continue;
                }
                let scope = scopes.last().cloned();
                let steps = steps_at(&mut mission, scope.as_ref());
                let label = step_label(&id, found);
                let lane = found.map_or(Lane::Build, lane_of);
                // A fresh attempt inside the same scope updates the step; a
                // re-entry after other steps (a retry loop) is its own entry.
                match steps.last_mut().filter(|s| s.node == id) {
                    Some(step) => {
                        step.state = StepState::Working;
                        step.attempts = step.attempts.max(*attempt);
                    },
                    None => steps.push(Step {
                        node: id.clone(),
                        label,
                        lane,
                        state: StepState::Working,
                        attempts: (*attempt).max(1),
                        summary: None,
                    }),
                }
                placed.insert(id, scope);
            },
            EventPayload::HumanInputRequested { node, .. } => {
                with_step(&mut mission, &placed, node.as_str(), |s| {
                    s.state = StepState::WaitingOnYou
                });
            },
            EventPayload::HumanInputResolved { node, .. } => {
                with_step(&mut mission, &placed, node.as_str(), |s| {
                    s.state = StepState::Working
                });
            },
            EventPayload::OutcomeReported { node, summary, .. } => {
                let summary = summary.trim().to_string();
                with_step(&mut mission, &placed, node.as_str(), |s| {
                    if !summary.is_empty() {
                        s.summary = Some(summary.clone());
                    }
                });
            },
            EventPayload::StageCompleted { node, outcome } => {
                let outcome = outcome.as_str().to_string();
                with_step(&mut mission, &placed, node.as_str(), |s| {
                    s.state = StepState::Done(outcome.clone())
                });
            },
            EventPayload::StageFailed { node, reason, .. } => {
                let reason = reason.clone();
                with_step(&mut mission, &placed, node.as_str(), |s| {
                    s.state = StepState::Failed(reason.clone())
                });
            },
            EventPayload::TaskStatusChanged { task_id, to, .. } => {
                if let Some(task) = task_mut(&mut mission, task_id) {
                    task.status = *to;
                }
            },
            EventPayload::TaskVerified { task_id, .. } => {
                if let Some(task) = task_mut(&mut mission, task_id) {
                    task.status = RoadmapStatus::Completed;
                    task.verified = true;
                }
            },
            EventPayload::TaskDiscovered {
                task_id,
                discovered_from,
                title,
            } => {
                if task_mut(&mut mission, task_id).is_none() {
                    let milestone = mission
                        .milestones
                        .iter()
                        .position(|m| m.tasks.iter().any(|t| &t.id == discovered_from))
                        .or_else(|| mission.milestones.len().checked_sub(1));
                    if let Some(i) = milestone {
                        mission.milestones[i].tasks.push(TaskView {
                            id: task_id.clone(),
                            title: title.clone(),
                            status: RoadmapStatus::Pending,
                            verified: false,
                            steps: Vec::new(),
                            discovered_from: Some(discovered_from.clone()),
                        });
                    }
                }
            },
            EventPayload::RunCompleted { .. } => mission.end = Some(RunEnd::Completed),
            EventPayload::RunFailed { error } => mission.end = Some(RunEnd::Failed(error.clone())),
            EventPayload::RunAborted { reason } => {
                mission.end = Some(RunEnd::Aborted(reason.clone()))
            },
            _ => {},
        }
    }

    // A finished run has nothing working any more; say so honestly.
    if mission.end.is_some() {
        let settle = |s: &mut Step| {
            if matches!(s.state, StepState::Working | StepState::WaitingOnYou) {
                s.state = StepState::Unfinished;
            }
        };
        mission.steps.iter_mut().for_each(settle);
        for m in &mut mission.milestones {
            m.steps.iter_mut().for_each(settle);
            for t in &mut m.tasks {
                t.steps.iter_mut().for_each(settle);
            }
        }
    }
    mission.now = now(&mission);
    mission
}

fn steps_at<'a>(mission: &'a mut Mission, scope: Option<&Scope>) -> &'a mut Vec<Step> {
    match scope {
        None => &mut mission.steps,
        Some(Scope::Milestone { index, .. }) => &mut mission.milestones[*index].steps,
        Some(Scope::Task {
            milestone, index, ..
        }) => &mut mission.milestones[*milestone].tasks[*index].steps,
    }
}

fn with_step(
    mission: &mut Mission,
    placed: &BTreeMap<String, Option<Scope>>,
    node: &str,
    f: impl FnOnce(&mut Step),
) {
    let Some(scope) = placed.get(node) else {
        return;
    };
    if let Some(step) = steps_at(mission, scope.as_ref())
        .iter_mut()
        .rev()
        .find(|s| s.node == node)
    {
        f(step);
    }
}

fn task_mut<'a>(mission: &'a mut Mission, id: &str) -> Option<&'a mut TaskView> {
    mission
        .milestones
        .iter_mut()
        .flat_map(|m| m.tasks.iter_mut())
        .find(|t| t.id == id)
}

/// A plain headline for a run's terminal error; the raw text stays
/// available underneath for developers.
pub fn failure_headline(error: &str) -> &'static str {
    let e = error.to_lowercase();
    if e.contains("handshake") || e.contains("open_session") || e.contains("spawn") {
        "The coding agent did not start"
    } else if e.contains("rate limit") || e.contains("quota") || e.contains("429") {
        "The agent hit its usage limit"
    } else if e.contains("budget") {
        "The budget ran out"
    } else if e.contains("timed out") || e.contains("timeout") {
        "A step took too long"
    } else if e.contains("verif") {
        "A check did not pass"
    } else {
        "Stopped with an error"
    }
}

/// "Timer core › Countdown › Build", for the step working right now.
fn now(mission: &Mission) -> Option<String> {
    let live = |s: &Step| matches!(s.state, StepState::Working | StepState::WaitingOnYou);
    for m in &mission.milestones {
        for t in &m.tasks {
            if let Some(s) = t.steps.iter().rev().find(|s| live(s)) {
                return Some(format!("{} › {} › {}", m.title, t.title, s.label));
            }
        }
        if let Some(s) = m.steps.iter().rev().find(|s| live(s)) {
            return Some(format!("{} › {}", m.title, s.label));
        }
    }
    mission
        .steps
        .iter()
        .rev()
        .find(|s| live(s))
        .map(|s| s.label.clone())
}

#[cfg(test)]
mod tests {
    use super::{PlanState, RunEnd, StepState, fold};
    use surge_core::keys::{NodeKey, OutcomeKey};
    use surge_core::roadmap::RoadmapStatus;
    use surge_core::{BootstrapDecision, BootstrapStage, ContentHash, EventPayload};

    fn graph() -> surge_core::graph::Graph {
        toml::from_str(include_str!("../testdata/nested_loops_flow.toml")).expect("fixture parses")
    }

    fn key(s: &str) -> NodeKey {
        s.try_into().unwrap()
    }
    fn out(s: &str) -> OutcomeKey {
        s.try_into().unwrap()
    }
    fn enter(n: &str, attempt: u32) -> EventPayload {
        EventPayload::StageEntered {
            node: key(n),
            attempt,
        }
    }
    fn done(n: &str, o: &str) -> EventPayload {
        EventPayload::StageCompleted {
            node: key(n),
            outcome: out(o),
        }
    }
    fn iter(loop_id: &str, item: &str, index: u32) -> EventPayload {
        EventPayload::LoopIterationStarted {
            loop_id: key(loop_id),
            item: toml::from_str(item).unwrap(),
            index,
        }
    }

    #[test]
    fn steps_land_on_their_own_level_and_wiring_is_hidden() {
        let g = graph();
        let events = vec![
            EventPayload::PipelineMaterialized {
                graph: Box::new(g),
                graph_hash: ContentHash::compute(b"g"),
            },
            enter("milestone_loop", 1),
            iter(
                "milestone_loop",
                "id = 'm1'\ntitle = 'Timer core'\ntasks = []",
                0,
            ),
            enter("task_loop", 1),
            iter("task_loop", "id = 't1'\ntitle = 'Countdown'", 0),
            enter("spec_task", 1),
            done("spec_task", "pass"),
            enter("impl_task", 1),
            done("impl_task", "pass"),
            enter("verify_task", 1),
            EventPayload::StageFailed {
                node: key("verify_task"),
                reason: "2 tests fail".into(),
                retry_available: true,
            },
            enter("impl_task", 2),
            done("impl_task", "pass"),
            enter("verify_task", 2),
            done("verify_task", "pass"),
            EventPayload::TaskVerified {
                task_id: "t1".into(),
                node: key("verify_task"),
                evidence: ContentHash::compute(b"r"),
            },
            enter("task_end", 1),
            EventPayload::LoopIterationCompleted {
                loop_id: key("task_loop"),
                index: 0,
                outcome: out("completed"),
            },
            EventPayload::LoopIterationCompleted {
                loop_id: key("milestone_loop"),
                index: 0,
                outcome: out("completed"),
            },
            enter("final_verify", 1),
        ];
        let mission = fold(&[], &events, None);

        assert_eq!(mission.milestones.len(), 1);
        let m = &mission.milestones[0];
        assert_eq!(m.title, "Timer core");
        assert_eq!(m.finished.as_deref(), Some("completed"));
        let t = &m.tasks[0];
        assert!(t.verified);
        assert_eq!(t.status, RoadmapStatus::Completed);
        // The retry is visible as its own entries, in order.
        let labels: Vec<&str> = t.steps.iter().map(|s| s.label.as_str()).collect();
        assert_eq!(labels, ["Spec", "Build", "Check", "Build", "Check"]);
        assert!(matches!(t.steps[2].state, StepState::Failed(ref r) if r == "2 tests fail"));
        // Loops and terminals are wiring, not steps.
        assert!(!t.steps.iter().any(|s| s.node == "task_end"));
        assert!(m.steps.iter().all(|s| s.node != "task_loop"));
        // The final check ran outside the loops and is working now.
        assert_eq!(mission.steps.len(), 1);
        assert_eq!(mission.now.as_deref(), Some("Check"));
    }

    #[test]
    fn a_finished_run_leaves_nothing_working() {
        let events = vec![
            iter("task_loop", "id = 't1'\ntitle = 'Only task'", 0),
            enter("impl_task", 1),
            EventPayload::RunCompleted {
                terminal_node: key("end"),
            },
        ];
        let mission = fold(&[], &events, None);
        assert_eq!(mission.end, Some(RunEnd::Completed));
        let step = &mission.milestones[0].tasks[0].steps[0];
        assert_eq!(step.state, StepState::Unfinished);
        assert!(mission.now.is_none());
    }

    #[test]
    fn plan_stages_track_drafts_edits_and_your_approval() {
        let planning = vec![
            EventPayload::BootstrapStageStarted {
                stage: BootstrapStage::Description,
            },
            EventPayload::BootstrapApprovalRequested {
                stage: BootstrapStage::Description,
                channel: surge_core::approvals::ApprovalChannel::Email { to_ref: "x".into() },
            },
            EventPayload::BootstrapApprovalDecided {
                stage: BootstrapStage::Description,
                decision: BootstrapDecision::Edit,
                comment: None,
            },
            EventPayload::BootstrapApprovalRequested {
                stage: BootstrapStage::Description,
                channel: surge_core::approvals::ApprovalChannel::Email { to_ref: "x".into() },
            },
            EventPayload::BootstrapApprovalDecided {
                stage: BootstrapStage::Description,
                decision: BootstrapDecision::Approve,
                comment: None,
            },
            EventPayload::BootstrapStageStarted {
                stage: BootstrapStage::Roadmap,
            },
            EventPayload::BootstrapApprovalRequested {
                stage: BootstrapStage::Roadmap,
                channel: surge_core::approvals::ApprovalChannel::Email { to_ref: "x".into() },
            },
        ];
        let mission = fold(&planning, &[], None);
        assert_eq!(mission.plan[0].state, PlanState::Approved { edits: 1 });
        assert_eq!(mission.plan[1].state, PlanState::AwaitingYou { edits: 0 });
        assert_eq!(mission.plan[2].state, PlanState::NotStarted);
    }

    #[test]
    fn planned_tasks_show_before_they_start() {
        let roadmap: surge_core::roadmap::RoadmapArtifact = toml::from_str(
            "schema_version = 2\n[[milestones]]\nid = 'm1'\ntitle = 'M'\nstatus = 'pending'\n[[milestones.tasks]]\nid = 'a'\ntitle = 'A'\nstatus = 'pending'\n[[milestones.tasks]]\nid = 'b'\ntitle = 'B'\nstatus = 'pending'\n",
        )
        .unwrap();
        let mission = fold(&[], &[], Some(&roadmap));
        assert_eq!(mission.milestones[0].tasks.len(), 2);
        assert!(!mission.milestones[0].started);
    }
}
