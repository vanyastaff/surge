use super::*;
use crate::{EventPayload as E, run_state::RunMemory};
use std::collections::HashMap;

pub(super) fn compile(
    run: RunId,
    events: &[RunEvent],
) -> Result<TaskExecutionView, TaskExecutionError> {
    let mut previous = 0;
    for event in events {
        if event.run_id != run {
            return Err(TaskExecutionError::ForeignRun { seq: event.seq });
        }
        if event.seq == 0 || event.seq <= previous {
            return Err(TaskExecutionError::Sequence {
                seq: event.seq,
                previous,
            });
        }
        previous = event.seq;
    }
    crate::execution_recovery::seed_guard::preflight_unhydrated(events)
        .map_err(crate::run_state::FoldError::from)?;
    let mut projection = Projection::new(run);
    projection.acceptance_markers = events
        .iter()
        .filter_map(|event| match &event.payload {
            E::StageOutcomeCommitted { commit } => Some((event.seq, commit.clone())),
            _ => None,
        })
        .collect();
    let mut previous = 0;
    for event in events {
        if event.run_id != run {
            return Err(TaskExecutionError::ForeignRun { seq: event.seq });
        }
        if event.seq == 0 || event.seq <= previous {
            return Err(TaskExecutionError::Sequence {
                seq: event.seq,
                previous,
            });
        }
        if event.seq != previous + 1 {
            projection.gap(previous);
            projection
                .view
                .caveats
                .push(ExecutionCaveat::PartialJournal {
                    after: previous,
                    before: event.seq,
                });
        }
        previous = event.seq;
        projection.observe(event)?;
    }
    if events.is_empty() {
        projection.memory.recovery.invalidate();
    }
    projection.view.recovery = projection.memory.recovery.observation().clone();
    Ok(projection.view)
}
pub(super) struct Projection {
    view: TaskExecutionView,
    active: Vec<usize>,
    partial_after: Option<u64>,
    sessions: HashMap<SessionId, Option<(usize, usize)>>,
    accepted: HashMap<(StageInvocationId, u64), (usize, usize)>,
    writers: HashMap<crate::id::ExecutionWriterId, Vec<(usize, usize)>>,
    stage: Option<usize>,
    pub(super) memory: RunMemory,
    pending_human: Vec<(usize, NodeKey, Option<String>)>,
    acceptance_markers: Vec<(u64, crate::execution_recovery::commit::StageOutcomeCommit)>,
    establishment: Option<(
        NodeKey,
        StageInvocationId,
        Option<crate::stage_tool::StageToolContext>,
    )>,
}
impl Projection {
    pub(super) fn new(run: RunId) -> Self {
        Self {
            view: TaskExecutionView {
                run,
                tasks: Vec::new(),
                stages: Vec::new(),
                iterations: Vec::new(),
                caveats: Vec::new(),
                terminal: None,
                recovery: Default::default(),
            },
            active: Vec::new(),
            partial_after: None,
            sessions: HashMap::new(),
            accepted: HashMap::new(),
            writers: HashMap::new(),
            stage: None,
            memory: RunMemory::default(),
            pending_human: Vec::new(),
            acceptance_markers: Vec::new(),
            establishment: None,
        }
    }
    fn gap(&mut self, after: u64) {
        self.partial_after = Some(after);
        self.active.clear();
        self.stage = None;
        self.establishment = None;
        self.pending_human.clear();
        self.memory.verification = Default::default();
        self.memory.verification_graph = None;
        self.memory.ledger = Default::default();
        for task in &mut self.view.tasks {
            task.verified = false;
            if task.finished.is_none() && !task.replaced {
                task.continuity_unknown.get_or_insert(after);
            }
        }
        for stage in &mut self.view.stages {
            if stage.result.is_none() && stage.failure.is_none() {
                stage.continuity_unknown.get_or_insert(after);
            }
        }
    }
    fn scope(&self, observed: Vec<u64>) -> TaskScope {
        match self.partial_after {
            Some(after) => TaskScope::Partial { after, observed },
            None => TaskScope::Recorded(observed),
        }
    }
    fn unassigned(&mut self, seq: u64, fact: &str) {
        self.view.caveats.push(ExecutionCaveat::Unassigned {
            seq,
            fact: fact.into(),
        });
    }
    fn path(&self) -> Vec<u64> {
        self.active
            .iter()
            .map(|index| self.view.iterations[*index].seq)
            .collect()
    }
    fn current_task(&self) -> Option<TaskOccurrenceId> {
        let path = self.path();
        self.view
            .tasks
            .iter()
            .rev()
            .find(|task| {
                task.started.is_some()
                    && task
                        .scope
                        .observed()
                        .is_some_and(|scope| path.starts_with(scope))
            })
            .map(|task| task.id)
    }
    fn task_mut(&mut self, id: TaskOccurrenceId) -> Option<&mut TaskExecution> {
        self.view.tasks.iter_mut().find(|task| task.id == id)
    }
    fn active_stage(&mut self, node: &NodeKey, seq: u64) -> Option<&mut StageExecution> {
        if let Some(index) = self.stage
            && self.view.stages[index].node == *node
            && self.view.stages[index].scope == self.scope(self.path())
        {
            return self.view.stages.get_mut(index);
        }
        self.unassigned(seq, "stage record without its active StageEntered");
        None
    }
    fn observe(&mut self, event: &RunEvent) -> Result<(), TaskExecutionError> {
        let mut before = self.memory.verification.clone();
        self.memory.apply_event(event)?;
        let invalidation = before.observe(&event.payload);
        self.invalidate(invalidation);
        match &event.payload {
            E::LoopIterationStarted {
                loop_id,
                item,
                index,
            } => self.iteration(event.seq, loop_id, item, *index),
            E::LoopIterationCompleted {
                loop_id,
                outcome,
                index,
            } => self.iteration_done(event.seq, loop_id, *index, outcome.as_str()),
            E::TaskDiscovered {
                task_id,
                discovered_from,
                title,
            } => self.discovered(event.seq, task_id, discovered_from, title),
            E::TaskSplit {
                loop_id,
                index,
                task,
                into,
            } => self.split(event.seq, loop_id, *index, task.as_deref(), into),
            E::StageEntered { node, attempt } => self.enter(event.seq, node, *attempt),
            E::TaskStatusChanged {
                task_id,
                authority_node,
                ..
            } => self.ledger(event.seq, task_id, authority_node, None),
            E::TaskVerified {
                task_id,
                node,
                evidence,
                report,
            } => self.verification_claim(event.seq, task_id, node, *evidence, report.as_ref()),
            E::RunCompleted { .. } => self.view.terminal = Some((event.seq, "completed".into())),
            E::RunFailed { error } => {
                self.view.terminal = Some((event.seq, format!("failed: {error}")))
            },
            E::RunAborted { reason } => {
                self.view.terminal = Some((event.seq, format!("aborted: {reason}")))
            },
            _ => self.stage_event(event),
        }
        Ok(())
    }
    fn invalidate(&mut self, invalidation: crate::verification_evidence::VerificationInvalidation) {
        use crate::verification_evidence::VerificationInvalidation;
        for task in &mut self.view.tasks {
            if matches!(&invalidation, VerificationInvalidation::All)
                || matches!(&invalidation, VerificationInvalidation::Task(id) if *id == task.task_id)
            {
                task.verified = false;
            }
        }
    }
    fn item_task(
        &mut self,
        seq: u64,
        ordinal: u32,
        item: &toml::Value,
        scope: TaskScope,
        parent: Option<TaskOccurrenceId>,
        expected_origin: Option<&RoadmapTaskId>,
    ) -> Option<TaskOccurrenceId> {
        let id = item.get("id")?.as_str()?;
        let task_id: RoadmapTaskId = id.into();
        let pending: Vec<_> = self
            .view
            .tasks
            .iter()
            .enumerate()
            .filter(|(_, task)| {
                task.task_id == task_id
                    && task.started.is_none()
                    && (task.scope == scope
                        || (task.scope == TaskScope::Unassigned
                            && expected_origin.is_some()
                            && task.discovered_from.as_ref() == expected_origin))
                    && expected_origin.is_none_or(|origin| {
                        task.discovered_from
                            .as_ref()
                            .is_none_or(|recorded| recorded == origin)
                    })
            })
            .map(|(index, _)| index)
            .collect();
        let existing = if pending.len() == 1 {
            pending.first().copied()
        } else {
            None
        };
        let position = existing.unwrap_or_else(|| {
            self.view.tasks.push(TaskExecution {
                id: TaskOccurrenceId {
                    run: self.view.run,
                    seq,
                    ordinal,
                },
                task_id: task_id.clone(),
                title: id.into(),
                description: None,
                acceptance_criteria: Vec::new(),
                scope: scope.clone(),
                parent,
                discovered_from: None,
                split_origin: None,
                discovered: None,
                split: None,
                started: None,
                finished: None,
                continuity_unknown: None,
                replaced: false,
                status: RoadmapStatus::Pending,
                verified: false,
            });
            self.view.tasks.len() - 1
        });
        let task = &mut self.view.tasks[position];
        task.scope = scope;
        if let Some(title) = item.get("title").and_then(toml::Value::as_str) {
            task.title = title.into();
        }
        if let Some(description) = item.get("description").and_then(toml::Value::as_str) {
            task.description = Some(description.into());
        }
        if let Some(criteria) = item
            .get("acceptance_criteria")
            .and_then(toml::Value::as_array)
        {
            task.acceptance_criteria = criteria
                .iter()
                .filter_map(toml::Value::as_str)
                .map(str::to_owned)
                .collect();
        }
        Some(task.id)
    }
    fn iteration(&mut self, seq: u64, loop_id: &NodeKey, item: &toml::Value, index: u32) {
        if let Some(position) = self
            .active
            .iter()
            .position(|i| self.view.iterations[*i].loop_id == *loop_id)
        {
            self.active.truncate(position);
        }
        let parents = self.path();
        let parent_task = self.current_task();
        self.view.iterations.push(IterationOccurrence {
            seq,
            loop_id: loop_id.clone(),
            index,
            item: item.clone(),
            parents: parents.clone(),
            completed: None,
        });
        self.active.push(self.view.iterations.len() - 1);
        if item.get("tasks").is_some_and(toml::Value::is_array) {
            return;
        }
        if parent_task.is_some() && !self.is_subtask(loop_id) {
            return;
        }
        let origin = item
            .get("discovered_from")
            .and_then(toml::Value::as_str)
            .map(RoadmapTaskId::from);
        let Some(id) = self.item_task(
            seq,
            0,
            item,
            self.scope(parents),
            parent_task,
            origin.as_ref(),
        ) else {
            self.unassigned(seq, "loop item has no task identity");
            return;
        };
        let scope = self.scope(self.path());
        if let Some(task) = self.task_mut(id) {
            task.scope = scope;
            task.started = Some(seq);
        }
    }
    pub(super) fn is_subtask(&self, loop_id: &NodeKey) -> bool {
        let Some(graph) = self.memory.verification_graph.as_deref() else {
            return false;
        };
        let node = graph.nodes.get(loop_id).or_else(|| {
            graph
                .subgraphs
                .values()
                .find_map(|subgraph| subgraph.nodes.get(loop_id))
        });
        let Some(crate::NodeConfig::Loop(config)) = node.map(|node| &node.config) else {
            return false;
        };
        if config.iteration_var_name == "subtask" {
            return true;
        }
        use crate::loop_config::IterableSource;
        let path = match &config.iterates_over {
            IterableSource::Artifact { jsonpath, .. }
            | IterableSource::RunArtifact { jsonpath, .. }
            | IterableSource::LoopItem { jsonpath, .. } => jsonpath,
            IterableSource::Static(_) => return false,
        };
        path.split(|c: char| !c.is_alphanumeric() && c != '_')
            .any(|segment| segment == "subtasks")
    }
    fn iteration_done(&mut self, seq: u64, loop_id: &NodeKey, index: u32, outcome: &str) {
        let Some(position) = self.active.iter().rposition(|i| {
            self.view.iterations[*i].loop_id == *loop_id && self.view.iterations[*i].index == index
        }) else {
            self.unassigned(seq, "completion without loop entry");
            return;
        };
        let iteration = &mut self.view.iterations[self.active[position]];
        iteration.completed = Some((seq, outcome.into()));
        let anchor = iteration.seq;
        if let Some(task) = self
            .view
            .tasks
            .iter_mut()
            .find(|task| task.started == Some(anchor))
        {
            task.finished = Some((seq, outcome.into()));
        }
        self.active.truncate(position);
    }
    fn discovery_bound(&self, seq: u64, source: TaskOccurrenceId) -> bool {
        let next = self
            .acceptance_markers
            .partition_point(|(marker, _)| *marker <= seq);
        self.acceptance_markers
            .get(next)
            .is_some_and(|(marker, commit)| {
                seq >= marker.saturating_sub(u64::from(commit.effects_count()))
                    && self
                        .sessions
                        .get(&commit.provider_connection())
                        .copied()
                        .flatten()
                        .is_some_and(|(stage, _)| {
                            self.view.stages[stage].task == Some(source)
                                && owns_commit(&self.view.stages[stage], commit, self.view.run)
                        })
            })
    }
    fn discovered(
        &mut self,
        seq: u64,
        task_id: &RoadmapTaskId,
        origin: &RoadmapTaskId,
        title: &str,
    ) {
        let source = self
            .current_task()
            .and_then(|id| {
                self.view
                    .tasks
                    .iter()
                    .find(|task| task.id == id && task.task_id == *origin)
            })
            .cloned();
        let ambiguous = self
            .view
            .tasks
            .iter()
            .filter(|task| task.task_id == *origin)
            .count()
            > 1;
        let source = source.filter(|source| !ambiguous || self.discovery_bound(seq, source.id));
        let scope = source
            .as_ref()
            .map(|source| {
                source.scope.with_path(
                    source
                        .scope
                        .observed()
                        .map(|scope| scope[..scope.len().saturating_sub(1)].to_vec())
                        .unwrap_or_default(),
                )
            })
            .unwrap_or(TaskScope::Unassigned);
        let item = toml::Value::Table(toml::map::Map::from_iter([
            ("id".into(), task_id.as_str().into()),
            ("title".into(), title.into()),
        ]));
        if let Some(id) = self.item_task(
            seq,
            0,
            &item,
            scope,
            source.as_ref().and_then(|source| source.parent),
            Some(origin),
        ) && let Some(task) = self.task_mut(id)
        {
            task.discovered = Some(seq);
            task.discovered_from = Some(origin.clone());
            task.split_origin = source.as_ref().map(|source| source.id);
            if source.is_none() {
                task.scope = TaskScope::Unassigned;
            }
        }
        if source.is_none() {
            self.unassigned(
                seq,
                "discovery has no unambiguous recorded origin occurrence",
            );
        }
    }
    fn split(
        &mut self,
        seq: u64,
        loop_id: &NodeKey,
        index: u32,
        origin: Option<&str>,
        into: &[toml::Value],
    ) {
        let Some(active) = self
            .active
            .last()
            .map(|index| &self.view.iterations[*index])
        else {
            self.unassigned(seq, "split without active loop");
            return;
        };
        if active.loop_id != *loop_id || active.index != index {
            self.unassigned(seq, "split does not match active loop occurrence");
            return;
        }
        let Some(id) = self.current_task() else {
            self.unassigned(seq, "split without current task");
            return;
        };
        let Some(source) = self
            .view
            .tasks
            .iter()
            .find(|task| task.id == id && origin == Some(task.task_id.as_str()))
            .cloned()
        else {
            self.unassigned(seq, "split origin conflicts with current task");
            return;
        };
        if let Some(task) = self.task_mut(id) {
            task.replaced = true;
        }
        let parents = source.scope.with_path(
            source
                .scope
                .observed()
                .map(|scope| scope[..scope.len().saturating_sub(1)].to_vec())
                .unwrap_or_default(),
        );
        let mut children = Vec::new();
        for (ordinal, item) in (0u32..).zip(into) {
            if let Some(child) = self.item_task(
                seq,
                ordinal,
                item,
                parents.clone(),
                source.parent,
                Some(&source.task_id),
            ) && let Some(task) = self.task_mut(child)
            {
                task.split = Some(seq);
                task.discovered_from = Some(source.task_id.clone());
                task.split_origin = Some(id);
                children.push(child);
            }
        }
        let mut ordered = Vec::new();
        self.view.tasks.retain(|task| {
            if children.contains(&task.id) {
                ordered.push(task.clone());
                false
            } else {
                true
            }
        });
        ordered.sort_by_key(|task| children.iter().position(|id| *id == task.id));
        if let Some(position) = self.view.tasks.iter().position(|task| task.id == id) {
            self.view.tasks.splice(position + 1..position + 1, ordered);
        }
    }
    fn enter(&mut self, seq: u64, node: &NodeKey, attempt: u32) {
        self.view.stages.push(StageExecution {
            entered: seq,
            node: node.clone(),
            attempt,
            task: self.current_task(),
            scope: self.scope(self.path()),
            result: None,
            continuity_unknown: None,
            failure: None,
            summary: None,
            sessions: Vec::new(),
            accepted: Vec::new(),
            inputs: None,
            skills: Vec::new(),
            waiting: false,
            quota_invocation: None,
        });
        self.stage = Some(self.view.stages.len() - 1);
        self.establishment = None;
    }
    fn verification_claim(
        &mut self,
        seq: u64,
        task: &RoadmapTaskId,
        node: &NodeKey,
        evidence: crate::ContentHash,
        report: Option<&crate::roadmap::VerificationReportArtifact>,
    ) {
        use crate::verification_evidence::{VerificationClaim, classify_claim};
        let claim = classify_claim(
            self.memory.verification_graph.as_deref(),
            node,
            task,
            evidence,
            report,
            self.memory.verification.subject.as_ref(),
            self.memory.verification.criteria.get(task),
        );
        if claim == VerificationClaim::Unauthorized {
            self.unassigned(seq, "verification claim lacks graph authority");
            return;
        }
        self.ledger(seq, task, node, Some(claim == VerificationClaim::Verified));
    }
    fn ledger(
        &mut self,
        seq: u64,
        task_id: &RoadmapTaskId,
        node: &NodeKey,
        verified: Option<bool>,
    ) {
        let target = self
            .stage
            .and_then(|index| self.view.stages.get(index))
            .filter(|stage| stage.node == *node && stage.scope == self.scope(self.path()))
            .and_then(|stage| stage.task);
        let record = self.memory.ledger.tasks.get(task_id).cloned();
        if let (Some(id), Some(record)) = (target, record)
            && let Some(task) = self.task_mut(id)
            && task.task_id == *task_id
        {
            task.status = record.status;
            task.verified = verified.unwrap_or(task.verified && record.verified);
            return;
        }
        self.unassigned(seq, "ledger claim has no unique scoped stage owner");
    }
    fn stage_event(&mut self, event: &RunEvent) {
        match &event.payload {
            E::StageCompleted { node, outcome } => {
                if let Some(stage) = self.active_stage(node, event.seq) {
                    stage.result = Some((event.seq, outcome.as_str().into()));
                    stage.waiting = false;
                }
            },
            E::StageFailed { node, reason, .. } => {
                if let Some(stage) = self.active_stage(node, event.seq) {
                    stage.failure = Some(reason.clone());
                    stage.waiting = false;
                }
            },
            E::OutcomeReported { node, summary, .. } => {
                if let Some(stage) = self.active_stage(node, event.seq) {
                    stage.summary = Some(summary.clone());
                }
            },
            E::StageInputsResolved {
                node,
                attempt,
                bindings,
                ..
            } => {
                if let Some(stage) = self.active_stage(node, event.seq) {
                    if stage.attempt == *attempt {
                        stage.inputs = Some(bindings.keys().cloned().collect());
                    } else {
                        self.unassigned(
                            event.seq,
                            "resolved inputs name another or unknown attempt",
                        );
                    }
                }
            },
            E::SkillBound { node, name, .. } => {
                if let Some(stage) = self.active_stage(node, event.seq)
                    && !stage.skills.contains(name)
                {
                    stage.skills.push(name.clone());
                }
            },
            E::SessionEstablishmentRequested {
                node,
                invocation,
                authority,
                ..
            } => self.establishment = Some((node.clone(), *invocation, authority.clone())),
            E::SessionOpened { .. } => self.session_opened(event),
            E::SessionClosed {
                session,
                disposition,
            } => self.session_closed(event.seq, *session, *disposition),
            E::StageOutcomeCommitted { commit } => self.accept(event.seq, commit),
            E::StageRouteCommitted {
                invocation,
                outcome_commit_seq,
            } => self.route(event.seq, *invocation, *outcome_commit_seq),
            E::ExecutionWriterClosed { writer } => self.writer_closed(event.seq, *writer, true),
            E::ExecutionWriterGroupStopped { writer } => {
                self.writer_closed(event.seq, *writer, false)
            },
            E::QuotaStagePlanned {
                stage_entry_seq,
                node,
                logical_invocation,
                ..
            } => {
                if let Some(stage) = self
                    .view
                    .stages
                    .iter_mut()
                    .find(|stage| stage.entered == *stage_entry_seq && stage.node == *node)
                {
                    stage.quota_invocation = Some(*logical_invocation);
                } else {
                    self.unassigned(event.seq, "quota opening without exact stage entry");
                }
            },
            E::HumanInputRequested {
                node,
                session,
                call_id,
                ..
            } => self.human_requested(event.seq, node, *session, call_id),
            E::HumanInputResolved { node, call_id, .. }
            | E::HumanInputTimedOut { node, call_id, .. } => {
                self.human_resolved(event.seq, node, call_id)
            },
            _ => {},
        }
    }
    fn human_requested(
        &mut self,
        seq: u64,
        node: &NodeKey,
        session: Option<SessionId>,
        call_id: &Option<String>,
    ) {
        let Some(stage) = self.active_stage(node, seq) else {
            return;
        };
        if session.is_some_and(|connection| {
            !stage
                .sessions
                .iter()
                .any(|opened| opened.connection == connection)
        }) {
            self.unassigned(seq, "human input connection does not own this stage");
            return;
        }
        let Some(index) = self.stage else {
            return;
        };
        let pending = (index, node.clone(), call_id.clone());
        if !self.pending_human.contains(&pending) {
            self.pending_human.push(pending);
        }
        self.view.stages[index].waiting = true;
    }
    fn human_resolved(&mut self, seq: u64, node: &NodeKey, call_id: &Option<String>) {
        let matches: Vec<_> = self
            .pending_human
            .iter()
            .enumerate()
            .filter(|(_, (_, requested_node, requested_call))| {
                requested_node == node && requested_call == call_id
            })
            .map(|(index, _)| index)
            .collect();
        if matches.len() != 1 {
            self.unassigned(seq, "human response has no unique request occurrence");
            return;
        }
        let (stage, _, _) = self.pending_human.remove(matches[0]);
        self.view.stages[stage].waiting = self
            .pending_human
            .iter()
            .any(|(index, _, _)| *index == stage);
    }
    fn session_opened(&mut self, event: &RunEvent) {
        let E::SessionOpened {
            node,
            session,
            opened: provider,
            agent: profile,
            agent_id: runtime,
            handoff,
        } = &event.payload
        else {
            return;
        };
        let seq = event.seq;
        let connection = *session;
        let handoff = *handoff;
        let establishment = self
            .establishment
            .take()
            .filter(|(requested, _, _)| requested == node);
        let invocation = establishment.as_ref().map(|(_, invocation, _)| *invocation);
        let authority = establishment.and_then(|(_, _, authority)| authority);
        if provider.is_none() || authority.is_none() {
            self.view
                .caveats
                .push(ExecutionCaveat::LegacySession { seq });
        }
        if provider.as_ref().is_some_and(|opened| {
            opened.session != connection || Some(opened.descriptor.invocation()) != invocation
        }) || authority
            .as_ref()
            .is_some_and(|authority| authority.run != self.view.run)
        {
            self.unassigned(
                seq,
                "provider descriptor conflicts with connection or establishment identity",
            );
        }
        let stage_index = self.stage;
        let mut location = None;
        if let Some(stage) = self.active_stage(node, seq) {
            location = stage_index.map(|index| (index, stage.sessions.len()));
            stage.sessions.push(SessionExecution {
                opened: seq,
                connection,
                provider: provider.clone(),
                authority,
                invocation,
                handoff,
                profile: profile.into(),
                runtime: runtime.clone(),
                closed: None,
                writer_closed: None,
                group_stopped: None,
            });
        }
        if let Some(location) = location {
            self.sessions
                .entry(connection)
                .and_modify(|recorded| *recorded = None)
                .or_insert(Some(location));
            if let Some(writer) = provider
                .as_ref()
                .and_then(|opened| opened.execution_writer.as_ref())
            {
                self.writers
                    .entry(writer.writer())
                    .or_default()
                    .push(location);
            }
        }
    }

