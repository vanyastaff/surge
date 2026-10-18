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
    if matches!(
        report.semantic_history,
        surge_core::run_report::SemanticHistory::Unavailable { .. }
    ) {
        return Ok(report);
    }
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

#[cfg(test)]
mod continuation_tests {
    use super::*;
    mod fixture {
        use surge_core as domain;
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../scripts/test-support/continuation_seed.rs"
        ));
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn actual_operator_report_keeps_unavailable_semantics_and_terminal_fact() {
        let home = crate::runtime_home_fixture::FixtureHome::new().unwrap();
        let storage = Storage::open(home.path()).await.unwrap();
        let run = surge_core::RunId::new();
        let writer = storage.create_run(run, home.path(), None).await.unwrap();
        let (mut events, manifest) = fixture::startup(run);
        events.push(fixture::event(
            run,
            4,
            surge_core::EventPayload::ContinuationSeedBound {
                manifest: Box::new(manifest),
            },
        ));
        events.extend(fixture::positive_tail(run, 4));
        writer
            .append_events(
                events
                    .into_iter()
                    .map(|event| surge_core::VersionedEventPayload::new(event.payload))
                    .collect(),
            )
            .await
            .unwrap();
        writer.close().await.unwrap();
        let report = compile_report(&storage, &run.to_string()).await.unwrap();
        assert!(matches!(
            report.completion,
            surge_core::run_report::RunCompletion::Completed { .. }
        ));
        assert!(matches!(
            report.semantic_history,
            surge_core::run_report::SemanticHistory::Unavailable { .. }
        ));
        assert_eq!(report.evidence_backed, None);
        assert_eq!(
            report.freshness,
            surge_core::verification_evidence::ProofFreshness::Unknown
        );
        assert!(report.verdicts.is_empty());
        assert!(report.task_execution.is_none());
        let reader = storage.open_run_reader(run).await.unwrap();
        assert_eq!(
            reader
                .read_events(
                    surge_persistence::runs::EventSeq(1)..surge_persistence::runs::EventSeq(11)
                )
                .await
                .unwrap()
                .len(),
            10
        );
        drop(reader);
        drop(storage);
        home.close().unwrap();
    }
}
