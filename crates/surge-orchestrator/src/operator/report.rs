//! Run report and OTLP trace compilation, derived from the event log alone.

use std::sync::Arc;

use surge_core::run_report::RunReport;
use surge_persistence::runs::Storage;

use crate::operator::error::OperatorError;
use crate::operator::journal::read_run_events;
use crate::operator::run_id::resolve_run_id;

/// The OTLP/JSON trace for `run` (full ULID or unique suffix), as a JSON
/// value; adapters choose how to render it.
///
/// # Errors
/// Returns [`OperatorError`] if the run cannot be resolved or its event log
/// read.
pub async fn compile_trace(
    storage: &Arc<Storage>,
    run: &str,
) -> Result<serde_json::Value, OperatorError> {
    let (run_id, events) = load_events(storage, run).await?;
    Ok(surge_core::run_trace::to_otlp_json(run_id, &events))
}

/// Compile the Run Report for `run` (full ULID or unique short suffix) from
/// its event log.
///
/// # Errors
/// Returns [`OperatorError`] if the run cannot be resolved or its event log
/// read.
pub async fn compile_report(storage: &Arc<Storage>, run: &str) -> Result<RunReport, OperatorError> {
    let (run_id, events) = load_events(storage, run).await?;
    let mut report = RunReport::compile(run_id, &events);
    use surge_core::verification_evidence::ProofFreshness;
    let current = super::verification::current_proofs(storage, run_id);
    report.freshness = match current {
        Some(states)
            if !states.is_empty()
                && states
                    .values()
                    .all(|state| *state == ProofFreshness::Current)
                && report.evidence_backed == Some(true) =>
        {
            ProofFreshness::Current
        },
        Some(states)
            if states
                .values()
                .any(|state| *state == ProofFreshness::Unknown) =>
        {
            ProofFreshness::Unknown
        },
        Some(states) if !states.is_empty() => ProofFreshness::Stale,
        Some(_) => ProofFreshness::Unbound,
        None => ProofFreshness::Unknown,
    };
    if report.evidence_backed.is_some() && report.freshness != ProofFreshness::Current {
        report.evidence_backed = Some(false);
    }
    Ok(report)
}

async fn load_events(
    storage: &Arc<Storage>,
    run: &str,
) -> Result<(surge_core::RunId, Vec<surge_core::run_event::RunEvent>), OperatorError> {
    let run_id = resolve_run_id(storage, run).await?;
    let reader = storage
        .open_run_reader(run_id)
        .await
        .map_err(|source| OperatorError::OpenRun { run_id, source })?;
    let events = read_run_events(&reader).await?;
    Ok((run_id, events))
}
