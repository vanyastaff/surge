//! Test-only sequencing of confirmed completion, actual owner release and diagnostics.
use std::{future::Future, time::Duration};
use surge_core::RunId;
use surge_orchestrator::engine::{handle::RunOutcome, ipc::GlobalDaemonEvent};
use tokio::sync::broadcast;

pub async fn inspect_after_settlement<T, SF, IF>(
    observation: impl Future<Output = Result<(), String>>,
    settle: impl FnOnce(bool) -> SF,
    inspect: impl FnOnce() -> IF,
) -> Result<(Result<(), String>, T), String>
where
    SF: Future<Output = Result<(), String>>,
    IF: Future<Output = T>,
{
    let observed = observation.await;
    settle(observed.is_err()).await?;
    let inspected = inspect().await;
    Ok((observed, inspected))
}

pub async fn confirmed_completion(
    receiver: &mut broadcast::Receiver<GlobalDaemonEvent>,
    run: RunId,
    budget_abort: bool,
    timeout: Duration,
) -> Result<(), String> {
    tokio::time::timeout(timeout, async {
        loop {
            match receiver.recv().await.map_err(|error| error.to_string())? {
                GlobalDaemonEvent::RunAccepted { run_id } if run_id == run => {},
                GlobalDaemonEvent::RunFinished { run_id, outcome } if run_id == run => {
                    return match (&outcome, budget_abort) {
                        (RunOutcome::Completed { .. }, false) => Ok(()),
                        (RunOutcome::Aborted { reason }, true)
                            if reason.starts_with("budget exceeded (Tokens): ")
                                && reason.ends_with("tokens=1300") =>
                        {
                            Ok(())
                        },
                        _ => Err(format!("unexpected confirmed wake outcome: {outcome:?}")),
                    };
                },
                event => return Err(format!("unexpected wake observation: {event:?}")),
            }
        }
    })
    .await
    .map_err(|_| "confirmed wake completion timed out".to_owned())?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };
    use tokio::sync::oneshot;

    #[tokio::test]
    async fn journal_waits_for_confirmed_completion_and_actual_owner_release() {
        let run = RunId::new();
        let (sender, mut receiver) = broadcast::channel(4);
        let (owner_release, owner_released) = oneshot::channel();
        let reads = Arc::new(AtomicUsize::new(0));
        let released = Arc::new(AtomicBool::new(false));
        let observed_reads = reads.clone();
        let observed_release = released.clone();
        let settlement_release = released.clone();
        let mut operation = Box::pin(inspect_after_settlement(
            confirmed_completion(&mut receiver, run, false, Duration::from_secs(2)),
            move |failed| async move {
                assert!(!failed);
                owner_released.await.unwrap();
                settlement_release.store(true, Ordering::SeqCst);
                Ok(())
            },
            move || async move {
                observed_reads.fetch_add(1, Ordering::SeqCst);
                assert!(
                    observed_release.load(Ordering::SeqCst),
                    "journal read before actual owner release"
                );
            },
        ));
        assert!(futures::poll!(&mut operation).is_pending());
        assert_eq!(reads.load(Ordering::SeqCst), 0);
        sender
            .send(GlobalDaemonEvent::RunFinished {
                run_id: run,
                outcome: RunOutcome::Completed {
                    terminal: "end".parse().unwrap(),
                },
            })
            .unwrap();
        assert!(futures::poll!(&mut operation).is_pending());
        assert_eq!(reads.load(Ordering::SeqCst), 0);
        owner_release.send(()).unwrap();
        assert!(operation.await.unwrap().0.is_ok());
        assert_eq!(reads.load(Ordering::SeqCst), 1);
    }

    fn finished(run: RunId, outcome: RunOutcome) -> GlobalDaemonEvent {
        GlobalDaemonEvent::RunFinished {
            run_id: run,
            outcome,
        }
    }

    #[tokio::test]
    async fn exact_run_and_expected_budget_outcome_are_required() {
        let run = RunId::new();
        let invalid = [
            finished(
                RunId::new(),
                RunOutcome::Completed {
                    terminal: "end".parse().unwrap(),
                },
            ),
            finished(
                run,
                RunOutcome::Failed {
                    error: "failed".into(),
                },
            ),
            finished(
                run,
                RunOutcome::Aborted {
                    reason: "operator stop".into(),
                },
            ),
            finished(
                run,
                RunOutcome::RecoveryRequired {
                    control_generation: 1,
                    diagnostic: "unknown".into(),
                },
            ),
            finished(
                run,
                RunOutcome::SettlementFailed {
                    original: Box::new(RunOutcome::Completed {
                        terminal: "end".parse().unwrap(),
                    }),
                    failures: Vec::new(),
                },
            ),
            GlobalDaemonEvent::DaemonShuttingDown,
            GlobalDaemonEvent::SubscriberLagged { dropped: 1 },
        ];
        for event in invalid {
            let (sender, mut receiver) = broadcast::channel(4);
            sender.send(event).unwrap();
            assert!(
                confirmed_completion(&mut receiver, run, false, Duration::from_secs(1))
                    .await
                    .is_err()
            );
        }
        for (outcome, expected) in [
            (
                RunOutcome::Completed {
                    terminal: "end".parse().unwrap(),
                },
                false,
            ),
            (
                RunOutcome::Aborted {
                    reason: "budget exceeded (Tokens): cost=$0.0000, tokens=1300".into(),
                },
                true,
            ),
            (
                RunOutcome::Aborted {
                    reason: "budget exceeded (Cost): cost=$0.0000, tokens=1300".into(),
                },
                false,
            ),
            (
                RunOutcome::Aborted {
                    reason: "budget exceeded (Tokens): cost=$0.0000, tokens=1200".into(),
                },
                false,
            ),
        ] {
            let (sender, mut receiver) = broadcast::channel(4);
            sender.send(finished(run, outcome)).unwrap();
            assert_eq!(
                confirmed_completion(&mut receiver, run, true, Duration::from_secs(1))
                    .await
                    .is_ok(),
                expected
            );
        }
    }

    #[tokio::test]
    async fn lost_closed_or_absent_completion_is_not_success() {
        let run = RunId::new();
        let (sender, mut receiver) = broadcast::channel(1);
        sender
            .send(GlobalDaemonEvent::RunAccepted { run_id: run })
            .unwrap();
        sender
            .send(finished(
                run,
                RunOutcome::Completed {
                    terminal: "end".parse().unwrap(),
                },
            ))
            .unwrap();
        assert!(
            confirmed_completion(&mut receiver, run, false, Duration::from_secs(1))
                .await
                .unwrap_err()
                .contains("lagged")
        );
        let (sender, mut receiver) = broadcast::channel(1);
        drop(sender);
        assert!(
            confirmed_completion(&mut receiver, run, false, Duration::from_secs(1))
                .await
                .unwrap_err()
                .contains("closed")
        );
        let (_sender, mut receiver) = broadcast::channel(1);
        assert!(
            confirmed_completion(&mut receiver, run, false, Duration::from_millis(10))
                .await
                .unwrap_err()
                .contains("timed out")
        );
    }

    #[tokio::test]
    async fn refused_completion_still_settles_before_failure_diagnostics() {
        let settled = &AtomicBool::new(false);
        let (observed, diagnostics) = inspect_after_settlement(
            async { Err("unconfirmed completion".to_owned()) },
            |failed| async move {
                assert!(failed, "failed observation must stop the exact resumed run");
                settled.store(true, Ordering::SeqCst);
                Ok(())
            },
            || async {
                assert!(
                    settled.load(Ordering::SeqCst),
                    "failure diagnostics preceded owner release"
                );
                "durable diagnostics"
            },
        )
        .await
        .unwrap();
        assert_eq!(observed, Err("unconfirmed completion".to_owned()));
        assert_eq!(diagnostics, "durable diagnostics");
    }

    #[tokio::test]
    async fn failed_owner_settlement_forbids_even_failure_diagnostics() {
        let reads = AtomicUsize::new(0);
        let result = inspect_after_settlement(
            async { Err("timed out".to_owned()) },
            |failed| async move {
                assert!(failed);
                Err("owner still retained".to_owned())
            },
            || async {
                reads.fetch_add(1, Ordering::SeqCst);
            },
        )
        .await;
        assert_eq!(result.unwrap_err(), "owner still retained");
        assert_eq!(reads.load(Ordering::SeqCst), 0);
    }
}
