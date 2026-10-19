//! Background-only inspection. Each source has its own observation boundary.
use super::*;
use std::num::NonZeroU32;
use surge_core::{EventPayload as E, task_execution::TaskExecutionView};
use surge_persistence::runs::{Storage, seq::EventSeq};

pub(crate) struct RegistryBatch {
    pub(crate) summaries: Vec<RunSummary>,
    pub(crate) operations: HashMap<RunId, Arc<LegacyOperation>>,
    pub(crate) runs: HashMap<RunId, Arc<ProjectRun>>,
    pub(crate) cursor: Option<RunSummaryCursor>,
    pub(crate) source: SourceObservation,
    reads: usize,
}
pub(crate) async fn load_registry(
    scope: HistoryScope,
    mut cursor: Option<RunSummaryCursor>,
    retained: HashMap<RunId, Arc<ProjectRun>>,
) -> Result<RegistryBatch, String> {
    let mut batch = RegistryBatch {
        summaries: Vec::new(),
        operations: HashMap::new(),
        runs: HashMap::new(),
        cursor: None,
        source: SourceObservation::default(),
        reads: 0,
    };
    for _ in 0..4 {
        let page = Storage::inspect_existing_run_summary_page(
            scope.home.clone(),
            NonZeroU32::new(32).ok_or("invalid page bound")?,
            cursor,
        )
        .await
        .map_err(|error| error.to_string())?;
        let Some(page) = page else {
            batch.source = SourceObservation {
                availability: Availability::Absent,
                freshness: Freshness::Observed,
                coverage: Coverage::Complete,
                through: None,
            };
            return Ok(batch);
        };
        for summary in &page.entries {
            inspect_summary(&scope, summary, &retained, &mut batch).await?;
        }
        batch.summaries.extend(page.entries);
        cursor = page.next_cursor;
        if cursor.is_none() || !batch.operations.is_empty() || !batch.runs.is_empty() {
            break;
        }
    }
    batch.source = SourceObservation {
        availability: Availability::Available,
        freshness: Freshness::Observed,
        coverage: if cursor.is_some() {
            Coverage::Partial
        } else {
            Coverage::Complete
        },
        through: None,
    };
    batch.cursor = cursor;
    Ok(batch)
}
async fn inspect_summary(
    scope: &HistoryScope,
    summary: &RunSummary,
    retained: &HashMap<RunId, Arc<ProjectRun>>,
    batch: &mut RegistryBatch,
) -> Result<(), String> {
    let operation =
        Storage::inspect_existing_bootstrap_operation_for_run(scope.home.clone(), summary.id).await;
    let operation = match operation {
        Ok(operation) => operation,
        Err(error) => {
            batch.runs.insert(
                summary.id,
                Arc::new(unavailable_run(
                    summary.id,
                    RunOwnership::Unknown,
                    error.to_string(),
                    retained.get(&summary.id),
                )),
            );
            return Ok(());
        },
    };
    let ownership = if let Some(record) = operation {
        let owned=match &record.payload {
                    surge_persistence::runs::bootstrap_operations::BootstrapStoredPayload::V1{capture,..}=>scope.repository.as_ref().map_or_else(|| capture.fields().repository==scope.root, |repository| repository==&capture.fields().git_common_dir),
                    surge_persistence::runs::bootstrap_operations::BootstrapStoredPayload::Unsupported{..}=>false,
                };
        if !owned {
            return Ok(());
        }
        let owner = RunOwnership::Bootstrap {
            operation: record.status.operation_id,
            planning: summary.id == record.status.planning_run,
        };
        let (roadmap, roadmap_source) = approved_roadmap(scope, &record).await;
        batch.operations.insert(
            record.status.operation_id,
            Arc::new(LegacyOperation {
                record,
                roadmap,
                roadmap_source,
            }),
        );
        owner
    } else if recorded_path_matches(scope.clone(), summary.project_path.clone()).await? {
        RunOwnership::RecordedProject
    } else {
        return Ok(());
    };
    let ownership = retained
        .get(&summary.id)
        .filter(|run| matches!(run.requested, RunOwnership::Modern { .. }))
        .map_or(ownership, |run| run.requested.clone());
    if batch.reads < 2 {
        batch.reads += 1;
        let roadmap = match ownership {
            RunOwnership::Bootstrap { operation, .. } => batch
                .operations
                .get(&operation)
                .and_then(|operation| operation.roadmap.clone()),
            _ => None,
        };
        let observed = match load_run(
            scope.clone(),
            summary.id,
            ownership.clone(),
            retained
                .get(&summary.id)
                .filter(|run| run.requested == ownership)
                .map_or_else(Arc::default, |run| run.events.clone()),
            roadmap,
            Arc::default(),
        )
        .await
        {
            Ok(observed) => observed,
            Err(error) => unavailable_run(summary.id, ownership, error, retained.get(&summary.id)),
        };
        batch.runs.insert(summary.id, Arc::new(observed));
    } else {
        let requested = ownership.clone();
        let ownership = if ownership == RunOwnership::RecordedProject {
            RunOwnership::CandidateProject
        } else {
            ownership
        };
        batch.runs.entry(summary.id).or_insert_with(|| {
            Arc::new(ProjectRun {
                run: summary.id,
                ownership,
                requested,
                source: SourceObservation::default(),
                events: Arc::default(),
                execution: None,
                mission: None,
                decisions: Arc::default(),
            })
        });
        if let Some(previous) = retained.get(&summary.id) {
            batch.runs.insert(summary.id, previous.clone());
        }
    }
    Ok(())
}
fn unavailable_run(
    run: RunId,
    requested: RunOwnership,
    error: String,
    retained: Option<&Arc<ProjectRun>>,
) -> ProjectRun {
    if let Some(previous) = retained {
        let mut observed = previous.as_ref().clone();
        if observed.requested != requested {
            observed.ownership = RunOwnership::Unknown;
        }
        observed.requested = requested;
        observed.source.availability = Availability::Unavailable(error);
        observed.source.freshness = Freshness::Retained;
        observed.source.coverage = Coverage::Unknown;
        return observed;
    }
    ProjectRun {
        run,
        ownership: RunOwnership::Unknown,
        requested,
        source: SourceObservation {
            availability: Availability::Unavailable(error),
            freshness: Freshness::Observed,
            coverage: Coverage::Unknown,
            through: None,
        },
        events: Arc::default(),
        execution: None,
        mission: None,
        decisions: Arc::default(),
    }
}
async fn recorded_path_matches(scope: HistoryScope, path: PathBuf) -> Result<bool, String> {
    tokio::task::spawn_blocking(move || {
        let path = path.canonicalize().unwrap_or(path);
        path == scope.root
            || scope
                .repository
                .as_ref()
                .is_some_and(|ours| crate::project::git_common_dir(&path).as_ref() == Some(ours))
    })
    .await
    .map_err(|error| error.to_string())
}
async fn approved_roadmap(
    scope: &HistoryScope,
    record: &BootstrapOperationRecord,
) -> (Option<Arc<RoadmapArtifact>>, SourceObservation) {
    let Some(reference) = record
        .child
        .as_ref()
        .and_then(|child| {
            child
                .artifacts()
                .iter()
                .find(|reference| reference.name == "roadmap")
        })
        .cloned()
    else {
        return (None, SourceObservation::default());
    };
    let path = scope
        .home
        .join("runs")
        .join(record.status.planning_run.to_string())
        .join("artifacts")
        .join(reference.hash.to_hex());
    let result = tokio::task::spawn_blocking(move || read_accepted_roadmap(&path, reference.hash))
        .await
        .map_err(|error| error.to_string())
        .and_then(|result| result);
    match result {
        Ok(roadmap) => (
            Some(roadmap),
            SourceObservation {
                availability: Availability::Available,
                freshness: Freshness::Observed,
                coverage: Coverage::Complete,
                through: None,
            },
        ),
        Err(error) => (
            None,
            SourceObservation {
                availability: Availability::Unavailable(error),
                freshness: Freshness::Observed,
                coverage: Coverage::Unknown,
                through: None,
            },
        ),
    }
}
/// Load another bounded prefix without dropping already observed task history.
pub(crate) async fn load_run(
    scope: HistoryScope,
    run: RunId,
    expected: RunOwnership,
    events: Arc<Vec<surge_core::RunEvent>>,
    roadmap: Option<Arc<RoadmapArtifact>>,
    planning: Arc<Vec<surge_core::RunEvent>>,
) -> Result<ProjectRun, String> {
    let mut events = (*events).clone();
    let mut coverage = Coverage::Partial;
    for _ in 0..4 {
        let after = EventSeq(events.last().map_or(0, |event| event.seq));
        let page = Storage::inspect_existing_events_after(
            scope.home.join("runs"),
            run,
            after,
            NonZeroU32::new(256).ok_or("invalid event page bound")?,
        )
        .await
        .map_err(|error| error.to_string())?;
        let Some(page) = page else {
            if !events.is_empty() {
                return Err("previously observed run journal is now absent".into());
            }
            return Ok(ProjectRun {
                run,
                ownership: expected.clone(),
                requested: expected,
                source: SourceObservation {
                    availability: Availability::Absent,
                    freshness: Freshness::Observed,
                    coverage: Coverage::Unknown,
                    through: None,
                },
                events: Arc::new(events),
                execution: None,
                mission: None,
                decisions: Arc::default(),
            });
        };
        let count = page.len();
        events.extend(
            page.iter()
                .map(|event| event.to_run_event(run))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| error.to_string())?,
        );
        if count < 256 {
            coverage = Coverage::Complete;
            break;
        }
    }
    let execution = TaskExecutionView::compile(run, &events).map_err(|error| error.to_string())?;
    let ownership = checked_ownership(&scope, run, &expected, &events).await?;
    let mission =
        crate::mission::from_execution(&planning, &execution, &events, roadmap.as_deref())
            .map_err(|error| error.to_string())?;
    let pending = surge_orchestrator::operator::PendingDecisions::from_events(run, &events);
    let decisions = if matches!(
        pending.status(),
        surge_orchestrator::operator::FoldStatus::Complete
    ) {
        pending.open().cloned().collect()
    } else {
        Vec::new()
    };
    Ok(ProjectRun {
        run,
        ownership,
        requested: expected,
        source: SourceObservation {
            availability: Availability::Available,
            freshness: Freshness::Observed,
            coverage,
            through: events.last().map(|event| event.seq),
        },
        events: Arc::new(events),
        execution: Some(Arc::new(execution)),
        mission: Some(Arc::new(mission)),
        decisions: Arc::new(decisions),
    })
}
async fn checked_ownership(
    scope: &HistoryScope,
    run: RunId,
    expected: &RunOwnership,
    events: &[surge_core::RunEvent],
) -> Result<RunOwnership, String> {
    let starts: Vec<_> = events
        .iter()
        .filter_map(|event| match &event.payload {
            E::RunStarted { project_path, .. } => Some(project_path.clone()),
            _ => None,
        })
        .collect();
    let graph = events.iter().find_map(|event| match &event.payload {
        E::PipelineMaterialized { graph, graph_hash } => Some((graph, graph_hash)),
        _ => None,
    });
    let Some((graph, hash)) = graph else {
        return Ok(RunOwnership::Unknown);
    };
    if starts.len() != 1 || graph.identity().map_err(|error| error.to_string())? != *hash {
        return Ok(RunOwnership::Unknown);
    }
    let bindings: Vec<_> = events
        .iter()
        .filter_map(|event| match &event.payload {
            E::WorkItemAttemptBound { context } => Some(context.binding()),
            _ => None,
        })
        .collect();
    match expected {
        RunOwnership::Modern {
            binding,
            graph: expected_graph,
            ..
        } => {
            if bindings.len() == 1
                && bindings[0] == binding
                && expected_graph.is_none_or(|expected_hash| *hash == expected_hash)
            {
                Ok(expected.clone())
            } else {
                Ok(RunOwnership::Unknown)
            }
        },
        RunOwnership::Bootstrap {
            operation,
            planning,
        } => {
            if !bindings.is_empty() {
                return Ok(RunOwnership::Unknown);
            }
            let Some(record) =
                Storage::inspect_existing_bootstrap_operation_for_run(scope.home.clone(), run)
                    .await
                    .map_err(|error| error.to_string())?
            else {
                return Ok(RunOwnership::Unknown);
            };
            let surge_persistence::runs::bootstrap_operations::BootstrapStoredPayload::V1 {
                capture,
                ..
            } = &record.payload
            else {
                return Ok(RunOwnership::Unknown);
            };
            let expected_path = if *planning {
                &capture.fields().planning_worktree
            } else {
                &capture.fields().implementation_worktree
            };
            let expected_hash = if *planning {
                Some(capture.fields().graph.digest)
            } else {
                record.child.as_ref().map(|child| child.graph().digest)
            };
            if record.status.operation_id == *operation
                && starts[0] == *expected_path
                && expected_hash == Some(*hash)
            {
                Ok(expected.clone())
            } else {
                Ok(RunOwnership::Unknown)
            }
        },
        RunOwnership::RecordedProject => {
            if bindings.is_empty()
                && recorded_path_matches(scope.clone(), starts[0].clone()).await?
            {
                Ok(expected.clone())
            } else {
                Ok(RunOwnership::Unknown)
            }
        },
        _ => Ok(RunOwnership::Unknown),
    }
}

pub(super) fn read_accepted_roadmap(
    path: &std::path::Path,
    hash: surge_core::ContentHash,
) -> Result<Arc<RoadmapArtifact>, String> {
    let bytes = super::planning_input::read_regular_bounded(path)?;
    if surge_core::ContentHash::compute(&bytes) != hash {
        return Err("accepted roadmap content does not match its reference".into());
    }
    let text = std::str::from_utf8(&bytes).map_err(|error| error.to_string())?;
    toml::from_str::<RoadmapArtifact>(text)
        .map(Arc::new)
        .map_err(|error| format!("accepted roadmap unavailable: {error}"))
}
