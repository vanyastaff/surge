//! Real Engine events must reach IPC before gate approval and before terminal closure.
use std::{path::PathBuf, sync::Arc, time::Duration};
use surge_acp::bridge::{
    error::{BridgeError, CloseSessionError, OpenSessionError, ReplyToToolError, SendMessageError},
    event::{BridgeEvent, ToolResultPayload},
    facade::BridgeFacade,
    session::{MessageContent, SessionConfig, SessionState},
};
use surge_core::{RunId, SessionId, run_event::EventPayload};
use surge_daemon::{ServerConfig, admission::AdmissionController, broadcast::BroadcastRegistry};
use surge_orchestrator::engine::{
    Engine, EngineConfig, EngineRunConfig, RunOutcome,
    daemon_facade::DaemonEngineFacade,
    facade::{EngineFacade, LocalEngineFacade},
    handle::EngineRunEvent,
    tools::worktree::WorktreeToolDispatcher,
};
use surge_persistence::runs::{EventSeq, Storage};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

struct NoAgent(broadcast::Sender<BridgeEvent>);
#[async_trait::async_trait]
impl BridgeFacade for NoAgent {
    async fn open_session(
        &self,
        _: SessionConfig,
    ) -> Result<surge_core::execution_recovery::OpenedSession, OpenSessionError> {
        panic!("gate must not use an agent")
    }
    async fn send_message(&self, _: SessionId, _: MessageContent) -> Result<(), SendMessageError> {
        panic!("gate must not use an agent")
    }
    async fn session_state(&self, _: SessionId) -> Result<SessionState, BridgeError> {
        panic!("gate must not use an agent")
    }
    async fn close_session(&self, _: SessionId) -> Result<(), CloseSessionError> {
        panic!("gate must not use an agent")
    }
    async fn reply_to_tool(
        &self,
        _: SessionId,
        _: String,
        _: ToolResultPayload,
    ) -> Result<(), ReplyToToolError> {
        panic!("gate must not use an agent")
    }
    async fn reply_to_permission(
        &self,
        _: SessionId,
        _: String,
        _: surge_acp::bridge::RequestPermissionResponse,
    ) -> Result<(), surge_acp::bridge::ReplyToPermissionError> {
        panic!("gate must not use an agent")
    }
    fn subscribe(&self) -> broadcast::Receiver<BridgeEvent> {
        self.0.subscribe()
    }
}

const FLOW: &str = r#"# Smallest possible flow.toml — single Terminal node.
#
# When run via `surge engine run examples/flow_terminal_only.toml`,
# the run starts, immediately hits the Terminal node, emits
# RunCompleted, and exits cleanly — no agent / no MCP / no human
# input required. The fastest possible demo of the pipeline +
# persistence + IPC.

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
[[edges]]
id = "approved"
to = "end"
kind = "forward"
[edges.from]
node = "gate"
outcome = "approve"
[edges.policy]
on_max_exceeded = "escalate"
"#;

