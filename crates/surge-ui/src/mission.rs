//! A mission: one idea, planned and built — folded from the event logs.
//!
//! Surge works in levels, each with its own flow (see `docs/workflow.md`):
//! bootstrap (description → roadmap → flow, each approved by you), then the
//! roadmap's milestones — each running its own steps around an inner task
//! loop — and inside each milestone the tasks, each running its own steps
//! (spec → build → check …, with retries) and any recorded subtask loops.
//! Final steps run after the loops.
//!
//! [`fold`] turns the planning run's and the implementation run's events,
//! plus the run graph and the approved roadmap, into that hierarchy. It is
//! pure: every state shown comes from a recorded event or the plan itself.

use std::collections::{BTreeMap, HashMap};

use surge_core::graph::Graph;
use surge_core::node::{Node, NodeConfig};
use surge_core::roadmap::{MilestoneId, RoadmapArtifact, RoadmapStatus, RoadmapTaskId};
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

/// Participant identity recorded when the stage opened its actual session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedParticipant {
    pub profile: String,
    pub runtime: Option<String>,
    pub session: surge_core::SessionId,
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
    pub participant: Option<RecordedParticipant>,
    /// Binding names only; no hashes or resolved content are presented.
    pub inputs: Option<Vec<String>>,
    pub skills: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IterationState {
    Planned,
    Running,
    Finished(String),
    Unfinished,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TaskView {
    pub id: RoadmapTaskId,
    pub title: String,
    pub status: RoadmapStatus,
    pub verified: bool,
    pub steps: Vec<Step>,
    pub subtasks: Vec<TaskView>,
    pub iteration: IterationState,
    /// Found while doing another task.
    pub discovered_from: Option<RoadmapTaskId>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MilestoneView {
    pub id: MilestoneId,
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
#[derive(Clone, Debug, PartialEq, Eq)]
enum Scope {
    Milestone {
        loop_id: String,
        index: usize,
    },
    Task {
        loop_id: String,
        milestone: usize,
        index: usize,
        subtasks: Vec<usize>,
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
                            subtasks: Vec::new(),
                            iteration: IterationState::Planned,
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
                if let Some(Scope::Task {
                    milestone,
                    index: task_index,
                    subtasks,
                    ..
                }) = scopes.last().cloned()
                {
                    if !is_subtask_loop(graph.as_ref(), &loop_id) {
                        continue;
                    }
                    let (Some(id), Some(title)) = (item_str(item, "id"), item_str(item, "title"))
                    else {
                        continue;
                    };
                    let parent = task_at(&mut mission, milestone, task_index, &subtasks);
                    let child_index = match parent
                        .subtasks
                        .iter()
                        .position(|task| task.id.as_str() == id)
                    {
                        Some(index) => index,
                        None => {
                            parent.subtasks.push(TaskView {
                                id: id.into(),
                                title,
                                status: RoadmapStatus::Pending,
                                verified: false,
                                steps: Vec::new(),
                                subtasks: Vec::new(),
                                iteration: IterationState::Planned,
                                discovered_from: None,
                            });
                            parent.subtasks.len() - 1
                        },
                    };
                    parent.subtasks[child_index].iteration = IterationState::Running;
                    let mut child_path = subtasks;
                    child_path.push(child_index);
                    scopes.push(Scope::Task {
                        loop_id,
                        milestone,
                        index: task_index,
                        subtasks: child_path,
                    });
                    continue;
                }
                let id = item_str(item, "id").unwrap_or_else(|| format!("#{}", index + 1));
                let title = item_str(item, "title").unwrap_or_else(|| id.clone());
                let is_milestone = item.get("tasks").is_some_and(toml::Value::is_array);
                if is_milestone {
                    let index = match mission.milestones.iter().position(|m| m.id.as_str() == id) {
                        Some(i) => i,
                        None => {
                            mission.milestones.push(MilestoneView {
                                id: id.clone().into(),
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
                    let index = match m.tasks.iter().position(|t| t.id.as_str() == id) {
                        Some(i) => i,
                        None => {
                            m.tasks.push(TaskView {
                                id: id.clone().into(),
                                title,
                                status: RoadmapStatus::Pending,
                                verified: false,
                                steps: Vec::new(),
                                subtasks: Vec::new(),
                                iteration: IterationState::Planned,
                                discovered_from: None,
                            });
                            m.tasks.len() - 1
                        },
                    };
                    if m.tasks[index].status == RoadmapStatus::Pending {
                        m.tasks[index].status = RoadmapStatus::Running;
                    }
                    m.tasks[index].iteration = IterationState::Running;
                    scopes.push(Scope::Task {
                        loop_id,
                        milestone,
                        index,
                        subtasks: Vec::new(),
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
                    match &scopes[pos] {
                        Scope::Milestone { index, .. } => {
                            mission.milestones[*index].finished = Some(outcome.as_str().to_string())
                        },
                        Scope::Task {
                            milestone,
                            index,
                            subtasks,
                            ..
                        } => {
                            task_at(&mut mission, *milestone, *index, subtasks).iteration =
                                IterationState::Finished(outcome.as_str().to_string());
                        },
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
                        step.attempts = (*attempt).max(1);
                        step.summary = None;
                        step.participant = None;
                        step.inputs = None;
                        step.skills.clear();
                    },
                    None => steps.push(Step {
                        node: id.clone(),
                        label,
                        lane,
                        state: StepState::Working,
                        attempts: (*attempt).max(1),
                        summary: None,
                        participant: None,
                        inputs: None,
                        skills: Vec::new(),
                    }),
                }
                placed.insert(id, scope);
            },
            EventPayload::SessionOpened {
                node,
                session,
                agent,
                agent_id,
            } => {
                with_active_step(
                    &mut mission,
                    &placed,
                    scopes.last(),
                    node.as_str(),
                    |step| {
                        step.participant = Some(RecordedParticipant {
                            profile: agent.clone(),
                            runtime: agent_id.clone(),
                            session: *session,
                        });
                    },
                );
            },
            EventPayload::StageInputsResolved { node, bindings } => {
                with_active_step(
                    &mut mission,
                    &placed,
                    scopes.last(),
                    node.as_str(),
                    |step| {
                        step.inputs = Some(bindings.keys().cloned().collect());
                    },
                );
            },
            EventPayload::SkillBound { node, name, .. } => {
                with_active_step(
                    &mut mission,
                    &placed,
                    scopes.last(),
                    node.as_str(),
                    |step| {
                        if !step.skills.contains(name) {
                            step.skills.push(name.clone());
                        }
                    },
                );
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
                if let Some(task) = scoped_task_mut(&mut mission, &scopes, task_id) {
                    task.status = *to;
                }
            },
            EventPayload::TaskVerified { task_id, .. } => {
                if let Some(task) = scoped_task_mut(&mut mission, &scopes, task_id) {
                    task.status = RoadmapStatus::Completed;
                    task.verified = true;
                }
            },
            EventPayload::TaskDiscovered {
                task_id,
                discovered_from,
                title,
            } => {
                if task_occurrences(&mission, task_id) == 0 {
                    let milestone = mission
                        .milestones
                        .iter()
                        .position(|m| m.tasks.iter().any(|t| t.id == *discovered_from))
                        .or_else(|| mission.milestones.len().checked_sub(1));
                    if let Some(i) = milestone {
                        mission.milestones[i].tasks.push(TaskView {
                            id: task_id.clone(),
                            title: title.clone(),
                            status: RoadmapStatus::Pending,
                            verified: false,
                            steps: Vec::new(),
                            subtasks: Vec::new(),
                            iteration: IterationState::Planned,
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
            settle_tasks(&mut m.tasks);
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
            milestone,
            index,
            subtasks,
            ..
        }) => &mut task_at(mission, *milestone, *index, subtasks).steps,
    }
}

fn with_active_step(
    mission: &mut Mission,
    placed: &BTreeMap<String, Option<Scope>>,
    scope: Option<&Scope>,
    node: &str,
    update: impl FnOnce(&mut Step),
) {
    // These events carry a node but no attempt or ancestry identity. Only
    // the current scoped, open occurrence can own them; never a prior task.
    if placed
        .get(node)
        .is_none_or(|placed_scope| placed_scope.as_ref() != scope)
    {
        return;
    }
    with_step(mission, placed, node, |step| {
        if matches!(step.state, StepState::Working | StepState::WaitingOnYou) {
            update(step);
        }
    });
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

fn is_subtask_loop(graph: Option<&Graph>, loop_id: &str) -> bool {
    use surge_core::loop_config::IterableSource;
    let Some(node) = graph.and_then(|graph| all_nodes(graph).get(loop_id).copied()) else {
        return false;
    };
    let NodeConfig::Loop(config) = &node.config else {
        return false;
    };
    if config.iteration_var_name == "subtask" {
        return true;
    }
    let path = match &config.iterates_over {
        IterableSource::RunArtifact { jsonpath, .. }
        | IterableSource::Artifact { jsonpath, .. }
        | IterableSource::LoopItem { jsonpath, .. } => jsonpath,
        IterableSource::Static(_) => return false,
    };
    let path = path.trim().strip_prefix('$').unwrap_or(path.trim());
    path.split('.')
        .any(|segment| segment.strip_suffix("[*]").unwrap_or(segment) == "subtasks")
}

fn task_at<'a>(
    mission: &'a mut Mission,
    milestone: usize,
    index: usize,
    subtasks: &[usize],
) -> &'a mut TaskView {
    let mut task = &mut mission.milestones[milestone].tasks[index];
    for child_index in subtasks {
        task = &mut task.subtasks[*child_index];
    }
    task
}

fn find_task<'a>(tasks: &'a mut [TaskView], id: &RoadmapTaskId) -> Option<&'a mut TaskView> {
    for task in tasks {
        if task.id == *id {
            return Some(task);
        }
        if let Some(child) = find_task(&mut task.subtasks, id) {
            return Some(child);
        }
    }
    None
}

fn count_tasks(tasks: &[TaskView], id: &RoadmapTaskId) -> usize {
    tasks
        .iter()
        .map(|task| usize::from(task.id == *id) + count_tasks(&task.subtasks, id))
        .sum()
}

fn task_occurrences(mission: &Mission, id: &RoadmapTaskId) -> usize {
    mission
        .milestones
        .iter()
        .map(|milestone| count_tasks(&milestone.tasks, id))
        .sum()
}

fn task_mut<'a>(mission: &'a mut Mission, id: &RoadmapTaskId) -> Option<&'a mut TaskView> {
    // An unscoped ledger event with a repeated id cannot identify its target.
    if task_occurrences(mission, id) != 1 {
        return None;
    }
    mission
        .milestones
        .iter_mut()
        .find_map(|milestone| find_task(&mut milestone.tasks, id))
}

fn scoped_task_mut<'a>(
    mission: &'a mut Mission,
    scopes: &[Scope],
    id: &RoadmapTaskId,
) -> Option<&'a mut TaskView> {
    let active = scopes.iter().rev().find_map(|scope| {
        if let Scope::Task {
            milestone,
            index,
            subtasks,
            ..
        } = scope
            && task_at(mission, *milestone, *index, subtasks).id == *id
        {
            Some((*milestone, *index, subtasks.clone()))
        } else {
            None
        }
    });
    if let Some((milestone, index, subtasks)) = active {
        return Some(task_at(mission, milestone, index, &subtasks));
    }
    task_mut(mission, id)
}

fn settle_tasks(tasks: &mut [TaskView]) {
    for task in tasks {
        for step in &mut task.steps {
            if matches!(step.state, StepState::Working | StepState::WaitingOnYou) {
                step.state = StepState::Unfinished;
            }
        }
        if task.iteration == IterationState::Running {
            task.iteration = IterationState::Unfinished;
        }
        settle_tasks(&mut task.subtasks);
    }
}

fn active_task(task: &TaskView) -> Option<String> {
    for child in &task.subtasks {
        if let Some(active) = active_task(child) {
            return Some(format!("{} › {active}", task.title));
        }
    }
    task.steps
        .iter()
        .rev()
        .find(|step| matches!(step.state, StepState::Working | StepState::WaitingOnYou))
        .map(|step| format!("{} › {}", task.title, step.label))
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
            if let Some(active) = active_task(t) {
                return Some(format!("{} › {active}", m.title));
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
    fn subtask_graph() -> surge_core::graph::Graph {
        use surge_core::loop_config::IterableSource;
        let mut graph = graph();
        let task_body: surge_core::keys::SubgraphKey = "task_body".try_into().unwrap();
        let child_body: surge_core::keys::SubgraphKey = "subtask_body".try_into().unwrap();
        let child = graph.subgraphs[&task_body].clone();
        graph.subgraphs.insert(child_body.clone(), child);
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
            .get_mut(&task_body)
            .unwrap()
            .nodes
            .insert(loop_node.id.clone(), loop_node);
        graph
    }

    fn iteration_completed(loop_id: &str) -> EventPayload {
        EventPayload::LoopIterationCompleted {
            loop_id: key(loop_id),
            index: 0,
            outcome: out("completed"),
        }
    }

    #[test]
    fn nested_subtasks_preserve_parent_steps_and_repeated_child_ids() {
        let events = vec![
            EventPayload::PipelineMaterialized {
                graph: Box::new(subtask_graph()),
                graph_hash: ContentHash::compute(b"subtasks"),
            },
            iter(
                "milestone_loop",
                "id = 'm1'\ntitle = 'Timer'\ntasks = []",
                0,
            ),
            iter("task_loop", "id = 't1'\ntitle = 'First parent'", 0),
            enter("spec_task", 1),
            done("spec_task", "drafted"),
            iter(
                "subtask_loop",
                "id = 'leaf'\ntitle = 'First child with its full descriptive title'",
                0,
            ),
            enter("impl_task", 1),
            done("impl_task", "pass"),
            iteration_completed("subtask_loop"),
            enter("verify_task", 1),
            done("verify_task", "pass"),
            iteration_completed("task_loop"),
            iter("task_loop", "id = 't2'\ntitle = 'Second parent'", 1),
            iter("subtask_loop", "id = 'leaf'\ntitle = 'Second child'", 0),
            enter("impl_task", 1),
            EventPayload::TaskStatusChanged {
                task_id: "leaf".into(),
                from: RoadmapStatus::Pending,
                to: RoadmapStatus::Failed,
                authority_node: key("impl_task"),
            },
        ];
        let mission = fold(&[], &events, None);
        let milestone = &mission.milestones[0];
        assert_eq!(milestone.tasks.len(), 2);
        let first = &milestone.tasks[0];
        let second = &milestone.tasks[1];
        assert_eq!(
            first
                .steps
                .iter()
                .map(|step| step.node.as_str())
                .collect::<Vec<_>>(),
            ["spec_task", "verify_task"]
        );
        assert_eq!(first.subtasks.len(), 1);
        assert_eq!(first.subtasks[0].steps[0].node, "impl_task");
        assert_eq!(
            first.subtasks[0].iteration,
            super::IterationState::Finished("completed".into())
        );
        assert!(!first.subtasks[0].verified);
        assert_eq!(first.subtasks[0].status, RoadmapStatus::Pending);
        assert_eq!(second.subtasks[0].status, RoadmapStatus::Failed);
        assert_eq!(
            mission.now.as_deref(),
            Some("Timer › Second parent › Second child › Build")
        );
        let mut scoped_events = events.clone();
        scoped_events.push(EventPayload::TaskVerified {
            task_id: "leaf".into(),
            node: key("verify_task"),
            evidence: ContentHash::compute(b"second child scoped"),
        });
        let scoped = fold(&[], &scoped_events, None);
        assert!(!scoped.milestones[0].tasks[0].subtasks[0].verified);
        assert!(scoped.milestones[0].tasks[1].subtasks[0].verified);
        let mut late_events = events.clone();
        late_events.push(iteration_completed("subtask_loop"));
        late_events.push(iteration_completed("task_loop"));
        late_events.push(iteration_completed("milestone_loop"));
        late_events.push(EventPayload::TaskVerified {
            task_id: "leaf".into(),
            node: key("verify_task"),
            evidence: ContentHash::compute(b"ambiguous"),
        });
        let late = fold(&[], &late_events, None);
        assert!(!late.milestones[0].tasks[0].subtasks[0].verified);
        assert!(!late.milestones[0].tasks[1].subtasks[0].verified);
    }

    #[test]
    fn unrelated_nested_iterator_stays_in_parent_and_terminal_failure_settles_children() {
        let mut graph = subtask_graph();
        let task_body: surge_core::keys::SubgraphKey = "task_body".try_into().unwrap();
        let mut retry = graph.subgraphs[&task_body].nodes[&key("subtask_loop")].clone();
        retry.id = key("retry_loop");
        let surge_core::node::NodeConfig::Loop(config) = &mut retry.config else {
            panic!("fixture node is a loop");
        };
        config.iteration_var_name = "attempt".into();
        config.iterates_over = surge_core::loop_config::IterableSource::RunArtifact {
            name: "retry".into(),
            jsonpath: "not_subtasks".into(),
        };
        graph
            .subgraphs
            .get_mut(&task_body)
            .unwrap()
            .nodes
            .insert(retry.id.clone(), retry);
        let events = vec![
            EventPayload::PipelineMaterialized {
                graph: Box::new(graph),
                graph_hash: ContentHash::compute(b"subtasks"),
            },
            iter(
                "milestone_loop",
                "id = 'm1'\ntitle = 'Timer'\ntasks = []",
                0,
            ),
            iter("task_loop", "id = 't1'\ntitle = 'Parent'", 0),
            iter("retry_loop", "id = 'retry'\ntitle = 'Attempt'", 0),
            enter("spec_task", 1),
            done("spec_task", "drafted"),
            iteration_completed("retry_loop"),
            iter("subtask_loop", "id = 'leaf'\ntitle = 'Child'", 0),
            enter("impl_task", 1),
            EventPayload::RunFailed {
                error: "provider disconnected".into(),
            },
        ];
        let mission = fold(&[], &events, None);
        assert_eq!(mission.milestones[0].tasks.len(), 1);
        let parent = &mission.milestones[0].tasks[0];
        assert_eq!(parent.steps[0].node, "spec_task");
        assert_eq!(parent.subtasks.len(), 1);
        assert_eq!(
            parent.subtasks[0].iteration,
            super::IterationState::Unfinished
        );
        assert_eq!(parent.subtasks[0].steps[0].state, StepState::Unfinished);
        assert!(!parent.subtasks[0].verified);
        assert!(mission.now.is_none());
    }

    #[test]
    fn subtask_classifier_requires_an_exact_path_segment() {
        use surge_core::loop_config::IterableSource;
        let mut graph = subtask_graph();
        let body: surge_core::keys::SubgraphKey = "task_body".parse().unwrap();
        let node = graph
            .subgraphs
            .get_mut(&body)
            .unwrap()
            .nodes
            .get_mut(&key("subtask_loop"))
            .unwrap();
        let surge_core::node::NodeConfig::Loop(config) = &mut node.config else {
            panic!("loop fixture");
        };
        config.iteration_var_name = "item".into();
        for path in ["spec.retry_subtasks", "spec.subtasks_count"] {
            let node = graph
                .subgraphs
                .get_mut(&body)
                .unwrap()
                .nodes
                .get_mut(&key("subtask_loop"))
                .unwrap();
            let surge_core::node::NodeConfig::Loop(config) = &mut node.config else {
                panic!("loop fixture");
            };
            config.iterates_over = IterableSource::Artifact {
                node: key("spec_task"),
                name: "spec".into(),
                jsonpath: path.into(),
            };
            assert!(!super::is_subtask_loop(Some(&graph), "subtask_loop"));
        }
        let node = graph
            .subgraphs
            .get_mut(&body)
            .unwrap()
            .nodes
            .get_mut(&key("subtask_loop"))
            .unwrap();
        let surge_core::node::NodeConfig::Loop(config) = &mut node.config else {
            panic!("loop fixture");
        };
        config.iterates_over = IterableSource::Artifact {
            node: key("spec_task"),
            name: "spec".into(),
            jsonpath: "spec.subtasks".into(),
        };
        assert!(super::is_subtask_loop(Some(&graph), "subtask_loop"));
        let node = graph
            .subgraphs
            .get_mut(&body)
            .unwrap()
            .nodes
            .get_mut(&key("subtask_loop"))
            .unwrap();
        let surge_core::node::NodeConfig::Loop(config) = &mut node.config else {
            panic!("loop fixture");
        };
        config.iterates_over = IterableSource::Artifact {
            node: key("spec_task"),
            name: "spec".into(),
            jsonpath: "$.subtasks[*]".into(),
        };
        assert!(super::is_subtask_loop(Some(&graph), "subtask_loop"));
        let node = graph
            .subgraphs
            .get_mut(&body)
            .unwrap()
            .nodes
            .get_mut(&key("subtask_loop"))
            .unwrap();
        let surge_core::node::NodeConfig::Loop(config) = &mut node.config else {
            panic!("loop fixture");
        };
        config.iterates_over = IterableSource::Static(Vec::new());
        assert!(!super::is_subtask_loop(Some(&graph), "subtask_loop"));
        let node = graph
            .subgraphs
            .get_mut(&body)
            .unwrap()
            .nodes
            .get_mut(&key("subtask_loop"))
            .unwrap();
        let surge_core::node::NodeConfig::Loop(config) = &mut node.config else {
            panic!("loop fixture");
        };
        config.iteration_var_name = "subtask".into();
        assert!(super::is_subtask_loop(Some(&graph), "subtask_loop"));
    }
    #[test]
    fn third_level_loop_keeps_subtask_under_parent() {
        let events = vec![
            EventPayload::PipelineMaterialized {
                graph: Box::new(subtask_graph()),
                graph_hash: ContentHash::compute(b"subtasks"),
            },
            iter(
                "milestone_loop",
                "id = 'm1'\ntitle = 'Timer'\ntasks = []",
                0,
            ),
            iter("task_loop", "id = 't1'\ntitle = 'Parent'", 0),
            iter("subtask_loop", "id = 'leaf'\ntitle = 'Child'", 0),
            enter("impl_task", 1),
            done("impl_task", "pass"),
            iteration_completed("subtask_loop"),
            enter("verify_task", 1),
        ];
        let mission = fold(&[], &events, None);
        assert_eq!(
            mission.milestones[0].tasks.len(),
            1,
            "a subtask must not become a second roadmap task"
        );
        assert_eq!(
            mission.milestones[0].tasks[0].steps[0].node, "verify_task",
            "parent scope must survive child completion"
        );
    }
    fn participant(
        node: &str,
        profile: &str,
        runtime: Option<&str>,
        session: surge_core::SessionId,
    ) -> EventPayload {
        EventPayload::SessionOpened {
            node: key(node),
            session,
            agent: profile.into(),
            agent_id: runtime.map(str::to_string),
        }
    }
    fn inputs(node: &str, name: &str) -> EventPayload {
        EventPayload::StageInputsResolved {
            node: key(node),
            bindings: [(name.into(), ContentHash::compute(b"private input"))].into(),
        }
    }
    fn skill(node: &str, name: &str) -> EventPayload {
        EventPayload::SkillBound {
            node: key(node),
            name: name.into(),
            provider: surge_core::skill::SkillProvider::ProjectDir,
            hash: ContentHash::compute(b"private skill"),
            gate_enabled: true,
        }
    }

    #[test]
    fn participant_context_belongs_to_the_scoped_occurrence_and_latest_attempt() {
        let first = surge_core::SessionId::new();
        let retry = surge_core::SessionId::new();
        let parent = surge_core::SessionId::new();
        let second = surge_core::SessionId::new();
        let mut events = vec![
            EventPayload::PipelineMaterialized {
                graph: Box::new(subtask_graph()),
                graph_hash: ContentHash::compute(b"context"),
            },
            iter(
                "milestone_loop",
                "id = 'm1'\ntitle = 'Timer'\ntasks = []",
                0,
            ),
            iter("task_loop", "id = 't1'\ntitle = 'First parent'", 0),
            iter("subtask_loop", "id = 'leaf'\ntitle = 'First child'", 0),
            enter("impl_task", 1),
            participant("impl_task", "implementer@1", Some("claude"), first),
            inputs("impl_task", "old-context"),
            skill("impl_task", "old-skill"),
            EventPayload::OutcomeReported {
                node: key("impl_task"),
                outcome: out("pass"),
                summary: "old report".into(),
            },
            EventPayload::StageFailed {
                node: key("impl_task"),
                reason: "retry".into(),
                retry_available: true,
            },
            enter("impl_task", 2),
        ];
        let reset = fold(&[], &events, None);
        let child = &reset.milestones[0].tasks[0].subtasks[0].steps[0];
        assert_eq!(child.attempts, 2);
        assert!(child.participant.is_none());
        assert!(child.inputs.is_none());
        assert!(child.skills.is_empty());
        assert!(child.summary.is_none());
        events.extend([
            participant("impl_task", "reviewer@2", None, retry),
            inputs("impl_task", "new-context"),
            skill("impl_task", "new-skill"),
            done("impl_task", "pass"),
            iteration_completed("subtask_loop"),
            enter("impl_task", 1),
            participant("impl_task", "parent-worker@1", Some("codex"), parent),
            inputs("impl_task", "parent-context"),
            skill("impl_task", "parent-skill"),
            done("impl_task", "pass"),
            iteration_completed("task_loop"),
            iter("task_loop", "id = 't2'\ntitle = 'Second parent'", 1),
            // A node-only record before entry cannot overwrite the previous task.
            inputs("impl_task", "unscoped-stale"),
            skill("impl_task", "unscoped-stale"),
            iter("subtask_loop", "id = 'leaf'\ntitle = 'Second child'", 0),
            enter("impl_task", 1),
            participant("impl_task", "second-worker@1", Some("gemini"), second),
            inputs("impl_task", "second-context"),
            skill("impl_task", "second-skill"),
        ]);
        let mission = fold(&[], &events, None);
        let first_task = &mission.milestones[0].tasks[0];
        let child = &first_task.subtasks[0].steps[0];
        assert_eq!(child.participant.as_ref().unwrap().session, retry);
        assert_eq!(child.participant.as_ref().unwrap().profile, "reviewer@2");
        assert!(child.participant.as_ref().unwrap().runtime.is_none());
        assert_eq!(child.inputs.as_ref().unwrap(), &["new-context"]);
        assert_eq!(child.skills, ["new-skill"]);
        let parent_step = &first_task.steps[0];
        assert_eq!(parent_step.participant.as_ref().unwrap().session, parent);
        assert_eq!(parent_step.inputs.as_ref().unwrap(), &["parent-context"]);
        assert_eq!(parent_step.skills, ["parent-skill"]);
        let second_child = &mission.milestones[0].tasks[1].subtasks[0].steps[0];
        assert_eq!(second_child.participant.as_ref().unwrap().session, second);
        assert_eq!(second_child.inputs.as_ref().unwrap(), &["second-context"]);
        assert_eq!(second_child.skills, ["second-skill"]);
    }
}
