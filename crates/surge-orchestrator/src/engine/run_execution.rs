//! Joint owner of execution, the serialized writer seal, and the internal event forwarder.

use surge_persistence::runs::RunWriter;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;

use super::event_tap::RunEventTap;
use super::handle::{EngineRunEvent, RunOutcome, RunSettlementFailure};
use super::run_forwarder::{self, ForwarderResult};
use super::run_task::{self, ExecutionFinished, RunTaskParams};

/// Constructed before spawning public completion: even an unpolled future owns its writer.
pub(crate) struct RunExecution {
    // Field order ensures writer Drop signals priority stop before the join handle drops.
    params: RunTaskParams,
    forwarder: JoinHandle<ForwarderResult>,
}

impl RunExecution {
    pub fn new(
        params: RunTaskParams,
        tap: broadcast::Sender<RunEventTap>,
        feed: surge_persistence::runs::RunEventFeed,
    ) -> Self {
        let forwarder = run_forwarder::spawn(feed, tap);
        Self { params, forwarder }
    }

    pub async fn execute(self) -> RunOutcome {
        let Self {
            params,
            mut forwarder,
        } = self;
        let cancel = params.cancel.clone();
        let events = params.event_tx.clone();
        let storage = params.storage.clone();
        let run_id = params.run_id;
        let (finished, forwarded) = {
            let execution = run_task::execute(params);
            tokio::pin!(execution);
            tokio::select! {
                finished = &mut execution => (finished, join_result(forwarder.await)),
                joined = &mut forwarder => {
                    let forwarded = join_result(joined);
                    if forwarded.is_err() {
                        cancel.cancel();
                    }
                    (execution.await, forwarded)
                },
            }
        };
        let outcome = combine(finished, forwarded);
        // The registry reflects the sealed log before anyone hears `Terminal`.
        project_registry(&storage, run_id, &outcome).await;
        publish(&events, &outcome);
        outcome
    }
}

/// The registry status a settled run projects to (#95), from the sealed
/// log's verdict — never from the in-memory outcome alone. `verdict` is
/// `None` when the sealed log could not be read. `None` means write nothing:
/// `Parked`, `Suspended` and `RecoveryRequired` have their own writers.
fn registry_projection(
    outcome: &RunOutcome,
    verdict: Option<&surge_core::run_verdict::TerminalVerdict>,
) -> Option<surge_core::RunStatus> {
    use surge_core::RunStatus;
    use surge_core::run_verdict::TerminalVerdict;
    let claims_terminal = matches!(
        outcome,
        RunOutcome::Completed { .. } | RunOutcome::Failed { .. } | RunOutcome::Aborted { .. }
    );
    match (outcome, verdict) {
        (RunOutcome::SettlementFailed { .. }, _) | (_, Some(TerminalVerdict::Conflict { .. })) => {
            Some(RunStatus::Crashed)
        },
        (_, Some(TerminalVerdict::Terminal(claim))) => Some(
            crate::engine::run_execution::status_of(claim.outcome.kind()),
        ),
        // The engine claims an end the log does not contain (a lost append),
        // or the sealed log could not be read: unconfirmed. Recovery repairs
        // a Crashed row from the log.
        (_, Some(TerminalVerdict::Open) | None) if claims_terminal => Some(RunStatus::Crashed),
        (_, Some(TerminalVerdict::Open) | None) => None,
    }
}

fn status_of(kind: surge_core::run_verdict::TerminalKind) -> surge_core::RunStatus {
    use surge_core::run_verdict::TerminalKind;
    match kind {
        TerminalKind::Completed => surge_core::RunStatus::Completed,
        TerminalKind::Failed => surge_core::RunStatus::Failed,
        TerminalKind::Aborted => surge_core::RunStatus::Aborted,
    }
}

