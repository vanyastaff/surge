//! One daemon request owner shared by Home and Tasks; screens only request refreshes.
use super::*;
use crate::app_state::AppState;
use gpui_kit::{App, AsyncApp, Entity};
mod details;
pub(crate) use details::request_task_detail;

pub(crate) fn refresh_tasks_if_changed(state: &Entity<AppState>, cx: &mut App) -> bool {
    let current = state.read(cx);
    let identity = (
        current
            .daemon_state
            .facade()
            .map_or(0, |facade| std::sync::Arc::as_ptr(&facade) as usize),
        current.project_path.clone(),
    );
    if current.tasks.refresh_identity.as_ref() == Some(&identity) {
        return false;
    }
    let changed = current
        .tasks
        .refresh_identity
        .as_ref()
        .is_some_and(|old| old.1 != identity.1);
    state.update(cx, |state, _| {
        if changed {
            state.tasks.clear_project();
        }
        state.tasks.refresh_identity = Some(identity.clone());
        if identity.0 == 0 {
            state.tasks.disconnected();
        }
    });
    refresh_project_tasks(state, false, cx);
    changed
}
pub(crate) fn refresh_project_tasks(state: &Entity<AppState>, append: bool, cx: &mut App) {
    let current = state.read(cx);
    let Some(facade) = current.daemon_state.facade() else {
        return;
    };
    let Some(repository) = current
        .project_scope
        .as_ref()
        .and_then(|scope| scope.repository())
        .map(std::path::Path::to_path_buf)
    else {
        return;
    };
    if current.tasks.loading {
        return;
    }
    let after = if append {
        current.tasks.next_cursor.clone()
    } else {
        None
    };
    let scope = state.update(cx, |state, cx| {
        let scope = state.tasks.begin(repository.clone());
        cx.notify();
        scope
    });
    let state = state.downgrade();
    cx.spawn(async move |cx: &mut AsyncApp| {
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(8),
            list_project_tasks(&facade, &repository, after),
        )
        .await
        .map_err(|_| "Task list is unavailable; retry to refresh.".into())
        .and_then(|result| result);
        let Ok(items) = state.update(cx, |state, cx| {
            match result {
                Ok(page) => state.tasks.apply_page(&scope, page, append),
                Err(error) => state.tasks.fail(&scope, error),
            }
            cx.notify();
            if !state.tasks.accepts(&scope) {
                return Vec::new();
            }
            let mut records = state.tasks.records.iter().collect::<Vec<_>>();
            records.sort_by_key(|record| {
                state
                    .tasks
                    .details
                    .get(&record.id)
                    .is_some_and(|detail| detail_matches(record, detail))
                    && state.tasks.histories.contains_key(&record.id)
            });
            records
                .into_iter()
                .take(8)
                .map(|record| record.id)
                .collect::<Vec<_>>()
        }) else {
            return;
        };
        cx.update(|cx| {
            if let Some(state) = state.upgrade() {
                for item in items {
                    request_task_detail(&state, item, cx);
                }
            }
        });
    })
    .detach();
}
pub(crate) fn request_task_history(
    state: &Entity<AppState>,
    item: WorkItemId,
    kind: TaskHistoryKind,
    cx: &mut App,
) {
    let Some(facade) = state.read(cx).daemon_state.facade() else {
        return;
    };
    let Some(request) = state.update(cx, |state, _| state.tasks.begin_history(item, kind)) else {
        return;
    };
    let command = match kind {
        TaskHistoryKind::Attempts => WorkItemCommand::Attempts {
            item,
            after: Some(request.cursor.clone()),
            limit: 100,
        },
        TaskHistoryKind::Discussion => WorkItemCommand::Discussion {
            item,
            after: Some(request.cursor.clone()),
            limit: 100,
        },
        TaskHistoryKind::Revisions => WorkItemCommand::Revisions {
            item,
            after: Some(request.cursor.clone()),
            limit: 100,
        },
    };
    let state = state.downgrade();
    cx.spawn(async move |cx: &mut AsyncApp| {
        let result =
            tokio::time::timeout(std::time::Duration::from_secs(8), facade.work_item(command))
                .await
                .map_err(|_| "Task history is unavailable; retry to refresh.".to_owned())
                .and_then(|result| result.map_err(|error| error.to_string()));
        let _ = state.update(cx, |state, cx| {
            if !state.tasks.accepts_history_request(&request) {
                return;
            }
            match result {
                Ok(result) => state.tasks.apply_history(&request, result),
                Err(error) => state.tasks.error = Some(error.to_string()),
            }
            cx.notify();
        });
    })
    .detach();
}

#[cfg(all(test, unix))]
mod tests;
