//! Recovery must distinguish unreadable evidence from an absent run.
use surge_core::id::RunId;
use surge_persistence::runs::Storage;
use surge_telegram::cockpit::production::PersistenceSnapshots;
use surge_telegram::commands::status::RunSnapshotProvider;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unreadable_database_is_not_an_unknown_run() {
    let home = tempfile::tempdir().unwrap();
    let storage = Storage::open(home.path()).await.unwrap();
    let id = RunId::new();
    let directory = home.path().join("runs").join(id.to_string());
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(directory.join("events.sqlite"), b"not a SQLite database").unwrap();
    use surge_telegram::CardStore;
    let store = surge_telegram::cockpit::production::SqliteCardStore {
        storage: storage.clone(),
    };
    let card_id = store
        .upsert(&id.to_string(), "gate", 4, "human_gate", 42, "hash", 0)
        .await
        .unwrap();
    let provider = PersistenceSnapshots { storage };
    let result = provider.snapshot(id).await;
    assert!(
        result.is_err(),
        "read failure must not authorize closing a valid card: {result:?}"
    );
    let reconcile = surge_telegram::cockpit::reconcile_open_cards(
        &store,
        &provider,
        &RecordingApi::default(),
        1,
    )
    .await;
    assert!(reconcile.is_err());
    assert!(
        store
            .find_by_id(&card_id)
            .await
            .unwrap()
            .unwrap()
            .closed_at
            .is_none()
    );
}

