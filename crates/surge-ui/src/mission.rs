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

use std::{collections::HashMap, sync::Arc};

use surge_core::graph::Graph;
use surge_core::node::{Node, NodeConfig};
use surge_core::roadmap::{MilestoneId, RoadmapArtifact, RoadmapStatus, RoadmapTaskId};
use surge_core::task_execution::{
    StageExecution, TaskExecution, TaskExecutionError, TaskExecutionView,
};
use surge_core::{BootstrapDecision, BootstrapStage, EventPayload, RunEvent, RunId};

use crate::flow_diagram::{Lane, lane_of};

/// Where a step stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StepState {
    Working,
    WaitingOnYou,
    Done(String),
    Failed(String),
    /// A journal gap may have changed this unfinished occurrence.
    Unknown,
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
    Discovered,
    Queued,
    Replaced,
    Running,
    Finished(String),
    Unknown,
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
    /// Journal coverage and attribution limitations.
    pub caveats: Vec<String>,
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

/// Fold actual journal envelopes through the shared execution projection.
pub fn fold(
    planning: &[RunEvent],
    run: RunId,
    implementation: &[RunEvent],
    roadmap: Option<&RoadmapArtifact>,
) -> Result<Mission, TaskExecutionError> {
    if let Some(first) = planning.first() {
        TaskExecutionView::compile(first.run_id, planning)?;
    }
    let view = TaskExecutionView::compile(run, implementation)?;
    Ok(from_execution(planning, &view, implementation, roadmap))
}

/// Present an already compiled execution view; planning seeds are not execution evidence.
pub(crate) fn from_execution(
    planning: &[RunEvent],
    view: &TaskExecutionView,
    implementation: &[RunEvent],
    roadmap: Option<&RoadmapArtifact>,
) -> Mission {
    let plan_events: Vec<_> = if planning.is_empty() {
        implementation
    } else {
        planning
    }
    .iter()
    .map(|event| event.payload.clone())
    .collect();
    let mut mission = Mission {
        plan: fold_plan(&plan_events),
        milestones: seed(roadmap),
        caveats: view
            .caveats
            .iter()
            .map(|caveat| caveat.to_string())
            .collect(),
        ..Mission::default()
    };
    let graphs = graphs_at_entry(implementation);
    for iteration in &view.iterations {
        if iteration
            .item
            .get("tasks")
            .is_some_and(toml::Value::is_array)
        {
            let id = item_str(&iteration.item, "id")
                .unwrap_or_else(|| format!("#{}", iteration.index + 1));
            let milestone = ensure_milestone(
                &mut mission,
                &id,
                &item_str(&iteration.item, "title").unwrap_or_else(|| id.clone()),
            );
            mission.milestones[milestone].started = true;
            mission.milestones[milestone].finished = iteration
                .completed
                .as_ref()
                .map(|(_, outcome)| outcome.clone());
        }
    }
    let mut recorded: Vec<Vec<TaskView>> = vec![Vec::new(); mission.milestones.len().max(1)];
    for task in view.tasks.iter().filter(|task| task.parent.is_none()) {
        let milestone = match &task.scope {
            surge_core::task_execution::TaskScope::Recorded(scope) => {
                milestone_for(&mut mission, view, scope)
            },
            surge_core::task_execution::TaskScope::Partial { .. } => {
                ensure_milestone(&mut mission, "unassigned-work", "Unassigned work")
            },
            surge_core::task_execution::TaskScope::Unassigned => {
                ensure_milestone(&mut mission, "unassigned", "Unassigned discoveries")
            },
        };
        while recorded.len() <= milestone {
            recorded.push(Vec::new());
        }
        let mut rendered = task_view(task, view, &graphs);
        if let Some(seed) = mission.milestones[milestone]
            .tasks
            .iter()
            .find(|seed| seed.id == rendered.id)
            && rendered.title == rendered.id.as_str()
        {
            rendered.title = seed.title.clone();
        }
        recorded[milestone].push(rendered);
    }
    for (milestone, mut tasks) in recorded.into_iter().enumerate() {
        if let Some(target) = mission.milestones.get_mut(milestone) {
            let recorded_ids: Vec<_> = tasks.iter().map(|task| task.id.clone()).collect();
            tasks.extend(
                target
                    .tasks
                    .iter()
                    .filter(|planned| !recorded_ids.contains(&planned.id))
                    .cloned(),
            );
            target.tasks = tasks;
        }
    }
    for stage in view.stages.iter().filter(|stage| stage.task.is_none()) {
        let Some(step) = step_view(
            stage,
            graphs.get(&stage.entered).map(Arc::as_ref),
            view.terminal.is_some(),
        ) else {
            continue;
        };
        match stage.scope.recorded() {
            Some([]) => push_step(&mut mission.steps, step),
            Some(scope) => {
                let index = milestone_for(&mut mission, view, scope);
                push_step(&mut mission.milestones[index].steps, step);
            },
            None => {
                let index = ensure_milestone(&mut mission, "unassigned-work", "Unassigned work");
                push_step(&mut mission.milestones[index].steps, step);
            },
        }
    }
    mission.end = implementation
        .iter()
        .rev()
        .find_map(|event| match &event.payload {
            EventPayload::RunCompleted { .. } => Some(RunEnd::Completed),
            EventPayload::RunFailed { error } => Some(RunEnd::Failed(error.clone())),
            EventPayload::RunAborted { reason } => Some(RunEnd::Aborted(reason.clone())),
            _ => None,
        });
    mission.now = now(&mission);
    mission
}

