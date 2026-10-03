//! Controlled ACP protocol oracle; it does not establish production containment.
use agent_client_protocol::schema::v1::RequestPermissionResponse;
use interprocess::local_socket::tokio::prelude::*;
use serde_json::{Value, json};
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use surge_acp::bridge::error::{
    BridgeError, CloseSessionError, OpenSessionError, ReplyToPermissionError, ReplyToToolError,
    SendMessageError,
};
use surge_acp::bridge::facade::BridgeFacade;
use surge_acp::bridge::{
    AcpBridge, BridgeEvent, MessageContent, SessionConfig, SessionState, ToolResultPayload,
};
use surge_core::SessionId;
use surge_orchestrator::engine::facade::LocalEngineFacade;
use surge_orchestrator::engine::tools::worktree::WorktreeToolDispatcher;
use surge_orchestrator::engine::{Engine, EngineConfig};
use surge_orchestrator::profile_loader::{DiskProfileSet, ProfileRegistry};
use surge_persistence::runs::Storage;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio_util::sync::CancellationToken;
struct ObservedBridge {
    bridge: AcpBridge,
    quota_errors: AtomicUsize,
    primary_session: tokio::sync::Mutex<Option<SessionId>>,
    primary_ready: tokio::sync::Notify,
    primary_continue: tokio::sync::Notify,
}
#[async_trait::async_trait]
impl BridgeFacade for ObservedBridge {
    async fn open_session(
        &self,
        config: SessionConfig,
    ) -> Result<surge_core::execution_recovery::OpenedSession, OpenSessionError> {
        let primary = config.runtime == "quota-a";
        let opened = self.bridge.open_session(config).await?;
        if primary {
            *self.primary_session.lock().await = Some(opened.session);
        }
        Ok(opened)
    }
    async fn send_message(
        &self,
        id: SessionId,
        content: MessageContent,
    ) -> Result<(), SendMessageError> {
        let first_primary = {
            let mut primary = self.primary_session.lock().await;
            if *primary == Some(id) {
                primary.take();
                true
            } else {
                false
            }
        };
        if first_primary {
            self.primary_ready.notify_one();
            self.primary_continue.notified().await;
        }
        let result = self.bridge.send_message(id, content).await;
        if matches!(&result, Err(SendMessageError::RateLimited { .. })) {
            self.quota_errors.fetch_add(1, Ordering::SeqCst);
        }
        result
    }
    async fn session_state(&self, id: SessionId) -> Result<SessionState, BridgeError> {
        self.bridge.session_state(id).await
    }
    async fn close_session(&self, id: SessionId) -> Result<(), CloseSessionError> {
        self.bridge.close_session(id).await
    }
    async fn reply_to_tool(
        &self,
        id: SessionId,
        call: String,
        payload: ToolResultPayload,
    ) -> Result<(), ReplyToToolError> {
        self.bridge.reply_to_tool(id, call, payload).await
    }
    async fn reply_to_permission(
        &self,
        id: SessionId,
        request: String,
        response: RequestPermissionResponse,
    ) -> Result<(), ReplyToPermissionError> {
        self.bridge.reply_to_permission(id, request, response).await
    }
    fn subscribe(&self) -> tokio::sync::broadcast::Receiver<BridgeEvent> {
        self.bridge.subscribe()
    }
}

async fn request(socket: &Path, command: Value) -> Value {
    let name = surge_orchestrator::engine::ipc::local_socket_name_from_path(socket).unwrap();
    let stream = LocalSocketStream::connect(name).await.unwrap();
    let (read, mut write) = stream.split();
    let mut frame =
        serde_json::to_vec(&json!({"method":"work_item","request_id":1,"command":command}))
            .unwrap();
    frame.push(b'\n');
    write.write_all(&frame).await.unwrap();
    let mut line = String::new();
    tokio::time::timeout(
        Duration::from_secs(10),
        BufReader::new(read).read_line(&mut line),
    )
    .await
    .unwrap()
    .unwrap();
    serde_json::from_str(&line).unwrap()
}

