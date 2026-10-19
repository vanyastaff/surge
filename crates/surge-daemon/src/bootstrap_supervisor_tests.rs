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

// This waits for the whole parent+child operation, including both worktree
// preparations, planning approvals, implementation startup, and durable
// settlement. Native Windows preparation alone took nearly 8s before the
// first planning event; allow bounded margin for both workflows to finish.
const BOOTSTRAP_OPERATION_COMPLETION_TIMEOUT: Duration = Duration::from_secs(30);

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

#[tokio::test(flavor = "multi_thread")]
async fn recovery_observability_confirmed_refusal_keeps_identity_and_cold_attention() {
    check_confirmed_recovery_attention(false).await;
    check_confirmed_recovery_attention(true).await;
}

async fn check_confirmed_recovery_attention(cancel_requested: bool) {
    let fixture = Fixture::new().await;
    let id = RunId::new();
    let accepted = fixture.owner.submit(id, &fixture.intent()).await.unwrap();
    let record = fixture.owner.record(id).unwrap();
    let record = fixture
        .owner
        .store
        .claim(id, record.status.revision)
        .unwrap();
    fixture
        .owner
        .store
        .mark_started(id, record.status.revision)
        .unwrap();
    let launch = expected(&fixture.owner.record(id).unwrap()).unwrap();
    let writer = fixture.started_writer(&launch).await;
    let diagnostic = "gate occurrence conflicts; inspect recorded history";
    writer
        .append_event(surge_core::VersionedEventPayload::new(
            EventPayload::RunRecoveryRequired {
                control_generation: 0,
                diagnostic: diagnostic.into(),
            },
        ))
        .await
        .unwrap();
    writer.close().await.unwrap();
    let inspection = fixture.storage.inspect_run(launch.run_id).await.unwrap();
    let surge_persistence::runs::inspection::RunDatabaseInspection::Present { events } =
        inspection.database
    else {
        panic!("recorded run missing");
    };
    let outcome = RunOutcome::RecoveryRequired {
        control_generation: 0,
        diagnostic: diagnostic.into(),
    };
    assert!(crate::tracked_run::confirms(&events, &outcome));
    let tracked = tokio::spawn(async move { Ok(outcome) });
    let outcome = fixture
        .owner
        .wait_run(id, launch.run_id, tracked)
        .await
        .unwrap();
    if cancel_requested {
        fixture.owner.cancel(id).unwrap();
    }
    let reason = fixture.owner.finish(id, outcome).await.unwrap_err();
    fixture.owner.attention(id, reason);
    let restarted = fixture.reopened_owner().await;
    let status = restarted.status(id).unwrap();
    let stored = serde_json::to_value(&status.state).unwrap();
    assert_eq!(status.planning_run, accepted.planning_run);
    assert_eq!(status.implementation_run, accepted.implementation_run);
    assert_eq!(status.state.phase(), Some(Phase::Planning));
    assert!(status.result.is_none());
    assert_eq!(status.cancel_requested, cancel_requested);
    assert!(!serde_json::to_string(&status).unwrap().contains(diagnostic));
    drop(restarted);
    fixture.close().await;
    assert_eq!(stored["reason"], "recovery_required");
}

