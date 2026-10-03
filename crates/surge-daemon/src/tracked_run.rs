//! Shared daemon tracking: persisted events precede authoritative completion.
use crate::{admission::AdmissionController, broadcast::BroadcastRegistry};
use std::{path::PathBuf, sync::Arc};
use surge_core::{RunId, graph::Graph, run_event::EventPayload};
use surge_orchestrator::engine::ipc::GlobalDaemonEvent;
use surge_orchestrator::engine::{
    Engine, EngineError, EngineRunConfig, RunEventTap, RunOutcome,
    facade::EngineFacade,
    handle::{EngineRunEvent, RunHandle},
};
use surge_persistence::runs::{ReadEvent, Storage, inspection::RunDatabaseInspection};
use tokio::{sync::broadcast, task::JoinHandle};

/// Required production event source; synthetic facades opt into an explicit adapter.
#[derive(Clone)]
pub struct TrackingContext(Source);
#[derive(Clone)]
enum Source {
    Durable {
        engine: Arc<Engine>,
        storage: Arc<Storage>,
    },
    Synthetic,
}

impl TrackingContext {
    /// Use the actual engine and its storage, never a reconstructed runtime.
    #[must_use]
    pub fn new(engine: Arc<Engine>, storage: Arc<Storage>) -> Self {
        Self(Source::Durable { engine, storage })
    }
    /// Explicit adapter for synthetic facades without a persisted event log.
    #[must_use]
    pub fn synthetic() -> Self {
        Self(Source::Synthetic)
    }

    /// Durable host capabilities; synthetic adapters cannot mutate tasks.
    pub(crate) fn task_sources(&self) -> Option<(Arc<Engine>, Arc<Storage>)> {
        match &self.0 {
            Source::Durable { engine, storage } => Some((engine.clone(), storage.clone())),
            Source::Synthetic => None,
        }
    }

    /// Subscribe before the owned task startup transaction.
    pub(crate) async fn task_run(
        &self,
        claim: &surge_persistence::work_items::WorkItemLaunchClaim,
        resume: bool,
    ) -> Result<TrackedRun, EngineError> {
        let prepared = self.prepare(claim.run(), resume).await?;
        let Some((engine, _)) = self.task_sources() else {
            return Err(EngineError::Storage("task host unavailable".into()));
        };
        let handle = if resume {
            engine.resume_work_item(claim).await?
        } else {
            engine.start_work_item(claim).await?
        };
        Ok(TrackedRun { handle, prepared })
    }

    async fn prepare(&self, id: RunId, resume: bool) -> Result<Prepared, EngineError> {
        match &self.0 {
            Source::Synthetic => Ok(Prepared::Synthetic),
            Source::Durable { engine, storage } => {
                // Subscribe before inspecting/resuming or starting: no start window is unobserved.
                let tap = engine.subscribe_tap();
                let last_seq = if resume {
                    let inspected = storage
                        .inspect_run(id)
                        .await
                        .map_err(|e| EngineError::Storage(e.to_string()))?;
                    match inspected.database {
                        RunDatabaseInspection::Present { events } => {
                            events.last().map_or(0, |event| event.seq.0)
                        },
                        RunDatabaseInspection::Absent => {
                            return Err(EngineError::Storage("resume event log is absent".into()));
                        },
                    }
                } else {
                    0
                };
                Ok(Prepared::Durable {
                    engine: engine.clone(),
                    storage: storage.clone(),
                    tap,
                    last_seq,
                    reconcile_period: std::time::Duration::from_secs(1),
                })
            },
        }
    }

    /// Subscribe to persisted events before delegating the start.
    pub async fn start(
        &self,
        facade: &dyn EngineFacade,
        id: RunId,
        graph: Graph,
        path: PathBuf,
        config: EngineRunConfig,
    ) -> Result<TrackedRun, EngineError> {
        let prepared = self.prepare(id, false).await?;
        let handle = facade.start_run(id, graph, path, config).await?;
        Ok(TrackedRun { handle, prepared })
    }
    /// Establish a historical sequence boundary before resuming.
    pub async fn resume(
        &self,
        facade: &dyn EngineFacade,
        id: RunId,
        path: PathBuf,
    ) -> Result<TrackedRun, EngineError> {
        let prepared = self.prepare(id, true).await?;
        let handle = facade.resume_run(id, path).await?;
        Ok(TrackedRun { handle, prepared })
    }
}

