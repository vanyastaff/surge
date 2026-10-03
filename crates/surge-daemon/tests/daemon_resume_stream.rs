//! Park/resume is a new live stream generation, including on the same connection.
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use surge_acp::bridge::{
    error::{BridgeError, CloseSessionError, OpenSessionError, ReplyToToolError, SendMessageError},
    event::{BridgeEvent, ToolResultPayload},
    facade::BridgeFacade,
    session::{MessageContent, SessionConfig, SessionState},
};
use surge_core::{
    RunId, SessionId,
    graph::Graph,
    keys::{EdgeKey, NodeKey, OutcomeKey},
    node::Node,
    run_event::EventPayload,
};
use surge_daemon::{
    ServerConfig, admission::AdmissionController, broadcast::BroadcastRegistry,
    tracked_run::TrackingContext,
};
use surge_orchestrator::{
    engine::{
        Engine, EngineConfig, EngineRunConfig, RunOutcome,
        daemon_facade::DaemonEngineFacade,
        facade::{EngineFacade, LocalEngineFacade},
        handle::EngineRunEvent,
        tools::worktree::WorktreeToolDispatcher,
    },
    profile_loader::{DiskProfileSet, ProfileRegistry},
};
use surge_persistence::runs::Storage;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

struct ParkOnce {
    sent: AtomicBool,
    events: broadcast::Sender<BridgeEvent>,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn another_client_discovers_resumed_run_before_it_finishes() {
    use surge_orchestrator::engine::ipc::GlobalDaemonEvent;

    let fixture = Fixture::new().await;
    let id = fixture.park(false).await;
    let observer = DaemonEngineFacade::connect(fixture.socket.clone())
        .await
        .unwrap();
    let mut events = observer.subscribe_global().await.unwrap();
    let resumed = fixture
        .client
        .resume_run(id, fixture.root.path().into())
        .await
        .unwrap();
    let first = tokio::time::timeout(Duration::from_secs(3), events.recv()).await;
    let completed = tokio::time::timeout(Duration::from_secs(3), resumed.completion)
        .await
        .unwrap()
        .unwrap();
    fixture.close().await;
    assert!(matches!(completed, RunOutcome::Completed { .. }));
    assert!(
        matches!(first, Ok(Ok(GlobalDaemonEvent::RunAccepted { run_id })) if run_id == id),
        "a separate UI client must discover a resumed run before its terminal event: {first:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn global_stream_closes_on_disconnect_while_facade_is_retained() {
    let fixture = Fixture::new().await;
    let mut events = fixture.client.subscribe_global().await.unwrap();
    fixture.shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            match events.recv().await {
                Err(broadcast::error::RecvError::Closed) => break,
                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {},
            }
        }
    })
    .await
    .expect("global stream must close without dropping the facade");
    assert!(fixture.client.subscribe_global().await.is_err());
    fixture.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn recovered_run_is_announced_before_completion() {
    use surge_orchestrator::engine::ipc::GlobalDaemonEvent;

    let fixture = Fixture::new().await;
    let id = fixture.park(false).await;
    let mut events = fixture.client.subscribe_global().await.unwrap();
    surge_daemon::server::resume_run_tracked(
        id,
        fixture.root.path().into(),
        fixture.facade.as_ref(),
        &fixture.tracking,
        &fixture.admission,
        &fixture.registry,
    )
    .await
    .unwrap();
    let first = tokio::time::timeout(Duration::from_secs(3), events.recv())
        .await
        .unwrap()
        .unwrap();
    let second = tokio::time::timeout(Duration::from_secs(3), events.recv())
        .await
        .unwrap()
        .unwrap();
    fixture.close().await;
    assert!(matches!(first, GlobalDaemonEvent::RunAccepted { run_id } if run_id == id));
    assert!(matches!(second, GlobalDaemonEvent::RunFinished {
        run_id, outcome: RunOutcome::Completed { .. }
    } if run_id == id));
}
#[async_trait::async_trait]
impl BridgeFacade for ParkOnce {
    fn legacy_stage_event_adapter(&self) -> bool {
        true
    }
    async fn open_session(
        &self,
        config: SessionConfig,
    ) -> Result<surge_core::execution_recovery::OpenedSession, OpenSessionError> {
        Ok(surge_core::execution_recovery::OpenedSession::new(
            SessionId::new(),
            surge_core::execution_recovery::ProviderSessionDescriptor::new(
                surge_core::execution_recovery::ProviderSessionId::new("park-once".into()).unwrap(),
                config.invocation,
                config.runtime,
                surge_core::ContentHash::compute(b"park-once"),
                config.working_dir,
                Default::default(),
            )
            .unwrap(),
            surge_core::execution_recovery::SessionOpenMode::New,
        )
        .unwrap())
    }
    async fn send_message(
        &self,
        session: SessionId,
        _: MessageContent,
    ) -> Result<(), SendMessageError> {
        if !self.sent.swap(true, Ordering::SeqCst) {
            return Err(SendMessageError::RateLimited {
                retry_after: None,
                details: "park fixture".into(),
            });
        }
        self.events
            .send(BridgeEvent::OutcomeReported {
                session,
                outcome: OutcomeKey::try_new("done").unwrap(),
                summary: "resumed".into(),
                artifacts_produced: vec![],

                verification_report: None,
            })
            .unwrap();
        Ok(())
    }
    async fn session_state(&self, _: SessionId) -> Result<SessionState, BridgeError> {
        Err(BridgeError::WorkerDead)
    }
    async fn close_session(&self, _: SessionId) -> Result<(), CloseSessionError> {
        Ok(())
    }
    async fn reply_to_tool(
        &self,
        _: SessionId,
        _: String,
        _: ToolResultPayload,
    ) -> Result<(), ReplyToToolError> {
        panic!("no tool calls")
    }
    async fn reply_to_permission(
        &self,
        _: SessionId,
        _: String,
        _: surge_acp::bridge::RequestPermissionResponse,
    ) -> Result<(), surge_acp::bridge::ReplyToPermissionError> {
        panic!("no permissions")
    }
    fn subscribe(&self) -> broadcast::Receiver<BridgeEvent> {
        self.events.subscribe()
    }
}