#[tokio::test(flavor = "multi_thread")]
async fn recovery_observability_tracking_and_settlement_remain_unconfirmed() {
    use crate::tracked_run::TrackingError;
    use surge_orchestrator::engine::handle::RunSettlementFailure;
    let fixture = Fixture::new().await;
    let id = RunId::new();
    let accepted = fixture.owner.submit(id, &fixture.intent()).await.unwrap();
    for error in [
        TrackingError::Events("event read unavailable".into()),
        TrackingError::OutcomeMismatch,
        TrackingError::Settlement {
            original: Box::new(RunOutcome::RecoveryRequired {
                control_generation: 0,
                diagnostic: "recorded refusal".into(),
            }),
            failures: vec![RunSettlementFailure::WriterSeal],
        },
    ] {
        let tracked = tokio::spawn(async move { Err(error) });
        assert_eq!(
            fixture
                .owner
                .wait_run(id, accepted.planning_run, tracked)
                .await
                .unwrap_err(),
            Attention::StorageUnconfirmed
        );
    }
    let reason = fixture
        .owner
        .finish(
            id,
            RunOutcome::SettlementFailed {
                original: Box::new(RunOutcome::RecoveryRequired {
                    control_generation: 0,
                    diagnostic: "recorded refusal".into(),
                }),
                failures: vec![RunSettlementFailure::WriterSeal],
            },
        )
        .await
        .unwrap_err();
    assert_eq!(reason, Attention::StorageUnconfirmed);
    fixture.owner.cancel(id).unwrap();
    let ordered = fixture.owner.admission_order.clone().lock_owned().await;
    fixture.owner.process(id, ordered).await.unwrap();
    fixture.close().await;
}

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
    async fn open_session(
        &self,
        config: SessionConfig,
    ) -> Result<surge_core::execution_recovery::OpenedSession, OpenSessionError> {
        let session = SessionId::new();
        let index = self
            .opened
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.sessions
            .lock()
            .await
            .insert(session, (config.working_dir.clone(), index));
        let writer = config.writer_id;
        let hash = surge_core::ContentHash::compute(format!("{:?}", config.agent_kind).as_bytes());
        let mut opened = surge_core::execution_recovery::OpenedSession::new(
            session,
            surge_core::execution_recovery::ProviderSessionDescriptor::new(
                surge_core::execution_recovery::ProviderSessionId::new("author".into()).unwrap(),
                config.invocation,
                config.runtime,
                hash,
                config.working_dir,
                surge_core::execution_recovery::SessionRestoreCapabilities::default(),
                None,
            )
            .unwrap(),
            surge_core::execution_recovery::SessionOpenMode::New,
        )
        .unwrap();
        opened.execution_writer = Some(
            surge_core::execution_recovery::process::ExecutionWriterObservation::new(writer, None)
                .unwrap(),
        );
        Ok(opened)
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
    project: std::path::PathBuf,
    owner: Arc<BootstrapSupervisor>,
    engine: Arc<Engine>,
    storage: Arc<Storage>,
    home: crate::runtime_home_fixture::FixtureHome,
    root: tempfile::TempDir,
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
        let home = crate::runtime_home_fixture::FixtureHome::new().unwrap();
        let storage = Storage::open(home.path()).await.unwrap();
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
                memory_store_path: Some(home.path().join("memory.db")),
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
            project,
            owner,
            engine,
            storage,
            home,
            root,
        }
    }
    async fn close(self) {
        self.owner.services.shutdown.cancel();
        let workers = std::mem::take(&mut *self.owner.workers.lock().await);
        for (_, worker) in workers {
            worker.await.unwrap();
        }
        assert_eq!(self.engine.snapshot_active_runs().await.len(), 0);
        let Self {
            project: _,
            owner,
            engine,
            storage,
            home,
            root,
        } = self;
        drop(owner);
        drop(engine);
        drop(storage);
        home.close().unwrap();
        root.close().unwrap();
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
                memory_store_path: Some(self.home.path().join("restarted-memory.db")),
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
        let result = tokio::time::timeout(BOOTSTRAP_OPERATION_COMPLETION_TIMEOUT, async {
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
    assert!(approvals.await.unwrap_err().is_cancelled());
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
    assert!(fixture.home.path().exists());
    assert!(
        fixture
            .storage
            .inspect_run(accepted.implementation_run)
            .await
            .unwrap()
            .registry
            .is_some()
    );
    fixture.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn inactive_started_and_parked_cancel_once_and_reopen_as_cancelled() {
    check_inactive_cancel(false).await;
    check_inactive_cancel(true).await;
}

async fn check_inactive_cancel(parked: bool) {
    use surge_core::VersionedEventPayload;
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
    assert_cancelled_history(&reopened, id, launch.run_id).await;
    drop(reopened);
    fixture.close().await;
}

async fn assert_cancelled_history(storage: &Arc<Storage>, id: RunId, run: RunId) {
    use surge_core::RunStatus;
    use surge_persistence::runs::inspection::RunDatabaseInspection;
    let status = storage
        .bootstrap_operation_store()
        .get(id)
        .unwrap()
        .unwrap()
        .status;
    assert_eq!(status.state, State::Cancelled);
    assert_eq!(status.result, Some(BootstrapTerminal::Cancelled));
    let inspection = storage.inspect_run(run).await.unwrap();
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
    {
        let approvals = fixture.approve();
        let id = RunId::new();
        fixture.owner.submit(id, &fixture.intent()).await.unwrap();
        // Run exactly one owned phase, then stop before implementation admission.
        let ordered = fixture.owner.admission_order.clone().lock_owned().await;
        fixture.owner.process(id, ordered).await.unwrap();
        approvals.abort();
        assert!(approvals.await.unwrap_err().is_cancelled());
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
    fixture.close().await;
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
    assert_eq!(fixture.engine.snapshot_active_runs().await.len(), 0);
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
    drop(global);
    drop(tap);
    fixture.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn production_lost_reply_retry_precedes_changed_environment_and_conflict() {
    let fixture = Fixture::new().await;
    {
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
        assert_eq!(fixture.engine.snapshot_active_runs().await.len(), 0);
    }
    fixture.close().await;
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
    assert_eq!(fixture.engine.snapshot_active_runs().await.len(), 0);
    fixture.owner.services.shutdown.cancel();
    supervisor.await.unwrap();
    server.await.unwrap().unwrap();
    drop(client);
    fixture.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn partial_engine_startup_is_preserved_for_attention_without_retrying_agents() {
    let fixture = Fixture::new().await;
    {
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
        assert_eq!(fixture.engine.snapshot_active_runs().await.len(), 0);
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
    fixture.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn explicit_attention_retry_requires_restored_git_ownership() {
    let fixture = Fixture::new().await;
    {
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
        assert_eq!(fixture.engine.snapshot_active_runs().await.len(), 0);
    }
    fixture.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancellation_attention_retry_after_reopen_only_settles_owned_run() {
    use surge_persistence::runs::inspection::RunDatabaseInspection;
    let fixture = Fixture::new().await;
    {
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
    fixture.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn restart_after_parent_completion_before_child_commit_reuses_parent_evidence() {
    let fixture = Fixture::new().await;
    {
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
        assert!(approvals.await.unwrap_err().is_cancelled());
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
        assert_eq!(
            restarted.services.engine.snapshot_active_runs().await.len(),
            0
        );
        let next = RunId::new();
        restarted.submit(next, &fixture.intent()).await.unwrap();
        restarted.cancel(next).unwrap();
        let ordered = restarted.admission_order.clone().lock_owned().await;
        restarted.process(next, ordered).await.unwrap();
        assert_eq!(restarted.services.admission.snapshot().await.active, 0);
    }
    fixture.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancellation_after_child_commit_cannot_admit_implementation() {
    let fixture = Fixture::new().await;
    {
        let approvals = fixture.approve();
        let id = RunId::new();
        fixture.owner.submit(id, &fixture.intent()).await.unwrap();
        let ordered = fixture.owner.admission_order.clone().lock_owned().await;
        fixture.owner.process(id, ordered).await.unwrap();
        approvals.abort();
        assert!(approvals.await.unwrap_err().is_cancelled());
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
    fixture.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_bootstrap_admission_is_bounded_without_orphan_operations() {
    let fixture = Fixture::with_capacity(1, 1).await;
    {
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
    fixture.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn zero_bootstrap_queue_requires_immediate_shared_active_slot() {
    let fixture = Fixture::new().await;
    {
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
    fixture.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn bootstrap_admission_commit_failure_releases_provisional_slot_and_rolls_back_row() {
    let fixture = Fixture::new().await;
    {
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
    fixture.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn bootstrap_admission_reopen_counts_future_ownership_and_cancel_releases_capacity() {
    let fixture = Fixture::with_capacity(1, 1).await;
    {
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
    fixture.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn bootstrap_preflight_error_releases_execution_slot_but_retains_ownership() {
    let fixture = Fixture::new().await;
    {
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
        assert_eq!(fixture.engine.snapshot_active_runs().await.len(), 0);
        assert_eq!(fixture.owner.store.list().unwrap().len(), 1);
    }
    fixture.close().await;
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
    drop(tap);
    fixture.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn failed_continuation_cold_reserved_is_unknown_without_second_fork() {
    use surge_core::bootstrap_attempt::BootstrapAttemptState as A;
    let fixture = Fixture::new().await;
    let (status, attempt) = reserve_collision_attempt(&fixture, RunId::new()).await;
    let operation = status.operation_id;
    let restarted = fixture.reopened_owner().await;
    restarted.reconcile().await.unwrap();
    let cold = restarted.status(operation).unwrap();
    assert_eq!(
        cold.state,
        surge_core::bootstrap_operation::BootstrapState::Failed
    );
    assert_eq!(cold.continuations.len(), 1);
    assert_eq!(cold.continuations[0].state, A::NeedsAttention);
    assert_eq!(cold.continuations[0].run, attempt.run);
    let repeated = restarted
        .continue_implementation(
            operation,
            status.revision,
            status.implementation_run,
            attempt.continuation_id,
        )
        .await
        .unwrap();
    assert_eq!(repeated, cold);
    assert!(
        restarted
            .services
            .engine
            .snapshot_active_runs()
            .await
            .is_empty()
    );
    assert!(
        !fixture
            .home
            .path()
            .join("runs")
            .join(attempt.run.to_string())
            .exists()
    );
    assert!(matches!(
        fixture.owner.store.advance_attempt(
            attempt.continuation_id,
            A::NeedsAttention,
            A::ForkReady
        ),
        Err(BootstrapStoreError::InvalidTransition)
    ));
    drop(restarted);
    fixture.close().await;
}

async fn reserve_collision_attempt(
    fixture: &Fixture,
    nonce: RunId,
) -> (
    BootstrapOperationStatus,
    surge_core::bootstrap_attempt::BootstrapAttempt,
) {
    use surge_core::bootstrap_continuation::{
        BootstrapContinuation, BootstrapFailure, BootstrapTerminal,
    };
    use surge_core::bootstrap_operation::BootstrapContentRef;
    use surge_core::{
        ContentHash,
        bootstrap_attempt::{BootstrapAttempt, BootstrapAttemptState as A},
    };
    let operation = RunId::new();
    let status = fixture
        .owner
        .submit(operation, &fixture.intent())
        .await
        .unwrap();
    let claimed = fixture
        .owner
        .store
        .claim(operation, status.revision)
        .unwrap();
    let planning = fixture
        .owner
        .store
        .mark_started(operation, claimed.status.revision)
        .unwrap();
    let child = BootstrapContinuation::new(
        status.planning_run,
        BootstrapContentRef {
            name: "fixed flow".into(),
            digest: ContentHash::compute(b"fixed graph"),
        },
        ["description", "roadmap", "flow"]
            .into_iter()
            .map(|name| surge_core::run_state::ArtifactRef {
                hash: ContentHash::compute(name.as_bytes()),
                path: name.into(),
                name: name.into(),
                produced_by: surge_core::keys::NodeKey::try_new("generator").unwrap(),
                produced_at_seq: 1,
            })
            .collect(),
        surge_core::budget::BudgetGuard::default(),
    )
    .unwrap();
    let queued = fixture
        .owner
        .store
        .continue_planning(operation, planning.status.revision, &child)
        .unwrap();
    let claimed = fixture
        .owner
        .store
        .claim(operation, queued.status.revision)
        .unwrap();
    let implementation = fixture
        .owner
        .store
        .mark_started(operation, claimed.status.revision)
        .unwrap();
    let failed = fixture
        .owner
        .store
        .settle(
            operation,
            implementation.status.revision,
            &BootstrapTerminal::Failed {
                run_id: status.implementation_run,
                reason: BootstrapFailure::RunFailed,
            },
        )
        .unwrap();
    let attempt = BootstrapAttempt {
        continuation_id: nonce,
        operation_id: operation,
        source_run: status.implementation_run,
        run: RunId::new(),
        boundary: 87,
        runtime: ContentHash::compute(b"host runtime"),
        state: A::Reserved,
    };
    fixture
        .owner
        .store
        .reserve_attempt(failed.status.revision, &attempt)
        .unwrap();
    (failed.status, attempt)
}

#[tokio::test(flavor = "multi_thread")]
async fn failed_continuation_live_nonce_collision_keeps_original_writer_and_reconciles_attempt() {
    use surge_core::bootstrap_attempt::BootstrapAttemptState as A;
    let fixture = Fixture::with_capacity(2, 2).await;
    let nonce = RunId::new();
    let (_, attempt) = reserve_collision_attempt(&fixture, nonce).await;
    let original = fixture
        .owner
        .submit(nonce, &fixture.intent())
        .await
        .unwrap();
    let writer = fixture
        .storage
        .create_run(
            original.planning_run,
            fixture.project.to_str().unwrap(),
            None,
        )
        .await
        .unwrap();
    let (release, released) = tokio::sync::oneshot::channel();
    let worker = tokio::spawn(async move {
        released.await.unwrap();
        writer.close().await.unwrap();
    });
    let original_worker = worker.id();
    fixture
        .owner
        .workers
        .lock()
        .await
        .insert(WorkerKey::Operation(nonce), worker);
    fixture.owner.reconcile().await.unwrap();
    let observed = fixture.owner.status(attempt.operation_id).unwrap();
    let retained = fixture
        .owner
        .workers
        .lock()
        .await
        .get(&WorkerKey::Operation(nonce))
        .map(tokio::task::JoinHandle::id);
    release.send(()).unwrap();
    fixture.close().await;
    assert_eq!(
        retained,
        Some(original_worker),
        "unrelated original native writer must remain owned and joined"
    );
    assert_eq!(observed.continuations[0].state, A::NeedsAttention);
}

#[tokio::test(flavor = "multi_thread")]
async fn failed_continuation_cold_nonce_collision_reconciles_both_namespaces() {
    use surge_core::bootstrap_attempt::BootstrapAttemptState as A;
    let fixture = Fixture::with_capacity(2, 2).await;
    let nonce = RunId::new();
    let (_, attempt) = reserve_collision_attempt(&fixture, nonce).await;
    fixture
        .owner
        .submit(nonce, &fixture.intent())
        .await
        .unwrap();
    let restarted = fixture.reopened_owner().await;
    restarted.reconcile().await.unwrap();
    let observed = restarted.status(attempt.operation_id).unwrap();
    let original_owned = restarted
        .workers
        .lock()
        .await
        .contains_key(&WorkerKey::Operation(nonce));
    restarted.services.shutdown.cancel();
    restarted.clone().run().await;
    drop(restarted);
    fixture.close().await;
    assert!(
        original_owned,
        "cold original operation retains its own worker"
    );
    assert_eq!(observed.continuations[0].state, A::NeedsAttention);
}

#[tokio::test(flavor = "multi_thread")]
async fn failed_continuation_shared_registration_cannot_replace_live_original_owner() {
    let fixture = Fixture::new().await;
    let collision = RunId::new();
    let original_run = RunId::new();
    let successor_run = RunId::new();
    let original = fixture
        .storage
        .create_run(original_run, fixture.project.to_str().unwrap(), None)
        .await
        .unwrap();
    let successor = fixture
        .storage
        .create_run(successor_run, fixture.project.to_str().unwrap(), None)
        .await
        .unwrap();
    let (release, wait) = tokio::sync::oneshot::channel();
    let original_worker = tokio::spawn(async move {
        wait.await.unwrap();
        original.close().await.unwrap();
    });
    let original_id = original_worker.id();
    let successor_worker = tokio::spawn(async move {
        successor.close().await.unwrap();
    });
    let mut workers = fixture.owner.workers.lock().await;
    workers.insert(WorkerKey::Operation(collision), original_worker);
    let replaced =
        super::attempts::register_continuation_worker(&mut workers, collision, successor_worker);
    let replaced_original = replaced
        .as_ref()
        .is_some_and(|worker| worker.id() == original_id);
    let count = workers.len();
    drop(workers);
    release.send(()).unwrap();
    if let Some(worker) = replaced {
        worker.await.unwrap();
    }
    fixture.close().await;
    assert!(
        !replaced_original,
        "actual shared Continue/reconcile registration dropped an unrelated native owner"
    );
    assert_eq!(count, 2);
}
