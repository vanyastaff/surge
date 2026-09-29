use super::*;
use surge_acp::bridge::{
    error::{BridgeError, CloseSessionError, OpenSessionError, ReplyToToolError, SendMessageError},
    event::{BridgeEvent, ToolResultPayload},
    facade::BridgeFacade,
    session::{MessageContent, SessionConfig, SessionState},
};
use surge_core::{SessionId, keys::OutcomeKey, run_event::EventPayload};
use surge_orchestrator::{
    engine::{EngineConfig, facade::LocalEngineFacade, tools::worktree::WorktreeToolDispatcher},
    profile_loader::{DiskProfileSet, ProfileRegistry},
};
use tokio::sync::broadcast;

const DESCRIPTION: &str = "## Goal\nBuild the app.\n## Context\nExisting project.\n## Requirements\nTest behavior.\n## Out of Scope\nDeployment.\n";
const ROADMAP: &str = "## Milestones\n1. Complete app.\n## Dependencies\nNone.\n## Risks\nNone.\n";
const ROADMAP_TOML: &str = r#"schema_version = 2
[[milestones]]
id = "app"
title = "App"
[[milestones.tasks]]
id = "build"
title = "Build app"
size = "s"
acceptance_criteria = ["App works"]
"#;
// Generated flows must declare their archetype (ADR 0005); the example is a
// hand-authored template, so the fixture appends the block itself.
const FLOW: &str = concat!(
    include_str!("../../../examples/flow_terminal_only.toml"),
    "\n[metadata.archetype]\nname = \"single-task\"\n"
);

