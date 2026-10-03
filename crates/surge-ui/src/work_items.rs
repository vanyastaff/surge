//! Desktop cache and immutable command retries for daemon-owned tasks.

use std::collections::HashMap;
use std::path::PathBuf;

use surge_core::id::WorkItemId;
use surge_core::work_item::{
    WorkItemAttempt, WorkItemCommand, WorkItemDetail, WorkItemDiscussion, WorkItemPage,
    WorkItemRecord, WorkItemRevision,
};

#[derive(Clone)]
pub(crate) struct TaskHistory {
    pub(crate) attempts: WorkItemPage<WorkItemAttempt>,
    pub(crate) discussion: WorkItemPage<WorkItemDiscussion>,
    pub(crate) revisions: WorkItemPage<WorkItemRevision>,
}

#[derive(Clone, Copy)]
pub(crate) enum TaskHistoryKind {
    Attempts,
    Discussion,
    Revisions,
}

impl TaskHistory {
    pub(crate) fn next(&self, kind: TaskHistoryKind) -> Option<String> {
        match kind {
            TaskHistoryKind::Attempts => self.attempts.next_cursor.clone(),
            TaskHistoryKind::Discussion => self.discussion.next_cursor.clone(),
            TaskHistoryKind::Revisions => self.revisions.next_cursor.clone(),
        }
    }
}

pub(crate) fn task_status(
    detail: Option<&WorkItemDetail>,
    history: Option<&TaskHistory>,
    record: &WorkItemRecord,
) -> &'static str {
    use surge_core::execution_recovery::ExecutionControlState as Control;
    use surge_core::work_item::WorkItemAttemptState as Attempt;
    if record.archived_at_ms.is_some() {
        return "Archived";
    }
    if record.active_run.is_some() && detail.is_some_and(|detail| !detail_matches(record, detail)) {
        return "Run state is unconfirmed";
    }
    let attempt = history.and_then(|history| {
        if let Some(run) = record.active_run {
            history
                .attempts
                .entries
                .iter()
                .find(|attempt| attempt.run == run)
        } else {
            history
                .attempts
                .entries
                .iter()
                .max_by_key(|attempt| attempt.ordinal)
        }
    });
    if let Some(attempt) = attempt {
        if record.active_run.is_none()
            && (attempt.binding.revision != record.accepted_revision
                || attempt.accepted_revision_relation
                    == surge_core::work_item::AcceptedRevisionRelation::Superseded)
        {
            return "Needs execution for current revision";
        }
        match attempt.state {
            Attempt::Completed => return "Completed",
            Attempt::Failed => return "Failed",
            Attempt::Aborted => return "Stopped",
            Attempt::Rejected => return "Rejected",
            _ => {},
        }
    }
    if let Some(control) = detail
        .and_then(|detail| detail.control.as_ref())
        .filter(|control| Some(control.run) == record.active_run)
    {
        return match control.state {
            Control::SuspendRequested => "Suspending",
            Control::Suspended => "Suspended",
            Control::ContinueReserved => "Continuing",
            Control::Executing => "Running",
            Control::Attention => "Attention",
            Control::Terminal => "Execution finished",
        };
    }
    match attempt.map(|attempt| attempt.state) {
        Some(Attempt::Suspended) => "Suspended",
        Some(Attempt::Attention) => "Attention",
        Some(Attempt::Reserved) => "Reserved",
        Some(Attempt::Launched) => "Status unavailable",
        _ if record.active_run.is_some() => "Status unavailable",
        _ => "No active attempt",
    }
}

pub(crate) fn detail_matches(record: &WorkItemRecord, detail: &WorkItemDetail) -> bool {
    let observed = &detail.item;
    observed.id == record.id
        && observed.project == record.project
        && observed.version == record.version
        && observed.accepted_revision == record.accepted_revision
        && observed.generation == record.generation
        && observed.active_run == record.active_run
        && observed.archived_at_ms == record.archived_at_ms
        && detail.revision.revision == record.accepted_revision
        && detail.control.as_ref().is_none_or(|control| {
            control.item == record.id
                && record.active_run.map_or(
                    control.state
                        == surge_core::execution_recovery::ExecutionControlState::Terminal,
                    |run| control.run == run,
                )
                && control.attempt_generation == record.generation
        })
}

