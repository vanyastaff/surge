use super::*;
use crate::work_items::{TaskCache, detail_matches};
use surge_core::{
    id::WorkItemId,
    work_item::{AcceptedWorkItemOrigin, WorkItemRevision},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum OperationKey {
    Task {
        project: surge_core::id::WorkItemProjectId,
        item: WorkItemId,
    },
    Bootstrap(RunId),
    Legacy(RunId),
}
#[derive(Clone)]
pub(crate) struct HomeOperation {
    pub(crate) key: OperationKey,
    pub(crate) title: String,
    pub(crate) asked: Option<String>,
    pub(crate) accepted: Option<WorkItemRevision>,
    pub(crate) revisions: Vec<WorkItemRevision>,
    pub(crate) runs: Vec<RunId>,
    pub(crate) status: String,
    pub(crate) history_more: bool,
}
#[derive(Clone)]
pub(crate) struct CurrentWork {
    pub(crate) run: RunId,
    pub(crate) entered: u64,
    pub(crate) title: String,
    pub(crate) profile: Option<String>,
    pub(crate) runtime: Option<String>,
    pub(crate) session: Option<surge_core::SessionId>,
}
#[derive(Default)]
pub(crate) struct HomeProjection {
    pub(crate) operations: Vec<HomeOperation>,
    pub(crate) decisions: Vec<surge_orchestrator::operator::PendingDecision>,
    pub(crate) decisions_uncertain: bool,
    pub(crate) verified: usize,
    pub(crate) recorded_tasks: usize,
    pub(crate) unknown_runs: usize,
    pub(crate) owners: HashMap<RunId, RunOwnership>,
    pub(crate) current: Vec<CurrentWork>,
    pub(crate) unknown: Vec<(RunId, String)>,
}
impl HomeProjection {
    /// Compose existing accepted records and execution projections. No files or event folds.
    pub(crate) fn compile(tasks: &TaskCache, history: &ProjectHistory) -> Self {
        let mut result = Self {
            decisions_uncertain: history.source.availability != Availability::Available
                || history.source.coverage != Coverage::Complete
                || history.source.freshness != Freshness::Observed,
            ..Self::default()
        };
        let Some(scope) = history.scope.as_ref() else {
            return result;
        };
        let mut associated = std::collections::HashSet::new();
        append_tasks(&mut result, tasks, scope, &mut associated);
        append_bootstrap(&mut result, history, &mut associated);
        append_legacy(&mut result, history, &mut associated);
        append_run_facts(&mut result, history, &associated);
        result
            .decisions
            .sort_by_key(|decision| (decision.kind.rank(), decision.requested_at));
        result
            .current
            .sort_by_key(|work| (work.run.to_string(), work.entered));
        result.unknown.sort_by_key(|(run, _)| run.to_string());
        result
    }
}
pub(crate) fn origin_text(origin: &AcceptedWorkItemOrigin) -> String {
    match origin {
        AcceptedWorkItemOrigin::Requirements(requirements) => requirements.text().into(),
        AcceptedWorkItemOrigin::Flow(flow) => flow.raw_prompt().into(),
    }
}
fn bootstrap_status(state: &surge_core::bootstrap_operation::BootstrapState) -> &'static str {
    use surge_core::bootstrap_operation::BootstrapState;
    match state {
        BootstrapState::Pending { .. } => "Planning or execution in progress",
        BootstrapState::NeedsAttention { .. } => "Needs attention",
        BootstrapState::Cancelling { .. } => "Stopping",
        BootstrapState::Completed => "Execution finished",
        BootstrapState::Failed => "Execution failed",
        BootstrapState::Cancelled => "Stopped",
    }
}

fn register_owner(owners: &mut HashMap<RunId, RunOwnership>, run: RunId, owner: RunOwnership) {
    use std::collections::hash_map::Entry;
    match owners.entry(run) {
        Entry::Vacant(entry) => {
            entry.insert(owner);
        },
        Entry::Occupied(mut entry) => {
            if entry.get() != &owner {
                entry.insert(RunOwnership::Unknown);
            }
        },
    }
}