enum Prepared {
    Synthetic,
    Durable {
        engine: Arc<Engine>,
        storage: Arc<Storage>,
        tap: broadcast::Receiver<RunEventTap>,
        last_seq: u64,
        reconcile_period: std::time::Duration,
    },
}
/// Started handle together with the event subscription established before startup.
pub struct TrackedRun {
    handle: RunHandle,
    prepared: Prepared,
}

/// Unconfirmed observation, distinct from a durable run outcome.
#[derive(Debug, thiserror::Error)]
pub enum TrackingError {
    /// Run task could not produce an authoritative outcome.
    #[error("run completion could not be confirmed: {0}")]
    Join(String),
    /// Persisted events could not be read or had a sequence gap.
    #[error("persisted run events could not be confirmed: {0}")]
    Events(String),
    /// The durable terminal record did not match the joined outcome.
    #[error("run completion disagrees with the durable event log")]
    OutcomeMismatch,
}

/// Shared ownership of publication, completion and admission release.
/// The returned result is available to a durable-operation settlement observer.
pub fn spawn_tracked_run(
    id: RunId,
    run: TrackedRun,
    publisher: broadcast::Sender<EngineRunEvent>,
    admission: Arc<AdmissionController>,
    registry: Arc<BroadcastRegistry>,
) -> JoinHandle<Result<RunOutcome, TrackingError>> {
    tokio::spawn(async move {
        let result = drive(id, run, &publisher).await;
        match &result {
            Ok(outcome) => {
                let _ = publisher.send(EngineRunEvent::Terminal {
                    outcome: outcome.clone(),
                });
                registry.publish_global(GlobalDaemonEvent::RunFinished {
                    run_id: id,
                    outcome: outcome.clone(),
                });
            },
            Err(error) => {
                tracing::error!(run_id = %id, %error, "run tracking is unconfirmed");
                let _ = publisher.send(EngineRunEvent::StreamError {
                    message: error.to_string(),
                });
            },
        }
        registry.deregister(id).await;
        admission.notify_completed(id).await;
        result
    })
}

async fn drive(
    id: RunId,
    run: TrackedRun,
    publisher: &broadcast::Sender<EngineRunEvent>,
) -> Result<RunOutcome, TrackingError> {
    let TrackedRun {
        mut handle,
        prepared,
    } = run;
    match prepared {
        Prepared::Synthetic => drive_synthetic(handle, publisher).await,
        Prepared::Durable {
            engine,
            storage,
            mut tap,
            mut last_seq,
            reconcile_period,
        } => {
            // A low-frequency durable fallback also detects a failed Engine tap producer.
            let period = reconcile_period;
            let mut reconcile =
                tokio::time::interval_at(tokio::time::Instant::now() + period, period);
            loop {
                let observation = tokio::select! {
                    joined = &mut handle.completion => {
                        let outcome = joined.map_err(|e| TrackingError::Join(e.to_string()))?;
                        let final_events = final_flush(&storage, id, &mut last_seq, publisher).await?;
                        if !confirms(&final_events, &outcome) { return Err(TrackingError::OutcomeMismatch); }
                        return Ok(outcome);
                    },
                    tapped = tap.recv() => match tapped {
                        Ok(event) if event.run_id == id => {
                            if event.event.seq.0 > last_seq.saturating_add(1) {
                                catch_up(&storage, id, &mut last_seq, publisher).await
                            } else { publish(event.event, &mut last_seq, publisher) }
                        },
                        Ok(_) => Ok(()),
                        Err(broadcast::error::RecvError::Lagged(_)) => catch_up(&storage, id, &mut last_seq, publisher).await,
                        Err(broadcast::error::RecvError::Closed) => Err(TrackingError::Events("engine tap closed".into())),
                    },
                    _ = reconcile.tick() => catch_up(&storage, id, &mut last_seq, publisher).await,
                };
                if let Err(error) = observation {
                    // Stop and join before releasing admission. An error must not detach live work.
                    let _ = engine
                        .stop_run(id, "persisted event tracking failed".into())
                        .await;
                    let _ = handle.completion.await;
                    return Err(error);
                }
            }
        },
    }
}