pub(crate) fn mutation_target(command: &WorkItemCommand) -> Option<(WorkItemId, u64)> {
    match command {
        WorkItemCommand::Edit {
            item,
            expected_version,
            ..
        }
        | WorkItemCommand::Discuss {
            item,
            expected_version,
            ..
        }
        | WorkItemCommand::AcceptProposal {
            item,
            expected_version,
            ..
        }
        | WorkItemCommand::Start {
            item,
            expected_version,
            ..
        }
        | WorkItemCommand::Suspend {
            item,
            expected_version,
            ..
        }
        | WorkItemCommand::Continue {
            item,
            expected_version,
            ..
        }
        | WorkItemCommand::Archive {
            item,
            expected_version,
            ..
        }
        | WorkItemCommand::AttachPr {
            item,
            expected_version,
            ..
        } => Some((*item, *expected_version)),
        _ => None,
    }
}

/// A request belongs to one project view; late responses cannot retarget it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TaskRequestScope {
    pub(crate) generation: u64,
    pub(crate) repository: PathBuf,
}

#[derive(Default)]
pub(crate) struct TaskCache {
    pub(crate) records: Vec<WorkItemRecord>,
    pub(crate) details: HashMap<WorkItemId, WorkItemDetail>,
    pub(crate) histories: HashMap<WorkItemId, TaskHistory>,
    pub(crate) next_cursor: Option<String>,
    pub(crate) loading: bool,
    pub(crate) error: Option<String>,
    pub(crate) fresh: bool,
    pub(crate) scope: Option<TaskRequestScope>,
    generation: u64,
    detail_generation: u64,
    detail_requests: HashMap<WorkItemId, u64>,
}

impl TaskCache {
    pub(crate) fn disconnected(&mut self) {
        self.generation = self.generation.saturating_add(1);
        if let Some(scope) = &mut self.scope {
            scope.generation = self.generation;
        }
        self.fresh = false;
        self.loading = false;
        self.error = Some("Daemon disconnected; showing last known task information.".into());
    }
    pub(crate) fn clear_project(&mut self) {
        self.generation = self.generation.saturating_add(1);
        self.scope = None;
        self.records.clear();
        self.details.clear();
        self.histories.clear();
        self.detail_requests.clear();
        self.next_cursor = None;
        self.loading = false;
        self.fresh = false;
        self.error = None;
    }
    pub(crate) fn begin(&mut self, repository: PathBuf) -> TaskRequestScope {
        self.generation = self.generation.saturating_add(1);
        let scope = TaskRequestScope {
            generation: self.generation,
            repository,
        };
        if self
            .scope
            .as_ref()
            .is_none_or(|old| old.repository != scope.repository)
        {
            self.records.clear();
            self.details.clear();
            self.histories.clear();
            self.detail_requests.clear();
            self.next_cursor = None;
        }
        self.scope = Some(scope.clone());
        self.loading = true;
        self.fresh = false;
        self.error = None;
        scope
    }

    pub(crate) fn accepts(&self, scope: &TaskRequestScope) -> bool {
        self.scope.as_ref() == Some(scope)
    }

    pub(crate) fn begin_detail(&mut self, item: WorkItemId) -> u64 {
        self.detail_generation = self.detail_generation.saturating_add(1);
        self.detail_requests.insert(item, self.detail_generation);
        self.detail_generation
    }

    pub(crate) fn apply_detail(
        &mut self,
        request: u64,
        detail: WorkItemDetail,
        history: TaskHistory,
    ) {
        let item = detail.item.id;
        if self.detail_requests.get(&item) != Some(&request)
            || !detail_matches(&detail.item, &detail)
            || self.records.iter().any(|record| {
                record.id == item
                    && (record.project != detail.item.project
                        || record.version > detail.item.version
                        || record.generation > detail.item.generation
                        || (record.version == detail.item.version
                            && !detail_matches(record, &detail)))
            })
            || self.details.get(&item).is_some_and(|old| {
                old.item.version > detail.item.version
                    || (old.item.active_run == detail.item.active_run
                        && old.control.is_some()
                        && detail.control.is_none())
                    || old
                        .control
                        .as_ref()
                        .zip(detail.control.as_ref())
                        .is_some_and(|(old, new)| {
                            old.run == new.run
                                && old.attempt_generation == new.attempt_generation
                                && old.generation > new.generation
                        })
            })
        {
            return;
        }
        if let Some(record) = self.records.iter_mut().find(|record| record.id == item) {
            *record = detail.item.clone();
        }
        self.details.insert(item, detail);
        self.histories.insert(item, history);
    }

