//! Production routes against a real Engine, SQLite, and a local Bot API.
use serde_json::json;
use std::{sync::Arc, time::Duration};
use surge_core::{EventPayload, id::RunId};
use surge_orchestrator::engine::{
    Engine, EngineConfig, EngineRunConfig, facade::LocalEngineFacade,
    tools::worktree::WorktreeToolDispatcher,
};
use surge_persistence::runs::Storage;
use surge_telegram::{
    CardStore,
    cockpit::{production::*, run::UpdateRoutes},
};
use wiremock::{Mock, MockServer, ResponseTemplate};
const FLOW: &str = r#"# Human gate revisits itself after edit; approve completes.

schema_version = 1
start = "gate"


[metadata]
name = "flow_terminal_only"
created_at = "2026-05-05T00:00:00Z"

[nodes.end]
id = "end"
declared_outcomes = []

[nodes.end.position]
x = 0.0
y = 0.0

[nodes.end.config]
node_kind = "terminal"

[nodes.end.config.kind]
type = "success"

[nodes.gate]
id = "gate"
[nodes.gate.position]
x = 0.0
y = 0.0
[[nodes.gate.declared_outcomes]]
id = "approve"
description = "Approved"
edge_kind_hint = "forward"
is_terminal = false
[[nodes.gate.declared_outcomes]]
id = "edit"
description = "Revise"
edge_kind_hint = "backtrack"
is_terminal = false
[nodes.gate.config]
node_kind = "human_gate"
delivery_channels = []
mode = { bootstrap = { stage = "description" } }
[nodes.gate.config.summary]
title = "Approval"
body = "Decide"
[[nodes.gate.config.options]]
outcome = "approve"
label = "Approve"
[[nodes.gate.config.options]]
outcome = "edit"
label = "Edit"
[[edges]]
id = "approved"
to = "end"
kind = "forward"
[edges.from]
node = "gate"
outcome = "approve"
[edges.policy]
on_max_exceeded = "escalate"
[[edges]]
id = "again"
to = "gate"
kind = "backtrack"
[edges.from]
node = "gate"
outcome = "edit"
[edges.policy]
max_traversals = 3
on_max_exceeded = "fail"
"#;

fn routes(storage: &Arc<Storage>, engine: Arc<Engine>, bot: teloxide::Bot) -> ProductionRoutes {
    let facade = Arc::new(LocalEngineFacade::new(engine));
    ProductionRoutes {
        inbox: None,
        admission: PairingsAdmission {
            storage: storage.clone(),
        },
        engine: EngineFacadeResolver {
            storage: storage.clone(),
            engine: facade.clone(),
        },
        store: SqliteCardStore {
            storage: storage.clone(),
        },
        bot,
        pairing_consumer: PersistencePairingConsumer {
            storage: storage.clone(),
        },
        snapshots: PersistenceSnapshots {
            storage: storage.clone(),
        },
        run_list: PersistenceRunList {
            storage: storage.clone(),
        },
        run_aborter: EngineFacadeAborter { engine: facade },
        run_starter: DeferredRunStarter,
        snooze_writer: PersistenceCockpitSnoozeWriter {
            storage: storage.clone(),
        },
    }
}