fn graphs_at_entry(events: &[RunEvent]) -> HashMap<u64, Arc<Graph>> {
    let mut current = None;
    let mut entries = HashMap::new();
    let mut previous = 0;
    for event in events {
        if event.seq != previous + 1 {
            current = None;
        }
        previous = event.seq;
        match &event.payload {
            EventPayload::PipelineMaterialized { graph, .. }
            | EventPayload::GraphRevisionAccepted { graph, .. } => {
                current = Some(Arc::new(
                    surge_core::escalation::with_default_escalation_gates(graph),
                ))
            },
            EventPayload::StageEntered { .. } => {
                if let Some(graph) = &current {
                    entries.insert(event.seq, Arc::clone(graph));
                }
            },
            _ => {},
        }
    }
    entries
}

fn ensure_milestone(mission: &mut Mission, id: &str, title: &str) -> usize {
    if let Some(index) = mission
        .milestones
        .iter()
        .position(|milestone| milestone.id.as_str() == id)
    {
        return index;
    }
    mission.milestones.push(MilestoneView {
        id: id.into(),
        title: title.into(),
        steps: Vec::new(),
        tasks: Vec::new(),
        started: true,
        finished: None,
    });
    mission.milestones.len() - 1
}
fn milestone_for(mission: &mut Mission, view: &TaskExecutionView, scope: &[u64]) -> usize {
    if let Some(iteration) = scope
        .iter()
        .rev()
        .filter_map(|seq| {
            view.iterations
                .iter()
                .find(|iteration| iteration.seq == *seq)
        })
        .find(|iteration| {
            iteration
                .item
                .get("tasks")
                .is_some_and(toml::Value::is_array)
        })
    {
        let id =
            item_str(&iteration.item, "id").unwrap_or_else(|| format!("#{}", iteration.index + 1));
        return ensure_milestone(
            mission,
            &id,
            &item_str(&iteration.item, "title").unwrap_or_else(|| id.clone()),
        );
    }
    if mission.milestones.is_empty() {
        ensure_milestone(mission, "tasks", "Tasks");
    }
    0
}
fn task_view(
    task: &TaskExecution,
    view: &TaskExecutionView,
    graphs: &HashMap<u64, Arc<Graph>>,
) -> TaskView {
    let mut steps = Vec::new();
    for stage in view
        .stages
        .iter()
        .filter(|stage| stage.task == Some(task.id))
    {
        if let Some(step) = step_view(
            stage,
            graphs.get(&stage.entered).map(Arc::as_ref),
            view.terminal.is_some(),
        ) {
            push_step(&mut steps, step);
        }
    }
    let iteration = if task.replaced {
        IterationState::Replaced
    } else if let Some((_, outcome)) = &task.finished {
        IterationState::Finished(outcome.clone())
    } else if task.continuity_unknown.is_some() {
        IterationState::Unknown
    } else if task.started.is_some() {
        if view.terminal.is_some() {
            IterationState::Unfinished
        } else {
            IterationState::Running
        }
    } else if task.split.is_some() {
        IterationState::Queued
    } else {
        IterationState::Discovered
    };
    TaskView {
        id: task.task_id.clone(),
        title: task.title.clone(),
        status: task.status,
        verified: task.verified,
        steps,
        subtasks: view
            .tasks
            .iter()
            .filter(|child| child.parent == Some(task.id))
            .map(|child| task_view(child, view, graphs))
            .collect(),
        iteration,
        discovered_from: task.discovered_from.clone(),
    }
}
fn step_view(stage: &StageExecution, graph: Option<&Graph>, terminal: bool) -> Option<Step> {
    let nodes = graph.map(all_nodes).unwrap_or_default();
    let node = nodes.get(stage.node.as_str()).copied();
    if node.is_some_and(|node| !is_work(node)) {
        return None;
    }
    let state = if let Some(error) = &stage.failure {
        StepState::Failed(error.clone())
    } else if let Some((_, outcome)) = &stage.result {
        StepState::Done(outcome.clone())
    } else if stage.continuity_unknown.is_some() {
        StepState::Unknown
    } else if terminal {
        StepState::Unfinished
    } else if stage.waiting {
        StepState::WaitingOnYou
    } else {
        StepState::Working
    };
    let participant = stage.sessions.last().map(|session| RecordedParticipant {
        profile: session.profile.clone(),
        runtime: session.runtime.clone(),
        session: session.connection,
    });
    Some(Step {
        node: stage.node.as_str().into(),
        label: step_label(stage.node.as_str(), node),
        lane: node.map_or(Lane::Build, lane_of),
        state,
        attempts: stage.attempt.max(1),
        summary: stage.summary.clone(),
        participant,
        inputs: stage.inputs.clone(),
        skills: stage.skills.clone(),
    })
}
fn push_step(steps: &mut Vec<Step>, step: Step) {
    if steps.last().is_some_and(|previous| {
        previous.node == step.node && previous.label == step.label && previous.lane == step.lane
    }) {
        if let Some(previous) = steps.last_mut() {
            *previous = step;
        }
    } else {
        steps.push(step);
    }
}