    pub(crate) fn apply_page(
        &mut self,
        scope: &TaskRequestScope,
        page: WorkItemPage<WorkItemRecord>,
        append: bool,
    ) {
        if !self.accepts(scope) {
            return;
        }
        let mut known: HashMap<_, _> = self
            .records
            .iter()
            .map(|record| (record.id, record.clone()))
            .collect();
        for detail in self.details.values() {
            if known
                .get(&detail.item.id)
                .is_none_or(|record| record.version < detail.item.version)
            {
                known.insert(detail.item.id, detail.item.clone());
            }
        }
        if !append {
            self.records.clear();
        }
        for mut item in page
            .entries
            .into_iter()
            .filter(|item| item.workspace.repository == scope.repository)
        {
            if let Some(current) = known.get(&item.id).filter(|current| {
                current.project == item.project
                    && (current.version > item.version || current.generation > item.generation)
            }) {
                item = current.clone();
            }
            if let Some(old) = self.records.iter_mut().find(|old| old.id == item.id) {
                *old = item;
            } else {
                self.records.push(item);
            }
        }
        self.next_cursor = page.next_cursor;
        self.loading = false;
        self.fresh = true;
        self.error = None;
    }

    pub(crate) fn fail(&mut self, scope: &TaskRequestScope, error: String) {
        if !self.accepts(scope) {
            return;
        }
        self.loading = false;
        self.fresh = false;
        self.error = Some(error);
    }
}

/// Freeze the complete mutation, including versions and operation ID, at submission.
#[derive(Clone)]
pub(crate) struct TaskSubmission {
    command: WorkItemCommand,
}

impl TaskSubmission {
    pub(crate) fn new(command: WorkItemCommand) -> Self {
        Self { command }
    }
    pub(crate) fn retry(&self) -> WorkItemCommand {
        self.command.clone()
    }
}

pub(crate) fn reviewed_start_command(
    item: WorkItemId,
    version: u64,
    source: &str,
) -> Result<WorkItemCommand, String> {
    if source.len() > 1024 * 1024 {
        return Err("Workflow is larger than 1 MiB".into());
    }
    let graph: surge_core::graph::Graph =
        toml::from_str(source).map_err(|error| format!("Invalid workflow: {error}"))?;
    let report = surge_core::validation::validate(&graph);
    if report.has_errors() {
        return Err(format!(
            "Workflow validation failed: {:?}",
            report.errors().collect::<Vec<_>>()
        ));
    }
    Ok(WorkItemCommand::Start {
        operation_id: surge_core::id::WorkItemOperationId::new(),
        item,
        expected_version: version,
        graph: Box::new(graph),
        quota_recovery: None,
    })
}

/// Fetch every global page before project filtering; empty matching pages are not EOF.
pub(crate) async fn list_project_tasks(
    facade: &surge_orchestrator::engine::daemon_facade::DaemonEngineFacade,
    repository: &std::path::Path,
    mut after: Option<String>,
) -> Result<WorkItemPage<WorkItemRecord>, String> {
    use surge_core::work_item::WorkItemResult;
    let mut cursors = std::collections::HashSet::new();
    let mut records = Vec::new();
    for _ in 0..20 {
        let result = facade
            .work_item(WorkItemCommand::List { after, limit: 100 })
            .await
            .map_err(|error| error.to_string())?;
        let WorkItemResult::Items(page) = result else {
            return Err("Daemon returned an unexpected task-list response".into());
        };
        records.extend(
            page.entries
                .into_iter()
                .filter(|item| item.workspace.repository == repository),
        );
        let Some(cursor) = page.next_cursor else {
            return Ok(WorkItemPage {
                entries: records,
                next_cursor: None,
            });
        };
        if !cursors.insert(cursor.clone()) {
            return Err("Daemon repeated a task-list cursor".into());
        }
        after = Some(cursor);
    }
    Ok(WorkItemPage {
        entries: records,
        next_cursor: after,
    })
}