struct AuthorBridge {
    sessions: Mutex<HashMap<SessionId, (std::path::PathBuf, usize)>>,
    opened: std::sync::atomic::AtomicUsize,
    events: broadcast::Sender<BridgeEvent>,
}
impl AuthorBridge {
    fn new() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            opened: std::sync::atomic::AtomicUsize::new(0),
            events: broadcast::channel(64).0,
        }
    }
}
#[async_trait::async_trait]
impl BridgeFacade for AuthorBridge {
    fn legacy_stage_event_adapter(&self) -> bool {
        true
    }
    async fn open_session(&self, config: SessionConfig) -> Result<SessionId, OpenSessionError> {
        let session = SessionId::new();
        let index = self
            .opened
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.sessions
            .lock()
            .await
            .insert(session, (config.working_dir, index));
        Ok(session)
    }
    async fn send_message(
        &self,
        session: SessionId,
        _: MessageContent,
    ) -> Result<(), SendMessageError> {
        let (path, index) = self.sessions.lock().await[&session].clone();
        let (name, content) = match index {
            0 => ("description.md", DESCRIPTION),
            1 => ("roadmap.md", ROADMAP),
            2 => ("flow.toml", FLOW),
            _ => panic!("unexpected extra planning agent"),
        };
        tokio::fs::write(path.join(name), content).await.unwrap();
        let artifacts_produced = if index == 1 {
            tokio::fs::write(path.join("roadmap.toml"), ROADMAP_TOML)
                .await
                .unwrap();
            vec!["roadmap.toml".into(), "roadmap.md".into()]
        } else {
            vec![name.into()]
        };
        self.events
            .send(BridgeEvent::OutcomeReported {
                session,
                outcome: OutcomeKey::try_new("drafted").unwrap(),
                summary: "fixture".into(),
                artifacts_produced,
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
        panic!("no tool")
    }
    async fn reply_to_permission(
        &self,
        _: SessionId,
        _: String,
        _: surge_acp::bridge::RequestPermissionResponse,
    ) -> Result<(), surge_acp::bridge::ReplyToPermissionError> {
        panic!("no permission")
    }
    fn subscribe(&self) -> broadcast::Receiver<BridgeEvent> {
        self.events.subscribe()
    }
}

struct Fixture {
    root: tempfile::TempDir,
    project: std::path::PathBuf,
    owner: Arc<BootstrapSupervisor>,
    engine: Arc<Engine>,
    storage: Arc<Storage>,
}
fn git(path: &std::path::Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
impl Fixture {
    async fn new() -> Self {
        Self::with_capacity(1, 0).await
    }
    async fn with_capacity(max_active: usize, max_queue: usize) -> Self {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("project");
        std::fs::create_dir(&project).unwrap();
        std::fs::write(project.join("surge.toml"), "schema_version = 1\n").unwrap();
        git(&project, &["init"]);
        git(&project, &["config", "user.name", "Test"]);
        git(&project, &["config", "user.email", "test@example.invalid"]);
        git(&project, &["add", "."]);
        git(&project, &["commit", "-m", "base"]);
        let project = project.canonicalize().unwrap();
        let config = Arc::new(surge_core::SurgeConfig::load(&project.join("surge.toml")).unwrap());
        let profiles_root = root.path().join("profiles");
        std::fs::create_dir(&profiles_root).unwrap();
        // Exercise daemon contract validation without depending on a separately built CLI hook binary.
        for mut profile in surge_core::profile::bundled::BundledRegistry::all() {
            if ["description-author", "roadmap-planner", "flow-generator"]
                .contains(&profile.role.id.as_str())
            {
                profile.hooks = surge_core::profile::ProfileHooks::default();
                std::fs::write(
                    profiles_root.join(format!("{}.toml", profile.role.id)),
                    toml::to_string(&profile).unwrap(),
                )
                .unwrap();
            }
        }
        let profiles = Arc::new(ProfileRegistry::new(
            DiskProfileSet::scan(&profiles_root).unwrap(),
        ));
        let agents = Arc::new(surge_acp::Registry::for_run(&config));
        let storage = Storage::open(root.path().join("home")).await.unwrap();
        let engine = Arc::new(Engine::new_full(
            Arc::new(AuthorBridge::new()),
            storage.clone(),
            Arc::new(WorktreeToolDispatcher::new(root.path().into())),
            Arc::new(surge_notify::MultiplexingNotifier::new()),
            None,
            Some(profiles.clone()),
            EngineConfig {
                agent_registry: Some(agents.clone()),
                capacity: (&config.capacity).into(),
                memory_store_path: Some(root.path().join("memory.db")),
                ..Default::default()
            },
        ));
        let runtime = BootstrapRuntime::new(
            config,
            profiles,
            agents,
            root.path().canonicalize().unwrap().join("worktrees"),
            profiles_root,
        )
        .unwrap();
        let owner = BootstrapSupervisor::new(BootstrapServices {
            engine: engine.clone(),
            facade: Arc::new(LocalEngineFacade::new(engine.clone())),
            storage: storage.clone(),
            runtime: Some(runtime),
            admission: Arc::new(AdmissionController::new(max_active, max_queue)),
            broadcast: Arc::new(BroadcastRegistry::new()),
            shutdown: CancellationToken::new(),
        });
        Self {
            root,
            project,
            owner,
            engine,
            storage,
        }
    }
    async fn reopened_owner(&self) -> Arc<BootstrapSupervisor> {
        let capacity = self.owner.services.admission.snapshot().await;
        let storage = Storage::open(self.storage.home()).await.unwrap();
        let config =
            Arc::new(surge_core::SurgeConfig::load(&self.project.join("surge.toml")).unwrap());
        let profiles = Arc::new(ProfileRegistry::new(
            DiskProfileSet::scan(&self.root.path().join("profiles")).unwrap(),
        ));
        let agents = Arc::new(surge_acp::Registry::for_run(&config));
        let engine = Arc::new(Engine::new_full(
            Arc::new(AuthorBridge::new()),
            storage.clone(),
            Arc::new(WorktreeToolDispatcher::new(self.root.path().into())),
            Arc::new(surge_notify::MultiplexingNotifier::new()),
            None,
            Some(profiles),
            EngineConfig {
                agent_registry: Some(agents),
                capacity: (&config.capacity).into(),
                memory_store_path: Some(self.root.path().join("restarted-memory.db")),
                ..Default::default()
            },
        ));
        BootstrapSupervisor::new(BootstrapServices {
            engine: engine.clone(),
            facade: Arc::new(LocalEngineFacade::new(engine)),
            storage: storage.clone(),
            runtime: self.owner.services.runtime.clone(),
            admission: Arc::new(AdmissionController::new(
                capacity.max_active,
                capacity.max_queue,
            )),
            broadcast: Arc::new(BroadcastRegistry::new()),
            shutdown: CancellationToken::new(),
        })
    }
    async fn started_writer(
        &self,
        launch: &ExpectedBootstrapRun,
    ) -> surge_persistence::runs::RunWriter {
        let graph = self.owner.services.runtime.as_ref().unwrap().graph();
        let writer = self
            .storage
            .create_run(launch.run_id, launch.worktree.to_str().unwrap(), None)
            .await
            .unwrap();
        for payload in [
            EventPayload::RunStarted {
                pipeline_template: None,
                project_path: launch.worktree.clone(),
                initial_prompt: launch.initial_prompt.clone(),
                config: launch.config.clone(),
            },
            EventPayload::PipelineMaterialized {
                graph: Box::new(graph),
                graph_hash: launch.graph_hash,
            },
        ] {
            writer
                .append_event(surge_core::VersionedEventPayload::new(payload))
                .await
                .unwrap();
        }
        writer.flush().await.unwrap();
        writer
    }
    fn intent(&self) -> BootstrapIntent {
        BootstrapIntent::new(
            self.project.clone(),
            " exact prompt\ncreate an app ".into(),
            surge_core::budget::BudgetGuard::default(),
        )
        .unwrap()
    }
    fn approve(&self) -> tokio::task::JoinHandle<()> {
        let mut tap = self.engine.subscribe_tap();
        let engine = self.engine.clone();
        tokio::spawn(async move {
            while let Ok(event) = tap.recv().await {
                if let EventPayload::HumanInputRequested { node, call_id, .. } =
                    event.event.payload.payload
                {
                    engine
                        .resolve_requested_input(
                            event.run_id,
                            node,
                            call_id,
                            serde_json::json!({"outcome":"approve"}),
                        )
                        .await
                        .unwrap();
                }
            }
        })
    }
    async fn terminal(&self, id: RunId) -> BootstrapOperationStatus {
        let result = tokio::time::timeout(Duration::from_secs(8), async {
            loop {
                let status = self.owner.status(id).unwrap();
                if status.state.phase().is_none()
                    || matches!(status.state, State::NeedsAttention { .. })
                {
                    return status;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await;
        if let Ok(status) = result {
            return status;
        }
        let record = self.owner.record(id).unwrap();
        let events = self
            .storage
            .inspect_run(record.status.planning_run)
            .await
            .unwrap();
        panic!(
            "operation did not settle: {:?}; events: {:?}",
            record.status, events.database
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_planning_continues_to_child_with_one_admission_slot_and_isolation() {
    let fixture = Fixture::new().await;
    let approvals = fixture.approve();
    let id = RunId::new();
    let accepted = fixture.owner.submit(id, &fixture.intent()).await.unwrap();
    let supervisor = tokio::spawn(fixture.owner.clone().run());
    let terminal = fixture.terminal(id).await;
    fixture.owner.services.shutdown.cancel();
    supervisor.await.unwrap();
    approvals.abort();
    if terminal.state != State::Completed {
        let inspection = fixture
            .storage
            .inspect_run(accepted.planning_run)
            .await
            .unwrap();
        if let surge_persistence::runs::inspection::RunDatabaseInspection::Present { events } =
            inspection.database
        {
            let failures: Vec<_> = events
                .iter()
                .filter(|event| {
                    matches!(
                        event.payload.payload,
                        EventPayload::RunFailed { .. } | EventPayload::StageFailed { .. }
                    )
                })
                .collect();
            panic!("{terminal:?}: {failures:?}");
        }
    }
    assert_eq!(terminal.state, State::Completed, "{terminal:?}");
    assert!(
        matches!(terminal.result, Some(BootstrapTerminal::Completed { run_id, .. }) if run_id == accepted.implementation_run)
    );
    assert_eq!(fixture.owner.services.admission.snapshot().await.active, 0);
    let record = fixture.owner.record(id).unwrap();
    let BootstrapStoredPayload::V1 { capture, .. } = record.payload else {
        panic!("v1")
    };
    assert_ne!(capture.fields().planning_worktree, fixture.project);
    assert_ne!(capture.fields().implementation_worktree, fixture.project);
    assert!(!fixture.project.join("description.md").exists());
    assert_eq!(
        std::fs::read_to_string(
            capture
                .fields()
                .implementation_worktree
                .join("description.md")
        )
        .unwrap(),
        DESCRIPTION
    );
    assert!(fixture.root.path().join("home").exists());
    assert!(
        fixture
            .storage
            .inspect_run(accepted.implementation_run)
            .await
            .unwrap()
            .registry
            .is_some()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn inactive_started_and_parked_cancel_once_and_reopen_as_cancelled() {
    check_inactive_cancel(false).await;
    check_inactive_cancel(true).await;
}

async fn check_inactive_cancel(parked: bool) {
    use surge_core::{RunStatus, VersionedEventPayload};
    use surge_persistence::runs::inspection::RunDatabaseInspection;
    let fixture = Fixture::new().await;
    let id = RunId::new();
    fixture.owner.submit(id, &fixture.intent()).await.unwrap();
    let record = fixture.owner.record(id).unwrap();
    let launch = expected(&record).unwrap();
    let writer = fixture.started_writer(&launch).await;
    let wake_at = chrono::Utc::now() + chrono::Duration::hours(1);
    if parked {
        writer
            .append_event(VersionedEventPayload::new(EventPayload::RunParked {
                wake_at,
                runtime: None,
                worktree: launch.worktree.clone(),
                basis: surge_core::capacity::WakeBasis::PolicyBackoff,
                reason: "fixture".into(),
            }))
            .await
            .unwrap();
    }
    writer.flush().await.unwrap();
    writer.close().await.unwrap();
    if parked {
        fixture
            .storage
            .set_run_parked(&launch.run_id, wake_at.timestamp_millis())
            .await
            .unwrap();
    }
    if parked {
        let ordered = fixture.owner.admission_order.clone().lock_owned().await;
        fixture.owner.process(id, ordered).await.unwrap();
        assert_eq!(fixture.owner.services.admission.snapshot().await.active, 0);
    }
    fixture.owner.cancel(id).unwrap();
    assert!(
        crate::bootstrap_cancel::cancel_inactive(
            &fixture.storage,
            &fixture.engine,
            RunId::new(),
            &launch
        )
        .await
        .is_err()
    );
    let evidence =
        crate::bootstrap_cancel::cancel_inactive(&fixture.storage, &fixture.engine, id, &launch)
            .await
            .unwrap();
    assert_eq!(
        evidence,
        crate::bootstrap_cancel::CancellationEvidence::Aborted
    );
    // Simulate interruption after durable abort but before journal settlement.
    let again =
        crate::bootstrap_cancel::cancel_inactive(&fixture.storage, &fixture.engine, id, &launch)
            .await
            .unwrap();
    assert_eq!(
        again,
        crate::bootstrap_cancel::CancellationEvidence::Aborted
    );
    let ordered = fixture.owner.admission_order.clone().lock_owned().await;
    fixture.owner.process(id, ordered).await.unwrap();
    assert_eq!(fixture.owner.services.admission.snapshot().await.active, 0);
    let reopened = Storage::open(fixture.storage.home()).await.unwrap();
    let status = reopened
        .bootstrap_operation_store()
        .get(id)
        .unwrap()
        .unwrap()
        .status;
    assert_eq!(status.state, State::Cancelled);
    assert_eq!(status.result, Some(BootstrapTerminal::Cancelled));
    let inspection = reopened.inspect_run(launch.run_id).await.unwrap();
    assert_eq!(inspection.registry.unwrap().status, RunStatus::Aborted);
    let RunDatabaseInspection::Present { events } = inspection.database else {
        panic!("missing database")
    };
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event.payload.payload, EventPayload::RunAborted { .. }))
            .count(),
        1
    );
    assert!(
        events
            .iter()
            .all(|event| !matches!(event.payload.payload, EventPayload::SessionOpened { .. }))
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn restart_after_child_commit_uses_committed_artifact_not_newer_parent_version() {
    use surge_core::{VersionedEventPayload, keys::NodeKey};
    let fixture = Fixture::new().await;
    let approvals = fixture.approve();
    let id = RunId::new();
    fixture.owner.submit(id, &fixture.intent()).await.unwrap();
    // Run exactly one owned phase, then stop before implementation admission.
    let ordered = fixture.owner.admission_order.clone().lock_owned().await;
    fixture.owner.process(id, ordered).await.unwrap();
    approvals.abort();
    let committed = fixture.owner.record(id).unwrap();
    assert_eq!(
        committed.status.state,
        State::Pending {
            phase: Phase::QueuedImplementation
        }
    );
    let child = committed.child.as_ref().unwrap();
    assert_eq!(
        child.artifacts()[0].hash,
        surge_core::ContentHash::compute(DESCRIPTION.as_bytes())
    );
    let artifact =
        surge_persistence::artifacts::ArtifactStore::new(fixture.storage.home().join("runs"))
            .put(
                committed.status.planning_run,
                "description",
                DESCRIPTION
                    .replace("Build the app", "Different later description")
                    .as_bytes(),
            )
            .await
            .unwrap();
    let writer = fixture
        .storage
        .open_run_writer(committed.status.planning_run)
        .await
        .unwrap();
    writer
        .append_event(VersionedEventPayload::new(EventPayload::ArtifactProduced {
            node: NodeKey::try_new("description_author").unwrap(),
            artifact: artifact.hash,
            path: artifact.path,
            name: "description".into(),
            source_path: None,
        }))
        .await
        .unwrap();
    writer.flush().await.unwrap();
    writer.close().await.unwrap();
    let restarted = fixture.reopened_owner().await;
    let storage = restarted.services.storage.clone();
    let ordered = restarted.admission_order.clone().lock_owned().await;
    restarted.process(id, ordered).await.unwrap();
    let result = restarted.record(id).unwrap();
    assert_eq!(result.status.state, State::Completed);
    let BootstrapStoredPayload::V1 { capture, .. } = result.payload else {
        panic!("v1")
    };
    assert_eq!(
        tokio::fs::read_to_string(
            capture
                .fields()
                .implementation_worktree
                .join("description.md")
        )
        .await
        .unwrap(),
        DESCRIPTION
    );
    let inspection = storage
        .inspect_run(committed.status.implementation_run)
        .await
        .unwrap();
    let surge_persistence::runs::inspection::RunDatabaseInspection::Present { events } =
        inspection.database
    else {
        panic!("database")
    };
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event.payload.payload, EventPayload::RunStarted { .. }))
            .count(),
        1
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn live_planning_gate_cancel_joins_and_never_launches_child() {
    let fixture = Fixture::new().await;
    let mut global = fixture.owner.services.broadcast.subscribe_global();
    let mut tap = fixture.engine.subscribe_tap();
    let id = RunId::new();
    let accepted = fixture.owner.submit(id, &fixture.intent()).await.unwrap();
    let supervisor = tokio::spawn(fixture.owner.clone().run());
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let event = tap.recv().await.unwrap();
            if matches!(
                event.event.payload.payload,
                EventPayload::HumanInputRequested { .. }
            ) {
                break;
            }
        }
    })
    .await
    .unwrap();
    let discovered = tokio::time::timeout(Duration::from_secs(2), global.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        discovered,
        surge_orchestrator::engine::ipc::GlobalDaemonEvent::RunAccepted { run_id }
            if run_id == accepted.planning_run
    ));
    fixture.owner.cancel(id).unwrap();
    let result = fixture.terminal(id).await;
    fixture.owner.services.shutdown.cancel();
    supervisor.await.unwrap();
    assert_eq!(result.state, State::Cancelled);
    assert!(fixture.engine.snapshot_active_runs().await.is_empty());
    assert_eq!(fixture.owner.services.admission.snapshot().await.active, 0);
    assert!(
        fixture
            .storage
            .inspect_run(accepted.implementation_run)
            .await
            .unwrap()
            .registry
            .is_none()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn production_lost_reply_retry_precedes_changed_environment_and_conflict() {
    let fixture = Fixture::new().await;
    let id = RunId::new();
    let intent = fixture.intent();
    let accepted = fixture.owner.submit(id, &intent).await.unwrap();
    std::fs::write(fixture.project.join("surge.toml"), "invalid config [").unwrap();
    std::fs::write(fixture.project.join("dirty.txt"), "new user work").unwrap();
    assert_eq!(fixture.owner.submit(id, &intent).await.unwrap(), accepted);
    let changed = BootstrapIntent::new(
        fixture.project.clone(),
        "changed prompt".into(),
        surge_core::budget::BudgetGuard::default(),
    )
    .unwrap();
    assert!(matches!(
        fixture.owner.submit(id, &changed).await,
        Err(BootstrapError::Store(BootstrapStoreError::IntentConflict))
    ));
    assert!(fixture.owner.submit(RunId::new(), &intent).await.is_err());
    assert!(fixture.engine.snapshot_active_runs().await.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn production_ipc_accepts_durable_operation_and_fences_raw_reserved_runs() {
    use surge_orchestrator::engine::daemon_facade::DaemonEngineFacade;
    let fixture = Fixture::new().await;
    let socket = fixture.root.path().join("bootstrap.sock");
    let services = &fixture.owner.services;
    let server = tokio::spawn(crate::server::run_with_supervisor(
        crate::server::ServerConfig {
            socket_path: socket.clone(),
            max_active: 1,
            max_queue: 0,
        },
        services.facade.clone(),
        TrackingContext::new(fixture.engine.clone(), fixture.storage.clone()),
        services.broadcast.clone(),
        services.admission.clone(),
        services.shutdown.clone(),
        fixture.owner.clone(),
    ));
    let client = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Ok(client) = DaemonEngineFacade::connect(socket.clone()).await {
                break client;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let id = RunId::new();
    let accepted = client.start_bootstrap(id, fixture.intent()).await.unwrap();
    assert!(matches!(
        client.start_bootstrap(RunId::new(), fixture.intent()).await,
        Err(
            surge_orchestrator::engine::daemon_facade::BootstrapClientError::Rejected {
                code: surge_orchestrator::engine::ipc::ErrorCode::QueueFull,
                ..
            }
        )
    ));

    assert_eq!(client.bootstrap_status(id).await.unwrap(), accepted);
    assert_eq!(
        client.start_bootstrap(id, fixture.intent()).await.unwrap(),
        accepted
    );
    assert!(
        client
            .resume_run(accepted.planning_run, fixture.project.clone())
            .await
            .is_err()
    );
    assert!(
        client
            .stop_run(accepted.planning_run, "raw cancellation".into())
            .await
            .is_err()
    );
    assert!(
        client
            .start_run(
                accepted.implementation_run,
                fixture.owner.services.runtime.as_ref().unwrap().graph(),
                fixture.project.clone(),
                EngineRunConfig::default()
            )
            .await
            .is_err()
    );
    client.cancel_bootstrap(id).await.unwrap();
    let supervisor = tokio::spawn(fixture.owner.clone().run());
    assert_eq!(fixture.terminal(id).await.state, State::Cancelled);
    let terminal = client.bootstrap_status(id).await.unwrap();
    assert_eq!(terminal.state, State::Cancelled);
    assert_eq!(client.cancel_bootstrap(id).await.unwrap(), terminal);
    assert!(fixture.engine.snapshot_active_runs().await.is_empty());
    fixture.owner.services.shutdown.cancel();
    supervisor.await.unwrap();
    server.await.unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn partial_engine_startup_is_preserved_for_attention_without_retrying_agents() {
    let fixture = Fixture::new().await;
    let id = RunId::new();
    let accepted = fixture.owner.submit(id, &fixture.intent()).await.unwrap();
    let record = fixture.owner.record(id).unwrap();
    let launch = expected(&record).unwrap();
    let writer = fixture
        .storage
        .create_run(
            accepted.planning_run,
            launch.worktree.to_str().unwrap(),
            None,
        )
        .await
        .unwrap();
    writer
        .append_event(surge_core::VersionedEventPayload::new(
            EventPayload::RunStarted {
                pipeline_template: None,
                project_path: launch.worktree,
                initial_prompt: launch.initial_prompt,
                config: launch.config,
            },
        ))
        .await
        .unwrap();
    writer.flush().await.unwrap();
    writer.close().await.unwrap();
    fixture.owner.reconcile().await.unwrap();
    let status = fixture.terminal(id).await;
    assert!(matches!(
        status.state,
        State::NeedsAttention {
            reason: Attention::PartialStartup,
            ..
        }
    ));
    assert!(!status.cancel_requested);
    assert!(fixture.engine.snapshot_active_runs().await.is_empty());
    assert_eq!(fixture.owner.services.admission.snapshot().await.active, 0);
    let inspection = fixture
        .storage
        .inspect_run(accepted.planning_run)
        .await
        .unwrap();
    let surge_persistence::runs::inspection::RunDatabaseInspection::Present { events } =
        inspection.database
    else {
        panic!("database")
    };
    assert_eq!(events.len(), 1);
    assert!(fixture.owner.retry(id, status.revision).await.is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn explicit_attention_retry_requires_restored_git_ownership() {
    let fixture = Fixture::new().await;
    let id = RunId::new();
    fixture.owner.submit(id, &fixture.intent()).await.unwrap();
    let record = fixture.owner.record(id).unwrap();
    let BootstrapStoredPayload::V1 { capture, .. } = &record.payload else {
        panic!("v1")
    };
    let path = &capture.fields().planning_worktree;
    std::fs::create_dir_all(path).unwrap();
    let blocked = fixture
        .owner
        .store
        .block(id, record.status.revision, Attention::WorktreeConflict)
        .unwrap();
    assert!(
        fixture
            .owner
            .retry(id, blocked.status.revision)
            .await
            .is_err()
    );
    assert_eq!(fixture.owner.status(id).unwrap(), blocked.status);
    std::fs::remove_dir(path).unwrap();
    let restored = fixture
        .owner
        .retry(id, blocked.status.revision)
        .await
        .unwrap();
    assert_eq!(
        restored.state,
        State::Pending {
            phase: Phase::QueuedPlanning
        }
    );
    assert!(path.join(".git").exists());
    assert!(fixture.engine.snapshot_active_runs().await.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancellation_attention_retry_after_reopen_only_settles_owned_run() {
    use surge_persistence::runs::inspection::RunDatabaseInspection;
    let fixture = Fixture::new().await;
    let id = RunId::new();
    fixture.owner.submit(id, &fixture.intent()).await.unwrap();
    let record = fixture.owner.record(id).unwrap();
    let launch = expected(&record).unwrap();
    let writer = fixture.started_writer(&launch).await;
    writer.flush().await.unwrap();
    fixture.owner.cancel(id).unwrap();
    // A separately held writer prevents confirmation; cancellation must remain durable.
    fixture.owner.reconcile().await.unwrap();
    let blocked = fixture.terminal(id).await;
    assert!(matches!(blocked.state, State::NeedsAttention { .. }));
    assert!(blocked.cancel_requested);
    assert_eq!(fixture.owner.services.admission.snapshot().await.active, 0);
    writer.close().await.unwrap();
    let storage = Storage::open(fixture.storage.home()).await.unwrap();
    let restarted = BootstrapSupervisor::new(BootstrapServices {
        engine: fixture.engine.clone(),
        facade: fixture.owner.services.facade.clone(),
        storage: storage.clone(),
        runtime: None,
        admission: Arc::new(AdmissionController::new(1, 0)),
        broadcast: Arc::new(BroadcastRegistry::new()),
        shutdown: CancellationToken::new(),
    });
    let retry = restarted.retry(id, blocked.revision).await.unwrap();
    assert!(retry.cancel_requested);
    assert!(matches!(retry.state, State::Cancelling { .. }));
    let ordered = restarted.admission_order.clone().lock_owned().await;
    restarted.process(id, ordered).await.unwrap();
    let terminal = restarted.status(id).unwrap();
    assert_eq!(terminal.state, State::Cancelled);
    assert_eq!(restarted.cancel(id).unwrap(), terminal);
    assert_eq!(restarted.services.admission.snapshot().await.active, 0);
    let inspection = storage.inspect_run(launch.run_id).await.unwrap();
    let RunDatabaseInspection::Present { events } = inspection.database else {
        panic!("database")
    };
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event.payload.payload, EventPayload::RunAborted { .. }))
            .count(),
        1
    );
    assert!(
        events
            .iter()
            .all(|event| !matches!(event.payload.payload, EventPayload::SessionOpened { .. }))
    );
    assert!(
        storage
            .inspect_run(record.status.implementation_run)
            .await
            .unwrap()
            .registry
            .is_none()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn restart_after_parent_completion_before_child_commit_reuses_parent_evidence() {
    let fixture = Fixture::new().await;
    let approvals = fixture.approve();
    let id = RunId::new();
    fixture.owner.submit(id, &fixture.intent()).await.unwrap();
    fixture.storage.acquire_registry_conn().unwrap().execute_batch(
        "CREATE TRIGGER fail_child_commit BEFORE UPDATE OF child_json ON bootstrap_operations WHEN NEW.child_json IS NOT NULL BEGIN SELECT RAISE(FAIL, 'fixture interrupted child commit'); END;"
    ).unwrap();
    let ordered = fixture.owner.admission_order.clone().lock_owned().await;
    assert_eq!(
        fixture.owner.process(id, ordered).await.unwrap_err(),
        Attention::StorageUnconfirmed
    );
    approvals.abort();
    let pending = fixture.owner.record(id).unwrap();
    assert!(pending.child.is_none());
    assert_eq!(
        pending.status.state,
        State::Pending {
            phase: Phase::Planning
        }
    );
    fixture
        .storage
        .acquire_registry_conn()
        .unwrap()
        .execute_batch("DROP TRIGGER fail_child_commit")
        .unwrap();
    let restarted = fixture.reopened_owner().await;
    let ordered = restarted.admission_order.clone().lock_owned().await;
    restarted.process(id, ordered).await.unwrap();
    let committed = restarted.record(id).unwrap();
    assert_eq!(
        committed.status.state,
        State::Pending {
            phase: Phase::QueuedImplementation
        }
    );
    assert_eq!(
        committed.child.as_ref().unwrap().parent(),
        pending.status.planning_run
    );
    let ordered = restarted.admission_order.clone().lock_owned().await;
    restarted.process(id, ordered).await.unwrap();
    assert_eq!(restarted.status(id).unwrap().state, State::Completed);
    assert!(
        restarted
            .services
            .engine
            .snapshot_active_runs()
            .await
            .is_empty()
    );
    let next = RunId::new();
    restarted.submit(next, &fixture.intent()).await.unwrap();
    restarted.cancel(next).unwrap();
    let ordered = restarted.admission_order.clone().lock_owned().await;
    restarted.process(next, ordered).await.unwrap();
    assert_eq!(restarted.services.admission.snapshot().await.active, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancellation_after_child_commit_cannot_admit_implementation() {
    let fixture = Fixture::new().await;
    let approvals = fixture.approve();
    let id = RunId::new();
    fixture.owner.submit(id, &fixture.intent()).await.unwrap();
    let ordered = fixture.owner.admission_order.clone().lock_owned().await;
    fixture.owner.process(id, ordered).await.unwrap();
    approvals.abort();
    let committed = fixture.owner.record(id).unwrap();
    assert!(committed.child.is_some());
    fixture.owner.cancel(id).unwrap();
    let ordered = fixture.owner.admission_order.clone().lock_owned().await;
    fixture.owner.process(id, ordered).await.unwrap();
    assert_eq!(fixture.owner.status(id).unwrap().state, State::Cancelled);
    assert!(
        fixture
            .storage
            .inspect_run(committed.status.implementation_run)
            .await
            .unwrap()
            .registry
            .is_none()
    );
    assert_eq!(fixture.owner.services.admission.snapshot().await.active, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_bootstrap_admission_is_bounded_without_orphan_operations() {
    let fixture = Fixture::with_capacity(1, 1).await;
    let barrier = Arc::new(tokio::sync::Barrier::new(8));
    let mut requests = Vec::new();
    for _ in 0..8 {
        let owner = fixture.owner.clone();
        let intent = fixture.intent();
        let barrier = barrier.clone();
        requests.push(tokio::spawn(async move {
            barrier.wait().await;
            owner.submit(RunId::new(), &intent).await
        }));
    }
    let mut accepted = Vec::new();
    for request in requests {
        match request.await.unwrap() {
            Ok(status) => accepted.push(status),
            Err(error) => assert!(
                matches!(error, BootstrapError::Store(BootstrapStoreError::QueueFull)),
                "{error:?}"
            ),
        }
    }
    assert_eq!(
        accepted.len(),
        2,
        "durable admission exceeded active+queue bound"
    );
    assert_eq!(fixture.owner.store.list().unwrap().len(), 2);
    assert_eq!(fixture.owner.services.admission.snapshot().await.active, 1);
    assert_eq!(
        fixture
            .owner
            .submit(accepted[0].operation_id, &fixture.intent())
            .await
            .unwrap(),
        accepted[0]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn zero_bootstrap_queue_requires_immediate_shared_active_slot() {
    let fixture = Fixture::new().await;
    let ordinary = RunId::new();
    assert!(
        fixture
            .owner
            .services
            .admission
            .try_admit_no_queue(ordinary)
            .await
    );
    let id = RunId::new();
    assert!(fixture.owner.submit(id, &fixture.intent()).await.is_err());
    assert!(fixture.owner.store.get(id).unwrap().is_none());
    fixture
        .owner
        .services
        .admission
        .notify_completed(ordinary)
        .await;
    fixture.owner.submit(id, &fixture.intent()).await.unwrap();
    assert_eq!(fixture.owner.services.admission.snapshot().await.active, 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn bootstrap_admission_commit_failure_releases_provisional_slot_and_rolls_back_row() {
    let fixture = Fixture::new().await;
    fixture.storage.acquire_registry_conn().unwrap().execute_batch(
        "CREATE TABLE admission_parent (id TEXT PRIMARY KEY);
         CREATE TABLE admission_child (id TEXT REFERENCES admission_parent(id) DEFERRABLE INITIALLY DEFERRED);
         CREATE TRIGGER fail_admission AFTER INSERT ON bootstrap_operations
         BEGIN INSERT INTO admission_child VALUES ('missing'); END;"
    ).unwrap();
    let id = RunId::new();
    assert!(matches!(
        fixture.owner.submit(id, &fixture.intent()).await,
        Err(BootstrapError::Store(BootstrapStoreError::Sqlite(_)))
    ));
    assert!(fixture.owner.store.get(id).unwrap().is_none());
    assert_eq!(fixture.owner.services.admission.snapshot().await.active, 0);
    fixture
        .storage
        .acquire_registry_conn()
        .unwrap()
        .execute_batch("DROP TRIGGER fail_admission")
        .unwrap();
    fixture.owner.submit(id, &fixture.intent()).await.unwrap();
    assert_eq!(fixture.owner.services.admission.snapshot().await.active, 1);
    fixture.owner.cancel(id).unwrap();
    let ordered = fixture.owner.admission_order.clone().lock_owned().await;
    fixture.owner.process(id, ordered).await.unwrap();
    assert_eq!(fixture.owner.services.admission.snapshot().await.active, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn bootstrap_admission_reopen_counts_future_ownership_and_cancel_releases_capacity() {
    let fixture = Fixture::with_capacity(1, 1).await;
    let first = RunId::new();
    let second = RunId::new();
    fixture
        .owner
        .submit(first, &fixture.intent())
        .await
        .unwrap();
    fixture
        .owner
        .submit(second, &fixture.intent())
        .await
        .unwrap();
    fixture
        .storage
        .acquire_registry_conn()
        .unwrap()
        .execute(
            "UPDATE bootstrap_operations SET payload_version=99 WHERE operation_id=?1",
            [first.to_string()],
        )
        .unwrap();
    let restarted = fixture.reopened_owner().await;
    let third = RunId::new();
    assert!(matches!(
        restarted.submit(third, &fixture.intent()).await,
        Err(BootstrapError::Store(BootstrapStoreError::QueueFull))
    ));
    assert!(restarted.store.get(third).unwrap().is_none());
    assert_eq!(restarted.services.admission.snapshot().await.active, 0);
    restarted.cancel(second).unwrap();
    let ordered = restarted.admission_order.clone().lock_owned().await;
    restarted.process(second, ordered).await.unwrap();
    assert_eq!(restarted.status(second).unwrap().state, State::Cancelled);
    restarted.submit(third, &fixture.intent()).await.unwrap();
    assert_eq!(restarted.services.admission.snapshot().await.active, 1);
    assert!(matches!(
        restarted.submit(RunId::new(), &fixture.intent()).await,
        Err(BootstrapError::Store(BootstrapStoreError::QueueFull))
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn bootstrap_preflight_error_releases_execution_slot_but_retains_ownership() {
    let fixture = Fixture::new().await;
    let id = RunId::new();
    fixture.owner.submit(id, &fixture.intent()).await.unwrap();
    assert_eq!(fixture.owner.services.admission.snapshot().await.active, 1);
    std::fs::write(fixture.project.join("surge.toml"), "invalid [").unwrap();
    fixture.owner.reconcile().await.unwrap();
    let blocked = fixture.terminal(id).await;
    assert!(matches!(
        blocked.state,
        State::NeedsAttention {
            reason: Attention::ConfigurationChanged,
            ..
        }
    ));
    assert_eq!(fixture.owner.services.admission.snapshot().await.active, 0);
    assert!(fixture.engine.snapshot_active_runs().await.is_empty());
    assert_eq!(fixture.owner.store.list().unwrap().len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn incoming_bootstrap_cannot_bypass_older_durable_queue() {
    let fixture = Fixture::with_capacity(1, 1).await;
    let first = RunId::new();
    let older = RunId::new();
    let incoming = RunId::new();
    fixture
        .owner
        .submit(first, &fixture.intent())
        .await
        .unwrap();
    let queued = fixture
        .owner
        .submit(older, &fixture.intent())
        .await
        .unwrap();
    fixture.owner.cancel(first).unwrap();
    let ordered = fixture.owner.admission_order.clone().lock_owned().await;
    fixture.owner.process(first, ordered).await.unwrap();
    assert_eq!(fixture.owner.services.admission.snapshot().await.active, 0);
    let later = fixture
        .owner
        .submit(incoming, &fixture.intent())
        .await
        .unwrap();
    for _ in 0..3 {
        assert_eq!(
            fixture
                .owner
                .submit(incoming, &fixture.intent())
                .await
                .unwrap(),
            later
        );
    }
    let mut tap = fixture.engine.subscribe_tap();
    let supervisor = tokio::spawn(fixture.owner.clone().run());
    let first_started = tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            let event = tap.recv().await.unwrap();
            if matches!(
                event.event.payload.payload,
                EventPayload::SessionOpened { .. }
            ) {
                break event.run_id;
            }
        }
    })
    .await
    .unwrap();
    fixture.owner.cancel(older).unwrap();
    fixture.owner.cancel(incoming).unwrap();
    fixture.terminal(older).await;
    fixture.terminal(incoming).await;
    fixture.owner.services.shutdown.cancel();
    supervisor.await.unwrap();
    assert_eq!(
        first_started, queued.planning_run,
        "new acceptance bypassed durable FIFO"
    );
    assert_eq!(fixture.owner.services.admission.snapshot().await.active, 0);
}