fn append_run_facts(
    result: &mut HomeProjection,
    history: &ProjectHistory,
    associated: &std::collections::HashSet<RunId>,
) {
    result.decisions_uncertain |= associated.iter().any(|run| {
        let Some(expected) = result
            .owners
            .get(run)
            .filter(|owner| **owner != RunOwnership::Unknown)
        else {
            return true;
        };
        history.runs.get(run).is_none_or(|observed| {
            &observed.ownership != expected
                || observed.source.availability != Availability::Available
                || observed.source.coverage != Coverage::Complete
                || observed.source.freshness != Freshness::Observed
        })
    });
    for run in history.runs.values() {
        if matches!(run.ownership, RunOwnership::Unknown) {
            if run.ownership == RunOwnership::Unknown {
                result.unknown_runs += 1;
                result.unknown.push((run.run, run.source.label()));
            }
            continue;
        }
        if !associated.contains(&run.run) || result.owners.get(&run.run) != Some(&run.ownership) {
            continue;
        }
        if let Some(execution) = &run.execution
            && run.source.availability == Availability::Available
        {
            if run.source.coverage == Coverage::Complete
                && run.source.freshness == Freshness::Observed
                && history
                    .live_observed
                    .get(&run.run)
                    .is_some_and(|seq| run.source.through.is_some_and(|through| through >= *seq))
            {
                append_current(result, run.run, execution);
            }
            if run.source.coverage == Coverage::Complete {
                let recorded = execution.tasks.iter().filter(|task| {
                    matches!(
                        task.scope,
                        surge_core::task_execution::TaskScope::Recorded(_)
                    )
                });
                result.recorded_tasks += recorded.clone().count();
                result.verified += recorded.filter(|task| task.verified).count();
            }
        }
        if run.source.availability == Availability::Available
            && run.source.coverage == Coverage::Complete
            && run.source.freshness == Freshness::Observed
        {
            result.decisions.extend(
                run.decisions
                    .iter()
                    .filter(|decision| decision.actionable)
                    .cloned(),
            );
        } else {
            result.decisions_uncertain = true;
        }
    }
}
fn append_current(
    result: &mut HomeProjection,
    run: RunId,
    execution: &surge_core::task_execution::TaskExecutionView,
) {
    if execution.terminal.is_some() {
        return;
    }
    if let Some(stage) = execution.stages.last().filter(|stage| {
        stage.result.is_none() && stage.failure.is_none() && stage.continuity_unknown.is_none()
    }) {
        let task = stage
            .task
            .and_then(|id| execution.tasks.iter().find(|task| task.id == id));
        let session = stage.sessions.last();
        result.current.push(CurrentWork {
            run,
            entered: stage.entered,
            title: task.map_or_else(|| "Project workflow".into(), |task| task.title.clone()),
            profile: session.map(|session| session.profile.clone()),
            runtime: session.and_then(|session| session.runtime.clone()),
            session: session.map(|session| session.connection),
        });
    }
}