pub(crate) async fn load_task(
    facade: &surge_orchestrator::engine::daemon_facade::DaemonEngineFacade,
    item: WorkItemId,
) -> Result<(WorkItemDetail, TaskHistory), String> {
    use surge_core::work_item::WorkItemResult;
    let WorkItemResult::Detail(detail) = facade
        .work_item(WorkItemCommand::Show { item })
        .await
        .map_err(|error| error.to_string())?
    else {
        return Err("Unexpected task detail response".into());
    };
    let WorkItemResult::Attempts(attempts) = facade
        .work_item(WorkItemCommand::Attempts {
            item,
            after: None,
            limit: 100,
        })
        .await
        .map_err(|error| error.to_string())?
    else {
        return Err("Unexpected task attempts response".into());
    };
    let WorkItemResult::Discussion(discussion) = facade
        .work_item(WorkItemCommand::Discussion {
            item,
            after: None,
            limit: 100,
        })
        .await
        .map_err(|error| error.to_string())?
    else {
        return Err("Unexpected task discussion response".into());
    };
    let WorkItemResult::Revisions(revisions) = facade
        .work_item(WorkItemCommand::Revisions {
            item,
            after: None,
            limit: 100,
        })
        .await
        .map_err(|error| error.to_string())?
    else {
        return Err("Unexpected task revisions response".into());
    };
    Ok((
        *detail,
        TaskHistory {
            attempts,
            discussion,
            revisions,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use surge_core::id::WorkItemOperationId;

    fn record(repository: PathBuf, title: &str) -> WorkItemRecord {
        WorkItemRecord {
            id: WorkItemId::new(),
            project: surge_core::id::WorkItemProjectId::new(),
            title: title.into(),
            accepted_revision: 1,
            version: 1,
            archived_at_ms: None,
            active_run: None,
            generation: 0,
            workspace: surge_core::work_item::WorkItemWorkspace {
                repository,
                checkout: PathBuf::from("/fixture/project"),
                path: PathBuf::from("/fixture/task"),
                ownership: "fixture".into(),
                branch: "task/fixture".into(),
                base_commit: "a".repeat(40),
            },
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn project_list_follows_empty_matching_page_through_real_facade() {
        use surge_core::work_item::WorkItemResult;
        use surge_orchestrator::engine::ipc::{DaemonRequest, DaemonResponse};
        let home = tempfile::tempdir().unwrap();
        let socket = home.path().join("pages.sock");
        let ours = home.path().join("ours.git");
        let first = record(home.path().join("theirs.git"), "Other project");
        let second = record(ours.clone(), "Our retained task");
        let listener = tokio::net::UnixListener::bind(&socket).unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (read, mut write) = stream.into_split();
            let mut reader = tokio::io::BufReader::new(read);
            for (expected, entry, next_cursor) in [
                (None, first, Some("global-page-two".to_owned())),
                (Some("global-page-two".to_owned()), second, None),
            ] {
                let Some(DaemonRequest::WorkItem {
                    request_id,
                    command,
                }) = surge_orchestrator::engine::ipc::read_request_frame(&mut reader)
                    .await
                    .unwrap()
                else {
                    panic!("expected task page request");
                };
                match *command {
                    WorkItemCommand::List { after, .. } => assert_eq!(after, expected),
                    _ => panic!("unexpected command"),
                }
                surge_orchestrator::engine::ipc::write_frame(
                    &mut write,
                    &DaemonResponse::WorkItemOk {
                        request_id,
                        result: Box::new(WorkItemResult::Items(WorkItemPage {
                            entries: vec![entry],
                            next_cursor,
                        })),
                    },
                )
                .await
                .unwrap();
            }
        });
        let facade = surge_orchestrator::engine::daemon_facade::DaemonEngineFacade::connect(socket)
            .await
            .unwrap();
        let page = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            list_project_tasks(&facade, &ours, None),
        )
        .await
        .unwrap()
        .unwrap();
        server.await.unwrap();
        assert_eq!(page.entries.len(), 1);
        assert_eq!(page.entries[0].title, "Our retained task");
        assert!(page.next_cursor.is_none());
    }

    #[test]
    fn historical_launch_without_current_control_is_unconfirmed() {
        use surge_core::work_item::{
            AcceptedRevisionRelation, WorkItemAttemptState, WorkItemBinding,
        };
        let mut item = record(PathBuf::from("/repo/.git"), "Historical launch");
        let run = surge_core::RunId::new();
        item.active_run = Some(run);
        let requirements = surge_core::work_item::WorkItemRequirements::new(
            "Keep task".into(),
            vec!["Keep history".into()],
        )
        .unwrap();
        let attempt = WorkItemAttempt {
            item: item.id,
            run,
            ordinal: 1,
            binding: WorkItemBinding {
                item: item.id,
                revision: 1,
                requirements_hash: requirements.hash().unwrap(),
                generation: 1,
            },
            accepted_revision_relation: AcceptedRevisionRelation::MatchesCurrent,
            graph: Box::new(
                toml::from_str(include_str!("../../../examples/flow_terminal_only.toml")).unwrap(),
            ),
            config: "{}".into(),
            state: WorkItemAttemptState::Launched,
            diagnostic: None,
            usage_seq: 0,
            input_tokens: 0,
            output_tokens: 0,
            known_cost_usd: 0.0,
            usage_unknown: true,
        };
        let history = TaskHistory {
            attempts: WorkItemPage {
                entries: vec![attempt],
                next_cursor: None,
            },
            discussion: WorkItemPage {
                entries: vec![],
                next_cursor: None,
            },
            revisions: WorkItemPage {
                entries: vec![],
                next_cursor: None,
            },
        };
        let mut cache = TaskCache::default();
        cache.records.push(item.clone());
        cache.histories.insert(item.id, history.clone());
        cache.fresh = true;
        cache.disconnected();
        assert!(!cache.fresh);
        assert_eq!(
            task_status(None, Some(&history), &item),
            "Status unavailable"
        );
        assert_eq!(cache.histories[&item.id].attempts.entries[0].run, run);
    }

    #[test]
    fn failed_refresh_preserves_retained_task_and_excludes_other_project() {
        let mut cache = TaskCache::default();
        let ours = PathBuf::from("/project/a/.git");
        let scope = cache.begin(ours.clone());
        cache.apply_page(
            &scope,
            WorkItemPage {
                entries: vec![
                    record(ours.clone(), "Retained"),
                    record(PathBuf::from("/project/b/.git"), "Other"),
                ],
                next_cursor: Some("next".into()),
            },
            false,
        );
        let refresh = cache.begin(ours);
        cache.fail(&refresh, "Disconnected".into());
        assert_eq!(cache.records.len(), 1);
        assert_eq!(cache.records[0].title, "Retained");
        assert!(!cache.fresh);
    }

    #[test]
    fn retry_preserves_operation_and_observed_version() {
        let command = WorkItemCommand::Suspend {
            operation_id: WorkItemOperationId::new(),
            item: WorkItemId::new(),
            expected_version: 17,
        };
        let expected = serde_json::to_value(&command).unwrap();
        let submission = TaskSubmission::new(command);
        assert_eq!(serde_json::to_value(submission.retry()).unwrap(), expected);
        assert_eq!(serde_json::to_value(submission.retry()).unwrap(), expected);
    }

    #[test]
    fn reviewed_start_is_task_bound_and_invalid_graph_cannot_be_submitted() {
        let item = WorkItemId::new();
        let command = reviewed_start_command(
            item,
            23,
            include_str!("../../../examples/flow_terminal_only.toml"),
        )
        .unwrap();
        match command {
            WorkItemCommand::Start {
                item: actual,
                expected_version,
                graph,
                ..
            } => {
                assert_eq!(actual, item);
                assert_eq!(expected_version, 23);
                assert_eq!(graph.start.as_str(), "end");
            },
            _ => panic!("expected task-owned Start"),
        }
        assert!(reviewed_start_command(item, 23, "not a workflow").is_err());
    }

    #[test]
    fn stale_project_reply_cannot_clear_current_loading_or_error() {
        let mut cache = TaskCache::default();
        let old = cache.begin(PathBuf::from("/project/a/.git"));
        let current = cache.begin(PathBuf::from("/project/b/.git"));
        cache.apply_page(
            &old,
            WorkItemPage {
                entries: Vec::new(),
                next_cursor: None,
            },
            false,
        );
        assert!(cache.loading);
        cache.fail(&old, "old error".into());
        assert!(cache.error.is_none());
        cache.fail(&current, "current error".into());
        assert_eq!(cache.error.as_deref(), Some("current error"));
        assert!(!cache.fresh);
    }

    #[test]
    fn return_to_same_project_does_not_accept_its_old_request() {
        let mut cache = TaskCache::default();
        let repository = PathBuf::from("/project/a/.git");
        let old = cache.begin(repository.clone());
        cache.clear_project();
        let current = cache.begin(repository);
        assert_ne!(old, current);
        assert!(!cache.accepts(&old));
    }

    #[test]
    fn disconnect_rejects_pending_reply_and_marks_cache_stale() {
        let mut cache = TaskCache::default();
        let scope = cache.begin(PathBuf::from("/project/a/.git"));
        cache.fresh = true;
        cache.disconnected();
        cache.apply_page(
            &scope,
            WorkItemPage {
                entries: Vec::new(),
                next_cursor: None,
            },
            false,
        );
        assert!(!cache.fresh);
        assert!(!cache.loading);
        assert!(cache.error.is_some());
    }
}
