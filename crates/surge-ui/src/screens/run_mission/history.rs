//! Results observations compose journals; they never authorize a start or recovery.
use std::path::{Path, PathBuf};

use surge_core::bootstrap_operation::{
    BootstrapAttentionReason, BootstrapOperationStatus, BootstrapPhase, BootstrapState,
};
use surge_core::{ContentHash, EventPayload, RunEvent, RunId};
use surge_persistence::runs::Storage;
use surge_persistence::runs::bootstrap_operations::{
    BootstrapOperationRecord, BootstrapStoredPayload,
};
use surge_persistence::runs::inspection::{RunDatabaseInspection, RunInspection};

use crate::mission::{self, Mission};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::screens::runs) struct MissionRequest {
    pub selected: RunId,
    pub project: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum ImplementationHistory {
    Ordinary,
    PlanningOnly,
    AwaitingStartup,
    Recorded,
    Unavailable(HistoryIssue),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum HistoryIssue {
    MissingAfterStartup,
    ContradictoryStorage,
    EmptyJournal,
    Identity,
    TerminalOutcome,
    Read(String),
}

impl HistoryIssue {
    pub(super) fn message(&self) -> String {
        match self {
            Self::MissingAfterStartup => "Implementation history is unavailable; startup or its outcome cannot be confirmed.".into(),
            Self::ContradictoryStorage => "Implementation history is incomplete; existing storage contradicts an unstarted run.".into(),
            Self::EmptyJournal => "Implementation history is incomplete; its journal exists but contains no events.".into(),
            Self::Identity => "Implementation history does not match this operation's recorded identity.".into(),
            Self::TerminalOutcome => "Implementation history does not confirm the recorded terminal outcome.".into(),
            Self::Read(error) => format!("Implementation history is unavailable: {error}"),
        }
    }
}

pub(super) struct MissionSnapshot {
    pub mission: Mission,
    pub implementation: ImplementationHistory,
    pub operation: Option<BootstrapOperationStatus>,
}

pub(super) enum LoadOutcome {
    Loaded(Box<MissionSnapshot>),
    Stale,
}

pub(super) async fn load(
    home: Option<PathBuf>,
    request: MissionRequest,
) -> Result<LoadOutcome, String> {
    load_observed(home, request, std::future::ready(())).await
}

// The observation boundary is explicit so ordering tests can pause here without
// replacing any filesystem/SQLite reader or introducing a production poller.
pub(super) async fn load_observed(
    home: Option<PathBuf>,
    request: MissionRequest,
    before_recheck: impl std::future::Future<Output = ()>,
) -> Result<LoadOutcome, String> {
    tokio::runtime::Handle::try_current()
        .map_err(|_| "Run storage is not available".to_string())?;
    let home = home.ok_or("Surge home unavailable")?;
    let record =
        Storage::inspect_existing_bootstrap_operation_for_run(home.clone(), request.selected)
            .await
            .map_err(|error| error.to_string())?;
    let result = match &record {
        Some(record) => bootstrap(&home, &request, record).await,
        None => ordinary(&home, request.selected).await,
    };
    before_recheck.await;
    // Registry and journal reads are separate observations. Never publish evidence
    // assembled across a committed operation transition, including a cold lookup.
    let latest = Storage::inspect_existing_bootstrap_operation_for_run(home, request.selected)
        .await
        .map_err(|error| error.to_string())?;
    if latest != record {
        return Ok(LoadOutcome::Stale);
    }
    result.map(|snapshot| LoadOutcome::Loaded(Box::new(snapshot)))
}

async fn ordinary(home: &Path, run: RunId) -> Result<MissionSnapshot, String> {
    let rows = Storage::inspect_existing_run_events(home.join("runs"), run)
        .await
        .map_err(|error| error.to_string())?;
    if rows.is_empty() {
        return Err("Run history is incomplete; its journal contains no events".into());
    }
    let events = rows
        .iter()
        .map(|row| row.to_run_event(run).map_err(|error| error.to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(MissionSnapshot {
        mission: mission::fold(&[], run, &events, None).map_err(|error| error.to_string())?,
        implementation: ImplementationHistory::Ordinary,
        operation: None,
    })
}

async fn bootstrap(
    home: &Path,
    request: &MissionRequest,
    record: &BootstrapOperationRecord,
) -> Result<MissionSnapshot, String> {
    let BootstrapStoredPayload::V1 { capture, .. } = &record.payload else {
        return Err("This operation uses an unsupported stored payload".into());
    };
    let pins = capture.fields();
    if !same_project(request.project.as_deref(), &pins.repository).await?
        || ![record.status.planning_run, record.status.implementation_run]
            .contains(&request.selected)
    {
        return Err("This operation belongs to a different project or run".into());
    }
    let planning = Storage::inspect_existing_run(home.into(), record.status.planning_run)
        .await
        .map_err(|error| error.to_string())?;
    let planning = bound_events(
        planning,
        record.status.planning_run,
        &pins.planning_worktree,
        pins.graph.digest,
    )
    .map_err(|issue| format!("Planning history is unavailable: {}", issue.message()))?;
    let roadmap = crate::roadmap_source::read_run_roadmap(home, record.status.planning_run)
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .and_then(|text| toml::from_str(&text).ok());
    let mut mission = mission::fold(
        &planning,
        record.status.implementation_run,
        &[],
        roadmap.as_ref(),
    )
    .map_err(|error| error.to_string())?;
    let implementation =
        match Storage::inspect_existing_run(home.into(), record.status.implementation_run).await {
            Err(error) => ImplementationHistory::Unavailable(HistoryIssue::Read(error.to_string())),
            Ok(inspection) => {
                match implementation_events(record, inspection, &pins.implementation_worktree) {
                    Err(issue) => ImplementationHistory::Unavailable(issue),
                    Ok(ImplementationObservation::Absent) => absence_state(record),
                    Ok(ImplementationObservation::Recorded(events)) => {
                        match mission::fold(
                            &planning,
                            record.status.implementation_run,
                            &events,
                            roadmap.as_ref(),
                        ) {
                            Err(error) => ImplementationHistory::Unavailable(HistoryIssue::Read(
                                error.to_string(),
                            )),
                            Ok(_) if !outcome_matches(record, &events) => {
                                ImplementationHistory::Unavailable(HistoryIssue::TerminalOutcome)
                            },
                            Ok(recorded) => {
                                mission = recorded;
                                ImplementationHistory::Recorded
                            },
                        }
                    },
                }
            },
        };
    Ok(MissionSnapshot {
        mission,
        implementation,
        operation: Some(record.status.clone()),
    })
}

async fn same_project(project: Option<&Path>, captured: &Path) -> Result<bool, String> {
    let Some(project) = project else {
        return Ok(true);
    };
    if project == captured {
        return Ok(true);
    }
    let project = project.to_path_buf();
    let captured = captured.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let project = crate::project::ProjectScope::resolve(&project);
        let captured = crate::project::ProjectScope::resolve(&captured);
        project.root() == captured.root()
    })
    .await
    .map_err(|error| error.to_string())
}

enum ImplementationObservation {
    Absent,
    Recorded(Vec<RunEvent>),
}

fn implementation_events(
    record: &BootstrapOperationRecord,
    inspection: RunInspection,
    path: &Path,
) -> Result<ImplementationObservation, HistoryIssue> {
    if matches!(inspection.database, RunDatabaseInspection::Absent) {
        return if inspection.registry.is_none() && !inspection.run_directory_present {
            Ok(ImplementationObservation::Absent)
        } else {
            Err(HistoryIssue::ContradictoryStorage)
        };
    }
    let Some(child) = &record.child else {
        return Err(HistoryIssue::ContradictoryStorage);
    };
    bound_events(
        inspection,
        record.status.implementation_run,
        path,
        child.graph().digest,
    )
    .map(ImplementationObservation::Recorded)
}

fn absence_state(record: &BootstrapOperationRecord) -> ImplementationHistory {
    use BootstrapPhase::{PreparingImplementation, QueuedImplementation};
    if record.child.is_none() {
        return ImplementationHistory::PlanningOnly;
    }
    let awaiting = match record.status.state {
        BootstrapState::Pending { phase } | BootstrapState::Cancelling { phase } => {
            matches!(phase, QueuedImplementation | PreparingImplementation)
        },
        BootstrapState::NeedsAttention { phase, reason, .. } => {
            matches!(phase, QueuedImplementation | PreparingImplementation)
                && !matches!(
                    reason,
                    BootstrapAttentionReason::PartialStartup
                        | BootstrapAttentionReason::StorageUnconfirmed
                )
        },
        _ => false,
    };
    if awaiting {
        ImplementationHistory::AwaitingStartup
    } else {
        ImplementationHistory::Unavailable(HistoryIssue::MissingAfterStartup)
    }
}

fn bound_events(
    inspection: RunInspection,
    run: RunId,
    path: &Path,
    expected_graph: ContentHash,
) -> Result<Vec<RunEvent>, HistoryIssue> {
    let RunDatabaseInspection::Present { events } = inspection.database else {
        return Err(HistoryIssue::MissingAfterStartup);
    };
    if events.is_empty() {
        return Err(HistoryIssue::EmptyJournal);
    }
    let Some(summary) = inspection.registry else {
        return Err(HistoryIssue::ContradictoryStorage);
    };
    if summary.id != run || summary.project_path != path {
        return Err(HistoryIssue::Identity);
    }
    let events = events
        .iter()
        .map(|row| {
            row.to_run_event(run)
                .map_err(|error| HistoryIssue::Read(error.to_string()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut starts = events.iter().filter_map(|event| match &event.payload {
        EventPayload::RunStarted { project_path, .. } => Some(project_path),
        _ => None,
    });
    if starts.next().is_none_or(|recorded| recorded != path) || starts.next().is_some() {
        return Err(HistoryIssue::Identity);
    }
    let mut graphs = events.iter().filter_map(|event| match &event.payload {
        EventPayload::PipelineMaterialized { graph, graph_hash } => Some((graph, graph_hash)),
        _ => None,
    });
    let Some((graph, hash)) = graphs.next() else {
        return Err(HistoryIssue::Identity);
    };
    if *hash != expected_graph
        || events
            .iter()
            .any(|event| matches!(event.payload, EventPayload::WorkItemAttemptBound { .. }))
    {
        return Err(HistoryIssue::Identity);
    }
    for (graph, hash) in std::iter::once((graph, hash)).chain(graphs) {
        if graph
            .identity()
            .map_err(|error| HistoryIssue::Read(error.to_string()))?
            != *hash
        {
            return Err(HistoryIssue::Identity);
        }
    }
    use surge_core::run_verdict::{TerminalKind, TerminalVerdict};
    let verdict = TerminalVerdict::of_run_events(&events);
    let expected = match summary.status {
        surge_core::RunStatus::Completed => Some(TerminalKind::Completed),
        surge_core::RunStatus::Failed => Some(TerminalKind::Failed),
        surge_core::RunStatus::Aborted => Some(TerminalKind::Aborted),
        _ => None,
    };
    if matches!(verdict, TerminalVerdict::Conflict { .. })
        || expected.is_some_and(|kind| {
            verdict
                .terminal()
                .is_none_or(|claim| claim.outcome.kind() != kind)
        })
    {
        return Err(HistoryIssue::TerminalOutcome);
    }
    Ok(events)
}

fn outcome_matches(record: &BootstrapOperationRecord, events: &[RunEvent]) -> bool {
    use surge_core::bootstrap_continuation::{BootstrapFailure, BootstrapTerminal};
    use surge_core::run_verdict::{TerminalOutcome, TerminalVerdict};
    let verdict = TerminalVerdict::of_run_events(events);
    match (&record.status.result, verdict) {
        (
            Some(BootstrapTerminal::Completed { run_id, terminal }),
            TerminalVerdict::Terminal(claim),
        ) => {
            *run_id == record.status.implementation_run
                && matches!(claim.outcome, TerminalOutcome::Completed { terminal_node } if terminal_node == *terminal)
        },
        (Some(BootstrapTerminal::Failed { run_id, reason }), TerminalVerdict::Terminal(claim)) => {
            *run_id == record.status.implementation_run
                && match reason {
                    BootstrapFailure::RunFailed | BootstrapFailure::BudgetExhausted => {
                        matches!(claim.outcome, TerminalOutcome::Failed { .. })
                    },
                    BootstrapFailure::RunAborted => {
                        matches!(claim.outcome, TerminalOutcome::Aborted { .. })
                    },
                }
        },
        (Some(BootstrapTerminal::Completed { .. } | BootstrapTerminal::Failed { .. }), _) => false,
        (_, TerminalVerdict::Conflict { .. }) => false,
        _ => true,
    }
}

pub(super) fn operation_message(status: &BootstrapOperationStatus) -> String {
    match status.state {
        BootstrapState::Pending { phase } => {
            format!("Operation: {}", super::super::phase_label(phase))
        },
        BootstrapState::NeedsAttention { reason, .. } => format!(
            "Operation needs attention: {}",
            super::super::attention_reason(reason).0
        ),
        BootstrapState::Cancelling { .. } => "Operation: cancelling".into(),
        BootstrapState::Completed => "Operation: completed".into(),
        BootstrapState::Failed => "Operation: failed".into(),
        BootstrapState::Cancelled => "Operation: cancelled".into(),
    }
}