struct Fixture {
    _home: tempfile::TempDir,
    storage: Arc<Storage>,
    engine: Arc<Engine>,
    id: RunId,
    handle: surge_orchestrator::engine::RunHandle,
    tap: tokio::sync::broadcast::Receiver<surge_orchestrator::engine::RunEventTap>,
    routes: ProductionRoutes,
    server: MockServer,
}
impl Fixture {
    async fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        let storage = Storage::open(home.path()).await.unwrap();
        {
            let conn = storage.acquire_registry_conn().unwrap();
            surge_persistence::telegram::pairings::pair(&conn, 42, "test", 0).unwrap();
            surge_persistence::telegram::pairings::pair(&conn, 43, "other", 0).unwrap();
        }
        let bridge = Arc::new(surge_acp::bridge::AcpBridge::spawn(8, 32).unwrap());
        let engine = Arc::new(Engine::new(
            bridge,
            storage.clone(),
            Arc::new(WorktreeToolDispatcher::new(home.path().into())),
            EngineConfig::default(),
        ));
        let tap = engine.subscribe_tap();
        let id = RunId::new();
        let handle = engine
            .start_run(
                id,
                toml::from_str(FLOW).unwrap(),
                home.path().into(),
                EngineRunConfig::default(),
            )
            .await
            .unwrap();
        let server = MockServer::start().await;
        Mock::given(wiremock::matchers::path("/bottest/AnswerCallbackQuery"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"ok":true,"result":true})),
            )
            .mount(&server)
            .await;
        Mock::given(wiremock::matchers::path("/bottest/SendMessage")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok":true,"result":{"message_id":99,"date":0,"chat":{"id":42,"type":"private"},"text":"result"}}))).mount(&server).await;
        let routes = routes(
            &storage,
            engine.clone(),
            teloxide::Bot::new("test").set_api_url(server.uri().parse().unwrap()),
        );
        Self {
            _home: home,
            storage,
            engine,
            id,
            handle,
            tap,
            routes,
            server,
        }
    }
    async fn card(&mut self) -> surge_persistence::telegram::cards::Card {
        let event = tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let e = self.tap.recv().await.unwrap();
                if matches!(
                    e.event.payload.payload,
                    EventPayload::HumanInputRequested { .. }
                ) {
                    break e;
                }
            }
        })
        .await
        .unwrap();
        let EventPayload::HumanInputRequested {
            schema: Some(schema),
            ..
        } = &event.event.payload.payload
        else {
            panic!("missing gate schema")
        };
        assert_eq!(schema["x-surge-bootstrap-stage"], "description");
        let id = self
            .routes
            .store
            .upsert(
                &self.id.to_string(),
                "gate",
                i64::try_from(event.event.seq.0).unwrap(),
                "human_gate",
                42,
                "hash",
                0,
            )
            .await
            .unwrap();
        self.routes.store.find_by_id(&id).await.unwrap().unwrap()
    }
    async fn action(&self, chat: i64, verb: &str, card: &surge_persistence::telegram::cards::Card) {
        tokio::time::timeout(
            Duration::from_secs(30),
            self.routes.handle_callback(
                chat,
                &format!("cockpit:{verb}:{}", card.card_id),
                "query1",
            ),
        )
        .await
        .expect("local Bot API callback exceeded watchdog");
    }
    async fn texts(&self) -> Vec<String> {
        self.server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|r| r.url.path().ends_with("SendMessage"))
            .map(|r| {
                r.body_json::<serde_json::Value>().unwrap()["text"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect()
    }
    async fn finish(self) -> surge_orchestrator::engine::RunOutcome {
        let _ = self.engine.stop_run(self.id, "test cleanup".into()).await;
        tokio::time::timeout(Duration::from_secs(3), self.handle.await_completion())
            .await
            .unwrap()
            .unwrap()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn production_approve_acknowledges_and_resolves_exact_gate() {
    let mut f = Fixture::new().await;
    let card = f.card().await;
    f.action(42, "approve", &card).await;
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if matches!(
                f.tap.recv().await.unwrap().event.payload.payload,
                EventPayload::RunCompleted { .. }
            ) {
                break;
            }
        }
    })
    .await
    .unwrap();
    f.action(42, "approve", &card).await;
    let calls = f.server.received_requests().await.unwrap();
    assert!(calls[0].url.path().ends_with("AnswerCallbackQuery"));
    assert_eq!(
        calls
            .iter()
            .filter(|r| r.url.path().ends_with("AnswerCallbackQuery"))
            .count(),
        2
    );
    let texts = f.texts().await;
    assert!(texts[0].contains("accepted"));
    assert!(texts[1].contains("stale"));
    let inspection = f.storage.inspect_run(f.id).await.unwrap();
    let surge_persistence::runs::inspection::RunDatabaseInspection::Present { events } =
        inspection.database
    else {
        panic!()
    };
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e.payload.payload, EventPayload::HumanInputResolved { .. }))
            .count(),
        1
    );
    assert!(matches!(
        f.finish().await,
        surge_orchestrator::engine::RunOutcome::Completed { .. }
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn old_open_card_cannot_approve_new_gate_and_reports_stale() {
    use surge_telegram::cockpit::callback::EngineResolver;
    let mut f = Fixture::new().await;
    let old = f.card().await;
    f.routes
        .engine
        .resolve_card(&old, json!({"outcome":"edit"}))
        .await
        .unwrap();
    let current = f.card().await;
    f.action(42, "approve", &old).await;
    assert!(f.texts().await.last().unwrap().contains("stale"));
    assert!(
        f.routes
            .store
            .find_by_id(&current.card_id)
            .await
            .unwrap()
            .unwrap()
            .closed_at
            .is_none()
    );
    f.action(42, "approve", &current).await;
    assert!(f.texts().await.last().unwrap().contains("accepted"));
    f.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn forced_reply_requires_exact_chat_prompt_and_card() {
    let mut f = Fixture::new().await;
    let card = f.card().await;
    f.action(42, "edit", &card).await;
    let saved = f
        .routes
        .store
        .find_by_id(&card.card_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved.pending_edit_prompt_message_id, Some(99));
    let calls = f.server.received_requests().await.unwrap();
    assert!(calls.iter().any(
        |r| r.body_json::<serde_json::Value>().unwrap()["reply_markup"]["force_reply"] == true
    ));
    f.routes.handle_reply(43, 99, "wrong chat").await;
    f.routes.handle_reply(42, 98, "wrong message").await;
    assert!(
        f.routes
            .store
            .find_by_id(&card.card_id)
            .await
            .unwrap()
            .unwrap()
            .closed_at
            .is_none()
    );
    f.routes.handle_reply(42, 99, "exact feedback").await;
    let next = f.card().await;
    f.routes.handle_reply(42, 99, "duplicate old reply").await;
    assert!(f.texts().await.last().unwrap().contains("stale"));
    f.action(42, "approve", &next).await;
    f.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resolver_failure_still_acknowledges_and_never_claims_success() {
    let mut f = Fixture::new().await;
    let mut card = f.card().await;
    // A restarted owner has not resumed this durable request: resolution must fail.
    let empty_owner = Arc::new(Engine::new(
        Arc::new(surge_acp::bridge::AcpBridge::spawn(8, 32).unwrap()),
        f.storage.clone(),
        Arc::new(WorktreeToolDispatcher::new(f._home.path().into())),
        EngineConfig::default(),
    ));
    f.routes.engine.engine = Arc::new(LocalEngineFacade::new(empty_owner));
    f.action(42, "approve", &card).await;
    assert!(
        f.texts()
            .await
            .last()
            .unwrap()
            .contains("could not be accepted")
    );
    let calls = f.server.received_requests().await.unwrap();
    assert!(calls[0].url.path().ends_with("AnswerCallbackQuery"));
    f.routes.engine.engine = Arc::new(LocalEngineFacade::new(f.engine.clone()));
    card.chat_id = 43;
    f.action(43, "approve", &card).await;
    assert!(f.texts().await.last().unwrap().contains("cannot act"));
    f.action(42, "approve", &card).await;
    f.finish().await;
}

#[derive(Default)]
struct InboxRecorder(std::sync::Mutex<Vec<(i64, String)>>);
#[async_trait::async_trait]
impl InboxCallbacks for InboxRecorder {
    async fn handle(&self, chat: i64, data: &str) -> surge_telegram::Result<String> {
        self.0.lock().unwrap().push((chat, data.to_owned()));
        Ok("Inbox action recorded.".into())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sole_update_stream_routes_inbox_and_correlated_feedback() {
    use surge_telegram::cockpit::run::{CockpitRuntime, drive_update_loop};
    let mut f = Fixture::new().await;
    let card = f.card().await;
    let inbox = Arc::new(InboxRecorder::default());
    let mut production = routes(&f.storage, f.engine.clone(), f.routes.bot.clone());
    production.inbox = Some(inbox.clone());
    let runtime = Arc::new(CockpitRuntime {
        dispatch_ctx: surge_telegram::CockpitCtx {
            emitter: surge_telegram::CardEmitter::new(
                f.routes.store.clone(),
                TeloxideTelegramApi {
                    bot: f.routes.bot.clone(),
                },
            ),
            admin_chat_id: 42,
        },
        snapshots: PersistenceSnapshots {
            storage: f.storage.clone(),
        },
        routes: production,
    });
    let callback = |id: u32, data: String| {
        serde_json::from_str::<teloxide::types::Update>(&json!({"update_id":id,"callback_query":{"id":format!("q{id}"),"from":{"id":42,"is_bot":false,"first_name":"Test"},"chat_instance":"fixture","message":{"message_id":10,"date":0,"chat":{"id":42,"type":"private"},"text":"card"},"data":data}}).to_string()).unwrap()
    };
    let reply=serde_json::from_str::<teloxide::types::Update>(&json!({"update_id":3,"message":{"message_id":100,"date":0,"chat":{"id":42,"type":"private"},"text":"feedback through sole stream","reply_to_message":{"message_id":99,"date":0,"chat":{"id":42,"type":"private"},"text":"feedback prompt"}}}).to_string()).unwrap();
    let first = callback(1, "inbox:start:fixture".into());
    assert!(
        matches!(first.kind, teloxide::types::UpdateKind::CallbackQuery(_)),
        "{first:?}"
    );
    assert!(
        matches!(reply.kind, teloxide::types::UpdateKind::Message(_)),
        "{reply:?}"
    );
    let updates = futures::stream::iter(vec![
        first,
        callback(2, format!("cockpit:edit:{}", card.card_id)),
        reply,
    ]);
    tokio::time::timeout(
        Duration::from_secs(3),
        drive_update_loop(runtime, updates, tokio_util::sync::CancellationToken::new()),
    )
    .await
    .unwrap();
    assert_eq!(
        *inbox.0.lock().unwrap(),
        vec![(42, "inbox:start:fixture".into())]
    );
    let current = f.card().await;
    let snapshot = f.storage.inspect_run(f.id).await.unwrap();
    let surge_persistence::runs::inspection::RunDatabaseInspection::Present { events } =
        snapshot.database
    else {
        panic!()
    };
    assert!(events.iter().any(|e|matches!(&e.payload.payload,EventPayload::HumanInputResolved{response,..} if response["comment"]=="feedback through sole stream")));
    f.action(42, "approve", &current).await;
    f.finish().await;
}

impl Fixture {
    fn fail_card_update(&self, column: &str) {
        let conn = self.storage.acquire_registry_conn().unwrap();
        conn.execute_batch(&format!("CREATE TRIGGER fixture_card_write_failure BEFORE UPDATE OF {column} ON telegram_cards BEGIN SELECT RAISE(FAIL, 'injected card write failure'); END;")).unwrap();
    }

    async fn terminal(&mut self) {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let event = self.tap.recv().await.unwrap();
                if matches!(
                    event.event.payload.payload,
                    EventPayload::RunCompleted { .. } | EventPayload::RunFailed { .. }
                ) {
                    break;
                }
            }
        })
        .await
        .unwrap();
    }

    async fn resolution_count(&self) -> usize {
        let snapshot = self.storage.inspect_run(self.id).await.unwrap();
        let surge_persistence::runs::inspection::RunDatabaseInspection::Present { events } =
            snapshot.database
        else {
            panic!()
        };
        events
            .iter()
            .filter(|event| {
                matches!(
                    event.payload.payload,
                    EventPayload::HumanInputResolved { .. }
                )
            })
            .count()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn accepted_callback_with_failed_card_close_never_says_decision_failed() {
    let mut f = Fixture::new().await;
    let card = f.card().await;
    f.fail_card_update("closed_at");
    f.action(42, "approve", &card).await;
    f.terminal().await;
    let result = f.texts().await.last().unwrap().clone();
    assert_eq!(f.resolution_count().await, 1);
    assert!(
        f.routes
            .store
            .find_by_id(&card.card_id)
            .await
            .unwrap()
            .unwrap()
            .closed_at
            .is_none()
    );
    f.action(42, "approve", &card).await;
    assert_eq!(f.resolution_count().await, 1);
    f.finish().await;
    assert!(
        result.contains("accepted")
            && result.contains("card")
            && result.contains("Do not submit again"),
        "wrong acceptance result: {result}"
    );
    assert!(!result.contains("could not be accepted"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn accepted_edit_with_failed_card_close_never_says_decision_failed() {
    let mut f = Fixture::new().await;
    let card = f.card().await;
    f.action(42, "edit", &card).await;
    f.fail_card_update("closed_at");
    f.routes
        .handle_reply(42, 99, "accepted edit despite close failure")
        .await;
    let current = f.card().await;
    let result = f.texts().await.last().unwrap().clone();
    assert_eq!(f.resolution_count().await, 1);
    f.routes.handle_reply(42, 99, "duplicate old prompt").await;
    assert_eq!(f.resolution_count().await, 1);
    assert!(
        f.routes
            .store
            .find_by_id(&current.card_id)
            .await
            .unwrap()
            .unwrap()
            .closed_at
            .is_none()
    );
    f.finish().await;
    assert!(
        result.contains("accepted")
            && result.contains("card")
            && result.contains("Do not submit again"),
        "wrong acceptance result: {result}"
    );
    assert!(!result.contains("could not be accepted"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn force_reply_correlation_survives_reopened_storage_and_routes() {
    let mut f = Fixture::new().await;
    let card = f.card().await;
    f.action(42, "edit", &card).await;
    let reopened = Storage::open(f._home.path()).await.unwrap();
    f.routes = routes(&reopened, f.engine.clone(), f.routes.bot.clone());
    assert_eq!(
        f.routes
            .store
            .find_by_id(&card.card_id)
            .await
            .unwrap()
            .unwrap()
            .pending_edit_prompt_message_id,
        Some(99)
    );
    f.routes
        .handle_reply(42, 99, "feedback after reopening storage")
        .await;
    let next = f.card().await;
    assert_eq!(f.resolution_count().await, 1);
    assert!(
        f.texts()
            .await
            .last()
            .unwrap()
            .contains("Feedback accepted")
    );
    f.action(42, "approve", &next).await;
    f.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reject_terminates_real_gate_after_foreign_paired_chat_is_denied() {
    let mut f = Fixture::new().await;
    let card = f.card().await;
    f.action(43, "reject", &card).await;
    assert_eq!(f.resolution_count().await, 0);
    assert!(f.texts().await.last().unwrap().contains("cannot act"));
    f.action(42, "reject", &card).await;
    f.terminal().await;
    assert_eq!(f.resolution_count().await, 1);
    assert!(
        f.texts()
            .await
            .last()
            .unwrap()
            .contains("Decision accepted")
    );
    assert!(matches!(
        f.finish().await,
        surge_orchestrator::engine::RunOutcome::Failed { .. }
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bot_api_failure_does_not_roll_back_accepted_engine_decision() {
    let mut f = Fixture::new().await;
    let card = f.card().await;
    Mock::given(wiremock::matchers::any())
        .respond_with(ResponseTemplate::new(500).set_body_json(
            json!({"ok":false,"error_code":500,"description":"injected API failure"}),
        ))
        .with_priority(1)
        .mount(&f.server)
        .await;
    f.action(42, "approve", &card).await;
    f.terminal().await;
    assert_eq!(f.resolution_count().await, 1);
    assert!(
        f.routes
            .store
            .find_by_id(&card.card_id)
            .await
            .unwrap()
            .unwrap()
            .closed_at
            .is_some()
    );
    let calls = f.server.received_requests().await.unwrap();
    assert!(calls[0].url.path().ends_with("AnswerCallbackQuery"));
    assert!(calls.iter().any(|r| r.url.path().ends_with("SendMessage")));
    f.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn force_reply_send_failure_grants_no_reply_authority() {
    let mut f = Fixture::new().await;
    let card = f.card().await;
    Mock::given(wiremock::matchers::path("/bottest/SendMessage"))
        .and(wiremock::matchers::body_partial_json(
            json!({"reply_markup":{"force_reply":true}}),
        ))
        .respond_with(ResponseTemplate::new(500).set_body_json(
            json!({"ok":false,"error_code":500,"description":"injected API failure"}),
        ))
        .with_priority(1)
        .mount(&f.server)
        .await;
    f.action(42, "edit", &card).await;
    assert_eq!(
        f.routes
            .store
            .find_by_id(&card.card_id)
            .await
            .unwrap()
            .unwrap()
            .pending_edit_prompt_message_id,
        None
    );
    f.routes.handle_reply(42, 99, "reply to failed send").await;
    assert_eq!(f.resolution_count().await, 0);
    f.action(42, "approve", &card).await;
    f.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn force_reply_sent_but_not_persisted_grants_no_reply_authority() {
    let mut f = Fixture::new().await;
    let card = f.card().await;
    f.fail_card_update("pending_edit_prompt_message_id");
    f.action(42, "edit", &card).await;
    let calls = f.server.received_requests().await.unwrap();
    assert!(calls.iter().any(
        |r| r.body_json::<serde_json::Value>().unwrap()["reply_markup"]["force_reply"] == true
    ));
    assert_eq!(
        f.routes
            .store
            .find_by_id(&card.card_id)
            .await
            .unwrap()
            .unwrap()
            .pending_edit_prompt_message_id,
        None
    );
    f.routes
        .handle_reply(42, 99, "reply to unrecorded prompt")
        .await;
    assert_eq!(f.resolution_count().await, 0);
    assert!(
        f.routes
            .store
            .find_by_id(&card.card_id)
            .await
            .unwrap()
            .unwrap()
            .closed_at
            .is_none()
    );
    f.action(42, "approve", &card).await;
    f.terminal().await;
    assert_eq!(f.resolution_count().await, 1);
    f.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn recovery_closes_exact_resolved_card_while_next_gate_stays_open() {
    let mut f = Fixture::new().await;
    let old = f.card().await;
    f.action(42, "edit", &old).await;
    f.fail_card_update("closed_at");
    f.routes.handle_reply(42, 99, "advance to next gate").await;
    let next = f.card().await;
    {
        let conn = f.storage.acquire_registry_conn().unwrap();
        conn.execute_batch("DROP TRIGGER fixture_card_write_failure")
            .unwrap();
    }
    surge_telegram::cockpit::reconcile_open_cards(
        &f.routes.store,
        &f.routes.snapshots,
        &TeloxideTelegramApi {
            bot: f.routes.bot.clone(),
        },
        1,
    )
    .await
    .unwrap();
    let old_closed = f
        .routes
        .store
        .find_by_id(&old.card_id)
        .await
        .unwrap()
        .unwrap()
        .closed_at
        .is_some();
    let next_open = f
        .routes
        .store
        .find_by_id(&next.card_id)
        .await
        .unwrap()
        .unwrap()
        .closed_at
        .is_none();
    f.finish().await;
    assert!(
        old_closed,
        "resolved request card remained open because run is still active"
    );
    assert!(next_open);
}
