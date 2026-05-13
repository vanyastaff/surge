//! End-to-end test for the inbox-action subsystem.
//!
//! Drives a `MockTaskSource` → `InboxActionConsumer` pipeline with a mocked
//! engine, asserting `ticket_index` state transitions and tracker comments
//! for each of Start / Snooze / Skip plus idempotency.

use async_trait::async_trait;
use chrono::Utc;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;
use surge_core::SurgeConfig;
use surge_core::id::RunId;
use surge_daemon::inbox::consumer::InboxActionConsumer;
use surge_intake::TaskSource;
use surge_intake::testing::MockTaskSource;
use surge_intake::types::{TaskDetails, TaskId};
use surge_orchestrator::bootstrap::{BootstrapGraphBuilder, MinimalBootstrapGraphBuilder};
use surge_orchestrator::engine::config::EngineRunConfig;
use surge_orchestrator::engine::error::EngineError;
use surge_orchestrator::engine::facade::EngineFacade;
use surge_orchestrator::engine::handle::{EngineRunEvent, RunHandle, RunOutcome, RunSummary};
use surge_persistence::inbox_queue::{self, InboxActionKind};
use surge_persistence::intake::{IntakeRepo, IntakeRow, TicketState};
use surge_persistence::runs::storage::Storage;
use tempfile::TempDir;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

// ===== Helpers ==========================================================

async fn build_storage() -> (Arc<Storage>, TempDir) {
    let tmp = TempDir::new().unwrap();
    let storage = Storage::open(tmp.path()).await.unwrap();
    (storage, tmp)
}

fn insert_ticket(storage: &Storage, task_id: &str, callback_token: &str) {
    let conn = storage.acquire_registry_conn().unwrap();
    let repo = IntakeRepo::new(&conn);
    let row = IntakeRow {
        task_id: task_id.into(),
        source_id: "mock:t".into(),
        provider: "mock".into(),
        run_id: None,
        triage_decision: None,
        duplicate_of: None,
        priority: Some("medium".into()),
        state: TicketState::InboxNotified,
        first_seen: Utc::now(),
        last_seen: Utc::now(),
        snooze_until: None,
        callback_token: Some(callback_token.into()),
        tg_chat_id: None,
        tg_message_id: None,
    };
    repo.insert(&row).unwrap();
}

fn fetch_state(storage: &Storage, task_id: &str) -> TicketState {
    let conn = storage.acquire_registry_conn().unwrap();
    IntakeRepo::new(&conn)
        .fetch(task_id)
        .unwrap()
        .unwrap()
        .state
}

fn make_task_details(task_id: &str) -> TaskDetails {
    TaskDetails {
        task_id: TaskId::try_new(task_id).unwrap(),
        source_id: "mock:t".into(),
        title: "Test task".into(),
        description: "A test task description.".into(),
        status: "open".into(),
        labels: vec![],
        url: format!("https://mock.invalid/{task_id}"),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        assignee: None,
        raw_payload: serde_json::json!({}),
    }
}

// ===== Mock engine ======================================================

#[derive(Debug, Clone)]
enum EngineBehavior {
    SucceedsThenCompletes,
    Errors,
}

#[derive(Debug, Default, Clone)]
struct EngineState {
    pub start_calls: Arc<StdMutex<Vec<RunId>>>,
}

struct MockEngineFacade {
    state: EngineState,
    behavior: tokio::sync::Mutex<EngineBehavior>,
    /// Shared storage: the mock inserts a fake `runs` row on each `start_run`
    /// so that `ticket_index.run_id` FK constraints are satisfied.
    storage: Arc<Storage>,
}

impl MockEngineFacade {
    fn new(behavior: EngineBehavior, storage: Arc<Storage>) -> (Arc<Self>, EngineState) {
        let state = EngineState::default();
        let f = Arc::new(Self {
            state: state.clone(),
            behavior: tokio::sync::Mutex::new(behavior),
            storage,
        });
        (f, state)
    }
}