fn append_tasks(
    result: &mut HomeProjection,
    tasks: &TaskCache,
    scope: &HistoryScope,
    associated: &mut std::collections::HashSet<RunId>,
) {
    for record in &tasks.records {
        if scope.repository.as_ref() != Some(&record.workspace.repository) {
            continue;
        }
        let detail = tasks
            .details
            .get(&record.id)
            .filter(|detail| detail_matches(record, detail));
        let task_history = tasks.histories.get(&record.id);
        let mut runs = Vec::new();
        if let Some(task_history) = task_history {
            for attempt in &task_history.attempts.entries {
                if attempt.item == record.id && attempt.binding.item == record.id {
                    runs.push(attempt.run);
                    associated.insert(attempt.run);
                    let owner = attempt
                        .graph
                        .identity()
                        .map_or(RunOwnership::Unknown, |graph| RunOwnership::Modern {
                            project: record.project,
                            binding: attempt.binding.clone(),
                            graph: Some(graph),
                        });
                    register_owner(&mut result.owners, attempt.run, owner);
                }
            }
        }
        if let Some(run) = record.active_run
            && !runs.contains(&run)
        {
            runs.push(run);
            associated.insert(run);
            if let Some(detail) = detail
                && record.generation > 0
            {
                register_owner(
                    &mut result.owners,
                    run,
                    RunOwnership::Modern {
                        project: record.project,
                        binding: surge_core::work_item::WorkItemBinding {
                            item: record.id,
                            revision: record.accepted_revision,
                            requirements_hash: detail.revision.hash,
                            generation: record.generation,
                        },
                        graph: None,
                    },
                );
            }
        }
        let accepted = detail.map(|detail| detail.revision.clone());
        result.operations.push(HomeOperation {
            key: OperationKey::Task {
                project: record.project,
                item: record.id,
            },
            title: record.title.clone(),
            asked: accepted
                .as_ref()
                .map(|revision| origin_text(&revision.origin)),
            accepted,
            revisions: task_history
                .map(|history| history.revisions.entries.clone())
                .unwrap_or_default(),
            runs,
            status: crate::work_items::task_status(
                detail.map(Arc::as_ref),
                task_history.map(Arc::as_ref),
                record,
            )
            .into(),
            history_more: task_history.is_some_and(|history| {
                history.attempts.next_cursor.is_some()
                    || history.revisions.next_cursor.is_some()
                    || history.discussion.next_cursor.is_some()
            }),
        });
    }
}

fn append_bootstrap(
    result: &mut HomeProjection,
    history: &ProjectHistory,
    associated: &mut std::collections::HashSet<RunId>,
) {
    let mut operations: Vec<_> = history.operations.values().collect();
    operations.sort_by_key(|operation| std::cmp::Reverse(operation.record.status.queue_sequence));
    for operation in operations {
        let status = &operation.record.status;
        let runs = vec![status.planning_run, status.implementation_run];
        associated.extend(runs.iter().copied());
        register_owner(
            &mut result.owners,
            status.planning_run,
            RunOwnership::Bootstrap {
                operation: status.operation_id,
                planning: true,
            },
        );
        register_owner(
            &mut result.owners,
            status.implementation_run,
            RunOwnership::Bootstrap {
                operation: status.operation_id,
                planning: false,
            },
        );
        let asked=match &operation.record.payload {
                surge_persistence::runs::bootstrap_operations::BootstrapStoredPayload::V1{intent,..}=>Some(intent.prompt().into()),
                surge_persistence::runs::bootstrap_operations::BootstrapStoredPayload::Unsupported{..}=>None,
            };
        result.operations.push(HomeOperation {
            key: OperationKey::Bootstrap(status.operation_id),
            title: asked
                .clone()
                .unwrap_or_else(|| "Earlier project request".into()),
            asked,
            accepted: None,
            revisions: Vec::new(),
            runs,
            status: bootstrap_status(&status.state).into(),
            history_more: false,
        });
    }
}

fn append_legacy(
    result: &mut HomeProjection,
    history: &ProjectHistory,
    associated: &mut std::collections::HashSet<RunId>,
) {
    for summary in &history.summaries {
        let Some(run) = history.runs.get(&summary.id) else {
            continue;
        };
        if !associated.contains(&run.run)
            && matches!(
                run.ownership,
                RunOwnership::RecordedProject | RunOwnership::CandidateProject
            )
        {
            associated.insert(run.run);
            register_owner(&mut result.owners, run.run, RunOwnership::RecordedProject);
            let asked = run.events.iter().find_map(|event| match &event.payload {
                surge_core::EventPayload::RunStarted { initial_prompt, .. } => {
                    Some(initial_prompt.clone())
                },
                _ => None,
            });
            result.operations.push(HomeOperation {
                key: OperationKey::Legacy(run.run),
                title: asked
                    .clone()
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or_else(|| "Earlier workflow".into()),
                asked,
                accepted: None,
                revisions: Vec::new(),
                runs: vec![run.run],
                status: "Recorded workflow".into(),
                history_more: false,
            });
        }
    }
}