async fn connect(path: PathBuf) -> DaemonEngineFacade {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Ok(client) = DaemonEngineFacade::connect(path.clone()).await {
                break client;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_gate_request_and_resolution_arrive_before_terminal_and_slot_release() {
    let root = tempfile::tempdir().unwrap();
    let storage = Storage::open(root.path()).await.unwrap();
    let engine = Arc::new(Engine::new(
        Arc::new(NoAgent(broadcast::channel(8).0)),
        storage.clone(),
        Arc::new(WorktreeToolDispatcher::new(root.path().to_path_buf())),
        EngineConfig::default(),
    ));
    let facade: Arc<dyn EngineFacade> = Arc::new(LocalEngineFacade::new(engine.clone()));
    let admission = Arc::new(AdmissionController::new(1, 1));
    let broadcast = Arc::new(BroadcastRegistry::new());
    let shutdown = CancellationToken::new();
    let socket = root.path().join("gate.sock");
    let server = tokio::spawn(surge_daemon::run_runs_only(
        ServerConfig {
            socket_path: socket.clone(),
            max_active: 1,
            max_queue: 1,
        },
        facade,
        surge_daemon::tracked_run::TrackingContext::new(engine.clone(), storage.clone()),
        broadcast,
        admission.clone(),
        shutdown.clone(),
    ));
    let client = connect(socket).await;
    let id = RunId::new();
    // Exercise the real client's StartRun/Subscribe ordering, no manual pre-subscribe.
    let mut handle = client
        .start_run(
            id,
            toml::from_str(FLOW).unwrap(),
            root.path().to_path_buf(),
            EngineRunConfig::default(),
        )
        .await
        .unwrap();
    let mut seen = Vec::new();
    let requested = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let EngineRunEvent::Persisted { seq, payload } = handle.events.recv().await.unwrap()
            else {
                continue;
            };
            seen.push((seq, payload.discriminant_str().to_string()));
            if let EventPayload::HumanInputRequested { node, call_id, .. } = payload.as_ref() {
                break (node.clone(), call_id.clone());
            }
        }
    })
    .await;
    if requested.is_err() {
        // Clean up the real run even on RED; no detached pending gate is acceptable.
        engine.stop_run(id, "test cleanup".into()).await.unwrap();
        tokio::time::timeout(Duration::from_secs(3), handle.completion)
            .await
            .unwrap()
            .unwrap();
        shutdown.cancel();
        tokio::time::timeout(Duration::from_secs(3), server)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        panic!("durably persisted HumanInputRequested never reached the IPC client");
    }
    let (node, call_id) = requested.unwrap();
    client
        .resolve_requested_input(id, node, call_id, serde_json::json!({"outcome":"approve"}))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            match handle.events.recv().await.unwrap() {
                EngineRunEvent::Persisted { seq, payload } => {
                    seen.push((seq, payload.discriminant_str().to_string()))
                },
                EngineRunEvent::Terminal { outcome } => {
                    assert!(matches!(outcome, RunOutcome::Completed { .. }));
                    break;
                },
                _ => {},
            }
        }
    })
    .await
    .unwrap();
    assert!(matches!(
        handle.completion.await.unwrap(),
        RunOutcome::Completed { .. }
    ));
    let durable = storage
        .open_run_reader(id)
        .await
        .unwrap()
        .read_events(EventSeq(0)..EventSeq(u64::MAX))
        .await
        .unwrap();
    let expected: Vec<_> = durable
        .iter()
        .map(|event| {
            (
                event.seq.0,
                event.payload.payload.discriminant_str().to_string(),
            )
        })
        .collect();
    assert_eq!(
        seen, expected,
        "every persisted event must precede terminal publication exactly once"
    );
    assert!(seen.iter().any(|(_, kind)| kind == "HumanInputResolved"));
    assert_eq!(seen.last().unwrap().1, "RunCompleted");
    tokio::time::timeout(Duration::from_secs(3), async {
        while admission.snapshot().await.active != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(3), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unreadable_run_stream_fails_waiter_but_another_run_stays_connected() {
    let root = tempfile::tempdir().unwrap();
    let storage = Storage::open(root.path()).await.unwrap();
    let engine = Arc::new(Engine::new(
        Arc::new(NoAgent(broadcast::channel(8).0)),
        storage.clone(),
        Arc::new(WorktreeToolDispatcher::new(root.path().to_path_buf())),
        EngineConfig::default(),
    ));
    let facade: Arc<dyn EngineFacade> = Arc::new(LocalEngineFacade::new(engine.clone()));
    let admission = Arc::new(AdmissionController::new(2, 1));
    let shutdown = CancellationToken::new();
    let socket = root.path().join("failure.sock");
    let server = tokio::spawn(surge_daemon::run_runs_only(
        ServerConfig {
            socket_path: socket.clone(),
            max_active: 2,
            max_queue: 1,
        },
        facade,
        surge_daemon::tracked_run::TrackingContext::new(engine.clone(), storage.clone()),
        Arc::new(BroadcastRegistry::new()),
        admission.clone(),
        shutdown.clone(),
    ));
    let client = connect(socket).await;
    let first_id = RunId::new();
    let second_id = RunId::new();
    let mut first = client
        .start_run(
            first_id,
            toml::from_str(FLOW).unwrap(),
            root.path().into(),
            EngineRunConfig::default(),
        )
        .await
        .unwrap();
    let mut second = client
        .start_run(
            second_id,
            toml::from_str(FLOW).unwrap(),
            root.path().into(),
            EngineRunConfig::default(),
        )
        .await
        .unwrap();
    wait_for_gate(&mut first.events).await;
    let (node, call_id) = wait_for_gate(&mut second.events).await;
    // Publish an unread, malformed journal row atomically, preserving the live
    // database and append-only triggers. Copy metadata from its actual gate.
    let mut connection = rusqlite::Connection::open(
        root.path()
            .join("runs")
            .join(first_id.to_string())
            .join("events.sqlite"),
    )
    .unwrap();
    let transaction = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .unwrap();
    let watermark: u64 = transaction
        .query_row("SELECT MAX(seq) FROM events", [], |row| row.get(0))
        .unwrap();
    let sequence = watermark.checked_add(1).unwrap();
    assert_eq!(
        transaction
            .execute(
                "INSERT INTO events (seq, timestamp, kind, payload, schema_version) \
                 SELECT ?1, timestamp, kind, ?2, schema_version FROM events \
                 WHERE seq=(SELECT seq FROM events WHERE kind='HumanInputRequested' ORDER BY seq LIMIT 1)",
                rusqlite::params![sequence, b"malformed fixture event".as_slice()],
            )
            .unwrap(),
        1,
        "fault injection must append exactly one unread row using actual gate metadata"
    );
    transaction.commit().unwrap();
    let scoped = storage
        .inspect_events_after(
            first_id,
            EventSeq(watermark),
            std::num::NonZeroU32::new(1).unwrap(),
        )
        .await
        .unwrap_err();
    let full = storage.inspect_run(first_id).await.unwrap_err();
    for inspection in [scoped, full] {
        assert!(
            matches!(
                &inspection,
                surge_persistence::runs::StorageError::MigrationFailed(reason)
                    if reason.contains(&format!("seq={sequence}:"))
                        && reason.contains("payload decode failed")
            ),
            "fixture must fail unread event decoding rather than SQL access: {inspection}"
        );
    }
    tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            match first.events.recv().await.unwrap() {
                EngineRunEvent::StreamError { .. } => break,
                EngineRunEvent::Terminal { .. } => {
                    panic!("unconfirmed stream invented terminal outcome")
                },
                _ => {},
            }
        }
    })
    .await
    .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(2), first.completion)
            .await
            .unwrap()
            .is_err()
    );
    client
        .resolve_requested_input(
            second_id,
            node,
            call_id,
            serde_json::json!({"outcome":"approve"}),
        )
        .await
        .unwrap();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(3), second.completion)
            .await
            .unwrap()
            .unwrap(),
        RunOutcome::Completed { .. }
    ));
    tokio::time::timeout(Duration::from_secs(3), async {
        while admission.snapshot().await.active != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    shutdown.cancel();
    server.await.unwrap().unwrap();
}

async fn wait_for_gate(
    events: &mut broadcast::Receiver<EngineRunEvent>,
) -> (surge_core::keys::NodeKey, Option<String>) {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let EngineRunEvent::Persisted { payload, .. } = events.recv().await.unwrap()
                && let EventPayload::HumanInputRequested { node, call_id, .. } = payload.as_ref()
            {
                break (node.clone(), call_id.clone());
            }
        }
    })
    .await
    .unwrap()
}