#[async_trait]
impl EngineFacade for MockEngineFacade {
    async fn start_run(
        &self,
        run_id: RunId,
        _graph: surge_core::graph::Graph,
        _worktree_path: PathBuf,
        _run_config: EngineRunConfig,
    ) -> Result<RunHandle, EngineError> {
        self.state.start_calls.lock().unwrap().push(run_id);
        let behavior = self.behavior.lock().await.clone();

        // Insert a fake `runs` row so that the `ticket_index.run_id` FK
        // constraint is satisfied when `handle_start` calls `set_run_id`.
        if matches!(behavior, EngineBehavior::SucceedsThenCompletes) {
            let conn = self
                .storage
                .acquire_registry_conn()
                .map_err(|e| EngineError::Internal(e.to_string()))?;
            conn.execute(
                "INSERT OR IGNORE INTO runs(id, project_path, status, started_at) \
                 VALUES (?1, '/fake', 'active', 0)",
                rusqlite::params![run_id.to_string()],
            )
            .map_err(|e| EngineError::Internal(e.to_string()))?;
        }
        match behavior {
            EngineBehavior::SucceedsThenCompletes => {
                let (tx, rx) = broadcast::channel(8);
                let tx_for_task = tx.clone();
                let completion = tokio::spawn(async move {
                    use surge_core::keys::NodeKey;
                    use surge_core::run_event::EventPayload;
                    // Emit a Persisted event so TicketStateSync flips to Active.
                    // StageEntered is simpler than RunStarted (which has complex
                    // fields). TicketStateSync only needs any Persisted event.
                    let node = NodeKey::try_new("bootstrap_agent").unwrap();
                    let _ = tx_for_task.send(EngineRunEvent::Persisted {
                        seq: 0,
                        payload: Box::new(EventPayload::StageEntered { node, attempt: 0 }),
                    });
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    let outcome = RunOutcome::Completed {
                        terminal: NodeKey::try_new("terminal_success").unwrap(),
                    };
                    let _ = tx_for_task.send(EngineRunEvent::Terminal {
                        outcome: outcome.clone(),
                    });
                    outcome
                });
                Ok(RunHandle {
                    run_id,
                    events: rx,
                    completion,
                })
            },
            EngineBehavior::Errors => Err(EngineError::Internal("simulated engine error".into())),
        }
    }

    async fn resume_run(
        &self,
        _run_id: RunId,
        _worktree_path: PathBuf,
    ) -> Result<RunHandle, EngineError> {
        unimplemented!("not used by inbox tests")
    }
    async fn stop_run(&self, _run_id: RunId, _reason: String) -> Result<(), EngineError> {
        Ok(())
    }
    async fn resolve_human_input(
        &self,
        _run_id: RunId,
        _call_id: Option<String>,
        _response: serde_json::Value,
    ) -> Result<(), EngineError> {
        Ok(())
    }
    async fn list_runs(&self) -> Result<Vec<RunSummary>, EngineError> {
        Ok(vec![])
    }
}

// ===== Test scaffold ====================================================

fn make_consumer(
    storage: Arc<Storage>,
    engine: Arc<dyn EngineFacade>,
    source: Arc<MockTaskSource>,
) -> InboxActionConsumer {
    let mut sources: HashMap<String, Arc<dyn TaskSource>> = HashMap::new();
    sources.insert("mock:t".into(), Arc::clone(&source) as Arc<dyn TaskSource>);
    let bootstrap: Arc<dyn BootstrapGraphBuilder> = Arc::new(MinimalBootstrapGraphBuilder::new());
    InboxActionConsumer {
        storage,
        bootstrap,
        engine,
        sources: Arc::new(sources),
        worktrees_root: std::env::temp_dir().join("inbox_test_worktrees"),
        project_root: std::env::temp_dir(),
        config: SurgeConfig::default(),
        poll_interval: Duration::from_millis(50),
    }
}

