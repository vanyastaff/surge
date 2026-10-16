//! Bounded readonly detail requests shared by project refresh and explicit inspection.
use super::*;
use surge_orchestrator::engine::daemon_facade::DaemonEngineFacade;

pub(crate) fn request_task_detail(state: &Entity<AppState>, item: WorkItemId, cx: &mut App) {
    let Some(facade) = state.read(cx).daemon_state.facade() else {
        return;
    };
    let worker = state.update(cx, |state, _| {
        let scope = state.tasks.scope.clone()?;
        let record = state
            .tasks
            .records
            .iter()
            .find(|record| record.id == item)?
            .clone();
        if !state.tasks.detail_queued.insert(item) {
            return None;
        }
        let request = state.tasks.begin_detail(item);
        state.tasks.detail_queue.push_back((record, request));
        if state.tasks.detail_worker.as_ref() == Some(&scope) {
            return None;
        }
        state.tasks.detail_worker = Some(scope.clone());
        let slots = state
            .tasks
            .detail_slots
            .get_or_insert_with(|| Arc::new(tokio::sync::Semaphore::new(2)))
            .clone();
        Some((scope, slots))
    });
    let Some((scope, slots)) = worker else {
        return;
    };
    let state = state.downgrade();
    cx.spawn(async move |cx: &mut AsyncApp| {
        loop {
            let requests = state
                .update(cx, |state, _| {
                    if !state.tasks.accepts(&scope)
                        || state.tasks.detail_worker.as_ref() != Some(&scope)
                    {
                        return Vec::new();
                    }
                    let requests = state
                        .tasks
                        .detail_queue
                        .drain(..state.tasks.detail_queue.len().min(2))
                        .collect::<Vec<_>>();
                    if requests.is_empty() {
                        state.tasks.detail_worker = None;
                    }
                    requests
                })
                .unwrap_or_default();
            if requests.is_empty() {
                break;
            }
            let first = fetch(&facade, slots.clone(), requests.first());
            let second = fetch(&facade, slots.clone(), requests.get(1));
            let (first, second) = tokio::join!(first, second);
            for (expected, request, result) in first.into_iter().chain(second) {
                let _ = state.update(cx, |state, cx| {
                    if !state.tasks.accepts(&scope)
                        || state.tasks.detail_requests.get(&expected.id) != Some(&request)
                    {
                        return;
                    }
                    state.tasks.detail_queued.remove(&expected.id);
                    match result {
                        Ok((detail, history))
                            if detail.item.id == expected.id
                                && detail.item.project == expected.project
                                && detail.item.version >= expected.version
                                && state
                                    .tasks
                                    .records
                                    .iter()
                                    .any(|current| same_record(current, &expected)) =>
                        {
                            state.tasks.apply_detail(request, detail, history)
                        },
                        Ok(_) => {
                            state.tasks.error = Some(
                                "Task changed while loading; refresh its accepted scope.".into(),
                            )
                        },
                        Err(error) => state.tasks.error = Some(error),
                    }
                    cx.notify();
                });
            }
        }
    })
    .detach();
}
fn same_record(current: &WorkItemRecord, expected: &WorkItemRecord) -> bool {
    current.id == expected.id
        && current.project == expected.project
        && current.version == expected.version
        && current.accepted_revision == expected.accepted_revision
        && current.generation == expected.generation
        && current.active_run == expected.active_run
        && current.archived_at_ms == expected.archived_at_ms
}
async fn fetch(
    facade: &DaemonEngineFacade,
    slots: Arc<tokio::sync::Semaphore>,
    request: Option<&(WorkItemRecord, u64)>,
) -> Option<(
    WorkItemRecord,
    u64,
    Result<(WorkItemDetail, TaskHistory), String>,
)> {
    let (expected, generation) = request?;
    let result = tokio::time::timeout(std::time::Duration::from_secs(8), async {
        let _permit = slots
            .acquire_owned()
            .await
            .map_err(|error| error.to_string())?;
        load_task(facade, expected.id).await
    })
    .await
    .map_err(|_| "Task details are still unavailable; retry to refresh.".into())
    .and_then(|result| result);
    Some((expected.clone(), *generation, result))
}
