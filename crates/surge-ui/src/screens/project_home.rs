//! Native project landing surface, backed by existing owners and read-only history.
use crate::{
    app_state::AppState,
    project_lifecycle::{
        self as lifecycle, Availability, Coverage, Freshness, HistoryScope, HomeProjection,
        RunOwnership,
    },
};
use gpui_kit::*;
use std::collections::HashSet;
use surge_core::{RunId, id::WorkItemId};
use tracking::{ModelRebuild, RunReads, needs_run_refresh};
mod render;
mod tracking;
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum HomeAction {
    PlanApplication,
    CreateTask,
    OpenTask(WorkItemId),
    OpenRun(RunId),
    OpenRecorded {
        run: RunId,
        target: crate::screens::runs::RecordedTarget,
    },
    OpenDecision {
        run: RunId,
        request_seq: u64,
    },
    OpenPlan,
}
impl EventEmitter<HomeAction> for ProjectHomeScreen {}
pub(crate) struct ProjectHomeScreen {
    state: Entity<AppState>,
    model: HomeProjection,
    rebuild: ModelRebuild,
    inflight: RunReads,
    expanded: HashSet<lifecycle::OperationKey>,
    last_render: Option<std::time::Instant>,
    provenance_open: bool,
}
impl ProjectHomeScreen {
    pub(crate) fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        cx.observe(&state, |this, _, cx| {
            this.sync(cx);
            cx.notify();
        })
        .detach();
        let mut screen = Self {
            state,
            model: HomeProjection::default(),
            rebuild: ModelRebuild::default(),
            inflight: RunReads::default(),
            expanded: HashSet::new(),
            last_render: None,
            provenance_open: false,
        };
        screen.sync(cx);
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(5))
                    .await;
                if this
                    .update(cx, |this, cx| {
                        if this
                            .last_render
                            .is_some_and(|time| time.elapsed() < std::time::Duration::from_secs(6))
                        {
                            crate::work_items::refresh_project_tasks(&this.state, false, cx);
                            this.schedule_runs(cx);
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        screen
    }
    fn sync(&mut self, cx: &mut Context<Self>) {
        crate::work_items::refresh_tasks_if_changed(&self.state, cx);
        let state = self.state.read(cx);
        let identity = state.project_scope.as_ref().map(|scope| {
            (
                scope.root().to_path_buf(),
                scope.repository().map(std::path::Path::to_path_buf),
            )
        });
        if let Some((root, repository)) = identity
            && state
                .project_history
                .scope
                .as_ref()
                .is_none_or(|old| old.root != root || old.repository != repository)
        {
            self.inflight.clear();
            self.expanded.clear();
            self.reload(false, cx);
        }
        self.rebuild_model(cx);
    }
    fn rebuild_model(&mut self, cx: &mut Context<Self>) {
        let Some(generation) = self.rebuild.request() else {
            return;
        };
        self.spawn_model(generation, cx);
    }
    fn spawn_model(&mut self, generation: u64, cx: &mut Context<Self>) {
        let state = self.state.read(cx);
        let tasks = state.tasks.clone();
        let mut history = state.project_history.clone();
        if matches!(
            state.daemon_state,
            crate::daemon_link::ConnectionState::Connected(_)
        ) {
            history.live_observed = state
                .run_streams
                .iter()
                .filter(|(run, stream)| {
                    stream.live
                        && state.runs.iter().any(|summary| {
                            summary.run_id == **run
                                && matches!(
                                    summary.status,
                                    surge_orchestrator::engine::RunStatus::Active
                                )
                        })
                })
                .map(|(run, stream)| (*run, stream.last_seq))
                .collect();
        } else {
            history.live_observed.clear();
        }
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let model = cx
                .background_executor()
                .spawn(async move { HomeProjection::compile(&tasks, &history) })
                .await;
            let _ = this.update(cx, |this, cx| {
                let (publish, next) = this.rebuild.complete(generation);
                if publish {
                    this.model = model;
                    this.schedule_runs(cx);
                    cx.notify();
                }
                if let Some(next) = next {
                    this.spawn_model(next, cx);
                }
            });
        })
        .detach();
    }
    fn reload(&mut self, append: bool, cx: &mut Context<Self>) {
        let state = self.state.read(cx);
        if state.project_history.loading {
            return;
        }
        let Some(project) = state.project_scope.as_ref() else {
            return;
        };
        let Some(home) = surge_core::home::surge_home_dir() else {
            return;
        };
        let root = project.root().to_path_buf();
        let repository = project.repository().map(std::path::Path::to_path_buf);
        let cursor = if append {
            state.project_history.next_cursor.clone()
        } else {
            None
        };
        let retained = state.project_history.runs.clone();
        let scope = self.state.update(cx, |state, cx| {
            let scope = state.project_history.begin(root, repository, home, append);
            cx.notify();
            scope
        });
        self.inflight.start_epoch(scope.generation);
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            self.state.update(cx, |state, cx| {
                state
                    .project_history
                    .fail(&scope, "Background history service is unavailable".into());
                cx.notify();
            });
            return;
        };
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let task_scope = scope.clone();
            let result = runtime
                .spawn(async move {
                    tokio::join!(
                        lifecycle::load_registry(task_scope.clone(), cursor, retained),
                        lifecycle::load_planning_inputs(task_scope.root),
                    )
                })
                .await;
            let (result, inputs) = match result {
                Ok((registry, inputs)) => (registry, Some(inputs)),
                Err(error) => (Err(error.to_string()), None),
            };
            let _ = this.update(cx, |this, cx| {
                this.state.update(cx, |state, cx| {
                    if !state.project_history.accepts(&scope) {
                        return;
                    }
                    if let Some(inputs) = inputs {
                        state.project_history.planning_inputs = std::sync::Arc::new(inputs);
                    }
                    match result {
                        Ok(batch) => {
                            for summary in batch.summaries {
                                if let Some(old) = state
                                    .project_history
                                    .summaries
                                    .iter_mut()
                                    .find(|old| old.id == summary.id)
                                {
                                    *old = summary;
                                } else {
                                    state.project_history.summaries.push(summary);
                                }
                            }
                            state.project_history.operations.extend(batch.operations);
                            state.project_history.runs.extend(batch.runs);
                            state.project_history.next_cursor = batch.cursor;
                            state.project_history.source = batch.source;
                            state.project_history.loading = false;
                        },
                        Err(error) => state.project_history.fail(&scope, error),
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }
    fn schedule_runs(&mut self, cx: &mut Context<Self>) {
        let state = self.state.read(cx);
        let Some(scope) = state.project_history.scope.clone() else {
            return;
        };
        let mut queue = Vec::new();
        for (run, owner) in &self.model.owners {
            if self.inflight.contains(scope.generation, *run) || *owner == RunOwnership::Unknown {
                continue;
            }
            let stored = state.project_history.runs.get(run);
            let known_seq = state
                .runs
                .iter()
                .find(|summary| summary.run_id == *run)
                .and_then(|summary| summary.last_event_seq)
                .into_iter()
                .chain(
                    state
                        .run_streams
                        .get(run)
                        .filter(|stream| stream.live)
                        .map(|stream| stream.last_seq),
                )
                .max();
            if needs_run_refresh(stored.map(std::sync::Arc::as_ref), owner, known_seq) {
                queue.push((
                    *run,
                    owner.clone(),
                    stored.is_some_and(|stored| {
                        stored.requested == *owner && !stored.events.is_empty()
                    }),
                ));
            }
        }
        queue.sort_by_key(|(run, _, _)| run.to_string());
        for (run, owner, append) in queue
            .into_iter()
            .take(2usize.saturating_sub(self.inflight.count(scope.generation)))
        {
            self.load_one(scope.clone(), run, owner, append, cx);
        }
    }
    fn load_one(
        &mut self,
        scope: HistoryScope,
        run: RunId,
        owner: RunOwnership,
        more: bool,
        cx: &mut Context<Self>,
    ) {
        if !self.inflight.begin(scope.generation, run) {
            return;
        }
        let state = self.state.read(cx);
        let events = if more {
            state
                .project_history
                .runs
                .get(&run)
                .map(|run| run.events.clone())
                .unwrap_or_default()
        } else {
            std::sync::Arc::default()
        };
        let operation = match &owner {
            RunOwnership::Bootstrap { operation, .. } => {
                state.project_history.operations.get(operation)
            },
            _ => None,
        };
        let roadmap = operation.and_then(|operation| operation.roadmap.clone());
        let planning = operation
            .and_then(|operation| {
                state
                    .project_history
                    .runs
                    .get(&operation.record.status.planning_run)
            })
            .map(|run| run.events.clone())
            .unwrap_or_default();
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            self.inflight.finish(scope.generation, run);
            return;
        };
        let requested = owner.clone();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let observed = scope.clone();
            let result = runtime
                .spawn(async move {
                    lifecycle::load_run(observed, run, owner, events, roadmap, planning).await
                })
                .await
                .map_err(|error| error.to_string())
                .and_then(|result| result);
            let _ = this.update(cx, |this, cx| {
                this.inflight.finish(scope.generation, run);
                if !this.state.read(cx).project_history.accepts(&scope) {
                    return;
                }
                this.state.update(cx, |state, cx| {
                    match result {
                        Ok(run) => {
                            state
                                .project_history
                                .runs
                                .insert(run.run, std::sync::Arc::new(run));
                        },
                        Err(error) => {
                            if let Some(old) = state.project_history.runs.get_mut(&run) {
                                let old = std::sync::Arc::make_mut(old);
                                old.source.availability = Availability::Unavailable(error.clone());
                                old.source.freshness = Freshness::Retained;
                            } else {
                                state.project_history.runs.insert(
                                    run,
                                    std::sync::Arc::new(lifecycle::ProjectRun {
                                        run,
                                        ownership: RunOwnership::Unknown,
                                        requested,
                                        source: lifecycle::SourceObservation {
                                            availability: Availability::Unavailable(error.clone()),
                                            freshness: Freshness::Retained,
                                            coverage: Coverage::Unknown,
                                            through: None,
                                        },
                                        events: std::sync::Arc::default(),
                                        execution: None,
                                        mission: None,
                                        decisions: std::sync::Arc::default(),
                                    }),
                                );
                            }
                        },
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests;