/// Read the sealed log's verdict and write the projected registry status.
/// Best-effort: a failed read or write is logged and never changes the
/// outcome.
async fn project_registry(
    storage: &std::sync::Arc<surge_persistence::runs::Storage>,
    run_id: surge_core::RunId,
    outcome: &RunOutcome,
) {
    let verdict = match read_verdict(storage, run_id).await {
        Ok(history) => {
            if !history.post_terminal_activity().is_empty() {
                tracing::warn!(%run_id, seqs = ?history.post_terminal_activity(),
                    "execution activity after the terminal event");
            }
            Some(history.into_verdict())
        },
        Err(error) => {
            tracing::warn!(%run_id, %error, "sealed log unreadable for the registry projection");
            None
        },
    };
    if let Some(surge_core::run_verdict::TerminalVerdict::Conflict { claims }) = &verdict {
        let seqs: Vec<u64> = claims.iter().map(|claim| claim.seq).collect();
        tracing::warn!(%run_id, ?seqs, "conflicting terminal history; registry marked unconfirmed");
    }
    let Some(status) = registry_projection(outcome, verdict.as_ref()) else {
        return;
    };
    if matches!(
        (outcome, &verdict),
        (
            RunOutcome::Completed { .. } | RunOutcome::Failed { .. } | RunOutcome::Aborted { .. },
            Some(surge_core::run_verdict::TerminalVerdict::Open)
        )
    ) {
        tracing::warn!(%run_id, ?outcome, "engine outcome has no durable terminal event");
    }
    let ended_at_ms = chrono::Utc::now().timestamp_millis();
    if let Err(error) = storage
        .set_run_status(&run_id, status, Some(ended_at_ms))
        .await
    {
        tracing::warn!(%run_id, ?status, %error, "could not record terminal run status in registry");
    }
}

async fn read_verdict(
    storage: &std::sync::Arc<surge_persistence::runs::Storage>,
    run_id: surge_core::RunId,
) -> Result<surge_core::run_verdict::TerminalHistory, String> {
    let reader = storage
        .open_run_reader(run_id)
        .await
        .map_err(|error| error.to_string())?;
    let last = reader
        .current_seq()
        .await
        .map_err(|error| error.to_string())?;
    let events = reader
        .read_events(surge_persistence::runs::EventSeq(0)..last.next())
        .await
        .map_err(|error| error.to_string())?;
    Ok(surge_persistence::runs::ReadEvent::terminal_history(
        &events,
    ))
}

fn join_result(result: Result<ForwarderResult, tokio::task::JoinError>) -> ForwarderResult {
    result.unwrap_or(Err(RunSettlementFailure::ForwarderJoin))
}

fn combine(finished: ExecutionFinished, forwarded: ForwarderResult) -> RunOutcome {
    let mut failures = Vec::new();
    let sealed = match finished.seal {
        Ok(prefix) => Some(prefix),
        Err(error) => {
            tracing::error!(%error, "run writer seal failed after joining its actor");
            failures.push(RunSettlementFailure::WriterSeal);
            None
        },
    };
    match forwarded {
        Ok(prefix) if sealed.is_some_and(|sealed| sealed != prefix) => {
            failures.push(RunSettlementFailure::PrefixMismatch);
        },
        Err(error) => failures.push(error),
        Ok(_) => {},
    }
    if failures.is_empty() {
        finished.outcome
    } else {
        RunOutcome::SettlementFailed {
            original: Box::new(finished.outcome),
            failures,
        }
    }
}

fn publish(events: &broadcast::Sender<EngineRunEvent>, outcome: &RunOutcome) {
    let event = if let RunOutcome::SettlementFailed { .. } = outcome {
        EngineRunEvent::StreamError {
            message: "run writer or event forwarding settlement could not be confirmed".into(),
        }
    } else {
        EngineRunEvent::Terminal {
            outcome: outcome.clone(),
        }
    };
    let _ = events.send(event);
}

/// Replayed terminal history still requires the actual writer and reader owners to settle.
///
/// The registry row is projected from the sealed log here too, so a stale
/// row is repaired by a replayed-terminal resume (#95).
pub(crate) async fn settle_terminal(
    writer: RunWriter,
    storage: &std::sync::Arc<surge_persistence::runs::Storage>,
    tap: broadcast::Sender<RunEventTap>,
    outcome: RunOutcome,
    feed: surge_persistence::runs::RunEventFeed,
) -> RunOutcome {
    let run_id = *writer.run_id();
    let task = run_forwarder::spawn(feed, tap);
    let seal = writer.seal_for_read_drain().await;
    let forwarded = join_result(task.await);
    let outcome = combine(ExecutionFinished { outcome, seal }, forwarded);
    project_registry(storage, run_id, &outcome).await;
    outcome
}

