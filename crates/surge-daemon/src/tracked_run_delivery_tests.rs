use super::*;
use surge_core::{VersionedEventPayload, keys::NodeKey};
use surge_orchestrator::engine::{EngineConfig, tools::worktree::WorktreeToolDispatcher};
use surge_persistence::runs::RunWriter;
use tokio::sync::oneshot;

fn engine(storage: &Arc<Storage>, path: &std::path::Path) -> Arc<Engine> {
    Arc::new(Engine::new(
        Arc::new(surge_acp::bridge::AcpBridge::with_defaults().unwrap()),
        storage.clone(),
        Arc::new(WorktreeToolDispatcher::new(path.to_path_buf())),
        EngineConfig::default(),
    ))
}

fn success() -> RunOutcome {
    RunOutcome::Completed {
        terminal: NodeKey::try_new("end").unwrap(),
    }
}

async fn append(writer: &RunWriter, payload: EventPayload) {
    writer
        .append_event(VersionedEventPayload::new(payload))
        .await
        .unwrap();
}

async fn next(receiver: &mut broadcast::Receiver<EngineRunEvent>) -> EngineRunEvent {
    tokio::time::timeout(std::time::Duration::from_secs(3), receiver.recv())
        .await
        .unwrap()
        .unwrap()
}

fn delayed_handle(
    id: RunId,
) -> (
    RunHandle,
    oneshot::Sender<RunOutcome>,
    broadcast::Sender<EngineRunEvent>,
) {
    let (sender, receiver) = oneshot::channel();
    let (events, receiver_events) = broadcast::channel(8);
    (
        RunHandle {
            run_id: id,
            events: receiver_events,
            completion: tokio::spawn(async move { receiver.await.unwrap() }),
        },
        sender,
        events,
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn production_drive_repairs_tap_lag_and_gap_before_final_flush() {
    for lag in [true, false] {
        let root = tempfile::tempdir().unwrap();
        let storage = Storage::open(root.path()).await.unwrap();
        let engine = engine(&storage, root.path());
        let id = RunId::new();
        let writer = storage.create_run(id, "/worktree", None).await.unwrap();
        for _ in 0..3 {
            append(&writer, EventPayload::RunWokeFromPark {}).await;
        }
        let rows = storage.inspect_run(id).await.unwrap();
        let RunDatabaseInspection::Present { events } = rows.database else {
            panic!("missing log")
        };
        let (tap_sender, tap) = broadcast::channel(if lag { 1 } else { 8 });
        for event in events.into_iter().filter(|event| lag || event.seq.0 == 3) {
            tap_sender.send(RunEventTap { run_id: id, event }).unwrap();
        }
        let (handle, complete, handle_events) = delayed_handle(id);
        // This notification must not bypass the authoritative join or durable final flush.
        handle_events
            .send(EngineRunEvent::Terminal { outcome: success() })
            .unwrap();
        let admission = Arc::new(AdmissionController::new(1, 0));
        assert!(admission.try_admit_no_queue(id).await);
        let registry = Arc::new(BroadcastRegistry::new());
        let publisher = registry.register(id).await;
        let mut receiver = publisher.subscribe();
        let run = TrackedRun {
            handle,
            prepared: Prepared::Durable {
                engine,
                storage,
                tap,
                last_seq: 0,
                // Every receive is bounded at 3s: fallback cannot rescue either tap branch.
                reconcile_period: std::time::Duration::from_secs(60),
            },
        };
        let tracked = spawn_tracked_run(id, run, publisher, admission.clone(), registry);
        for expected in 1..=3 {
            assert!(
                matches!(next(&mut receiver).await, EngineRunEvent::Persisted { seq, .. } if seq == expected)
            );
        }
        assert_eq!(admission.snapshot().await.active, 1);
        assert!(matches!(
            receiver.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
        append(
            &writer,
            EventPayload::RunCompleted {
                terminal_node: NodeKey::try_new("end").unwrap(),
            },
        )
        .await;
        // Deliberately never deliver seq4 to the tap.
        complete.send(success()).unwrap();
        assert!(matches!(
            next(&mut receiver).await,
            EngineRunEvent::Persisted { seq: 4, .. }
        ));
        assert!(matches!(
            next(&mut receiver).await,
            EngineRunEvent::Terminal { .. }
        ));
        assert_eq!(tracked.await.unwrap().unwrap(), success());
        assert_eq!(admission.snapshot().await.active, 0);
        assert!(matches!(
            receiver.recv().await,
            Err(broadcast::error::RecvError::Closed)
        ));
    }
}

fn request(prompt: &str) -> EventPayload {
    EventPayload::HumanInputRequested {
        node: NodeKey::try_new("gate").unwrap(),
        session: None,
        call_id: None,
        prompt: prompt.into(),
        schema: None,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn resume_watermark_excludes_historical_request_and_delivers_new_request() {
    let root = tempfile::tempdir().unwrap();
    let storage = Storage::open(root.path()).await.unwrap();
    let engine = engine(&storage, root.path());
    let id = RunId::new();
    let writer = storage.create_run(id, "/worktree", None).await.unwrap();
    append(&writer, request("already answered")).await;
    let context = TrackingContext::new(engine, storage.clone());
    let mut prepared = context.prepare(id, true).await.unwrap();
    append(&writer, request("fresh approval")).await;
    let (tap_sender, receiver_tap) = broadcast::channel(8);
    let Prepared::Durable { tap, last_seq, .. } = &mut prepared else {
        panic!("production context")
    };
    assert_eq!(*last_seq, 1);
    *tap = receiver_tap;
    let RunDatabaseInspection::Present { events } = storage.inspect_run(id).await.unwrap().database
    else {
        panic!("missing log")
    };
    for event in events {
        tap_sender.send(RunEventTap { run_id: id, event }).unwrap();
    }
    let (handle, complete, _handle_events) = delayed_handle(id);
    let admission = Arc::new(AdmissionController::new(1, 0));
    assert!(admission.try_admit_no_queue(id).await);
    let registry = Arc::new(BroadcastRegistry::new());
    let publisher = registry.register(id).await;
    let mut receiver = publisher.subscribe();
    let tracked = spawn_tracked_run(
        id,
        TrackedRun { handle, prepared },
        publisher,
        admission,
        registry,
    );
    let EngineRunEvent::Persisted { seq, payload } = next(&mut receiver).await else {
        panic!("expected fresh request")
    };
    assert_eq!(seq, 2);
    assert!(
        matches!(*payload, EventPayload::HumanInputRequested { prompt, .. } if prompt == "fresh approval")
    );
    append(
        &writer,
        EventPayload::RunCompleted {
            terminal_node: NodeKey::try_new("end").unwrap(),
        },
    )
    .await;
    complete.send(success()).unwrap();
    assert!(matches!(
        next(&mut receiver).await,
        EngineRunEvent::Persisted { seq: 3, .. }
    ));
    assert!(matches!(
        next(&mut receiver).await,
        EngineRunEvent::Terminal { .. }
    ));
    assert_eq!(tracked.await.unwrap().unwrap(), success());
}

#[test]
fn identical_duplicate_definitive_terminal_is_not_a_confirmed_outcome() {
    let payload = EventPayload::RunCompleted {
        terminal_node: NodeKey::try_new("end").unwrap(),
    };
    let events = [1, 2].map(|seq| ReadEvent {
        seq: surge_persistence::runs::EventSeq(seq),
        timestamp_ms: 0,
        kind: "RunCompleted".into(),
        payload: VersionedEventPayload::new(payload.clone()),
    });
    assert!(!confirms(&events, &success()));
}

#[tokio::test(flavor = "multi_thread")]
async fn periodic_reconciliation_delivers_durable_rows_without_tap_activity() {
    let root = tempfile::tempdir().unwrap();
    let storage = Storage::open(root.path()).await.unwrap();
    let engine = engine(&storage, root.path());
    let id = RunId::new();
    let writer = storage.create_run(id, "/worktree", None).await.unwrap();
    append(&writer, request("polling approval")).await;
    // Keep the tap open but never send. Only periodic reconciliation can deliver seq1.
    let (_tap_sender, tap) = broadcast::channel(8);
    let (handle, complete, _handle_events) = delayed_handle(id);
    let admission = Arc::new(AdmissionController::new(1, 0));
    assert!(admission.try_admit_no_queue(id).await);
    let registry = Arc::new(BroadcastRegistry::new());
    let publisher = registry.register(id).await;
    let mut receiver = publisher.subscribe();
    let tracked = spawn_tracked_run(
        id,
        TrackedRun {
            handle,
            prepared: Prepared::Durable {
                engine,
                storage,
                tap,
                last_seq: 0,
                reconcile_period: std::time::Duration::from_millis(10),
            },
        },
        publisher,
        admission,
        registry,
    );
    assert!(matches!(
        next(&mut receiver).await,
        EngineRunEvent::Persisted { seq: 1, .. }
    ));
    append(
        &writer,
        EventPayload::RunCompleted {
            terminal_node: NodeKey::try_new("end").unwrap(),
        },
    )
    .await;
    complete.send(success()).unwrap();
    assert!(matches!(
        next(&mut receiver).await,
        EngineRunEvent::Persisted { seq: 2, .. }
    ));
    assert!(matches!(
        next(&mut receiver).await,
        EngineRunEvent::Terminal { .. }
    ));
    assert_eq!(tracked.await.unwrap().unwrap(), success());
}