    fn session_closed(
        &mut self,
        seq: u64,
        connection: SessionId,
        disposition: crate::run_event::SessionDisposition,
    ) {
        if let Some((stage, session)) = self.sessions.get(&connection).copied().flatten() {
            self.view.stages[stage].sessions[session].closed = Some((seq, disposition));
        } else {
            self.unassigned(seq, "closure without unique provider connection");
        }
    }
    fn writer_closed(&mut self, seq: u64, writer: crate::id::ExecutionWriterId, confirmed: bool) {
        if let Some(locations) = self.writers.get(&writer) {
            for (stage, session) in locations {
                let session = &mut self.view.stages[*stage].sessions[*session];
                if confirmed {
                    session.writer_closed = Some(seq);
                } else {
                    session.group_stopped = Some(seq);
                }
            }
        }
    }
    fn accept(&mut self, seq: u64, commit: &crate::execution_recovery::commit::StageOutcomeCommit) {
        let location = self
            .sessions
            .get(&commit.provider_connection())
            .copied()
            .flatten();
        if let Some((stage, session)) = location
            && commit.context().run == self.view.run
            && self.view.stages[stage].node == commit.context().node
            && session_matches_commit(&self.view.stages[stage].sessions[session], commit)
        {
            let index = self.view.stages[stage].accepted.len();
            self.view.stages[stage].accepted.push(AcceptedOutcome {
                seq,
                commit: commit.clone(),
                routed: None,
                route_records: Vec::new(),
                conflicting: false,
            });
            self.accepted
                .insert((commit.invocation(), seq), (stage, index));
        } else {
            self.unassigned(
                seq,
                "acceptance without exact session, invocation and authority owner",
            );
        }
        self.withdraw_conflicting(seq, commit.invocation());
    }
    fn withdraw_conflicting(&mut self, seq: u64, invocation: StageInvocationId) -> bool {
        let conflicting = self
            .memory
            .committed_stage_outcomes
            .get(&invocation)
            .is_some_and(|record| record.conflicting);
        if conflicting {
            for ((recorded, _), (stage, accepted)) in &self.accepted {
                if *recorded == invocation {
                    let accepted = &mut self.view.stages[*stage].accepted[*accepted];
                    accepted.conflicting = true;
                    accepted.routed = None;
                }
            }
            self.view
                .caveats
                .push(ExecutionCaveat::ConflictingInvocation { seq, invocation });
        }
        conflicting
    }
    fn route(&mut self, seq: u64, invocation: StageInvocationId, acceptance_seq: u64) {
        let conflicting = self.withdraw_conflicting(seq, invocation);
        if let Some((stage, accepted)) = self.accepted.get(&(invocation, acceptance_seq)).copied() {
            let accepted = &mut self.view.stages[stage].accepted[accepted];
            accepted.route_records.push(seq);
            if !conflicting && accepted.routed.is_none() {
                accepted.routed = Some(seq);
                return;
            }
        }
        self.view
            .caveats
            .push(ExecutionCaveat::ConflictingRoute { seq });
    }
}

fn owns_commit(
    stage: &StageExecution,
    commit: &crate::execution_recovery::commit::StageOutcomeCommit,
    run: RunId,
) -> bool {
    commit.context().run == run
        && stage.node == commit.context().node
        && stage
            .sessions
            .iter()
            .any(|session| session_matches_commit(session, commit))
}
fn session_matches_commit(
    session: &SessionExecution,
    commit: &crate::execution_recovery::commit::StageOutcomeCommit,
) -> bool {
    session.connection == commit.provider_connection()
        && session.invocation == Some(commit.invocation())
        && session.authority.as_ref() == Some(commit.context())
        && session.provider.as_ref().is_some_and(|opened| {
            opened.session == session.connection
                && opened.descriptor.invocation() == commit.invocation()
        })
}
