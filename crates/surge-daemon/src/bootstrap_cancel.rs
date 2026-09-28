//! Cancellation of inactive owned runs without restarting agent execution.
use crate::bootstrap_recovery::{ExpectedBootstrapRun, StartupState, classify_startup};
use std::sync::Arc;
use surge_core::{
    RunId, RunStatus, VersionedEventPayload,
    bootstrap_operation::{BootstrapPhase, BootstrapState},
    run_event::EventPayload,
};
use surge_orchestrator::engine::{Engine, RunOutcome};
use surge_persistence::runs::{Storage, bootstrap_operations::BootstrapStoredPayload};

/// Evidence obtained while the supervisor owns this operation's side-effect guard.
#[derive(Debug, PartialEq)]
pub(crate) enum CancellationEvidence {
    Absent,
    Aborted,
    Active,
    Terminal(RunOutcome),
}

/// Refusal preserves evidence and requires attention rather than fabricated cancellation.
#[derive(Debug, thiserror::Error)]
#[error("bootstrap cancellation could not be confirmed")]
pub(crate) struct CancellationError;

pub(crate) async fn cancel_inactive(
    storage: &Arc<Storage>,
    engine: &Engine,
    operation: RunId,
    expected: &ExpectedBootstrapRun,
) -> Result<CancellationEvidence, CancellationError> {
    let record = storage
        .bootstrap_operation_store()
        .get(operation)
        .map_err(|_| CancellationError)?
        .ok_or(CancellationError)?;
    if !matches!(record.payload, BootstrapStoredPayload::V1 { .. })
        || !record.status.cancel_requested
        || !matches!(record.status.state, BootstrapState::Cancelling { .. })
        || match record.status.state.phase() {
            Some(
                BootstrapPhase::QueuedPlanning
                | BootstrapPhase::PreparingPlanning
                | BootstrapPhase::Planning,
            ) => expected.run_id != record.status.planning_run,
            Some(
                BootstrapPhase::QueuedImplementation
                | BootstrapPhase::PreparingImplementation
                | BootstrapPhase::Implementing,
            ) => expected.run_id != record.status.implementation_run,
            None => true,
        }
    {
        return Err(CancellationError);
    }
    if engine
        .snapshot_active_runs()
        .await
        .iter()
        .any(|run| run.run_id == expected.run_id)
    {
        return Ok(CancellationEvidence::Active);
    }
    let inspect = storage
        .inspect_run(expected.run_id)
        .await
        .map_err(|_| CancellationError)?;
    match classify_startup(&inspect, expected).map_err(|_| CancellationError)? {
        StartupState::Absent => return Ok(CancellationEvidence::Absent),
        StartupState::Terminal(RunOutcome::Aborted { .. }) => {
            storage
                .set_run_status(
                    &expected.run_id,
                    RunStatus::Aborted,
                    Some(chrono::Utc::now().timestamp_millis()),
                )
                .await
                .map_err(|_| CancellationError)?;
            return Ok(CancellationEvidence::Aborted);
        },
        StartupState::Terminal(outcome) => return Ok(CancellationEvidence::Terminal(outcome)),
        StartupState::Incomplete(_) => return Err(CancellationError),
        StartupState::Started | StartupState::Parked { .. } => {},
    }
    let writer = storage
        .open_run_writer(expected.run_id)
        .await
        .map_err(|_| CancellationError)?;
    writer
        .append_event(VersionedEventPayload::new(EventPayload::RunAborted {
            reason: "bootstrap operation cancellation requested".into(),
        }))
        .await
        .map_err(|_| CancellationError)?;
    writer.flush().await.map_err(|_| CancellationError)?;
    storage
        .set_run_status(
            &expected.run_id,
            RunStatus::Aborted,
            Some(chrono::Utc::now().timestamp_millis()),
        )
        .await
        .map_err(|_| CancellationError)?;
    let inspect = storage
        .inspect_run(expected.run_id)
        .await
        .map_err(|_| CancellationError)?;
    if !matches!(
        classify_startup(&inspect, expected).map_err(|_| CancellationError)?,
        StartupState::Terminal(RunOutcome::Aborted { .. })
    ) {
        return Err(CancellationError);
    }
    Ok(CancellationEvidence::Aborted)
}