// ===== Scenarios ========================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scenario_a_start_happy_path() {
    let (storage, _tmp) = build_storage().await;
    insert_ticket(&storage, "mock:t#1", "tok_start_1");

    let mock = Arc::new(MockTaskSource::new("mock:t", "mock"));
    mock.put_task(make_task_details("mock:t#1")).await;
    let (engine, engine_state) =
        MockEngineFacade::new(EngineBehavior::SucceedsThenCompletes, Arc::clone(&storage));
    let consumer = make_consumer(
        Arc::clone(&storage),
        engine.clone() as Arc<dyn EngineFacade>,
        Arc::clone(&mock),
    );

    // Enqueue Start action.
    {
        let conn = storage.acquire_registry_conn().unwrap();
        inbox_queue::append_action(
            &conn,
            InboxActionKind::Start,
            "mock:t#1",
            "tok_start_1",
            "telegram",
            None,
            None,
        )
        .unwrap();
    }

    let shutdown = CancellationToken::new();
    let consumer_handle = tokio::spawn(consumer.run(shutdown.clone()));
    // Allow consumer to process the action AND TicketStateSync to consume the
    // Persisted + Terminal events.
    tokio::time::sleep(Duration::from_millis(700)).await;
    shutdown.cancel();
    let _ = consumer_handle.await;

    // 1. engine.start_run was called.
    assert_eq!(engine_state.start_calls.lock().unwrap().len(), 1);
    // 2. ticket_index reached Completed (via TicketStateSync after engine emits terminal).
    assert_eq!(fetch_state(&storage, "mock:t#1"), TicketState::Completed);
    // 3. Tracker comments contain start + completion.
    let comments = mock.posted_comments().await;
    assert!(
        comments
            .iter()
            .any(|(_, body)| body.starts_with("Surge run #")),
        "expected start comment in {comments:?}"
    );
    assert!(
        comments.iter().any(|(_, body)| body.contains("✅")),
        "expected completion comment in {comments:?}"
    );
    // 4. callback_token cleared.
    let conn = storage.acquire_registry_conn().unwrap();
    let row = IntakeRepo::new(&conn).fetch("mock:t#1").unwrap().unwrap();
    assert_eq!(row.callback_token, None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scenario_b_snooze_then_re_emit() {
    let (storage, _tmp) = build_storage().await;
    insert_ticket(&storage, "mock:t#2", "tok_snooze_2");

    let mock = Arc::new(MockTaskSource::new("mock:t", "mock"));
    let (engine, _) =
        MockEngineFacade::new(EngineBehavior::SucceedsThenCompletes, Arc::clone(&storage));
    let consumer = make_consumer(
        Arc::clone(&storage),
        engine as Arc<dyn EngineFacade>,
        Arc::clone(&mock),
    );

    // Enqueue Snooze with snooze_until = past so re-emission is immediate.
    {
        let conn = storage.acquire_registry_conn().unwrap();
        inbox_queue::append_action(
            &conn,
            InboxActionKind::Snooze,
            "mock:t#2",
            "tok_snooze_2",
            "telegram",
            Some(Utc::now() - chrono::Duration::seconds(1)),
            None,
        )
        .unwrap();
    }

    let shutdown = CancellationToken::new();
    let consumer_handle = tokio::spawn(consumer.run(shutdown.clone()));
    tokio::time::sleep(Duration::from_millis(200)).await;

    // After consumer ticks, state should be Snoozed.
    assert_eq!(fetch_state(&storage, "mock:t#2"), TicketState::Snoozed);

    shutdown.cancel();
    let _ = consumer_handle.await;

    // Run the SnoozeScheduler manually.
    use surge_daemon::inbox::snooze_scheduler::SnoozeScheduler;
    let scheduler = SnoozeScheduler {
        storage: Arc::clone(&storage),
        poll_interval: Duration::from_millis(50),
    };
    let sched_shutdown = CancellationToken::new();
    let sched_handle = tokio::spawn(scheduler.run(sched_shutdown.clone()));
    tokio::time::sleep(Duration::from_millis(200)).await;
    sched_shutdown.cancel();
    let _ = sched_handle.await;

    // After re-emit: state back to InboxNotified, callback_token regenerated.
    let conn = storage.acquire_registry_conn().unwrap();
    let row = IntakeRepo::new(&conn).fetch("mock:t#2").unwrap().unwrap();
    assert_eq!(row.state, TicketState::InboxNotified);
    assert!(row.callback_token.is_some());
    assert_ne!(row.callback_token.as_deref(), Some("tok_snooze_2"));
    // A delivery row exists for the re-emission.
    let deliveries = inbox_queue::list_pending_telegram_deliveries(&conn).unwrap();
    assert_eq!(deliveries.len(), 1);
    assert_eq!(deliveries[0].task_id, "mock:t#2");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scenario_c_skip_sets_label_and_state() {
    let (storage, _tmp) = build_storage().await;
    insert_ticket(&storage, "mock:t#3", "tok_skip_3");

    let mock = Arc::new(MockTaskSource::new("mock:t", "mock"));
    let (engine, _) =
        MockEngineFacade::new(EngineBehavior::SucceedsThenCompletes, Arc::clone(&storage));
    let consumer = make_consumer(
        Arc::clone(&storage),
        engine as Arc<dyn EngineFacade>,
        Arc::clone(&mock),
    );

    {
        let conn = storage.acquire_registry_conn().unwrap();
        inbox_queue::append_action(
            &conn,
            InboxActionKind::Skip,
            "mock:t#3",
            "tok_skip_3",
            "telegram",
            None,
            None,
        )
        .unwrap();
    }

    let shutdown = CancellationToken::new();
    let h = tokio::spawn(consumer.run(shutdown.clone()));
    tokio::time::sleep(Duration::from_millis(200)).await;
    shutdown.cancel();
    let _ = h.await;

    assert_eq!(fetch_state(&storage, "mock:t#3"), TicketState::Skipped);
    let labels = mock.recorded_labels().await;
    assert!(
        labels
            .iter()
            .any(|(_, label, present)| label == "surge:skipped" && *present),
        "expected surge:skipped label set; got {labels:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scenario_d_idempotent_double_start() {
    let (storage, _tmp) = build_storage().await;
    insert_ticket(&storage, "mock:t#4", "tok_dbl_4");

    let mock = Arc::new(MockTaskSource::new("mock:t", "mock"));
    mock.put_task(make_task_details("mock:t#4")).await;
    let (engine, engine_state) =
        MockEngineFacade::new(EngineBehavior::SucceedsThenCompletes, Arc::clone(&storage));
    let consumer = make_consumer(
        Arc::clone(&storage),
        engine.clone() as Arc<dyn EngineFacade>,
        Arc::clone(&mock),
    );

    // Enqueue Start TWICE for the same token.
    {
        let conn = storage.acquire_registry_conn().unwrap();
        inbox_queue::append_action(
            &conn,
            InboxActionKind::Start,
            "mock:t#4",
            "tok_dbl_4",
            "telegram",
            None,
            None,
        )
        .unwrap();
        inbox_queue::append_action(
            &conn,
            InboxActionKind::Start,
            "mock:t#4",
            "tok_dbl_4",
            "telegram",
            None,
            None,
        )
        .unwrap();
    }

    let shutdown = CancellationToken::new();
    let h = tokio::spawn(consumer.run(shutdown.clone()));
    tokio::time::sleep(Duration::from_millis(700)).await;
    shutdown.cancel();
    let _ = h.await;

    // Engine.start_run called exactly once.
    assert_eq!(engine_state.start_calls.lock().unwrap().len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scenario_e_engine_failure_keeps_state_inbox_notified() {
    let (storage, _tmp) = build_storage().await;
    insert_ticket(&storage, "mock:t#5", "tok_fail_5");

    let mock = Arc::new(MockTaskSource::new("mock:t", "mock"));
    mock.put_task(make_task_details("mock:t#5")).await;
    let (engine, engine_state) =
        MockEngineFacade::new(EngineBehavior::Errors, Arc::clone(&storage));
    let consumer = make_consumer(
        Arc::clone(&storage),
        engine.clone() as Arc<dyn EngineFacade>,
        Arc::clone(&mock),
    );

    {
        let conn = storage.acquire_registry_conn().unwrap();
        inbox_queue::append_action(
            &conn,
            InboxActionKind::Start,
            "mock:t#5",
            "tok_fail_5",
            "telegram",
            None,
            None,
        )
        .unwrap();
    }

    let shutdown = CancellationToken::new();
    let h = tokio::spawn(consumer.run(shutdown.clone()));
    tokio::time::sleep(Duration::from_millis(300)).await;
    shutdown.cancel();
    let _ = h.await;

    // Engine called once.
    assert_eq!(engine_state.start_calls.lock().unwrap().len(), 1);
    // State remained InboxNotified (no transition on engine failure).
    assert_eq!(
        fetch_state(&storage, "mock:t#5"),
        TicketState::InboxNotified
    );
}