#[cfg(test)]
pub(crate) fn fixture_fold(
    planning: &[EventPayload],
    implementation: &[EventPayload],
    roadmap: Option<&RoadmapArtifact>,
) -> Mission {
    let run: RunId = "run-00000000000000000000000001".parse().unwrap();
    let envelopes = |events: &[EventPayload]| {
        events
            .iter()
            .enumerate()
            .map(|(index, payload)| RunEvent {
                run_id: run,
                seq: index as u64 + 1,
                timestamp: chrono::DateTime::UNIX_EPOCH,
                payload: payload.clone(),
            })
            .collect::<Vec<_>>()
    };
    fold(
        &envelopes(planning),
        run,
        &envelopes(implementation),
        roadmap,
    )
    .unwrap()
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
    use super::{PlanState, RunEnd, StepState, fixture_fold as fold};
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

    /// A step waits on the operator exactly while its request is open: a
    /// timeout ends the wait as surely as an answer (#102).
    #[test]
    fn a_timed_out_request_stops_waiting_on_you() {
        let started = EventPayload::RunStarted {
            pipeline_template: None,
            project_path: "/project".into(),
            initial_prompt: "build".into(),
            config: surge_core::RunConfig {
                budget: Default::default(),
                sandbox_default: surge_core::sandbox::SandboxMode::WorkspaceWrite,
                approval_default: surge_core::approvals::ApprovalPolicy::OnRequest,
                auto_pr: false,
                mcp_servers: Vec::new(),
                bootstrap_edit_loop_cap: None,
            },
        };
        let request = EventPayload::HumanInputRequested {
            node: key("final_verify"),
            session: None,
            call_id: Some(surge_core::id::GateRequestId::new().to_string()),
            prompt: "ship it?".into(),
            schema: None,
        };
        let EventPayload::HumanInputRequested { call_id, .. } = &request else {
            unreachable!()
        };
        let timed_out = EventPayload::HumanInputTimedOut {
            node: key("final_verify"),
            call_id: call_id.clone(),
            elapsed_seconds: 60,
        };
        let mut events = vec![
            started,
            EventPayload::PipelineMaterialized {
                graph: Box::new(graph()),
                graph_hash: ContentHash::compute(b"g"),
            },
            enter("final_verify", 1),
            request,
        ];
        let waiting = fold(&[], &events, None);
        assert_eq!(waiting.steps[0].state, StepState::WaitingOnYou);
        events.push(timed_out);
        let after = fold(&[], &events, None);
        assert_eq!(after.steps[0].state, StepState::Working);
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

                report: None,
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
        // An unbound legacy TaskVerified claim cannot establish current proof.
        assert!(!t.verified);
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
    #[test]
    fn split_children_display_before_the_next_planned_task() {
        let roadmap: surge_core::roadmap::RoadmapArtifact = toml::from_str(
            "schema_version = 2\n[[milestones]]\nid = 'm1'\ntitle = 'Authentication'\nstatus = 'pending'\n[[milestones.tasks]]\nid = 'login'\ntitle = 'Login form'\nstatus = 'pending'\n[[milestones.tasks]]\nid = 'logout'\ntitle = 'Logout'\nstatus = 'pending'\n",
        ).unwrap();
        let events = vec![
            EventPayload::PipelineMaterialized {
                graph: Box::new(graph()),
                graph_hash: ContentHash::compute(b"split-order"),
            },
            iter("milestone_loop", "id = 'm1'\ntitle = 'Authentication'\ntasks = []", 0),
            iter("task_loop", "id = 'login'\ntitle = 'Login form'", 0),
            EventPayload::TaskDiscovered {
                task_id: "login-a".into(), discovered_from: "login".into(),
                title: "Sign in valid users".into(),
            },
            EventPayload::TaskDiscovered {
                task_id: "login-b".into(), discovered_from: "login".into(),
                title: "Reject empty passwords".into(),
            },
            EventPayload::TaskSplit {
                loop_id: key("task_loop"), index: 0, task: Some("login".into()),
                into: vec![
                    toml::from_str("id = 'login-a'\ntitle = 'Sign in valid users'\nacceptance_criteria = ['Valid users sign in']\ndiscovered_from = 'login'").unwrap(),
                    toml::from_str("id = 'login-b'\ntitle = 'Reject empty passwords'\nacceptance_criteria = ['Empty passwords are rejected']\ndiscovered_from = 'login'").unwrap(),
                ],
            },
        ];
        let mission = fold(&[], &events, Some(&roadmap));
        let displayed: Vec<_> = mission.milestones[0]
            .tasks
            .iter()
            .map(|task| task.id.as_str())
            .collect();
        assert_eq!(displayed, ["login", "login-a", "login-b", "logout"]);
    }

    #[test]
    fn malformed_envelopes_are_errors_and_unassigned_discoveries_stay_visible() {
        let run: surge_core::RunId = "run-00000000000000000000000001".parse().unwrap();
        let events = vec![surge_core::RunEvent {
            run_id: run,
            seq: 1,
            timestamp: chrono::DateTime::UNIX_EPOCH,
            payload: EventPayload::TaskDiscovered {
                task_id: "orphan".into(),
                discovered_from: "missing".into(),
                title: "Unassigned child".into(),
            },
        }];
        let mission = super::fold(&[], run, &events, None).unwrap();
        assert_eq!(mission.milestones[0].title, "Unassigned discoveries");
        assert_eq!(
            mission.milestones[0].tasks[0].iteration,
            super::IterationState::Discovered
        );
        assert!(!mission.caveats.is_empty());
        let duplicate = vec![events[0].clone(), events[0].clone()];
        assert!(super::fold(&[], run, &duplicate, None).is_err());
    }

    #[test]
    fn historical_stage_roles_follow_the_graph_at_each_entry() {
        let original = graph();
        let mut amended = original.clone();
        let body: surge_core::keys::SubgraphKey = "task_body".parse().unwrap();
        let node = amended
            .subgraphs
            .get_mut(&body)
            .unwrap()
            .nodes
            .get_mut(&key("spec_task"))
            .unwrap();
        let surge_core::NodeConfig::Agent(agent) = &mut node.config else {
            panic!("agent fixture")
        };
        agent.profile = "implementer@1.0".parse().unwrap();
        let events = vec![
            EventPayload::PipelineMaterialized {
                graph: Box::new(original),
                graph_hash: ContentHash::compute(b"old"),
            },
            iter("task_loop", "id='login'\ntitle='Login'", 0),
            enter("spec_task", 1),
            done("spec_task", "drafted"),
            EventPayload::GraphRevisionAccepted {
                patch_id: surge_core::roadmap_patch::RoadmapPatchId::new("historical-role-change")
                    .unwrap(),
                target: surge_core::roadmap_patch::RoadmapPatchTarget::ProjectRoadmap {
                    roadmap_path: "roadmap.toml".into(),
                },
                previous_graph_hash: ContentHash::compute(b"old"),
                graph: Box::new(amended),
                graph_hash: ContentHash::compute(b"new"),
                active_pickup: Default::default(),
            },
            enter("spec_task", 1),
            done("spec_task", "done"),
        ];
        let mission = fold(&[], &events, None);
        assert_eq!(
            mission.milestones[0].tasks[0]
                .steps
                .iter()
                .map(|step| step.label.as_str())
                .collect::<Vec<_>>(),
            ["Spec", "Build"]
        );
    }

    #[test]
    fn a_gap_does_not_reuse_graph_roles_or_place_unknown_work_at_root() {
        let run: surge_core::RunId = "run-00000000000000000000000001".parse().unwrap();
        let payloads = [
            EventPayload::PipelineMaterialized {
                graph: Box::new(graph()),
                graph_hash: ContentHash::compute(b"old graph"),
            },
            iter("task_loop", "id='login'\ntitle='Login'", 0),
            enter("spec_task", 1),
            enter("spec_task", 1),
        ];
        let events = payloads
            .into_iter()
            .zip([1, 2, 3, 5])
            .map(|(payload, seq)| surge_core::RunEvent {
                run_id: run,
                seq,
                timestamp: chrono::DateTime::UNIX_EPOCH,
                payload,
            })
            .collect::<Vec<_>>();
        let mission = super::fold(&[], run, &events, None).unwrap();
        assert_eq!(mission.milestones[0].tasks[0].steps[0].label, "Spec");
        assert_eq!(
            mission.milestones[0].tasks[0].iteration,
            super::IterationState::Unknown
        );
        assert_eq!(
            mission.milestones[0].tasks[0].steps[0].state,
            StepState::Unknown
        );
        assert!(!super::now(&mission).unwrap_or_default().contains("Login"));
        assert!(mission.steps.is_empty());
        let unknown = mission
            .milestones
            .iter()
            .find(|milestone| milestone.id.as_str() == "unassigned-work")
            .unwrap();
        assert_eq!(unknown.steps[0].label, "Spec task");
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

            report: None,
        });
        let scoped = fold(&[], &scoped_events, None);
        assert!(!scoped.milestones[0].tasks[0].subtasks[0].verified);
        // No verifier stage or bound report established this claim's authority.
        assert!(!scoped.milestones[0].tasks[1].subtasks[0].verified);
        let mut late_events = events.clone();
        late_events.push(iteration_completed("subtask_loop"));
        late_events.push(iteration_completed("task_loop"));
        late_events.push(iteration_completed("milestone_loop"));
        late_events.push(EventPayload::TaskVerified {
            task_id: "leaf".into(),
            node: key("verify_task"),
            evidence: ContentHash::compute(b"ambiguous"),

            report: None,
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
            opened: None,
            handoff: None,
            node: key(node),
            session,
            agent: profile.into(),
            agent_id: runtime.map(str::to_string),
        }
    }
    fn inputs(node: &str, attempt: u32, name: &str) -> EventPayload {
        EventPayload::StageInputsResolved {
            node: key(node),
            attempt,
            bindings: [(name.into(), ContentHash::compute(b"private input"))].into(),
            memory_receipt: None,
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
            inputs("impl_task", 1, "old-context"),
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
            inputs("impl_task", 2, "new-context"),
            skill("impl_task", "new-skill"),
            done("impl_task", "pass"),
            iteration_completed("subtask_loop"),
            enter("impl_task", 1),
            participant("impl_task", "parent-worker@1", Some("codex"), parent),
            inputs("impl_task", 1, "parent-context"),
            skill("impl_task", "parent-skill"),
            done("impl_task", "pass"),
            iteration_completed("task_loop"),
            iter("task_loop", "id = 't2'\ntitle = 'Second parent'", 1),
            // A node-only record before entry cannot overwrite the previous task.
            inputs("impl_task", 1, "unscoped-stale"),
            skill("impl_task", "unscoped-stale"),
            iter("subtask_loop", "id = 'leaf'\ntitle = 'Second child'", 0),
            enter("impl_task", 1),
            participant("impl_task", "second-worker@1", Some("gemini"), second),
            inputs("impl_task", 1, "second-context"),
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