/// Preserve the original error while awaiting every ordinary preparation failure's writer exit.
pub(crate) async fn close_after_error(
    writer: RunWriter,
    operation: super::error::EngineError,
) -> super::error::EngineError {
    match writer.close().await {
        Ok(()) => operation,
        Err(close) => super::error::EngineError::OperationAndWriterClose {
            operation: Box::new(operation),
            close: Box::new(close),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use surge_core::NodeKey;
    use surge_persistence::runs::{CloseError, EventSeq};

    fn completed() -> RunOutcome {
        RunOutcome::Completed {
            terminal: NodeKey::try_new("end").unwrap(),
        }
    }

    #[test]
    fn independent_writer_and_forwarder_failures_preserve_original() {
        let original = completed();
        let result = combine(
            ExecutionFinished {
                outcome: original.clone(),
                seal: Err(CloseError::JoinFailed("test join failure".into())),
            },
            Err(RunSettlementFailure::ForwarderRead),
        );
        assert_eq!(
            result,
            RunOutcome::SettlementFailed {
                original: Box::new(original),
                failures: vec![
                    RunSettlementFailure::WriterSeal,
                    RunSettlementFailure::ForwarderRead
                ],
            }
        );
    }

    #[test]
    fn registry_projection_reads_the_verdict_not_the_claim() {
        use surge_core::RunStatus;
        use surge_core::run_verdict::{TerminalClaim, TerminalOutcome, TerminalVerdict};
        let claim = |outcome| TerminalClaim { seq: 3, outcome };
        let completed_claim = claim(TerminalOutcome::Completed {
            terminal_node: NodeKey::try_new("end").unwrap(),
        });
        let aborted_claim = claim(TerminalOutcome::Aborted {
            reason: "stop".into(),
        });
        let terminal = TerminalVerdict::Terminal(completed_claim.clone());
        let aborted = TerminalVerdict::Terminal(aborted_claim.clone());
        let conflict = TerminalVerdict::Conflict {
            claims: vec![completed_claim, aborted_claim],
        };
        let failed = RunOutcome::Failed { error: "x".into() };
        let parked = RunOutcome::Parked {
            wake_at: chrono::Utc::now(),
        };
        let settlement = RunOutcome::SettlementFailed {
            original: Box::new(completed()),
            failures: vec![RunSettlementFailure::WriterSeal],
        };
        let cases = [
            (&completed(), Some(&terminal), Some(RunStatus::Completed)),
            (&completed(), Some(&aborted), Some(RunStatus::Aborted)),
            (&completed(), Some(&conflict), Some(RunStatus::Crashed)),
            (
                &failed,
                Some(&TerminalVerdict::Open),
                Some(RunStatus::Crashed),
            ),
            (&completed(), None, Some(RunStatus::Crashed)),
            (&parked, Some(&TerminalVerdict::Open), None),
            (&parked, None, None),
            (&settlement, Some(&terminal), Some(RunStatus::Crashed)),
        ];
        for (outcome, verdict, expected) in cases {
            assert_eq!(
                registry_projection(outcome, verdict),
                expected,
                "{outcome:?} / {verdict:?}"
            );
        }
    }

    #[test]
    fn prefix_mismatch_never_publishes_terminal() {
        let outcome = combine(
            ExecutionFinished {
                outcome: completed(),
                seal: Ok(EventSeq(12)),
            },
            Ok(EventSeq(11)),
        );
        let (events, mut receiver) = broadcast::channel(2);
        publish(&events, &outcome);
        assert!(matches!(
            receiver.try_recv().unwrap(),
            EngineRunEvent::StreamError { .. }
        ));
        assert!(receiver.try_recv().is_err());
        assert!(
            matches!(outcome, RunOutcome::SettlementFailed { failures, .. }
            if failures == vec![RunSettlementFailure::PrefixMismatch])
        );
    }
}
