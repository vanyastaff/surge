//! CLI ownership of foreground run tasks and their exit status.

use std::future::Future;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use surge_core::id::RunId;
use surge_orchestrator::engine::{EngineError, EngineRunEvent, RunEventTap, RunHandle, RunOutcome};
use tokio::sync::broadcast::error::RecvError;
use tokio::task::JoinHandle;

/// A terminal notification is not a join: local cleanup still runs afterward.
pub(super) async fn drive_run<F, E, S, C>(
    handle: RunHandle,
    mut local_events: Option<tokio::sync::broadcast::Receiver<RunEventTap>>,
    fail_on_lag: bool,
    mut on_event: F,
    stop: S,
) -> Result<RunOutcome>
where
    F: FnMut(EngineRunEvent) -> E,
    E: Future<Output = Result<()>>,
    S: FnOnce(String) -> C,
    C: Future<Output = Result<(), EngineError>>,
{
    let RunHandle {
        run_id,
        mut events,
        mut completion,
    } = handle;
    let mut receive_events = true;
    let error = loop {
        tokio::select! {
            result = &mut completion => return result.context("run task join failed"),
            event = next_event(&mut events, &mut local_events, run_id), if receive_events => {
                match event {
                    Ok(event) => {
                        let terminal = matches!(event, EngineRunEvent::Terminal { .. });
                        if let Err(error) = on_event(event).await {
                            break error;
                        }
                        if terminal {
                            receive_events = false;
                        }
                    },
                    Err(RecvError::Closed) => receive_events = false,
                    Err(RecvError::Lagged(count)) if fail_on_lag => {
                        break anyhow!("run {run_id}: lost {count} events; a local approval may have been missed. Use --daemon with an approval client, or the bootstrap console for bootstrap flows");
                    },
                    Err(RecvError::Lagged(count)) => {
                        eprintln!("note: dropped {count} events while watching run {run_id}; continuing");
                    },
                }
            }
        }
    };
    let outcome = stop_and_join(&mut completion, stop(error.to_string()))
        .await
        .with_context(|| error.to_string())?;
    if let RunOutcome::Failed { error: failure } = outcome {
        return Err(anyhow!("run {run_id} failed while cancelling: {failure}")).context(error);
    }
    Err(error)
}

// Local handles currently carry terminal notifications only. The existing
// persisted tap carries stage and approval events; subscribe before starting.
async fn next_event(
    events: &mut tokio::sync::broadcast::Receiver<EngineRunEvent>,
    local_events: &mut Option<tokio::sync::broadcast::Receiver<RunEventTap>>,
    run_id: RunId,
) -> Result<EngineRunEvent, RecvError> {
    if let Some(tap) = local_events {
        loop {
            let event = tap.recv().await?;
            if event.run_id == run_id {
                return Ok(EngineRunEvent::Persisted {
                    seq: event.event.seq.as_u64(),
                    payload: Box::new(event.event.payload.payload),
                });
            }
        }
    }
    events.recv().await
}

/// Settle the owned task even when cancellation itself fails. A timeout is an
/// explicit interruption, never an implicit detached run or successful exit.
pub(super) async fn stop_and_join<T>(
    completion: &mut JoinHandle<T>,
    stop: impl Future<Output = Result<(), EngineError>>,
) -> Result<T> {
    let stop_result = stop.await;
    let joined = tokio::time::timeout(Duration::from_secs(10), &mut *completion).await;
    if joined.is_err() {
        completion.abort();
        let _ = completion.await;
        return Err(anyhow!(
            "run did not settle after cancellation; task interrupted, durable cancellation is unconfirmed (stop result: {stop_result:?})"
        ));
    }
    // RunNotFound is a possible completion race, but must not erase the
    // original foreground error. Other cancellation errors remain visible.
    match stop_result {
        Ok(()) | Err(EngineError::RunNotFound(_)) => {},
        Err(error) => return Err(error).context("could not cancel run; task has settled"),
    }
    joined
        .context("cancellation wait elapsed")?
        .context("run task join failed during cancellation")
}