fn initialize_project(path: &Path) {
    std::fs::write(path.join("tracked.txt"), b"original tracked source").unwrap();
    for args in [
        vec!["init"],
        vec!["config", "user.name", "Quota Fixture"],
        vec!["config", "user.email", "fixture@example.com"],
        vec!["add", "tracked.txt"],
        vec!["commit", "--allow-empty", "-m", "base"],
    ] {
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
}

fn write_profile(path: &Path, role: &str, runtime: &str) {
    std::fs::write(
        path.join(format!("{role}-1.0.toml")),
        format!(
            r#"
schema_version = 1
[role]
id = "{role}"
version = "1.0.0"
display_name = "Quota fixture"
category = "agents"
description = "Real ACP quota protocol"
when_to_use = "Tests"
[runtime]
recommended_model = "fixture-model"
agent_id = "{runtime}"
[[outcomes]]
id = "done"
description = "Success"
edge_kind_hint = "forward"
[prompt]
system = "Report done using the supplied stage outcome tool."
"#
        ),
    )
    .unwrap();
}

fn engine_config(home: &Path) -> EngineConfig {
    use surge_core::capacity::{CapacityPolicy, RotationPolicy};
    let profiles = home.join("profiles");
    std::fs::create_dir_all(&profiles).unwrap();
    write_profile(&profiles, "quota-primary", "quota-a");
    write_profile(&profiles, "quota-fallback", "quota-b");
    let binary = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/mock_acp_agent");
    assert!(binary.exists(), "build real mock_acp_agent prerequisite");
    let mut agents = std::collections::HashMap::new();
    for (runtime, scenario) in [
        ("quota-a", "prompt_error=429_retry_after"),
        ("quota-b", "report_outcome=done"),
    ] {
        let mut flags = vec![
            "--scenario".to_owned(),
            scenario.to_owned(),
            "--session-store".into(),
            home.join(format!("{runtime}-sessions.json"))
                .display()
                .to_string(),
            "--wire-log".into(),
            home.join(format!("{runtime}-wire.jsonl"))
                .display()
                .to_string(),
        ];
        flags.push("--stage-mcp".into());
        let config: surge_core::config::AgentConfig =
            serde_json::from_value(json!({"command":binary,"args":flags})).unwrap();
        agents.insert(runtime.to_owned(), config);
    }
    EngineConfig {
        profile_registry: Some(Arc::new(ProfileRegistry::new(
            DiskProfileSet::scan(&profiles).unwrap(),
        ))),
        agent_registry: Some(Arc::new(surge_acp::Registry::from_config(agents))),
        capacity: CapacityPolicy {
            blind_backoff: Some(Duration::from_secs(1)),
            jitter_max: Duration::ZERO,
            rotation: RotationPolicy::Candidate {
                profile: "quota-fallback@1.0".into(),
            },
        },
        ..EngineConfig::default()
    }
}

fn wire(home: &Path, runtime: &str) -> Vec<Value> {
    std::fs::read_to_string(home.join(format!("{runtime}-wire.jsonl")))
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn task_owned_quota_dispatches_actual_a_then_b_in_the_same_workspace() {
    use surge_core::{
        Graph,
        id::{WorkItemId, WorkItemOperationId},
        work_item::{WorkItemCommand, WorkItemRequirements, WorkItemResult},
    };
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    initialize_project(project.path());
    let storage = Storage::open(home.path()).await.unwrap();
    let bridge = Arc::new(ObservedBridge {
        bridge: AcpBridge::with_defaults().unwrap(),
        quota_errors: AtomicUsize::new(0),
        primary_session: tokio::sync::Mutex::new(None),
        primary_ready: tokio::sync::Notify::new(),
        primary_continue: tokio::sync::Notify::new(),
    });
    let engine = Arc::new(Engine::new_full(
        bridge.clone(),
        storage.clone(),
        Arc::new(WorktreeToolDispatcher::new(project.path().into())),
        Arc::new(surge_notify::MultiplexingNotifier::new()),
        None,
        None,
        engine_config(home.path()),
    ));
    let cancel = CancellationToken::new();
    let socket = home.path().join("quota.sock");
    let server = tokio::spawn(surge_daemon::run_runs_only(
        surge_daemon::ServerConfig {
            socket_path: socket.clone(),
            max_active: 2,
            max_queue: 2,
        },
        Arc::new(LocalEngineFacade::new(engine.clone())),
        surge_daemon::tracked_run::TrackingContext::new(engine.clone(), storage.clone()),
        Arc::new(surge_daemon::broadcast::BroadcastRegistry::new()),
        Arc::new(surge_daemon::admission::AdmissionController::new(2, 2)),
        cancel.clone(),
    ));
    tokio::time::timeout(Duration::from_secs(5), async {
        while !socket.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let requirements = WorkItemRequirements::new(
        "Preserve quota task".into(),
        vec!["Same task and workspace after fallback".into()],
    )
    .unwrap();
    let create = WorkItemCommand::Create {
        operation_id: WorkItemOperationId::new(),
        project: project.path().into(),
        title: "Quota task".into(),
        requirements: requirements.clone(),
    };
    let created = request(&socket, serde_json::to_value(create).unwrap()).await;
    assert_eq!(created["method"], "work_item_ok", "{created}");
    let item: WorkItemId =
        serde_json::from_value(created["result"]["value"]["item"]["id"].clone()).unwrap();
    let detail = storage.work_items().show(item).unwrap();
    let workspace = detail.item.workspace.clone();
    let dirty = workspace.path.join("retained-untracked.txt");
    let tracked = workspace.path.join("tracked.txt");
    let mut graph: Graph =
        toml::from_str(include_str!("../../../examples/flow_minimal_agent.toml")).unwrap();
    let raw = serde_json::to_value(&graph.nodes[&graph.start].config).unwrap();
    let mut raw = raw;
    raw["profile"] = json!("quota-primary@1.0");
    graph.nodes.get_mut(&graph.start).unwrap().config = serde_json::from_value(raw).unwrap();
    let quota_policy = {
        use surge_core::ContentHash;
        use surge_persistence::work_items::recovery_cycles::{
            AccountEvidence, FrozenQuotaCandidate, FrozenQuotaPolicy, FrozenQuotaStage,
            QuotaRoutingMode,
        };
        let binary =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/mock_acp_agent");
        let candidate = |runtime: &str, scenario: &str, stage_mcp: bool| {
            let mut args = vec![
                "--scenario".to_owned(),
                scenario.to_owned(),
                "--session-store".into(),
                home.path()
                    .join(format!("{runtime}-sessions.json"))
                    .display()
                    .to_string(),
                "--wire-log".into(),
                home.path()
                    .join(format!("{runtime}-wire.jsonl"))
                    .display()
                    .to_string(),
            ];
            if stage_mcp {
                args.push("--stage-mcp".into());
            }
            let kind = surge_acp::bridge::session::AgentKind::Custom {
                binary: binary.clone(),
                args,
            };
            FrozenQuotaCandidate::new(
                surge_persistence::work_items::recovery_cycles::RecoveryCandidate::new(
                    runtime.into(),
                    AccountEvidence::Unknown,
                )
                .unwrap(),
                None,
                ContentHash::compute(format!("{kind:?}").as_bytes()),
            )
            .unwrap()
        };
        let stage = FrozenQuotaStage::new(
            graph.start.clone(),
            QuotaRoutingMode::Configured,
            vec![
                candidate("quota-a", "prompt_error=429_retry_after", true),
                candidate("quota-b", "report_outcome=done", true),
            ],
            1000,
            60_000,
        )
        .unwrap();
        serde_json::to_value(FrozenQuotaPolicy::new(vec![stage]).unwrap()).unwrap()
    };
    let frozen_policy: surge_persistence::work_items::recovery_cycles::FrozenQuotaPolicy =
        serde_json::from_value(quota_policy.clone()).unwrap();
    let start = WorkItemCommand::Start {
        operation_id: WorkItemOperationId::new(),
        item,
        expected_version: detail.item.version,
        graph: Box::new(graph),
        quota_recovery: Some(quota_policy),
    };
    let started = request(&socket, serde_json::to_value(start).unwrap()).await;
    assert_eq!(started["method"], "work_item_ok", "{started}");
    let result: WorkItemResult = serde_json::from_value(started["result"].clone()).unwrap();
    let WorkItemResult::Attempt(attempt) = result else {
        panic!("actual start must return attempt")
    };
    if tokio::time::timeout(Duration::from_secs(5), bridge.primary_ready.notified())
        .await
        .is_err()
    {
        let diagnostic = storage.work_items().for_run(attempt.run).unwrap();
        let journal = storage.inspect_run(attempt.run).await.unwrap();
        let failures = match journal.database {
            surge_persistence::runs::inspection::RunDatabaseInspection::Present { events } => {
                events
                    .iter()
                    .filter_map(|row| match &row.payload.payload {
                        surge_core::EventPayload::StageFailed { node, reason, .. } => {
                            Some(format!("{node}: {reason}"))
                        },
                        surge_core::EventPayload::RunFailed { error } => {
                            Some(format!("run: {error}"))
                        },
                        _ => None,
                    })
                    .collect::<Vec<_>>()
            },
            _ => Vec::new(),
        };
        let _ = engine
            .stop_run(attempt.run, "quota prerequisite cleanup".into())
            .await;
        cancel.cancel();
        tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        panic!(
            "actual A must reach first prompt before quota oracle: {failures:?}; {diagnostic:?}"
        );
    }
    assert!(
        storage.work_items().workspace_prepared(item).unwrap(),
        "real host must prepare and record workspace before first prompt"
    );
    for runtime in ["quota-a", "quota-b"] {
        storage
            .observe_capacity(&surge_core::capacity::CapacityWindow::observed_429(
                runtime,
                Some(Duration::from_secs(1)),
                chrono::Utc::now() - chrono::Duration::seconds(2),
            ))
            .await
            .unwrap();
    }
    std::fs::write(&dirty, b"retain this user work").unwrap();
    std::fs::write(&tracked, b"retain dirty tracked source").unwrap();
    bridge.primary_continue.notify_one();
    let completed = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let inspected = storage.inspect_run(attempt.run).await.unwrap();
            if let surge_persistence::runs::inspection::RunDatabaseInspection::Present { events } =
                inspected.database
                && events.iter().any(|row| {
                    matches!(
                        row.payload.payload,
                        surge_core::EventPayload::RunCompleted { .. }
                    )
                })
            {
                break events;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    let diagnostic = storage.inspect_run(attempt.run).await.unwrap();
    let _ = engine
        .stop_run(attempt.run, "quota fixture cleanup".into())
        .await;
    cancel.cancel();
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let a = wire(home.path(), "quota-a");
    let b = wire(home.path(), "quota-b");
    assert_eq!(
        bridge.quota_errors.load(Ordering::SeqCst),
        1,
        "actual A transport must classify typed quota exhaustion; A={a:?}, B={b:?}, journal={diagnostic:?}"
    );
    assert_eq!(
        a.iter()
            .filter(|row| row["operation"] == "new_session")
            .count(),
        1
    );
    assert_eq!(
        a.iter().filter(|row| row["operation"] == "prompt").count(),
        1
    );
    assert_eq!(
        b.iter()
            .filter(|row| row["operation"] == "new_session")
            .count(),
        1,
        "eligible B must actually open after A quota exhaustion"
    );
    assert_eq!(
        b.iter().filter(|row| row["operation"] == "prompt").count(),
        1,
        "eligible B must actually receive the task prompt"
    );
    let events = completed.expect("fallback must complete instead of remaining capacity paused");
    assert!(
        matches!(
            storage.capacity_status("quota-a").await.unwrap(),
            surge_core::capacity::CapacityStatus::Known(_)
        ),
        "fallback B success cannot establish recovery of exhausted A"
    );
    assert_eq!(
        storage.capacity_status("quota-b").await.unwrap(),
        surge_core::capacity::CapacityStatus::NeverObserved,
        "actual successful provider B must clear its own stale exhaustion"
    );
    let opened: Vec<_> = events
        .iter()
        .filter_map(|row| match &row.payload.payload {
            surge_core::EventPayload::SessionOpened {
                opened: Some(opened),
                ..
            } => Some(opened),
            _ => None,
        })
        .collect();
    assert_eq!(opened.len(), 2, "retain both actual provider descriptors");
    assert!(
        storage
            .work_items()
            .inspect_current_recipe_exhaustion(
                &detail.item.project,
                &frozen_policy.stages()[0].candidates()[0],
                chrono::Utc::now().timestamp_millis(),
            )
            .unwrap()
            .is_some(),
        "production new_full must publish exact fresh primary marker through admitted opening"
    );
    let admission_count: i64 = storage
        .acquire_registry_conn()
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM recipe_opening_admissions",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        admission_count, 2,
        "primary and fallback both cross the admission barrier once"
    );
    assert_eq!(opened[0].descriptor.runtime(), "quota-a");
    assert_eq!(opened[1].descriptor.runtime(), "quota-b");
    assert_ne!(
        opened[0].descriptor.launch_hash(),
        opened[1].descriptor.launch_hash()
    );
    assert!(
        opened
            .iter()
            .all(|opened| opened.descriptor.cwd() == workspace.path)
    );
    assert_eq!(std::fs::read(&dirty).unwrap(), b"retain this user work");
    assert_eq!(
        std::fs::read(&tracked).unwrap(),
        b"retain dirty tracked source"
    );
    let final_detail = storage.work_items().show(item).unwrap();
    assert_eq!(final_detail.revision.requirements, requirements);
    assert_eq!(final_detail.item.workspace, workspace);
    let attempts = storage
        .work_items()
        .scan_attempts(None, 100)
        .unwrap()
        .entries;
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0].run, attempt.run);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn exhausted_candidates_park_and_scheduler_wakes_the_same_task() {
    use surge_core::{
        ContentHash, Graph,
        id::{WorkItemId, WorkItemOperationId},
        work_item::{WorkItemAttemptState, WorkItemCommand, WorkItemRequirements, WorkItemResult},
    };
    use surge_persistence::runs::{Clock, MockClock};
    use surge_persistence::work_items::recovery_cycles::{
        AccountEvidence, FrozenQuotaCandidate, FrozenQuotaPolicy, FrozenQuotaStage,
        QuotaRoutingMode, RecoveryCandidate,
    };

    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    initialize_project(project.path());
    let storage = Storage::open(home.path()).await.unwrap();
    let profiles = home.path().join("profiles");
    std::fs::create_dir_all(&profiles).unwrap();
    write_profile(&profiles, "quota-primary", "quota-a");
    write_profile(&profiles, "quota-fallback", "quota-b");
    let binary = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/mock_acp_agent");
    let mut agents = std::collections::HashMap::new();
    for (runtime, scenario) in [
        ("quota-a", "prompt_error=429_no_reset"),
        ("quota-b", "prompt_error=429_no_reset"),
    ] {
        let mut args = vec![
            "--scenario".to_owned(),
            scenario.to_owned(),
            "--prompt-error-once".into(),
            home.path()
                .join(format!("{runtime}-failed-once"))
                .display()
                .to_string(),
            "--session-store".into(),
            home.path()
                .join(format!("{runtime}-sessions.json"))
                .display()
                .to_string(),
            "--wire-log".into(),
            home.path()
                .join(format!("{runtime}-wire.jsonl"))
                .display()
                .to_string(),
            "--stage-mcp".into(),
        ];
        let config: surge_core::config::AgentConfig = serde_json::from_value(json!({
            "command": binary,
            "args": std::mem::take(&mut args),
        }))
        .unwrap();
        agents.insert(runtime.to_owned(), config);
    }
    let engine = Arc::new(Engine::new(
        Arc::new(AcpBridge::with_defaults().unwrap()),
        storage.clone(),
        Arc::new(WorktreeToolDispatcher::new(project.path().into())),
        EngineConfig {
            profile_registry: Some(Arc::new(ProfileRegistry::new(
                DiskProfileSet::scan(&profiles).unwrap(),
            ))),
            agent_registry: Some(Arc::new(surge_acp::Registry::from_config(agents))),
            ..EngineConfig::default()
        },
    ));
    let facade = Arc::new(LocalEngineFacade::new(engine.clone()));
    let cancel = CancellationToken::new();
    let clock = Arc::new(MockClock::new(chrono::Utc::now().timestamp_millis()));
    let scheduler = surge_daemon::wake_scheduler::WakeScheduler {
        tracking: surge_daemon::tracked_run::TrackingContext::new(engine.clone(), storage.clone()),
        storage: storage.clone(),
        facade: facade.clone(),
        admission: Arc::new(surge_daemon::admission::AdmissionController::new(2, 2)),
        broadcast: Arc::new(surge_daemon::broadcast::BroadcastRegistry::new()),
        clock: clock.clone(),
        notifier: Arc::new(surge_notify::MultiplexingNotifier::new()),
        blind_park_limit: 100,
        poll_interval: Duration::from_millis(10),
    };
    let scheduler_cancel = cancel.clone();
    let scheduler_task = tokio::spawn(scheduler.run(scheduler_cancel));
    let socket = home.path().join("quota-wake.sock");
    let server = tokio::spawn(surge_daemon::run_runs_only(
        surge_daemon::ServerConfig {
            socket_path: socket.clone(),
            max_active: 2,
            max_queue: 2,
        },
        facade,
        surge_daemon::tracked_run::TrackingContext::new(engine.clone(), storage.clone()),
        Arc::new(surge_daemon::broadcast::BroadcastRegistry::new()),
        Arc::new(surge_daemon::admission::AdmissionController::new(2, 2)),
        cancel.clone(),
    ));
    tokio::time::timeout(Duration::from_secs(5), async {
        while !socket.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();

    let requirements = WorkItemRequirements::new(
        "Recover an exhausted task".into(),
        vec!["Retry the same work after policy backoff".into()],
    )
    .unwrap();
    let created = request(
        &socket,
        serde_json::to_value(WorkItemCommand::Create {
            operation_id: WorkItemOperationId::new(),
            project: project.path().into(),
            title: "Quota sleep and wake".into(),
            requirements: requirements.clone(),
        })
        .unwrap(),
    )
    .await;
    assert_eq!(created["method"], "work_item_ok", "{created}");
    let item: WorkItemId =
        serde_json::from_value(created["result"]["value"]["item"]["id"].clone()).unwrap();
    let detail = storage.work_items().show(item).unwrap();
    let mut graph: Graph =
        toml::from_str(include_str!("../../../examples/flow_minimal_agent.toml")).unwrap();
    let mut raw = serde_json::to_value(&graph.nodes[&graph.start].config).unwrap();
    raw["profile"] = json!("quota-primary@1.0");
    graph.nodes.get_mut(&graph.start).unwrap().config = serde_json::from_value(raw).unwrap();
    let candidate = |runtime: &str| {
        let mut args = vec![
            "--scenario".to_owned(),
            "prompt_error=429_no_reset".to_owned(),
            "--prompt-error-once".into(),
            home.path()
                .join(format!("{runtime}-failed-once"))
                .display()
                .to_string(),
            "--session-store".into(),
            home.path()
                .join(format!("{runtime}-sessions.json"))
                .display()
                .to_string(),
            "--wire-log".into(),
            home.path()
                .join(format!("{runtime}-wire.jsonl"))
                .display()
                .to_string(),
            "--stage-mcp".into(),
        ];
        let kind = surge_acp::bridge::session::AgentKind::Custom {
            binary: binary.clone(),
            args: std::mem::take(&mut args),
        };
        FrozenQuotaCandidate::new(
            RecoveryCandidate::new(runtime.into(), AccountEvidence::Unknown).unwrap(),
            None,
            ContentHash::compute(format!("{kind:?}").as_bytes()),
        )
        .unwrap()
    };
    let stage = FrozenQuotaStage::new(
        graph.start.clone(),
        QuotaRoutingMode::Configured,
        vec![candidate("quota-a"), candidate("quota-b")],
        200,
        60_000,
    )
    .unwrap();
    let quota_policy = serde_json::to_value(FrozenQuotaPolicy::new(vec![stage]).unwrap()).unwrap();
    let started = request(
        &socket,
        serde_json::to_value(WorkItemCommand::Start {
            operation_id: WorkItemOperationId::new(),
            item,
            expected_version: detail.item.version,
            graph: Box::new(graph),
            quota_recovery: Some(quota_policy),
        })
        .unwrap(),
    )
    .await;
    assert_eq!(started["method"], "work_item_ok", "{started}");
    let WorkItemResult::Attempt(attempt) =
        serde_json::from_value(started["result"].clone()).unwrap()
    else {
        panic!("start must return the durable attempt")
    };

    tokio::time::timeout(Duration::from_secs(12), async {
        loop {
            let current = storage.work_items().for_run(attempt.run).unwrap().unwrap();
            let still_active = engine
                .snapshot_active_runs()
                .await
                .iter()
                .any(|run| run.run_id == attempt.run);
            if current.state == WorkItemAttemptState::Suspended
                && !still_active
            {
                break;
            }
            assert_ne!(
                current.state,
                WorkItemAttemptState::Attention,
                "exhausted quota recovery must not become manual attention; diagnostic={:?}, control={:?}, journal={:?}",
                current.diagnostic,
                storage.work_items().execution_control(attempt.run).unwrap(),
                storage.inspect_run(attempt.run).await.unwrap().database
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("both real ACP providers should exhaust and park the task");
    let parked_cycles = storage
        .work_items()
        .due_recovery_wakes_for_run(attempt.run, i64::MAX, 10)
        .unwrap();
    let due_at_ms = parked_cycles
        .first()
        .and_then(|cycle| cycle.wake.as_ref())
        .expect("exhausted quota must retain its durable wake")
        .due_at_ms();
    let registry_before_wake = storage.get_run(&attempt.run).await.unwrap();
    assert_eq!(
        storage
            .work_items()
            .for_run(attempt.run)
            .unwrap()
            .unwrap()
            .state,
        WorkItemAttemptState::Suspended
    );
    assert_eq!(
        wire(home.path(), "quota-a")
            .iter()
            .filter(|row| row["operation"] == "prompt")
            .count(),
        1,
        "primary must actually hit its typed quota response before parking"
    );
    assert_eq!(
        wire(home.path(), "quota-b")
            .iter()
            .filter(|row| row["operation"] == "prompt")
            .count(),
        1,
        "fallback must actually hit its typed quota response before parking"
    );

    clock.set(due_at_ms);
    let completed_result = tokio::time::timeout(Duration::from_secs(12), async {
        loop {
            let inspected = storage.inspect_run(attempt.run).await.unwrap();
            if let surge_persistence::runs::inspection::RunDatabaseInspection::Present { events } =
                inspected.database
                && events.iter().any(|row| {
                    matches!(
                        row.payload.payload,
                        surge_core::EventPayload::RunCompleted { .. }
                    )
                })
            {
                break events;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    let completed_events = match completed_result {
        Ok(events) => events,
        Err(_) => {
            let attempt_state = storage.work_items().for_run(attempt.run).unwrap();
            let control = storage.work_items().execution_control(attempt.run).unwrap();
            let run_registry = storage.get_run(&attempt.run).await.unwrap();
            let due = storage
                .work_items()
                .due_recovery_wakes_for_run(attempt.run, clock.now_ms(), 10)
                .unwrap();
            let quota_a_wire = wire(home.path(), "quota-a");
            let quota_b_wire = wire(home.path(), "quota-b");
            let marker_state = ["quota-a", "quota-b"]
                .map(|runtime| home.path().join(format!("{runtime}-failed-once")).exists());
            let events = storage.inspect_run(attempt.run).await.unwrap().database;
            let recent = match events {
                surge_persistence::runs::inspection::RunDatabaseInspection::Present { events } => {
                    events
                        .iter()
                        .rev()
                        .take(16)
                        .map(|event| match &event.payload.payload {
                            surge_core::EventPayload::StageFailed { node, reason, .. } => {
                                format!("{} {node}: {reason}", event.seq.0)
                            },
                            surge_core::EventPayload::RunParked {
                                reason, wake_at, ..
                            } => {
                                format!("{} parked until {wake_at}: {reason}", event.seq.0)
                            },
                            surge_core::EventPayload::RunSuspended { fence } => {
                                format!("{} suspended {:?}", event.seq.0, fence.reason)
                            },
                            _ => format!("{} {}", event.seq.0, event.kind),
                        })
                        .collect::<Vec<_>>()
                },
                _ => Vec::new(),
            };
            let attempt_summary = attempt_state
                .as_ref()
                .map(|attempt| (&attempt.state, &attempt.diagnostic));
            let control_summary = control.as_ref().map(|control| {
                (
                    control.generation,
                    control.state,
                    control.fence.as_ref().map(|fence| &fence.reason),
                )
            });
            let due_summary = due
                .iter()
                .map(|cycle| {
                    (
                        cycle.generation,
                        cycle.selected_runtime.as_deref(),
                        cycle.wake.as_ref().map(|wake| wake.due_at_ms()),
                    )
                })
                .collect::<Vec<_>>();
            panic!(
                "wake scheduler failed; pre_wake_status={:?}, run_status={:?}, attempt={attempt_summary:?}, control={control_summary:?}, due={due_summary:?}, wire_prompts={:?}/{:?}, markers={marker_state:?}, events={recent:?}",
                registry_before_wake.as_ref().map(|run| run.status),
                run_registry.as_ref().map(|run| run.status),
                quota_a_wire
                    .iter()
                    .filter(|row| row["operation"] == "prompt")
                    .count(),
                quota_b_wire
                    .iter()
                    .filter(|row| row["operation"] == "prompt")
                    .count()
            );
        },
    };
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if storage
                .work_items()
                .for_run(attempt.run)
                .unwrap()
                .is_some_and(|current| current.state == WorkItemAttemptState::Completed)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("task settlement must follow durable RunCompleted evidence");
    assert_eq!(
        storage
            .work_items()
            .for_run(attempt.run)
            .unwrap()
            .unwrap()
            .state,
        WorkItemAttemptState::Completed
    );
    assert!(
        wire(home.path(), "quota-a")
            .iter()
            .filter(|row| row["operation"] == "prompt")
            .count()
            > 1
            || wire(home.path(), "quota-b")
                .iter()
                .filter(|row| row["operation"] == "prompt")
                .count()
                > 1,
        "automatic wake must issue a new prompt after the two exhausted candidates"
    );
    let opened: Vec<_> = completed_events
        .iter()
        .filter_map(|row| match &row.payload.payload {
            surge_core::EventPayload::SessionOpened {
                opened: Some(opened),
                ..
            } => Some(opened),
            _ => None,
        })
        .collect();
    assert!(
        opened.len() >= 3,
        "wake must durably record a new provider session"
    );
    let admissions: i64 = storage
        .acquire_registry_conn()
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM recipe_opening_admissions",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        usize::try_from(admissions).unwrap(),
        opened.len(),
        "legacy constructor path admits primary, fallback and automatic continuation exactly once"
    );
    assert!(opened.iter().all(|opened| {
        opened.descriptor.cwd() == storage.work_items().show(item).unwrap().item.workspace.path
    }));
    assert_eq!(
        storage
            .work_items()
            .scan_attempts(None, 100)
            .unwrap()
            .entries
            .len(),
        1,
        "automatic capacity wake must resume the existing task attempt"
    );

    let _ = engine
        .stop_run(attempt.run, "quota wake fixture cleanup".into())
        .await;
    cancel.cancel();
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), scheduler_task)
        .await
        .unwrap()
        .unwrap();
}