async fn pending_log(
    storage: &std::sync::Arc<Storage>,
    id: RunId,
) -> surge_persistence::runs::run_writer::RunWriter {
    use surge_core::{
        ContentHash,
        run_event::{EventPayload, VersionedEventPayload},
    };
    let graph: surge_core::graph::Graph = serde_json::from_value(serde_json::json!({
        "schema_version":1, "metadata":{"name":"gate", "created_at":"2026-01-01T00:00:00Z"},
        "start":"gate", "nodes":{"gate":{"id":"gate", "position":{"x":0,"y":0},
            "declared_outcomes":[], "config":{"node_kind":"human_gate", "delivery_channels":[],
                "summary":{"title":"Review","body":"Review"},"options":[]}}}, "edges":[]
    }))
    .unwrap();
    let writer = storage.create_run(id, "/fixture", None).await.unwrap();
    let start = EventPayload::RunStarted {
        pipeline_template: None,
        project_path: "/fixture".into(),
        initial_prompt: String::new(),
        config: surge_core::run_event::RunConfig {
            sandbox_default: surge_core::sandbox::SandboxMode::WorkspaceWrite,
            approval_default: surge_core::approvals::ApprovalPolicy::OnRequest,
            auto_pr: false,
            mcp_servers: Vec::new(),
            budget: Default::default(),
        },
    };
    for payload in [
        start,
        EventPayload::PipelineMaterialized {
            graph_hash: ContentHash::compute(&serde_json::to_vec(&graph).unwrap()),
            graph: Box::new(graph),
        },
        EventPayload::StageEntered {
            node: "gate".try_into().unwrap(),
            attempt: 1,
        },
        EventPayload::HumanInputRequested {
            node: "gate".try_into().unwrap(),
            session: None,
            call_id: Some(surge_core::id::GateRequestId::new().to_string()),
            prompt: "Review".into(),
            schema: None,
        },
    ] {
        writer
            .append_event(VersionedEventPayload::new(payload))
            .await
            .unwrap();
    }
    writer
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn request_missed_before_card_creation_is_recovered_from_durable_events() {
    let home = tempfile::tempdir().unwrap();
    let storage = Storage::open(home.path()).await.unwrap();
    let id = RunId::new();
    let writer = pending_log(&storage, id).await;
    let provider = PersistenceSnapshots { storage };
    let requests = provider.pending_requests().await.unwrap().requests;
    assert_eq!(
        requests.len(),
        1,
        "no card row or live tap exists; durable request must be discovered"
    );
    assert_eq!(requests[0].run_id, id);
    assert_eq!(requests[0].event.seq.0, 4);
    writer.close().await.unwrap();
}

#[derive(Clone, Default)]
struct RecordingApi(std::sync::Arc<std::sync::atomic::AtomicUsize>);
#[async_trait::async_trait]
impl surge_telegram::TelegramApi for RecordingApi {
    async fn send_message(
        &self,
        _: i64,
        _: &str,
        _: &[Vec<surge_notify::telegram::InboxKeyboardButton>],
    ) -> surge_telegram::Result<i64> {
        Ok(self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst) as i64 + 1)
    }
    async fn edit_message_text(
        &self,
        _: i64,
        _: i64,
        _: &str,
        _: &[Vec<surge_notify::telegram::InboxKeyboardButton>],
    ) -> surge_telegram::Result<()> {
        Ok(())
    }
}
struct NoUpdates;
#[async_trait::async_trait]
impl surge_telegram::cockpit::run::UpdateRoutes for NoUpdates {
    async fn handle_callback(&self, _: i64, _: &str, _: &str) {}
    async fn handle_command(&self, _: i64, _: &str) {}
    async fn handle_reply(&self, _: i64, _: i64, _: &str) {}
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_startup_delivers_a_card_without_receiving_any_tap() {
    use surge_telegram::cockpit::{
        production::SqliteCardStore,
        run::{CockpitRuntime, drive_tap_loop},
    };
    use surge_telegram::{CardEmitter, CardStore};
    let home = tempfile::tempdir().unwrap();
    let storage = Storage::open(home.path()).await.unwrap();
    let id = RunId::new();
    let writer = pending_log(&storage, id).await;
    let store = SqliteCardStore {
        storage: storage.clone(),
    };
    assert!(store.find_open().await.unwrap().is_empty());
    let api = RecordingApi::default();
    let runtime = std::sync::Arc::new(CockpitRuntime {
        dispatch_ctx: surge_telegram::CockpitCtx {
            emitter: CardEmitter::new(store.clone(), api),
            admin_chat_id: 42,
        },
        snapshots: PersistenceSnapshots { storage },
        routes: NoUpdates,
    });
    let (_tap_sender, tap) = tokio::sync::broadcast::channel(1);
    let stop = tokio_util::sync::CancellationToken::new();
    let task = tokio::spawn(drive_tap_loop(runtime, tap, stop.clone()));
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let cards = store.find_open().await.unwrap();
            if cards.len() == 1 && cards[0].message_id.is_some() {
                assert_eq!(
                    cards[0].attempt_index, 4,
                    "durable request seq is the card identity"
                );
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    stop.cancel();
    task.await.unwrap();
    writer.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn recovery_selects_only_current_bound_unresolved_request() {
    use surge_core::{EventPayload, VersionedEventPayload};
    let home = tempfile::tempdir().unwrap();
    let storage = Storage::open(home.path()).await.unwrap();
    let id = RunId::new();
    let writer = pending_log(&storage, id).await;
    let provider = PersistenceSnapshots { storage };
    let first = provider
        .pending_requests()
        .await
        .unwrap()
        .requests
        .remove(0);
    let EventPayload::HumanInputRequested { node, call_id, .. } = first.event.payload.payload
    else {
        panic!()
    };
    writer
        .append_event(VersionedEventPayload::new(
            EventPayload::HumanInputResolved {
                node: node.clone(),
                call_id,
                response: serde_json::json!({"outcome":"edit"}),
            },
        ))
        .await
        .unwrap();
    assert!(
        provider
            .pending_requests()
            .await
            .unwrap()
            .requests
            .is_empty()
    );
    for current_id in [None, Some(surge_core::id::GateRequestId::new().to_string())] {
        writer
            .append_event(VersionedEventPayload::new(
                EventPayload::HumanInputRequested {
                    node: node.clone(),
                    session: None,
                    call_id: current_id.clone(),
                    prompt: "new".into(),
                    schema: None,
                },
            ))
            .await
            .unwrap();
        let requests = provider.pending_requests().await.unwrap().requests;
        assert_eq!(
            requests.len(),
            usize::from(current_id.is_some()),
            "legacy unbound requests must not be actionable"
        );
    }
    let current = provider
        .pending_requests()
        .await
        .unwrap()
        .requests
        .remove(0);
    assert!(current.event.seq.0 > first.event.seq.0);
    let EventPayload::HumanInputRequested { call_id, .. } = current.event.payload.payload else {
        panic!()
    };
    writer
        .append_event(VersionedEventPayload::new(
            EventPayload::HumanInputTimedOut {
                node: node.clone(),
                call_id,
                elapsed_seconds: 1,
            },
        ))
        .await
        .unwrap();
    assert!(
        provider
            .pending_requests()
            .await
            .unwrap()
            .requests
            .is_empty()
    );
    writer
        .append_event(VersionedEventPayload::new(
            EventPayload::HumanInputRequested {
                node: "other".try_into().unwrap(),
                session: None,
                call_id: Some(surge_core::id::GateRequestId::new().to_string()),
                prompt: "wrong node".into(),
                schema: None,
            },
        ))
        .await
        .unwrap();
    assert!(
        provider
            .pending_requests()
            .await
            .unwrap()
            .requests
            .is_empty()
    );
    writer.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_registered_database_is_an_error_and_inspection_does_not_create_it() {
    let home = tempfile::tempdir().unwrap();
    let storage = Storage::open(home.path()).await.unwrap();
    let id = RunId::new();
    storage
        .create_run(id, "/fixture", None)
        .await
        .unwrap()
        .close()
        .await
        .unwrap();
    let database = home
        .path()
        .join("runs")
        .join(id.to_string())
        .join("events.sqlite");
    std::fs::remove_file(&database).unwrap();
    let provider = PersistenceSnapshots { storage };
    assert!(provider.snapshot(id).await.is_err());
    assert!(!database.exists());
    let unknown = RunId::new();
    assert!(provider.snapshot(unknown).await.unwrap().is_none());
    assert!(!home.path().join("runs").join(unknown.to_string()).exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn corrupt_run_does_not_starve_healthy_missed_card() {
    use surge_telegram::cockpit::{
        production::SqliteCardStore,
        run::{CockpitRuntime, drive_tap_loop},
    };
    use surge_telegram::{CardEmitter, CardStore};
    let home = tempfile::tempdir().unwrap();
    let storage = Storage::open(home.path()).await.unwrap();
    let healthy = RunId::new();
    let corrupt = RunId::new();
    let writer = pending_log(&storage, healthy).await;
    pending_log(&storage, corrupt).await.close().await.unwrap();
    std::fs::write(
        home.path()
            .join("runs")
            .join(corrupt.to_string())
            .join("events.sqlite"),
        b"corrupt journal",
    )
    .unwrap();
    let store = SqliteCardStore {
        storage: storage.clone(),
    };
    let existing = store
        .upsert(&corrupt.to_string(), "gate", 4, "human_gate", 42, "hash", 0)
        .await
        .unwrap();
    let batch = PersistenceSnapshots {
        storage: storage.clone(),
    }
    .pending_requests()
    .await
    .unwrap();
    assert_eq!(batch.requests.len(), 1);
    assert_eq!(batch.requests[0].run_id, healthy);
    assert_eq!(batch.failures.len(), 1);
    assert_eq!(batch.failures[0].run_id, corrupt);
    assert!(matches!(
        batch.failures[0].error,
        surge_telegram::error::TelegramCockpitError::Persistence(_)
    ));
    let api = RecordingApi::default();
    let runtime = std::sync::Arc::new(CockpitRuntime {
        dispatch_ctx: surge_telegram::CockpitCtx {
            emitter: CardEmitter::new(store.clone(), api.clone()),
            admin_chat_id: 42,
        },
        snapshots: PersistenceSnapshots { storage },
        routes: NoUpdates,
    });
    let (_sender, tap) = tokio::sync::broadcast::channel(1);
    let stop = tokio_util::sync::CancellationToken::new();
    let task = tokio::spawn(drive_tap_loop(runtime, tap, stop.clone()));
    let delivered = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while api.0.load(std::sync::atomic::Ordering::SeqCst) == 0 {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await;
    if delivered.is_ok() {
        tokio::time::sleep(std::time::Duration::from_millis(5100)).await;
    }
    stop.cancel();
    tokio::time::timeout(std::time::Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap();
    writer.close().await.unwrap();
    assert!(
        delivered.is_ok(),
        "corrupt journal starved healthy missed card"
    );
    assert_eq!(api.0.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(
        store
            .find_by_id(&existing)
            .await
            .unwrap()
            .unwrap()
            .closed_at
            .is_none()
    );
    assert_eq!(store.find_open().await.unwrap().len(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exact_timeout_closes_old_card_but_wrong_identity_never_does() {
    use surge_core::{EventPayload, run_event::VersionedEventPayload};
    use surge_telegram::{CardStore, cockpit::production::SqliteCardStore};
    let home = tempfile::tempdir().unwrap();
    let storage = Storage::open(home.path()).await.unwrap();
    let id = RunId::new();
    let writer = pending_log(&storage, id).await;
    let provider = PersistenceSnapshots {
        storage: storage.clone(),
    };
    let request = provider
        .pending_requests()
        .await
        .unwrap()
        .requests
        .remove(0);
    let EventPayload::HumanInputRequested { call_id, .. } = request.event.payload.payload else {
        panic!("request required")
    };
    let store = SqliteCardStore {
        storage: storage.clone(),
    };
    let old = store
        .upsert(&id.to_string(), "gate", 4, "human_gate", 42, "old", 0)
        .await
        .unwrap();
    writer
        .append_event(VersionedEventPayload::new(
            EventPayload::HumanInputTimedOut {
                node: "gate".try_into().unwrap(),
                call_id: Some(surge_core::id::GateRequestId::new().to_string()),
                elapsed_seconds: 1,
            },
        ))
        .await
        .unwrap();
    let card = store.find_by_id(&old).await.unwrap().unwrap();
    assert!(!provider.request_settled(&card).await.unwrap());
    writer
        .append_event(VersionedEventPayload::new(
            EventPayload::HumanInputTimedOut {
                node: "gate".try_into().unwrap(),
                call_id,
                elapsed_seconds: 1,
            },
        ))
        .await
        .unwrap();
    writer
        .append_event(VersionedEventPayload::new(
            EventPayload::HumanInputRequested {
                node: "gate".try_into().unwrap(),
                session: None,
                call_id: Some(surge_core::id::GateRequestId::new().to_string()),
                prompt: "New request".into(),
                schema: None,
            },
        ))
        .await
        .unwrap();
    let newer = store
        .upsert(&id.to_string(), "gate", 7, "human_gate", 42, "new", 1)
        .await
        .unwrap();
    let report = surge_telegram::cockpit::reconcile_open_cards(
        &store,
        &provider,
        &RecordingApi::default(),
        2,
    )
    .await
    .unwrap();
    assert_eq!(report.closed, 1);
    assert!(
        store
            .find_by_id(&old)
            .await
            .unwrap()
            .unwrap()
            .closed_at
            .is_some()
    );
    assert!(
        store
            .find_by_id(&newer)
            .await
            .unwrap()
            .unwrap()
            .closed_at
            .is_none()
    );
    writer.close().await.unwrap();
}