async fn drive_synthetic(
    mut handle: RunHandle,
    publisher: &broadcast::Sender<EngineRunEvent>,
) -> Result<RunOutcome, TrackingError> {
    // Legacy fixtures model activity by holding their event sender open.
    // This adapter is explicit and never used by the production daemon.
    loop {
        match handle.events.recv().await {
            Ok(event @ EngineRunEvent::Persisted { .. }) => {
                let _ = publisher.send(event);
            },
            Ok(EngineRunEvent::StreamError { message }) => {
                handle.completion.abort();
                let _ = handle.completion.await;
                return Err(TrackingError::Events(message));
            },
            Ok(_) => {},
            Err(broadcast::error::RecvError::Lagged(_)) => {
                handle.completion.abort();
                let _ = handle.completion.await;
                return Err(TrackingError::Events(
                    "synthetic event stream lagged".into(),
                ));
            },
            Err(broadcast::error::RecvError::Closed) => {
                return handle
                    .completion
                    .await
                    .map_err(|e| TrackingError::Join(e.to_string()));
            },
        }
    }
}

fn publish(
    event: ReadEvent,
    last_seq: &mut u64,
    publisher: &broadcast::Sender<EngineRunEvent>,
) -> Result<(), TrackingError> {
    if event.seq.0 <= *last_seq {
        return Ok(());
    }
    if event.seq.0 != last_seq.saturating_add(1) {
        return Err(TrackingError::Events(
            "nonconsecutive event sequence".into(),
        ));
    }
    *last_seq = event.seq.0;
    let _ = publisher.send(EngineRunEvent::Persisted {
        seq: *last_seq,
        payload: Box::new(event.payload.payload),
    });
    Ok(())
}

async fn catch_up(
    storage: &Arc<Storage>,
    id: RunId,
    last_seq: &mut u64,
    publisher: &broadcast::Sender<EngineRunEvent>,
) -> Result<(), TrackingError> {
    const BATCH: std::num::NonZeroU32 = std::num::NonZeroU32::new(256).expect("positive batch");
    let events = storage
        .inspect_events_after(id, surge_persistence::runs::EventSeq(*last_seq), BATCH)
        .await
        .map_err(|error| TrackingError::Events(error.to_string()))?;
    for event in events {
        publish(event, last_seq, publisher)?;
    }
    Ok(())
}

async fn final_flush(
    storage: &Arc<Storage>,
    id: RunId,
    last_seq: &mut u64,
    publisher: &broadcast::Sender<EngineRunEvent>,
) -> Result<Vec<ReadEvent>, TrackingError> {
    let inspected = storage
        .inspect_run(id)
        .await
        .map_err(|e| TrackingError::Events(e.to_string()))?;
    let RunDatabaseInspection::Present { events } = inspected.database else {
        return Err(TrackingError::Events("event database disappeared".into()));
    };
    if events.last().map_or(0, |event| event.seq.0) < *last_seq {
        return Err(TrackingError::Events(
            "durable boundary moved behind delivered events".into(),
        ));
    }
    for event in &events {
        publish(event.clone(), last_seq, publisher)?;
    }
    Ok(events)
}

