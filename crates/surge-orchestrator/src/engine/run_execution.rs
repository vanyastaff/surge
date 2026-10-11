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
        publish(&events, &outcome);
        outcome
    }
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
pub(crate) async fn settle_terminal(
    writer: RunWriter,
    tap: broadcast::Sender<RunEventTap>,
    outcome: RunOutcome,
    feed: surge_persistence::runs::RunEventFeed,
) -> RunOutcome {
    let task = run_forwarder::spawn(feed, tap);
    let seal = writer.seal_for_read_drain().await;
    let forwarded = join_result(task.await);
    combine(ExecutionFinished { outcome, seal }, forwarded)
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
