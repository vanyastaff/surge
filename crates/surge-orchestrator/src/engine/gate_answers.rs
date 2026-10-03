//! Durable answers for an owned suspended host decision without dispatching a run.
use super::EngineError;
use std::sync::Arc;
use surge_core::execution_recovery::{ExecutionControlState, PendingStagePhase};
use surge_core::{NodeKey, RunId, RunState, id::GateRequestId};
use surge_persistence::runs::{EventSeq, Storage};

pub(super) async fn resolve_suspended(
    storage: &Arc<Storage>,
    run: RunId,
    node: NodeKey,
    request: GateRequestId,
    response: serde_json::Value,
) -> Result<(), EngineError> {
    let store = storage.work_items();
    let attempt = store
        .for_run(run)
        .map_err(storage_error)?
        .ok_or(EngineError::RunNotFound(run))?;
    if !attempt.state.is_active() {
        return Err(EngineError::StaleGateRequest);
    }
    let claim = store.claim(run).map_err(storage_error)?;
    let control = store
        .execution_control(run)
        .map_err(storage_error)?
        .ok_or(EngineError::StaleGateRequest)?;
    if !matches!(
        control.state,
        ExecutionControlState::Suspended | ExecutionControlState::ContinueReserved
    ) {
        return Err(EngineError::StaleGateRequest);
    }
    let fence = control
        .fence
        .as_ref()
        .filter(|fence| fence.cleanup_confirmed)
        .ok_or(EngineError::StaleGateRequest)?;
    let PendingStagePhase::WaitingHumanGate {
        node: original_node,
        request: original_request,
        stage_entry_seq,
        requested_seq,
    } = &fence.pending_stage
    else {
        return Err(EngineError::StaleGateRequest);
    };
    if original_node != &node || *original_request != request {
        return Err(EngineError::StaleGateRequest);
    }
    let inspected = storage
        .inspect_folded_run(run)
        .await
        .map_err(storage_error)?;
    let history = inspected.database.ok_or(EngineError::StaleGateRequest)?;
    let RunState::Pipeline { cursor, memory, .. } = history.state else {
        return Err(EngineError::StaleGateRequest);
    };
    let record = memory
        .gate_decisions
        .get(&request)
        .ok_or(EngineError::StaleGateRequest)?;
    if record.conflicting
        || record.node != node
        || cursor.node != node
        || record.purpose != surge_core::run_state::GateDecisionPurpose::HumanGate
        || record.stage_entry_seq != *stage_entry_seq
        || record.requested_seq != *requested_seq
        || memory.suspension.as_ref() != Some(fence)
    {
        return Err(EngineError::StaleGateRequest);
    }
    if let Some(accepted) = &record.response {
        return if accepted == &response {
            Ok(())
        } else {
            Err(EngineError::StaleGateRequest)
        };
    }
    let config = record
        .gate_config
        .as_ref()
        .ok_or(EngineError::StaleGateRequest)?;
    let outcomes: Vec<_> = config
        .options
        .iter()
        .map(|option| option.outcome.clone())
        .collect();
    validate_answer(record, &response, &outcomes, config.allow_freetext)?;
    let identity = gate_identity(node, request, *stage_entry_seq, *requested_seq)?;
    store.validate_claim(&claim).map_err(storage_error)?;
    validate_current_control(&store, run, &control)?;
    let writer = storage.open_run_writer(run).await.map_err(storage_error)?;
    let result = writer
        .commit_gate_answer(EventSeq(history.event_count), identity, response)
        .await
        .map_err(storage_error);
    let closed = writer.close().await.map_err(storage_error);
    result?;
    closed?;
    drop(claim);
    Ok(())
}

fn validate_answer(
    record: &surge_core::run_state::RecoveredGateDecision,
    response: &serde_json::Value,
    outcomes: &[surge_core::OutcomeKey],
    allow_freetext: bool,
) -> Result<(), EngineError> {
    surge_core::human_gate_config::validate_gate_response(response, outcomes, allow_freetext)
        .map_err(|error| EngineError::Internal(error.to_string()))?;
    validate_deadline(record)
}

fn gate_identity(
    node: NodeKey,
    request: GateRequestId,
    stage_entry_seq: u64,
    requested_seq: u64,
) -> Result<surge_core::execution_recovery::gate_commit::GateCommitRequest, EngineError> {
    surge_core::execution_recovery::gate_commit::GateCommitRequest::new(
        node,
        request,
        stage_entry_seq,
        requested_seq,
    )
    .map_err(|error| EngineError::Internal(error.to_string()))
}

fn validate_current_control(
    store: &surge_persistence::work_items::WorkItemStore,
    run: RunId,
    expected: &surge_core::execution_recovery::WorkItemExecutionControl,
) -> Result<(), EngineError> {
    let latest = store
        .execution_control(run)
        .map_err(storage_error)?
        .ok_or(EngineError::StaleGateRequest)?;
    let same_authority = latest.generation == expected.generation
        && latest.operation == expected.operation
        && latest.item == expected.item
        && latest.attempt_generation == expected.attempt_generation
        && latest.fence == expected.fence;
    let answerable = matches!(
        latest.state,
        ExecutionControlState::Suspended | ExecutionControlState::ContinueReserved
    );
    if !same_authority || !answerable {
        return Err(EngineError::StaleGateRequest);
    }
    Ok(())
}

fn validate_deadline(
    record: &surge_core::run_state::RecoveredGateDecision,
) -> Result<(), EngineError> {
    let deadline = record
        .schema
        .as_ref()
        .and_then(|schema| schema.get("x-surge-timeout-ms"))
        .ok_or(EngineError::StaleGateRequest)?;
    if deadline.is_null() {
        return Ok(());
    }
    let millis = deadline.as_u64().ok_or(EngineError::StaleGateRequest)?;
    let elapsed = (chrono::Utc::now() - record.requested_at)
        .to_std()
        .unwrap_or_default();
    if elapsed >= std::time::Duration::from_millis(millis) {
        return Err(EngineError::StaleGateRequest);
    }
    Ok(())
}
fn storage_error(error: impl std::fmt::Display) -> EngineError {
    EngineError::Storage(error.to_string())
}