pub(crate) fn confirms(events: &[ReadEvent], outcome: &RunOutcome) -> bool {
    let mut definitive = None;
    let mut parked = None;
    let mut suspended = None;
    let mut recovery = None;
    for (index, event) in events.iter().enumerate() {
        if event.seq.0 != index as u64 + 1 {
            return false;
        }
        let terminal = match &event.payload.payload {
            EventPayload::RunCompleted { terminal_node } => Some(RunOutcome::Completed {
                terminal: terminal_node.clone(),
            }),
            EventPayload::RunFailed { error } => Some(RunOutcome::Failed {
                error: error.clone(),
            }),
            EventPayload::RunAborted { reason } => Some(RunOutcome::Aborted {
                reason: reason.clone(),
            }),
            EventPayload::RunRecoveryRequired {
                control_generation,
                diagnostic,
            } => {
                recovery = Some(RunOutcome::RecoveryRequired {
                    control_generation: *control_generation,
                    diagnostic: diagnostic.clone(),
                });
                None
            },
            EventPayload::RunSuspended { fence } if fence.cleanup_confirmed => {
                suspended = Some(fence.clone());
                None
            },
            EventPayload::RunContinued { .. } => {
                suspended = None;
                recovery = None;
                None
            },
            EventPayload::RunParked { wake_at, .. } => {
                parked = Some(*wake_at);
                None
            },
            EventPayload::RunWokeFromPark {} => {
                parked = None;
                None
            },
            _ => None,
        };
        if let Some(terminal) = terminal {
            if definitive.is_some() {
                return false;
            }
            definitive = Some(terminal);
        }
    }
    definitive
        .or(recovery)
        .or_else(|| {
            suspended.map(|fence| RunOutcome::Suspended {
                fence: Box::new(fence),
            })
        })
        .or_else(|| parked.map(|wake_at| RunOutcome::Parked { wake_at }))
        .as_ref()
        == Some(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use surge_core::{VersionedEventPayload, keys::NodeKey};
    use surge_persistence::runs::EventSeq;

    fn event(seq: u64, payload: EventPayload) -> ReadEvent {
        ReadEvent {
            seq: EventSeq(seq),
            timestamp_ms: 0,
            kind: String::new(),
            payload: VersionedEventPayload::new(payload),
        }
    }

    #[test]
    fn conflicting_definitive_outcomes_are_not_confirmed_by_last_record() {
        let events = [
            event(
                1,
                EventPayload::RunCompleted {
                    terminal_node: NodeKey::try_new("end").unwrap(),
                },
            ),
            event(
                2,
                EventPayload::RunFailed {
                    error: "conflict".into(),
                },
            ),
        ];
        assert!(!confirms(
            &events,
            &RunOutcome::Failed {
                error: "conflict".into()
            }
        ));
    }

    #[test]
    fn terminal_confirmation_rejects_a_gap_before_the_live_watermark() {
        let terminal = NodeKey::try_new("end").unwrap();
        let events = [event(
            2,
            EventPayload::RunCompleted {
                terminal_node: terminal.clone(),
            },
        )];
        assert!(!confirms(&events, &RunOutcome::Completed { terminal }));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn final_flush_delivers_persisted_rows_even_without_tap_delivery() {
        let root = tempfile::tempdir().unwrap();
        let storage = Storage::open(root.path()).await.unwrap();
        let id = RunId::new();
        let writer = storage.create_run(id, "/worktree", None).await.unwrap();
        for reason in ["first", "second"] {
            writer
                .append_event(VersionedEventPayload::new(EventPayload::RunAborted {
                    reason: reason.into(),
                }))
                .await
                .unwrap();
        }
        let (sender, mut receiver) = broadcast::channel(8);
        let mut seq = 0;
        final_flush(&storage, id, &mut seq, &sender).await.unwrap();
        assert_eq!(seq, 2);
        for expected in 1..=2 {
            assert!(
                matches!(receiver.recv().await.unwrap(), EngineRunEvent::Persisted { seq, .. } if seq == expected)
            );
        }
        final_flush(&storage, id, &mut seq, &sender).await.unwrap();
        assert!(matches!(
            receiver.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
    }

    #[tokio::test]
    async fn synthetic_stream_failure_joins_task_and_releases_slot_without_terminal() {
        let id = RunId::new();
        let admission = Arc::new(AdmissionController::new(1, 1));
        assert!(admission.try_admit_no_queue(id).await);
        let registry = Arc::new(BroadcastRegistry::new());
        let publisher = registry.register(id).await;
        let mut receiver = publisher.subscribe();
        let (events, events_rx) = broadcast::channel(4);
        let completion = tokio::spawn(std::future::pending());
        let completion_abort = completion.abort_handle();
        let run = TrackedRun {
            handle: RunHandle {
                run_id: id,
                events: events_rx,
                completion,
            },
            prepared: Prepared::Synthetic,
        };
        let tracked = spawn_tracked_run(id, run, publisher, admission.clone(), registry);
        events
            .send(EngineRunEvent::StreamError {
                message: "lost events".into(),
            })
            .unwrap();
        assert!(tracked.await.unwrap().is_err());
        assert!(completion_abort.is_finished());
        assert!(matches!(
            receiver.recv().await.unwrap(),
            EngineRunEvent::StreamError { .. }
        ));
        assert!(matches!(
            receiver.recv().await,
            Err(broadcast::error::RecvError::Closed)
        ));
        assert_eq!(admission.snapshot().await.active, 0);
    }
}

#[cfg(test)]
#[path = "tracked_run_delivery_tests.rs"]
mod delivery_tests;