struct Fixture {
    root: tempfile::TempDir,
    client: DaemonEngineFacade,
    admission: Arc<AdmissionController>,
    shutdown: CancellationToken,
    server: tokio::task::JoinHandle<Result<(), surge_daemon::DaemonError>>,
    socket: PathBuf,
    facade: Arc<dyn EngineFacade>,
    tracking: TrackingContext,
    registry: Arc<BroadcastRegistry>,
}

impl Fixture {
    async fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let profiles = root.path().join("profiles");
        std::fs::create_dir(&profiles).unwrap();
        std::fs::write(
            profiles.join("resume-test-1.0.toml"),
            r#"
schema_version = 1
[role]
id = "resume-test"
version = "1.0.0"
display_name = "Resume test"
category = "agents"
description = "Fixture"
when_to_use = "Tests"
[runtime]
recommended_model = "test-model"
agent_id = "claude-code"
[[outcomes]]
id = "done"
description = "Done"
edge_kind_hint = "forward"
[prompt]
system = "test"
"#,
        )
        .unwrap();
        let storage = Storage::open(root.path()).await.unwrap();
        let bridge = Arc::new(ParkOnce {
            sent: AtomicBool::new(false),
            events: broadcast::channel(32).0,
        });
        let engine = Arc::new(Engine::new_full(
            bridge,
            storage.clone(),
            Arc::new(WorktreeToolDispatcher::new(root.path().into())),
            Arc::new(surge_notify::MultiplexingNotifier::new()),
            None,
            Some(Arc::new(ProfileRegistry::new(
                DiskProfileSet::scan(&profiles).unwrap(),
            ))),
            EngineConfig::default(),
        ));
        let facade: Arc<dyn EngineFacade> = Arc::new(LocalEngineFacade::new(engine.clone()));
        let admission = Arc::new(AdmissionController::new(1, 1));
        let shutdown = CancellationToken::new();
        let socket = root.path().join("resume.sock");
        let tracking = TrackingContext::new(engine, storage);
        let registry = Arc::new(BroadcastRegistry::new());
        let server = tokio::spawn(surge_daemon::run_runs_only(
            ServerConfig {
                socket_path: socket.clone(),
                max_active: 1,
                max_queue: 1,
            },
            facade.clone(),
            tracking.clone(),
            registry.clone(),
            admission.clone(),
            shutdown.clone(),
        ));
        let client = tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if let Ok(client) = DaemonEngineFacade::connect(socket.clone()).await {
                    break client;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        Self {
            root,
            client,
            admission,
            shutdown,
            server,
            socket,
            facade,
            tracking,
            registry,
        }
    }
    async fn park(&self, gate: bool) -> RunId {
        let id = RunId::new();
        let handle = self
            .client
            .start_run(
                id,
                graph(gate),
                self.root.path().into(),
                // The example's agent step binds `spec` from the initial prompt.
                EngineRunConfig {
                    initial_prompt: "Resume fixture".into(),
                    ..EngineRunConfig::default()
                },
            )
            .await
            .unwrap();
        let outcome = tokio::time::timeout(Duration::from_secs(3), handle.completion)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(outcome, RunOutcome::Parked { .. }), "{outcome:?}");
        tokio::time::timeout(Duration::from_secs(3), async {
            while self.admission.snapshot().await.active != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        id
    }
    async fn close(self) {
        self.shutdown.cancel();
        self.server.await.unwrap().unwrap();
    }
}

fn graph(gate: bool) -> Graph {
    let mut graph: Graph =
        toml::from_str(include_str!("../../../examples/flow_minimal_agent.toml")).unwrap();
    let surge_core::node::NodeConfig::Agent(agent) =
        &mut graph.nodes.get_mut(&graph.start).unwrap().config
    else {
        panic!("agent")
    };
    agent.profile = "resume-test@1.0".parse().unwrap();
    if !gate {
        return graph;
    }
    let node: Node = toml::from_str(
        r#"
id = "gate"
[[declared_outcomes]]
id = "approve"
description = "Approve"
edge_kind_hint = "forward"
is_terminal = false
[config]
node_kind = "human_gate"
delivery_channels = []
mode = { bootstrap = { stage = "description" } }
[config.summary]
title = "Resume approval"
body = "Fresh request"
[[config.options]]
outcome = "approve"
label = "Approve"
"#,
    )
    .unwrap();
    graph.nodes.insert(node.id.clone(), node);
    graph.edges[0].to = NodeKey::try_new("gate").unwrap();
    let mut edge = graph.edges[0].clone();
    edge.id = EdgeKey::try_new("gate_to_end").unwrap();
    edge.from.node = NodeKey::try_new("gate").unwrap();
    edge.from.outcome = OutcomeKey::try_new("approve").unwrap();
    edge.to = NodeKey::try_new("end").unwrap();
    graph.edges.push(edge);
    graph
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn same_connection_park_resume_replaces_finished_forwarder_and_delivers_gate() {
    let fixture = Fixture::new().await;
    let id = fixture.park(true).await;
    let mut resumed = fixture
        .client
        .resume_run(id, fixture.root.path().into())
        .await
        .unwrap();
    let delivered = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let EngineRunEvent::Persisted { payload, .. } = resumed.events.recv().await.unwrap()
                && let EventPayload::HumanInputRequested { node, call_id, .. } = payload.as_ref()
            {
                break (node.clone(), call_id.clone());
            }
        }
    })
    .await;
    if delivered.is_err() {
        fixture
            .client
            .stop_run(id, "RED cleanup".into())
            .await
            .unwrap();
        fixture.close().await;
        resumed.completion.abort();
        let _ = resumed.completion.await;
        panic!("resumed fresh gate was lost behind stale connection forwarder");
    }
    let (node, call_id) = delivered.unwrap();
    fixture
        .client
        .resolve_requested_input(id, node, call_id, serde_json::json!({"outcome":"approve"}))
        .await
        .unwrap();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(3), resumed.completion)
            .await
            .unwrap()
            .unwrap(),
        RunOutcome::Completed { .. }
    ));
    fixture.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fresh_connection_receives_fast_resume_before_followup_subscribe() {
    use interprocess::local_socket::tokio::prelude::*;
    use surge_orchestrator::engine::ipc::{
        DaemonEvent, DaemonRequest, InboundServerFrame, local_socket_name_from_path,
        read_inbound_server_frame, write_frame,
    };
    let fixture = Fixture::new().await;
    let id = fixture.park(false).await;
    let stream = LocalSocketStream::connect(local_socket_name_from_path(&fixture.socket).unwrap())
        .await
        .unwrap();
    let (read, mut write) = stream.split();
    let mut read = tokio::io::BufReader::new(read);
    write_frame(
        &mut write,
        &DaemonRequest::ResumeRun {
            request_id: 1,
            run_id: id,
            worktree_path: fixture.root.path().into(),
        },
    )
    .await
    .unwrap();
    // Deliberately withhold Subscribe: ResumeRun itself must establish its live stream.
    let observed = tokio::time::timeout(Duration::from_secs(2), async {
        let mut persisted_completion = false;
        loop {
            let frame = read_inbound_server_frame(&mut read).await.unwrap().unwrap();
            let InboundServerFrame::Event(event) = frame else {
                continue;
            };
            let DaemonEvent::PerRun { event, .. } = *event else {
                continue;
            };
            match *event {
                EngineRunEvent::Persisted { payload, .. } => {
                    persisted_completion |= matches!(*payload, EventPayload::RunCompleted { .. });
                },
                EngineRunEvent::Terminal { outcome } => {
                    assert!(persisted_completion);
                    assert!(matches!(outcome, RunOutcome::Completed { .. }));
                    break;
                },
                _ => {},
            }
        }
    })
    .await;
    fixture.close().await;
    assert!(
        observed.is_ok(),
        "fresh resumed stream dropped early durable completion before Subscribe"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejected_resume_removes_preattached_forwarder_and_releases_admission() {
    let fixture = Fixture::new().await;
    let mut events = fixture.client.subscribe_global().await.unwrap();
    let unknown = RunId::new();
    assert!(
        fixture
            .client
            .resume_run(unknown, fixture.root.path().into())
            .await
            .is_err()
    );
    assert_eq!(fixture.admission.snapshot().await.active, 0);
    // An orphaned forwarder would make Subscribe incorrectly return success.
    assert!(fixture.client.subscribe_to_run(unknown).await.is_err());
    assert!(
        tokio::time::timeout(Duration::from_millis(100), events.recv())
            .await
            .is_err(),
        "rejected resume must not announce activity"
    );
    fixture.park(false).await;
    fixture.close().await;
}