pub(super) fn require_completed(run_id: RunId, outcome: RunOutcome) -> Result<()> {
    match outcome {
        RunOutcome::Completed { .. } => Ok(()),
        RunOutcome::Failed { error } => Err(anyhow!("run {run_id} failed: {error}")),
        RunOutcome::Aborted { reason } => Err(anyhow!("run {run_id} aborted: {reason}")),
        RunOutcome::Parked { wake_at } => Err(anyhow!(
            "run {run_id} is not completed: parked until {wake_at}; the worktree and event log are intact. Resume through the daemon after that time with `surge engine resume {run_id} --daemon`"
        )),
        _ => Err(anyhow!(
            "run {run_id} has an unknown outcome; completion is unconfirmed"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::{broadcast, oneshot};

    fn success() -> RunOutcome {
        RunOutcome::Completed {
            terminal: "end".try_into().unwrap(),
        }
    }

    async fn bounded<T>(task: impl Future<Output = T>) -> T {
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .expect("waiter hung")
    }

    #[tokio::test]
    async fn terminal_event_does_not_skip_cleanup() {
        let (events, receiver) = broadcast::channel(2);
        let (finish, finished) = oneshot::channel();
        let (observed, observation) = oneshot::channel();
        let mut observed = Some(observed);
        let handle = RunHandle {
            run_id: RunId::new(),
            events: receiver,
            completion: tokio::spawn(async {
                finished.await.unwrap();
                success()
            }),
        };
        events
            .send(EngineRunEvent::Terminal { outcome: success() })
            .unwrap();
        let waiter = tokio::spawn(drive_run(
            handle,
            None,
            true,
            move |_| {
                observed.take().unwrap().send(()).unwrap();
                std::future::ready(Ok(()))
            },
            |_| async { panic!("must not cancel a successful run") },
        ));
        bounded(observation).await.unwrap();
        assert!(!waiter.is_finished());
        finish.send(()).unwrap();
        assert_eq!(bounded(waiter).await.unwrap().unwrap(), success());
    }

    #[tokio::test]
    async fn closed_events_still_join_completion() {
        let (events, receiver) = broadcast::channel(1);
        drop(events);
        let (finish, finished) = oneshot::channel();
        let handle = RunHandle {
            run_id: RunId::new(),
            events: receiver,
            completion: tokio::spawn(async {
                finished.await.unwrap();
                success()
            }),
        };
        let waiter = tokio::spawn(drive_run(
            handle,
            None,
            true,
            |_| async { Ok(()) },
            |_| async { panic!("closed events must not cancel") },
        ));
        tokio::task::yield_now().await;
        assert!(!waiter.is_finished());
        finish.send(()).unwrap();
        assert_eq!(bounded(waiter).await.unwrap().unwrap(), success());
    }

    #[tokio::test]
    async fn local_lag_cancels_and_joins() {
        let (events, receiver) = broadcast::channel(1);
        for _ in 0..3 {
            events
                .send(EngineRunEvent::Terminal { outcome: success() })
                .unwrap();
        }
        let (cancel, cancelled) = oneshot::channel();
        let handle = RunHandle {
            run_id: RunId::new(),
            events: receiver,
            completion: tokio::spawn(async {
                cancelled.await.unwrap();
                RunOutcome::Aborted {
                    reason: "cancelled".into(),
                }
            }),
        };
        let error = bounded(drive_run(
            handle,
            None,
            true,
            |_| async { Ok(()) },
            |reason| async move {
                assert!(reason.contains("lost"));
                cancel.send(()).unwrap();
                Ok(())
            },
        ))
        .await
        .unwrap_err();
        assert!(error.to_string().contains("lost"));
    }

    #[tokio::test]
    async fn completion_panic_is_not_hidden_by_open_events() {
        let (_events, receiver) = broadcast::channel(1);
        let handle = RunHandle {
            run_id: RunId::new(),
            events: receiver,
            completion: tokio::spawn(async { panic!("task failed") }),
        };
        let error = bounded(drive_run(
            handle,
            None,
            true,
            |_| async { Ok(()) },
            |_| async { panic!("already finished") },
        ))
        .await
        .unwrap_err();
        assert!(error.to_string().contains("join failed"));
    }

    #[tokio::test]
    async fn daemon_lag_keeps_waiting_for_completion() {
        let (events, receiver) = broadcast::channel(1);
        for _ in 0..3 {
            events
                .send(EngineRunEvent::Terminal { outcome: success() })
                .unwrap();
        }
        let (finish, finished) = oneshot::channel();
        let mut finish = Some(finish);
        let handle = RunHandle {
            run_id: RunId::new(),
            events: receiver,
            completion: tokio::spawn(async {
                finished.await.unwrap();
                success()
            }),
        };
        let outcome = bounded(drive_run(
            handle,
            None,
            false,
            move |_| {
                finish.take().unwrap().send(()).unwrap();
                std::future::ready(Ok(()))
            },
            |_| async { panic!("daemon lag must not cancel") },
        ))
        .await
        .unwrap();
        assert_eq!(outcome, success());
    }

    #[tokio::test]
    async fn cancellation_error_still_settles_task_and_remains_visible() {
        let (finish, finished) = oneshot::channel();
        let mut task = tokio::spawn(async { finished.await.unwrap() });
        let error = bounded(stop_and_join(&mut task, async {
            finish.send(()).unwrap();
            Err(EngineError::Internal("stop failed".into()))
        }))
        .await
        .unwrap_err();
        assert!(task.is_finished());
        assert!(format!("{error:#}").contains("stop failed"));
    }

    #[tokio::test]
    async fn cancellation_persistence_failure_is_not_discarded() {
        let (events, receiver) = broadcast::channel(1);
        let (cancel, cancelled) = oneshot::channel();
        let handle = RunHandle {
            run_id: RunId::new(),
            events: receiver,
            completion: tokio::spawn(async {
                cancelled.await.unwrap();
                RunOutcome::Failed {
                    error: "persist RunAborted: disk failure".into(),
                }
            }),
        };
        events
            .send(EngineRunEvent::Terminal { outcome: success() })
            .unwrap();
        let error = bounded(drive_run(
            handle,
            None,
            true,
            |_| async { Err(anyhow!("approval input closed")) },
            |_| async move {
                cancel.send(()).unwrap();
                Ok(())
            },
        ))
        .await
        .unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("approval input closed"), "{message}");
        assert!(
            message.contains("persist RunAborted: disk failure"),
            "{message}"
        );
    }

    #[tokio::test]
    async fn local_tap_routes_only_the_owned_run() {
        use surge_core::run_event::{EventPayload, VersionedEventPayload};
        use surge_persistence::runs::{EventSeq, ReadEvent};
        let id = RunId::new();
        let (tap, receiver) = broadcast::channel(4);
        let (_events, mut events) = broadcast::channel(1);
        for run_id in [RunId::new(), id] {
            tap.send(RunEventTap {
                run_id,
                event: ReadEvent {
                    seq: EventSeq(7),
                    timestamp_ms: 0,
                    kind: "test".into(),
                    payload: VersionedEventPayload::new(EventPayload::HumanInputRequested {
                        node: "gate".try_into().unwrap(),
                        session: None,
                        call_id: None,
                        prompt: run_id.to_string(),
                        schema: None,
                    }),
                },
            })
            .unwrap();
        }
        let event = bounded(next_event(&mut events, &mut Some(receiver), id))
            .await
            .unwrap();
        match event {
            EngineRunEvent::Persisted { seq, payload } => {
                assert_eq!(seq, 7);
                assert!(
                    matches!(*payload, EventPayload::HumanInputRequested { prompt, .. } if prompt == id.to_string())
                );
            },
            _ => panic!("expected persisted approval"),
        }
    }

    #[test]
    fn only_completed_outcome_is_success() {
        let id = RunId::new();
        require_completed(id, success()).unwrap();
        for (outcome, expected) in [
            (
                RunOutcome::Failed {
                    error: "broken".into(),
                },
                "failed: broken",
            ),
            (
                RunOutcome::Aborted {
                    reason: "connection lost".into(),
                },
                "aborted: connection lost",
            ),
            (
                RunOutcome::Parked {
                    wake_at: chrono::Utc::now(),
                },
                "not completed: parked",
            ),
        ] {
            let message = require_completed(id, outcome).unwrap_err().to_string();
            assert!(message.contains(expected), "{message}");
            assert!(message.contains(&id.to_string()));
        }
    }
}
