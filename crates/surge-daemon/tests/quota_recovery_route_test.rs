//! Controlled ACP protocol oracle; it does not establish production containment.
#[path = "support/cold_host.rs"]
mod cold_host;
#[path = "support/host_budget_bridge.rs"]
mod host_budget_bridge;
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
    b_sessions: tokio::sync::Mutex<std::collections::HashSet<SessionId>>,
    b_closes: AtomicUsize,
    warmup_sessions: tokio::sync::Mutex<std::collections::HashSet<SessionId>>,
    change_after_warmup: tokio::sync::Mutex<Option<std::path::PathBuf>>,
    reset_b_on_second_close: Option<std::path::PathBuf>,
}
#[async_trait::async_trait]
impl BridgeFacade for ObservedBridge {
    async fn open_session(
        &self,
        config: SessionConfig,
    ) -> Result<surge_core::execution_recovery::OpenedSession, OpenSessionError> {
        assert_eq!(
            config.env.get("SURGE_QUOTA_ORACLE_AUTH"),
            Some(&format!("env-secret-sentinel-{}", config.runtime)),
            "exact frozen explicit environment reaches actual ACP opening"
        );
        let primary = config.runtime == "quota-a";
        let secondary = config.runtime == "quota-b";
        let warmup = config.runtime == "quota-c";
        let opened = self.bridge.open_session(config).await?;
        if secondary {
            self.b_sessions.lock().await.insert(opened.session);
        }
        if warmup {
            self.warmup_sessions.lock().await.insert(opened.session);
        }
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
        self.bridge.close_session(id).await?;
        if self.warmup_sessions.lock().await.remove(&id)
            && let Some(path) = &*self.change_after_warmup.lock().await
        {
            std::fs::write(path, b"changed-after-frozen-start-secret").unwrap();
        }
        if self.b_sessions.lock().await.remove(&id)
            && self.b_closes.fetch_add(1, Ordering::SeqCst) == 1
            && let Some(path) = &self.reset_b_on_second_close
        {
            std::fs::remove_file(path).unwrap();
        }
        Ok(())
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
recommended_model = "sonnet"
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

fn engine_config(home: &Path, all_exhausted: bool) -> EngineConfig {
    use surge_core::capacity::{CapacityPolicy, RotationPolicy};
    let profiles = home.join("profiles");
    std::fs::create_dir_all(&profiles).unwrap();
    if !profiles.join("quota-primary-1.0.toml").exists() {
        write_profile(&profiles, "quota-primary", "quota-a");
        write_profile(&profiles, "quota-fallback", "quota-b");
        write_profile(&profiles, "quota-warmup", "quota-c");
    }
    let binary = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/mock_acp_agent");
    assert!(binary.exists(), "build real mock_acp_agent prerequisite");
    let mut agents = std::collections::HashMap::new();
    for (runtime, scenario) in [
        ("quota-a", "prompt_error=429_retry_after"),
        (
            "quota-b",
            if all_exhausted {
                "prompt_error=429_retry_after"
            } else {
                "report_outcome=done"
            },
        ),
        ("quota-c", "report_outcome=done"),
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
        if all_exhausted {
            flags.push("--prompt-error-once".into());
            flags.push(
                home.join(format!("{runtime}-failed-once"))
                    .display()
                    .to_string(),
            );
        }
        flags.push("--pid-file".into());
        flags.push(
            home.join(format!("{runtime}-provider.pid"))
                .display()
                .to_string(),
        );
        if runtime == "quota-a"
            && matches!(
                std::env::var("SURGE_QUOTA_PARK_PHASE").as_deref(),
                Ok("opening" | "contain-opening")
            )
        {
            flags.push("--stall-new-session".into());
        }
        flags.push("--stage-mcp".into());
        flags.push("--config-options".into());
        flags.push("--usage".into());
        let auth_file = home.join(format!("{runtime}-auth-source"));
        if !auth_file.exists() {
            std::fs::write(&auth_file, format!("file-secret-sentinel-{runtime}")).unwrap();
        }
        let config: surge_core::config::AgentConfig =
            serde_json::from_value(json!({"command":binary,"args":flags,"env":{"SURGE_QUOTA_ORACLE_AUTH":format!("env-secret-sentinel-{runtime}")},"capacity_route":{"provider_family":"quota-fixture","configured_route":runtime,"auth_sources":[{"kind":"env","target_key":"SURGE_QUOTA_ORACLE_AUTH"},{"kind":"file","path":auth_file}],"completeness":"complete_configured_sources"}})).unwrap();
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
        .split_inclusive('\n')
        .filter(|line| line.ends_with('\n'))
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn task_owned_quota_dispatches_actual_a_then_b_in_the_same_workspace() {
    quota_dispatch_fixture(false, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn normal_task_start_skips_fresh_exhausted_primary_before_any_provider_effect() {
    quota_dispatch_fixture(true, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn normal_task_all_fresh_exhausted_parks_without_opening_any_provider() {
    quota_dispatch_fixture(true, true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn normal_task_mixed_skip_and_actual_429_never_reopens_primary() {
    quota_dispatch_fixture_mode(true, true, true).await;
}

async fn quota_dispatch_fixture(check_predispatch: bool, all_exhausted: bool) {
    quota_dispatch_fixture_mode(check_predispatch, all_exhausted, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn planned_host_budget_retains_prior_charge_and_aborts_at_frozen_cap() {
    quota_dispatch_fixture_budget(true, true, false, Some(1000)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn planned_host_budget_positive_cap_completes_without_repeating_warmup() {
    quota_dispatch_fixture_budget(true, true, false, Some(1500)).await;
}

async fn quota_dispatch_fixture_mode(check_predispatch: bool, all_exhausted: bool, mixed: bool) {
    quota_dispatch_fixture_budget(check_predispatch, all_exhausted, mixed, None).await;
}

async fn quota_dispatch_fixture_budget(
    check_predispatch: bool,
    all_exhausted: bool,
    mixed: bool,
    budget_cap: Option<u64>,
) {
    quota_dispatch_fixture_source(
        check_predispatch,
        all_exhausted,
        mixed,
        budget_cap,
        SourceFixtureOptions {
            source_changed: false,
            change_after_warmup: None,
            cross_project: false,
            relative_cwd: false,
            preparation_failure: None,
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_429_after_declared_source_change_is_audit_only_and_primary_is_attempted_again() {
    quota_dispatch_fixture_source(
        true,
        false,
        false,
        None,
        SourceFixtureOptions {
            source_changed: true,
            change_after_warmup: None,
            cross_project: false,
            relative_cwd: false,
            preparation_failure: None,
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn selected_route_source_change_after_freeze_prevents_any_provider_effect() {
    quota_dispatch_fixture_source(
        true,
        false,
        false,
        None,
        SourceFixtureOptions {
            source_changed: false,
            change_after_warmup: Some("quota-b"),
            cross_project: false,
            relative_cwd: false,
            preparation_failure: None,
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn skipped_route_source_change_after_freeze_prevents_fallback_effect() {
    quota_dispatch_fixture_source(
        true,
        false,
        false,
        None,
        SourceFixtureOptions {
            source_changed: false,
            change_after_warmup: Some("quota-a"),
            cross_project: false,
            relative_cwd: false,
            preparation_failure: None,
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn skipped_route_source_change_prevents_stale_all_exhausted_park() {
    quota_dispatch_fixture_source(
        true,
        true,
        false,
        None,
        SourceFixtureOptions {
            source_changed: false,
            change_after_warmup: Some("quota-a"),
            cross_project: false,
            relative_cwd: false,
            preparation_failure: None,
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pinned_exhaustion_from_other_project_cannot_skip_primary() {
    quota_dispatch_fixture_source(
        true,
        false,
        false,
        None,
        SourceFixtureOptions {
            source_changed: false,
            change_after_warmup: None,
            cross_project: true,
            relative_cwd: false,
            preparation_failure: None,
        },
    )
    .await;
}

#[derive(Default)]
struct SourceFixtureOptions {
    source_changed: bool,
    change_after_warmup: Option<&'static str>,
    cross_project: bool,
    relative_cwd: bool,
    preparation_failure: Option<RelativePreparationFailure>,
}

async fn quota_dispatch_fixture_source(
    check_predispatch: bool,
    all_exhausted: bool,
    mixed: bool,
    budget_cap: Option<u64>,
    source_options: SourceFixtureOptions,
) {
    let SourceFixtureOptions {
        source_changed,
        change_after_warmup,
        cross_project,
        relative_cwd,
        preparation_failure,
    } = source_options;
    use surge_core::{
        Graph,
        id::{WorkItemId, WorkItemOperationId},
        work_item::{WorkItemCommand, WorkItemRequirements, WorkItemResult},
    };
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    initialize_project(project.path());
    let other_project = tempfile::tempdir().unwrap();
    if cross_project {
        initialize_project(other_project.path());
    }
    if relative_cwd {
        std::fs::write(project.path().join(".git/info/exclude"), b".surge-auth/\n").unwrap();
    }
    let storage = Storage::open(home.path()).await.unwrap();
    let cancel = CancellationToken::new();
    let bridge = Arc::new(ObservedBridge {
        bridge: AcpBridge::with_defaults().unwrap(),
        quota_errors: AtomicUsize::new(0),
        primary_session: tokio::sync::Mutex::new(None),
        primary_ready: tokio::sync::Notify::new(),
        primary_continue: tokio::sync::Notify::new(),
        b_sessions: tokio::sync::Mutex::new(std::collections::HashSet::new()),
        b_closes: AtomicUsize::new(0),
        warmup_sessions: tokio::sync::Mutex::new(std::collections::HashSet::new()),
        change_after_warmup: tokio::sync::Mutex::new(
            change_after_warmup.map(|runtime| home.path().join(format!("{runtime}-auth-source"))),
        ),
        reset_b_on_second_close: mixed.then(|| home.path().join("quota-b-failed-once")),
    });
    let host_budget =
        host_budget_bridge::HostBudgetBridge::new(bridge.clone(), storage.clone(), cancel.clone());
    let engine = Arc::new(Engine::new_full(
        host_budget.clone(),
        storage.clone(),
        Arc::new(WorktreeToolDispatcher::new(project.path().into())),
        Arc::new(surge_notify::MultiplexingNotifier::new()),
        None,
        None,
        if relative_cwd {
            relative_source_engine_config(home.path(), all_exhausted)
        } else {
            engine_config(home.path(), all_exhausted)
        },
    ));
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
        operation_id: if let Ok(item) = std::env::var("SURGE_START_PREPARATION_ITEM") {
            let item: WorkItemId = item.parse().unwrap();
            item.as_ulid().to_string().parse().unwrap()
        } else {
            WorkItemOperationId::new()
        },
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
    if relative_cwd {
        prepare_relative_source_fixture(&workspace);
    }
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
    let mut start_version = detail.item.version;
    if let Some(failure) = preparation_failure {
        match failure {
            RelativePreparationFailure::Dirty => {
                std::fs::write(
                    detail.item.workspace.path.join("user-dirty.rs"),
                    "user staged bytes",
                )
                .unwrap();
                assert!(
                    std::process::Command::new("git")
                        .args(["add", "user-dirty.rs"])
                        .current_dir(&detail.item.workspace.path)
                        .status()
                        .unwrap()
                        .success()
                );
            },
            RelativePreparationFailure::Foreign => {
                std::fs::write(
                    detail.item.workspace.path.join(".git"),
                    format!("gitdir: {}\n", detail.item.workspace.repository.display()),
                )
                .unwrap();
                std::fs::write(
                    detail.item.workspace.path.join("foreign.marker"),
                    "retain foreign files",
                )
                .unwrap();
            },
            RelativePreparationFailure::MissingPrepared => {
                let prior = WorkItemCommand::Start {
                    operation_id: WorkItemOperationId::new(),
                    item,
                    expected_version: start_version,
                    graph: Box::new(graph.clone()),
                    quota_recovery: None,
                };
                let store = storage.work_items();
                let WorkItemResult::Attempt(prior) =
                    store.mutate(&prior, None, Some("{}"), "host", 0).unwrap()
                else {
                    panic!("prior");
                };
                let claim = store.claim(prior.run).unwrap();
                store.mark_workspace_prepared(&claim).unwrap();
                store
                    .settle(
                        prior.run,
                        prior.binding.generation,
                        surge_core::work_item::WorkItemAttemptState::Failed,
                        Some("prior attempt failed before provider".into()),
                    )
                    .unwrap();
                drop(claim);
                start_version = store.show(item).unwrap().item.version;
                std::fs::remove_dir_all(&detail.item.workspace.path).unwrap();
            },
        }
    }
    let retained_git = (relative_cwd
        && !matches!(
            preparation_failure,
            Some(RelativePreparationFailure::MissingPrepared | RelativePreparationFailure::Foreign)
        ))
    .then(|| {
        (
            relative_fixture_git_state(&detail.item.workspace.path),
            relative_fixture_git_state(&detail.item.workspace.checkout),
        )
    });
    let start = WorkItemCommand::Start {
        operation_id: WorkItemOperationId::new(),
        item,
        expected_version: start_version,
        graph: Box::new(graph.clone()),
        quota_recovery: (!relative_cwd).then_some(quota_policy),
    };
    if let Some(path) = std::env::var_os("SURGE_START_PREPARATION_MANIFEST") {
        let temporary = std::path::Path::new(&path).with_extension("publishing");
        std::fs::write(
            &temporary,
            serde_json::to_vec(
                &json!({"home":home.path(),"project":project.path(),"socket":socket,"item":item,"command":start}),
            )
            .unwrap(),
        )
        .unwrap();
        std::fs::rename(temporary, path).unwrap();
    }
    let started = request(&socket, serde_json::to_value(start).unwrap()).await;
    if let Some(failure) = preparation_failure {
        assert_eq!(started["method"], "error", "{started}");
        assert!(wire(home.path(), "quota-a").is_empty());
        assert!(wire(home.path(), "quota-b").is_empty());
        assert!(
            storage
                .work_items()
                .show(item)
                .unwrap()
                .item
                .active_run
                .is_none()
        );
        assert_eq!(
            storage.work_items().workspace_prepared(item).unwrap(),
            matches!(failure, RelativePreparationFailure::MissingPrepared)
        );
        if let Some((worktree, checkout)) = retained_git {
            assert_eq!(
                relative_fixture_git_state(&detail.item.workspace.path),
                worktree
            );
            assert_eq!(
                relative_fixture_git_state(&detail.item.workspace.checkout),
                checkout
            );
            assert_eq!(
                std::fs::read(detail.item.workspace.path.join("user-dirty.rs")).unwrap(),
                b"user staged bytes"
            );
        }
        if matches!(failure, RelativePreparationFailure::MissingPrepared) {
            assert!(!detail.item.workspace.path.exists());
        }
        if matches!(failure, RelativePreparationFailure::Foreign) {
            assert_eq!(
                std::fs::read(detail.item.workspace.path.join("foreign.marker")).unwrap(),
                b"retain foreign files"
            );
        }
        cancel.cancel();
        tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        return;
    }
    assert_eq!(started["method"], "work_item_ok", "{started}");
    if let Some(path) = std::env::var_os("SURGE_START_PREPARATION_ACCEPTED") {
        let path = std::path::PathBuf::from(path);
        let temporary = path.with_extension("publishing");
        std::fs::write(&temporary, serde_json::to_vec(&started).unwrap()).unwrap();
        std::fs::rename(temporary, &path).unwrap();
        tokio::time::timeout(Duration::from_secs(10), async {
            while !path.with_extension("continue").exists() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
    }
    let result: WorkItemResult = serde_json::from_value(started["result"].clone()).unwrap();
    let WorkItemResult::Attempt(attempt) = result else {
        panic!("actual start must return attempt")
    };
    let host_config: Value = serde_json::from_str(&attempt.config).unwrap();
    let frozen_policy: surge_persistence::work_items::recovery_cycles::FrozenQuotaPolicy =
        serde_json::from_value(host_config["quota_recovery"].clone()).unwrap();
    if std::env::var("SURGE_QUOTA_PARK_PHASE").as_deref() == Ok("opening") {
        tokio::time::timeout(Duration::from_secs(5), async {
            while !wire(home.path(), "quota-a")
                .iter()
                .any(|row| row["operation"] == "new_session")
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        publish_selected_crash_boundary(
            &storage,
            home.path(),
            project.path(),
            item,
            &attempt,
            "opening",
        )
        .await;
        std::future::pending::<()>().await;
    }
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
    if std::env::var("SURGE_QUOTA_PARK_PHASE").as_deref() == Ok("opened") {
        publish_selected_crash_boundary(
            &storage,
            home.path(),
            project.path(),
            item,
            &attempt,
            "opened",
        )
        .await;
        std::future::pending::<()>().await;
    }
    assert!(
        storage.work_items().workspace_prepared(item).unwrap(),
        "real host must prepare and record workspace before first prompt"
    );
    if let Some((worktree, checkout)) = retained_git {
        assert_eq!(
            relative_fixture_git_state(&detail.item.workspace.path),
            worktree,
            "preparation must preserve raw worktree index and HEAD"
        );
        assert_eq!(
            relative_fixture_git_state(&detail.item.workspace.checkout),
            checkout,
            "preparation must preserve raw checkout index and HEAD"
        );
    }

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
    if source_changed {
        std::fs::write(
            if relative_cwd {
                workspace.path.join(".surge-auth/quota-a")
            } else {
                home.path().join("quota-a-auth-source")
            },
            b"changed-file-secret-sentinel-quota-a",
        )
        .unwrap();
    }
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
                    ) || (all_exhausted
                        && matches!(
                            row.payload.payload,
                            surge_core::EventPayload::RunSuspended { .. }
                        ))
                })
            {
                break events;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    let diagnostic = storage.inspect_run(attempt.run).await.unwrap();
    if check_predispatch {
        assert!(
            completed.is_ok(),
            "real typed primary seed and fallback must complete"
        );
        if source_changed {
            let conn = storage.acquire_registry_conn().unwrap();
            let row:(Option<String>,Option<String>)=conn.query_row("SELECT exhaustion_receipt,configured_pin FROM recipe_opening_admissions WHERE runtime='quota-a' ORDER BY epoch DESC LIMIT 1",[],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
            assert!(
                row.0.is_some(),
                "actual typed rate limit must remain sealed for audit"
            );
            assert!(
                row.1.is_none(),
                "changed declared source cannot publish a reusable pin"
            );
        }
        let before_a = wire(home.path(), "quota-a").len();
        let before_b = wire(home.path(), "quota-b").len();
        let created = request(
            &socket,
            serde_json::to_value(WorkItemCommand::Create {
                operation_id: WorkItemOperationId::new(),
                project: if cross_project {
                    other_project.path()
                } else {
                    project.path()
                }
                .into(),
                title: "Second ordinary task".into(),
                requirements: requirements.clone(),
            })
            .unwrap(),
        )
        .await;
        assert_eq!(created["method"], "work_item_ok");
        let next_item: WorkItemId =
            serde_json::from_value(created["result"]["value"]["item"]["id"].clone()).unwrap();
        let current = storage.work_items().show(next_item).unwrap();
        if relative_cwd {
            prepare_relative_source_fixture(&current.item.workspace);
            if let Some(runtime) = change_after_warmup {
                *bridge.change_after_warmup.lock().await = Some(
                    current
                        .item
                        .workspace
                        .path
                        .join(".surge-auth")
                        .join(runtime),
                );
            }
        }
        let mut next_graph = graph;
        if all_exhausted || change_after_warmup.is_some() {
            let original = next_graph.start.clone();
            let warmup: surge_core::NodeKey = "warmup".try_into().unwrap();
            let mut node = next_graph.nodes[&original].clone();
            node.id = warmup.clone();
            let surge_core::node::NodeConfig::Agent(agent) = &mut node.config else {
                panic!("agent node")
            };
            agent.profile = if mixed {
                "quota-fallback@1.0"
            } else {
                "quota-warmup@1.0"
            }
            .try_into()
            .unwrap();
            if mixed {
                agent.custom_fields.insert(
                    "runtime".into(),
                    toml::from_str::<toml::Value>(r#"model = "opus""#).unwrap(),
                );
            }
            next_graph.nodes.insert(warmup.clone(), node);
            let mut edge = next_graph.edges[0].clone();
            edge.id = "warmup_to_impl".try_into().unwrap();
            edge.from.node = warmup.clone();
            edge.to = original;
            next_graph.edges.push(edge);
            next_graph.start = warmup;
        }
        let start = WorkItemCommand::Start {
            operation_id: WorkItemOperationId::new(),
            item: next_item,
            expected_version: current.item.version,
            graph: Box::new(next_graph.clone()),
            quota_recovery: None,
        };
        if let Some(cap) = budget_cap {
            let config = surge_orchestrator::engine::EngineRunConfig {
                budget: surge_core::budget::BudgetGuard {
                    limits: surge_core::budget::BudgetLimits {
                        usd: None,
                        tokens: Some(cap),
                        warn_threshold_pct: 50,
                    },
                    policy: surge_core::budget::BudgetPolicy::Abort,
                },
                ..surge_orchestrator::engine::EngineRunConfig::default()
            };
            let frozen = engine
                .freeze_work_item_config(&next_graph, config, &current.item.workspace.path)
                .await
                .unwrap();
            let config = serde_json::to_string(&frozen).unwrap();
            let reserved = storage
                .work_items()
                .mutate(
                    &start,
                    None,
                    Some(&config),
                    "host-budget-oracle",
                    chrono::Utc::now().timestamp_millis(),
                )
                .unwrap();
            let WorkItemResult::Attempt(reserved) = reserved else {
                panic!("host reservation required")
            };
            host_budget
                .designate(reserved.run, current.item.workspace.path.clone(), 600)
                .await
                .unwrap();
        }
        if source_changed || cross_project {
            bridge.primary_continue.notify_one();
        }
        let response = request(&socket, serde_json::to_value(start).unwrap()).await;
        assert_eq!(response["method"], "work_item_ok", "{response}");
        let result: WorkItemResult = serde_json::from_value(response["result"].clone()).unwrap();
        let WorkItemResult::Attempt(next) = result else {
            panic!("start attempt required")
        };
        let finished = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let journal = storage.inspect_run(next.run).await.unwrap();
                if let surge_persistence::runs::inspection::RunDatabaseInspection::Present {
                    events,
                } = journal.database
                    && events.iter().any(|row| {
                        matches!(
                            row.payload.payload,
                            surge_core::EventPayload::RunCompleted { .. }
                        ) || (all_exhausted
                            && matches!(
                                row.payload.payload,
                                surge_core::EventPayload::RunSuspended { .. }
                            ))
                            || (change_after_warmup.is_some()
                                && matches!(
                                    row.payload.payload,
                                    surge_core::EventPayload::RunRecoveryRequired { .. }
                                ))
                    })
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await;
        let a = wire(home.path(), "quota-a");
        let b = wire(home.path(), "quota-b");
        let next_journal = storage.inspect_run(next.run).await.unwrap();
        let failures = match &next_journal.database {
            surge_persistence::runs::inspection::RunDatabaseInspection::Present { events } => {
                events
                    .iter()
                    .filter_map(|row| match &row.payload.payload {
                        surge_core::EventPayload::RunRecoveryRequired { diagnostic, .. } => {
                            Some(diagnostic.clone())
                        },
                        surge_core::EventPayload::RunFailed { error } => Some(error.clone()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
            },
            _ => Vec::new(),
        };
        if change_after_warmup.is_some() {
            assert!(
                finished.is_ok(),
                "changed source must require reconciliation: {failures:?}"
            );
            assert_eq!(
                a.len(),
                before_a,
                "changed selected or skipped source must prevent primary RPC"
            );
            assert_eq!(
                b.len(),
                before_b,
                "changed selected or skipped source must prevent fallback RPC"
            );
            let surge_persistence::runs::inspection::RunDatabaseInspection::Present { events } =
                &next_journal.database
            else {
                panic!("journal required")
            };
            assert!(events.iter().any(|row| matches!(
                row.payload.payload,
                surge_core::EventPayload::RunRecoveryRequired { .. }
            )));
            assert!(
                !events.iter().any(|row| matches!(
                    row.payload.payload,
                    surge_core::EventPayload::RunSuspended { .. }
                )),
                "changed source cannot seal a stale capacity suspension"
            );
            assert!(
                storage
                    .work_items()
                    .due_recovery_wakes_for_run(next.run, i64::MAX, 10)
                    .unwrap()
                    .is_empty()
            );
            cancel.cancel();
            tokio::time::timeout(Duration::from_secs(5), server)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            return;
        }
        assert!(
            finished.is_ok(),
            "normal start must finish or durably park according to exact capacity proof: {failures:?}"
        );
        if all_exhausted {
            use surge_persistence::runs::MockClock;
            let due = wait_for_durable_capacity_wake(&storage, next.run).await;
            // Settle the real durable usage projection before freezing the process
            // readiness proof; startup reconciliation may otherwise advance its cursor.
            storage.work_items().sync_usage(next.run).unwrap();
            let parked = storage.work_items().for_run(next.run).unwrap().unwrap();
            if let Some(cap) = budget_cap {
                let frozen: surge_orchestrator::engine::EngineRunConfig =
                    serde_json::from_str(&parked.config).unwrap();
                assert_eq!(frozen.budget.limits.tokens, Some(cap));
                let fold = storage
                    .inspect_folded_run(next.run)
                    .await
                    .unwrap()
                    .database
                    .unwrap();
                let surge_core::RunState::Pipeline { memory, .. } = fold.state else {
                    panic!("parked pipeline")
                };
                assert_eq!(
                    memory.costs.tokens_in + memory.costs.tokens_out,
                    600,
                    "explicit host usage oracle must be retained before planned park"
                );
            }

            let parked_workspace = storage.work_items().show(next_item).unwrap().item.workspace;
            let retained = parked_workspace.path.join("planned-wake-retained.txt");
            std::fs::write(&retained, b"preserve all-skipped workspace").unwrap();
            let before_control = storage.work_items().execution_control(next.run).unwrap();
            assert_eq!(wire(home.path(), "quota-a").len(), before_a);
            assert_eq!(
                wire(home.path(), "quota-b")
                    .iter()
                    .skip(before_b)
                    .filter(|row| row["operation"] == "new_session")
                    .count(),
                if mixed { 2 } else { 0 }
            );
            // Stop only the separate source task; the all-skipped task stays parked.
            let seed = storage.work_items().show(item).unwrap();
            let manual_stop = request(
                &socket,
                serde_json::to_value(WorkItemCommand::Suspend {
                    operation_id: WorkItemOperationId::new(),
                    item,
                    expected_version: seed.item.version,
                })
                .unwrap(),
            )
            .await;
            assert_eq!(
                manual_stop["method"], "work_item_ok",
                "manual source-task suspension must invalidate its wake: {manual_stop}"
            );
            assert!(
                storage
                    .work_items()
                    .due_recovery_wakes_for_run(attempt.run, i64::MAX, 10)
                    .unwrap()
                    .is_empty()
            );
            if std::env::var("SURGE_QUOTA_PARK_PHASE").as_deref() == Ok("park") {
                let ready =
                    std::path::PathBuf::from(std::env::var_os("SURGE_QUOTA_PARK_READY").unwrap());
                let temporary = ready.with_extension("publishing");
                std::fs::write(&temporary,serde_json::to_vec(&json!({"home":home.path(),"project":project.path(),"item":next_item,"run":next.run,"attempt":parked,"control":before_control,"due":due,"workspace":parked_workspace,"a_count":before_a,"b_count":before_b})).unwrap()).unwrap();
                std::fs::rename(temporary, ready).unwrap();
                std::future::pending::<()>().await;
            }
            bridge.primary_continue.notify_one();
            // A fresh engine and ACP bridge must restore the real persisted plan,
            // frozen routing snapshot and task workspace rather than in-memory state.
            let restored_budget = host_budget_bridge::HostBudgetBridge::new(
                Arc::new(AcpBridge::with_defaults().unwrap()),
                storage.clone(),
                cancel.clone(),
            );
            if budget_cap.is_some() {
                restored_budget
                    .designate(next.run, parked_workspace.path.clone(), 600)
                    .await
                    .unwrap();
            }
            let restored_engine = Arc::new(Engine::new_full(
                restored_budget,
                storage.clone(),
                Arc::new(WorktreeToolDispatcher::new(project.path().into())),
                Arc::new(surge_notify::MultiplexingNotifier::new()),
                None,
                None,
                engine_config(home.path(), true),
            ));
            let clock = Arc::new(MockClock::new(due));
            let scheduler = surge_daemon::wake_scheduler::WakeScheduler {
                tracking: surge_daemon::tracked_run::TrackingContext::new(
                    restored_engine.clone(),
                    storage.clone(),
                ),
                storage: storage.clone(),
                facade: Arc::new(LocalEngineFacade::new(restored_engine.clone())),
                admission: Arc::new(surge_daemon::admission::AdmissionController::new(2, 2)),
                broadcast: Arc::new(surge_daemon::broadcast::BroadcastRegistry::new()),
                clock,
                notifier: Arc::new(surge_notify::MultiplexingNotifier::new()),
                blind_park_limit: 100,
                poll_interval: Duration::from_millis(10),
            };
            let scheduler_task = tokio::spawn(scheduler.run(cancel.clone()));
            let awakened = tokio::time::timeout(
                Duration::from_secs(5),
                wait_for_terminal_journal(&storage, next.run),
            )
            .await;
            let after_control = storage.work_items().execution_control(next.run).unwrap();
            let after_journal = storage.inspect_run(next.run).await.unwrap();
            if let surge_persistence::runs::inspection::RunDatabaseInspection::Present { events } =
                &after_journal.database
            {
                if let Some(cap) = budget_cap {
                    assert_retained_budget_result(events, cap);
                }

                assert_eq!(events.iter().filter(|row|matches!(&row.payload.payload,surge_core::EventPayload::StageEntered {node,..} if node.as_str()=="warmup")).count(),1,"wake cannot repeat prior authenticated stage");
            }
            let diagnostics = match after_journal.database {
                surge_persistence::runs::inspection::RunDatabaseInspection::Present { events } => {
                    events
                        .into_iter()
                        .filter_map(|row| match row.payload.payload {
                            surge_core::EventPayload::RunRecoveryRequired {
                                diagnostic, ..
                            } => Some(diagnostic),
                            surge_core::EventPayload::RunFailed { error } => Some(error),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                },
                _ => Vec::new(),
            };
            assert!(
                awakened.is_ok(),
                "planned parked task must wake and complete: before={before_control:?}, after={after_control:?}, diagnostics={diagnostics:?}"
            );
            let current = storage.work_items().for_run(next.run).unwrap().unwrap();
            assert_eq!(current.run, parked.run, "wake must preserve task attempt");
            assert_eq!(
                current.config, parked.config,
                "wake must retain frozen budget and configuration"
            );
            assert_eq!(
                storage.work_items().show(next_item).unwrap().item.workspace,
                parked_workspace
            );
            assert_eq!(
                std::fs::read(&retained).unwrap(),
                b"preserve all-skipped workspace"
            );
            let a_now = wire(home.path(), "quota-a");
            let b_now = wire(home.path(), "quota-b");
            assert_eq!(
                a_now[before_a..]
                    .iter()
                    .filter(|row| row["operation"] == "new_session")
                    .count()
                    + b_now[before_b..]
                        .iter()
                        .filter(|row| row["operation"] == "new_session")
                        .count(),
                if mixed { 3 } else { 1 }
            );
            assert_eq!(
                a_now[before_a..]
                    .iter()
                    .filter(|row| row["operation"] == "prompt")
                    .count()
                    + b_now[before_b..]
                        .iter()
                        .filter(|row| row["operation"] == "prompt")
                        .count(),
                if mixed { 3 } else { 1 }
            );
            cancel.cancel();
            scheduler_task.await.unwrap();
        }
        let _ = engine
            .stop_run(next.run, "predispatch fixture cleanup".into())
            .await;
        cancel.cancel();
        tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(
            a[before_a..]
                .iter()
                .filter(|row| row["operation"] == "new_session")
                .count(),
            usize::from(source_changed || cross_project),
            "only unchanged trusted primary evidence may suppress an actual opening"
        );
        assert_eq!(
            a[before_a..]
                .iter()
                .filter(|row| row["operation"] == "prompt")
                .count(),
            usize::from(source_changed || cross_project)
        );
        if !source_changed && !cross_project {
            assert!(
                a[before_a..].is_empty(),
                "fresh exhausted A has no provider effects"
            );
        }
        assert_capacity_storage_has_no_source_bytes(&storage);
        let accepted = storage
            .work_items()
            .for_run(next.run)
            .unwrap()
            .unwrap()
            .config;
        let serialized_journal = match &next_journal.database {
            surge_persistence::runs::inspection::RunDatabaseInspection::Present { events } => {
                serde_json::to_string(&events.iter().map(|row| &row.payload).collect::<Vec<_>>())
                    .unwrap()
            },
            _ => panic!("journal required"),
        };
        for sentinel in [
            "env-secret-sentinel",
            "file-secret-sentinel",
            "changed-file-secret-sentinel",
        ] {
            assert!(
                !accepted.contains(sentinel),
                "accepted run config cannot contain credential source bytes"
            );
            assert!(
                !serialized_journal.contains(sentinel),
                "journal cannot contain credential source bytes"
            );
        }
        assert_eq!(
            b[before_b..]
                .iter()
                .filter(|row| row["operation"] == "new_session")
                .count(),
            if mixed {
                2
            } else {
                usize::from(!all_exhausted)
            }
        );
        assert_eq!(
            b[before_b..]
                .iter()
                .filter(|row| row["operation"] == "prompt")
                .count(),
            if mixed {
                2
            } else {
                usize::from(!all_exhausted)
            }
        );
        assert!(
            finished.is_ok(),
            "normal start must finish or durably park according to exact capacity proof: {failures:?}"
        );
        if std::env::var_os("SURGE_START_PREPARATION_MANIFEST").is_some() {
            let _ = home.keep();
            let _ = project.keep();
        }
        return;
    }
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
    assert_eq!(
        final_detail.revision.origin.requirements(),
        Some(&requirements)
    );
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn child_planned_park_restart_probe() {
    let Ok(phase) = std::env::var("SURGE_QUOTA_PARK_PHASE") else {
        return;
    };
    if phase == "reentry" || phase == "reentry-continue" {
        cold_committed_reentry_probe(phase == "reentry").await;
        return;
    }
    if phase == "park" {
        quota_dispatch_fixture(true, true).await;
        panic!("park child must stay alive at durable ready boundary");
    }
    if phase == "opening" || phase == "opened" {
        quota_dispatch_fixture(false, false).await;
        panic!("crash child must retain selected boundary");
    }
    if phase == "contain-opening" || phase == "contain-opened" {
        cold_selected_containment_probe().await;
        return;
    }
    assert_eq!(phase, "wake");
    let ready = std::path::PathBuf::from(std::env::var_os("SURGE_QUOTA_PARK_READY").unwrap());
    let proof: Value = serde_json::from_slice(&std::fs::read(&ready).unwrap()).unwrap();
    let home = std::path::PathBuf::from(proof["home"].as_str().unwrap());
    let project = std::path::PathBuf::from(proof["project"].as_str().unwrap());
    let item: surge_core::id::WorkItemId = serde_json::from_value(proof["item"].clone()).unwrap();
    let run: surge_core::RunId = serde_json::from_value(proof["run"].clone()).unwrap();
    let storage = Storage::open(&home).await.unwrap();
    let parked = storage.work_items().for_run(run).unwrap().unwrap();
    assert_eq!(serde_json::to_value(&parked).unwrap(), proof["attempt"]);
    assert_eq!(
        serde_json::to_value(storage.work_items().execution_control(run).unwrap()).unwrap(),
        proof["control"]
    );
    let cancel = CancellationToken::new();
    let engine = Arc::new(Engine::new_full(
        Arc::new(AcpBridge::with_defaults().unwrap()),
        storage.clone(),
        Arc::new(WorktreeToolDispatcher::new(project)),
        Arc::new(surge_notify::MultiplexingNotifier::new()),
        None,
        None,
        engine_config(&home, true),
    ));
    let admission = Arc::new(surge_daemon::admission::AdmissionController::new(2, 2));
    let broadcast = Arc::new(surge_daemon::broadcast::BroadcastRegistry::new());
    let tracking = surge_daemon::tracked_run::TrackingContext::new(engine.clone(), storage.clone());
    let socket = home.join("quota.sock");
    let server = tokio::spawn(surge_daemon::run_runs_only(
        surge_daemon::ServerConfig {
            socket_path: socket.clone(),
            max_active: 2,
            max_queue: 2,
        },
        Arc::new(LocalEngineFacade::new(engine.clone())),
        tracking.clone(),
        broadcast.clone(),
        admission.clone(),
        cancel.clone(),
    ));
    tokio::time::timeout(Duration::from_secs(5), async {
        while !socket.exists() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    // Startup reconciliation may inspect parked ownership, but cannot open either provider.
    assert_eq!(
        wire(&home, "quota-a").len(),
        proof["a_count"].as_u64().unwrap() as usize
    );
    assert_eq!(
        wire(&home, "quota-b").len(),
        proof["b_count"].as_u64().unwrap() as usize
    );
    assert_eq!(
        storage.work_items().for_run(run).unwrap().unwrap().config,
        parked.config
    );
    let due = proof["due"].as_i64().unwrap();
    assert_eq!(
        storage
            .work_items()
            .due_recovery_wakes_for_run(run, i64::MAX, 10)
            .unwrap()[0]
            .wake
            .as_ref()
            .unwrap()
            .due_at_ms(),
        due
    );
    let scheduler = surge_daemon::wake_scheduler::WakeScheduler {
        storage: storage.clone(),
        facade: Arc::new(LocalEngineFacade::new(engine.clone())),
        admission,
        broadcast,
        tracking,
        clock: Arc::new(surge_persistence::runs::MockClock::new(due)),
        notifier: Arc::new(surge_notify::MultiplexingNotifier::new()),
        blind_park_limit: 100,
        poll_interval: Duration::from_millis(10),
    };
    let scheduler_task = tokio::spawn(scheduler.run(cancel.clone()));
    tokio::time::timeout(Duration::from_secs(8),async {loop {let inspection=storage.inspect_run(run).await.unwrap();if let surge_persistence::runs::inspection::RunDatabaseInspection::Present {events}=inspection.database && events.iter().any(|row|matches!(row.payload.payload,surge_core::EventPayload::RunCompleted {..})) {assert_eq!(events.iter().filter(|row|matches!(&row.payload.payload,surge_core::EventPayload::StageEntered {node,..} if node.as_str()=="warmup")).count(),1);break;}tokio::time::sleep(Duration::from_millis(10)).await;}}).await.unwrap();
    let after = storage.work_items().for_run(run).unwrap().unwrap();
    assert_eq!(after.run, parked.run);
    assert_eq!(after.config, parked.config);
    let workspace = storage.work_items().show(item).unwrap().item.workspace;
    assert_eq!(
        serde_json::to_value(&workspace).unwrap(),
        proof["workspace"]
    );
    assert_eq!(
        std::fs::read(workspace.path.join("planned-wake-retained.txt")).unwrap(),
        b"preserve all-skipped workspace"
    );
    let a = wire(&home, "quota-a");
    let b = wire(&home, "quota-b");
    for operation in ["new_session", "prompt"] {
        assert_eq!(
            a[proof["a_count"].as_u64().unwrap() as usize..]
                .iter()
                .chain(b[proof["b_count"].as_u64().unwrap() as usize..].iter())
                .filter(|row| row["operation"] == operation)
                .count(),
            1,
            "cold process wake must perform one actual provider {operation}"
        );
    }
    cancel.cancel();
    scheduler_task.await.unwrap();
    server.await.unwrap().unwrap();
    let temporary = ready.with_extension("publishing");
    std::fs::write(&temporary, b"cold process wake accepted").unwrap();
    std::fs::rename(temporary, ready.with_extension("woke")).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn planned_no_open_park_survives_killed_host_and_periodic_wake_in_new_process() {
    let diagnostics = tempfile::tempdir().unwrap();
    let ready = diagnostics.path().join("park.json");
    let vars = |phase: &str| {
        vec![
            (
                std::ffi::OsString::from("SURGE_QUOTA_PARK_PHASE"),
                std::ffi::OsString::from(phase),
            ),
            (
                std::ffi::OsString::from("SURGE_QUOTA_PARK_READY"),
                ready.as_os_str().to_os_string(),
            ),
        ]
    };
    let mut first = cold_host::ColdHost::spawn(
        "child_planned_park_restart_probe",
        vars("park"),
        ready.clone(),
        diagnostics.path(),
    )
    .unwrap();
    let json = match first.wait_ready(Duration::from_secs(12)).await {
        Ok(json) => json,
        Err(error) => {
            let retained = diagnostics.keep();
            panic!(
                "first cold host failed: {error}; retained diagnostics {}",
                retained.display()
            );
        },
    };
    let proof: Value = serde_json::from_str(&json).unwrap();
    first.stop_and_wait().unwrap();
    let home = std::path::PathBuf::from(proof["home"].as_str().unwrap());
    let project = std::path::PathBuf::from(proof["project"].as_str().unwrap());
    std::fs::remove_file(home.join("quota.sock")).unwrap();
    let mut second = cold_host::ColdHost::spawn(
        "child_planned_park_restart_probe",
        vars("wake"),
        ready.with_extension("woke"),
        diagnostics.path(),
    )
    .unwrap();
    if let Err(error) = second.wait_ready(Duration::from_secs(12)).await {
        let (out, err) = second.diagnostics_paths();
        let stdout = std::fs::read_to_string(out).unwrap();
        let stderr = std::fs::read_to_string(err).unwrap();
        let retained = diagnostics.keep();
        panic!(
            "cold wake failed: {error}; retained diagnostics {}; stdout={stdout}; stderr={stderr}",
            retained.display()
        );
    }
    second.stop_and_wait().unwrap();
    std::fs::remove_dir_all(home).unwrap();
    std::fs::remove_dir_all(project).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn host_freeze_rejects_different_declared_families_before_provider_effects() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    initialize_project(project.path());
    let storage = Storage::open(home.path()).await.unwrap();
    let mut config = engine_config(home.path(), false);
    let mut agents = config
        .agent_registry
        .as_ref()
        .unwrap()
        .list()
        .iter()
        .map(|entry| (entry.id.clone(), entry.to_agent_config()))
        .collect::<std::collections::HashMap<_, _>>();
    agents
        .get_mut("quota-b")
        .unwrap()
        .capacity_route
        .as_mut()
        .unwrap()
        .provider_family = "different-family".into();
    config.agent_registry = Some(Arc::new(surge_acp::Registry::from_config(agents)));
    let engine = Engine::new_full(
        Arc::new(AcpBridge::with_defaults().unwrap()),
        storage,
        Arc::new(WorktreeToolDispatcher::new(project.path().into())),
        Arc::new(surge_notify::MultiplexingNotifier::new()),
        None,
        None,
        config,
    );
    let mut graph: surge_core::Graph =
        toml::from_str(include_str!("../../../examples/flow_minimal_agent.toml")).unwrap();
    let surge_core::node::NodeConfig::Agent(agent) =
        &mut graph.nodes.get_mut(&graph.start).unwrap().config
    else {
        panic!("agent")
    };
    agent.profile = "quota-primary@1.0".try_into().unwrap();
    assert!(
        engine
            .freeze_work_item_config(
                &graph,
                surge_orchestrator::engine::EngineRunConfig::default(),
                project.path()
            )
            .await
            .is_err()
    );
    assert!(wire(home.path(), "quota-a").is_empty());
    assert!(wire(home.path(), "quota-b").is_empty());
}

fn assert_capacity_storage_has_no_source_bytes(storage: &Storage) {
    let connection = storage.acquire_registry_conn().unwrap();
    let tables = connection
        .prepare("SELECT name FROM sqlite_master WHERE type='table'")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    for table in tables {
        let quoted = table.replace('"', "\"\"");
        let mut statement = connection
            .prepare(&format!("SELECT * FROM \"{quoted}\""))
            .unwrap();
        let columns = statement.column_count();
        let mut rows = statement.query([]).unwrap();
        while let Some(row) = rows.next().unwrap() {
            for column in 0..columns {
                let bytes = match row.get_ref(column).unwrap() {
                    rusqlite::types::ValueRef::Text(value)
                    | rusqlite::types::ValueRef::Blob(value) => value,
                    _ => continue,
                };
                let text = String::from_utf8_lossy(bytes);
                for sentinel in [
                    "env-secret-sentinel",
                    "file-secret-sentinel",
                    "changed-file-secret-sentinel",
                    "changed-after-frozen-start-secret",
                ] {
                    assert!(
                        !text.contains(sentinel),
                        "credential bytes must not occur in durable registry table {table}"
                    );
                }
            }
        }
    }
}

async fn publish_selected_crash_boundary(
    storage: &Arc<Storage>,
    home: &Path,
    project: &Path,
    item: surge_core::id::WorkItemId,
    attempt: &surge_core::work_item::WorkItemAttempt,
    phase: &str,
) {
    let journal = storage.inspect_run(attempt.run).await.unwrap();
    let surge_persistence::runs::inspection::RunDatabaseInspection::Present { events } =
        journal.database
    else {
        panic!("real journal")
    };
    assert!(events.iter().any(|row| matches!(
        row.payload.payload,
        surge_core::EventPayload::QuotaStagePlanned { .. }
    )));
    assert!(events.iter().any(|row| matches!(
        row.payload.payload,
        surge_core::EventPayload::ExecutionWriterIntent { .. }
    )));
    assert!(events.iter().any(|row| matches!(
        row.payload.payload,
        surge_core::EventPayload::SessionEstablishmentRequested { .. }
    )));
    let operation:String=storage.acquire_registry_conn().unwrap().query_row("SELECT operation FROM work_item_quota_handoffs WHERE run=? ORDER BY rowid DESC LIMIT 1",[attempt.run.to_string()],|row|row.get(0)).unwrap();
    let operation: surge_core::id::WorkItemOperationId = operation.parse().unwrap();
    let handoff = storage.work_items().quota_handoff(operation).unwrap();
    use surge_persistence::work_items::recovery_cycles::QuotaHandoffState;
    assert_eq!(
        handoff.state(),
        if phase == "opening" {
            QuotaHandoffState::OpeningUnknown
        } else {
            QuotaHandoffState::Executing
        }
    );
    let opened = events.iter().find_map(|row| match &row.payload.payload {
        surge_core::EventPayload::SessionOpened {
            opened: Some(opened),
            ..
        } => Some(opened),
        _ => None,
    });
    assert_eq!(opened.is_some(), phase == "opened");
    if let Some(opened) = opened {
        assert_eq!(handoff.internal_session(), Some(opened.session));
        assert!(
            handoff.prompt_authorization_seq().is_some(),
            "bridge-send barrier is after durable prompt authorization"
        );
    }
    let a = wire(home, "quota-a");
    assert_eq!(
        a.iter()
            .filter(|row| row["operation"] == "new_session")
            .count(),
        1
    );
    assert!(!a.iter().any(|row| row["operation"] == "prompt"));
    let ready = std::path::PathBuf::from(std::env::var_os("SURGE_QUOTA_PARK_READY").unwrap());
    let temporary = ready.with_extension("publishing");
    std::fs::write(&temporary,serde_json::to_vec(&json!({"phase":phase,"entry_count":events.iter().filter(|row|matches!(row.payload.payload,surge_core::EventPayload::StageEntered {..})).count(),"plan_count":events.iter().filter(|row|matches!(row.payload.payload,surge_core::EventPayload::QuotaStagePlanned {..})).count(),"admission_count":storage.acquire_registry_conn().unwrap().query_row("SELECT COUNT(*) FROM recipe_opening_admissions",[],|row|row.get::<_,u64>(0)).unwrap(),"provider_invocation":handoff.launch().provider_invocation(),"home":home,"project":project,"item":item,"run":attempt.run,"config":attempt.config,"workspace":storage.work_items().show(item).unwrap().item.workspace,"operation":operation,"opened":opened,"a_count":a.len(),"b_count":wire(home,"quota-b").len()})).unwrap()).unwrap();
    std::fs::rename(temporary, ready).unwrap();
}

async fn cold_selected_containment_probe() {
    let ready = std::path::PathBuf::from(std::env::var_os("SURGE_QUOTA_PARK_READY").unwrap());
    let proof: Value = serde_json::from_slice(&std::fs::read(&ready).unwrap()).unwrap();
    let home = std::path::PathBuf::from(proof["home"].as_str().unwrap());
    let project = std::path::PathBuf::from(proof["project"].as_str().unwrap());
    let run: surge_core::RunId = serde_json::from_value(proof["run"].clone()).unwrap();
    let item: surge_core::id::WorkItemId = serde_json::from_value(proof["item"].clone()).unwrap();
    let operation: surge_core::id::WorkItemOperationId =
        serde_json::from_value(proof["operation"].clone()).unwrap();
    let storage = Storage::open(&home).await.unwrap();
    let cancel = CancellationToken::new();
    let engine = Arc::new(Engine::new_full(
        Arc::new(AcpBridge::with_defaults().unwrap()),
        storage.clone(),
        Arc::new(WorktreeToolDispatcher::new(project)),
        Arc::new(surge_notify::MultiplexingNotifier::new()),
        None,
        None,
        engine_config(&home, false),
    ));
    let socket = home.join("quota.sock");
    let server = tokio::spawn(surge_daemon::run_runs_only(
        surge_daemon::ServerConfig {
            socket_path: socket.clone(),
            max_active: 2,
            max_queue: 2,
        },
        Arc::new(LocalEngineFacade::new(engine.clone())),
        surge_daemon::tracked_run::TrackingContext::new(engine, storage.clone()),
        Arc::new(surge_daemon::broadcast::BroadcastRegistry::new()),
        Arc::new(surge_daemon::admission::AdmissionController::new(2, 2)),
        cancel.clone(),
    ));
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            let attempt = storage.work_items().for_run(run).unwrap().unwrap();
            if attempt.state == surge_core::work_item::WorkItemAttemptState::Attention {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let current = storage.work_items().for_run(run).unwrap().unwrap();
    assert_eq!(current.config, proof["config"].as_str().unwrap());
    assert_eq!(
        serde_json::to_value(storage.work_items().show(item).unwrap().item.workspace).unwrap(),
        proof["workspace"]
    );
    assert_eq!(
        wire(&home, "quota-a").len(),
        proof["a_count"].as_u64().unwrap() as usize
    );
    assert_eq!(
        wire(&home, "quota-b").len(),
        proof["b_count"].as_u64().unwrap() as usize
    );
    let handoff = storage.work_items().quota_handoff(operation).unwrap();
    assert_eq!(
        serde_json::to_value(handoff.launch().provider_invocation()).unwrap(),
        proof["provider_invocation"]
    );
    assert_eq!(
        storage
            .acquire_registry_conn()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM recipe_opening_admissions",
                [],
                |row| row.get::<_, u64>(0)
            )
            .unwrap(),
        proof["admission_count"].as_u64().unwrap()
    );
    let journal = storage.inspect_run(run).await.unwrap();
    let surge_persistence::runs::inspection::RunDatabaseInspection::Present { events } =
        journal.database
    else {
        panic!("retained original journal")
    };
    assert_eq!(
        events
            .iter()
            .filter(|row| matches!(
                row.payload.payload,
                surge_core::EventPayload::StageEntered { .. }
            ))
            .count() as u64,
        proof["entry_count"].as_u64().unwrap()
    );
    assert_eq!(
        events
            .iter()
            .filter(|row| matches!(
                row.payload.payload,
                surge_core::EventPayload::QuotaStagePlanned { .. }
            ))
            .count() as u64,
        proof["plan_count"].as_u64().unwrap()
    );
    if !proof["opened"].is_null() {
        let opened: surge_core::execution_recovery::OpenedSession =
            serde_json::from_value(proof["opened"].clone()).unwrap();
        assert_eq!(handoff.internal_session(), Some(opened.session));
        assert!(events.iter().any(|row|matches!(&row.payload.payload,surge_core::EventPayload::SessionOpened {opened:Some(actual),..} if actual==&opened)));
    }
    cancel.cancel();
    server.await.unwrap().unwrap();
    let temporary = ready.with_extension("publishing");
    std::fs::write(&temporary, b"selected opening retained containment").unwrap();
    std::fs::rename(temporary, ready.with_extension("contained")).unwrap();
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn selected_new_session_rpc_crash_does_not_open_or_prompt_again() {
    selected_crash_fixture("opening").await;
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn selected_opened_before_wire_prompt_crash_does_not_open_or_prompt_again() {
    selected_crash_fixture("opened").await;
}

#[cfg(unix)]
async fn selected_crash_fixture(phase: &str) {
    let diagnostics = tempfile::tempdir().unwrap();
    let ready = diagnostics.path().join("selected.json");
    let vars = |phase: &str| {
        vec![
            (
                std::ffi::OsString::from("SURGE_QUOTA_PARK_PHASE"),
                std::ffi::OsString::from(phase),
            ),
            (
                std::ffi::OsString::from("SURGE_QUOTA_PARK_READY"),
                ready.as_os_str().to_os_string(),
            ),
        ]
    };
    let mut first = cold_host::ColdHost::spawn(
        "child_planned_park_restart_probe",
        vars(phase),
        ready.clone(),
        diagnostics.path(),
    )
    .unwrap();
    let proof = match first.wait_ready(Duration::from_secs(12)).await {
        Ok(json) => serde_json::from_str::<Value>(&json).unwrap(),
        Err(error) => {
            let retained = diagnostics.keep();
            panic!(
                "selected crash boundary failed: {error}; diagnostics {}",
                retained.display()
            );
        },
    };
    let home = std::path::PathBuf::from(proof["home"].as_str().unwrap());
    let project = std::path::PathBuf::from(proof["project"].as_str().unwrap());
    let pid: u32 = std::fs::read_to_string(home.join("quota-a-provider.pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let identity = surge_acp::process_evidence::observe(pid).unwrap();
    first.stop_and_wait().unwrap();
    if surge_acp::process_evidence::observe(pid).ok().as_ref() == Some(&identity) {
        nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(i32::try_from(pid).unwrap()),
            nix::sys::signal::Signal::SIGKILL,
        )
        .unwrap();
    }
    std::fs::remove_file(home.join("quota.sock")).unwrap();
    let mut second = cold_host::ColdHost::spawn(
        "child_planned_park_restart_probe",
        vars(&format!("contain-{phase}")),
        ready.with_extension("contained"),
        diagnostics.path(),
    )
    .unwrap();
    if let Err(error) = second.wait_ready(Duration::from_secs(12)).await {
        let retained = diagnostics.keep();
        panic!(
            "selected cold containment failed: {error}; diagnostics {}",
            retained.display()
        );
    }
    second.stop_and_wait().unwrap();
    std::fs::remove_dir_all(home).unwrap();
    std::fs::remove_dir_all(project).unwrap();
}

async fn wait_for_durable_capacity_wake(storage: &Storage, run: surge_core::RunId) -> i64 {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let cycles = storage
                .work_items()
                .due_recovery_wakes_for_run(run, i64::MAX, 10)
                .unwrap();
            if let Some(wake) = cycles.first().and_then(|cycle| cycle.wake.as_ref()) {
                break wake.due_at_ms();
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("planned suspension must durably confirm its wake")
}

async fn wait_for_terminal_journal(storage: &Arc<Storage>, run: surge_core::RunId) {
    loop {
        let journal = storage.inspect_run(run).await.unwrap();
        if let surge_persistence::runs::inspection::RunDatabaseInspection::Present { events } =
            journal.database
            && events.iter().any(|row| {
                matches!(
                    row.payload.payload,
                    surge_core::EventPayload::RunCompleted { .. }
                        | surge_core::EventPayload::RunAborted { .. }
                )
            })
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn assert_retained_budget_result(events: &[surge_persistence::runs::ReadEvent], cap: u64) {
    let total: u64 = events
        .iter()
        .filter_map(|row| match row.payload.payload {
            surge_core::EventPayload::TokensConsumed {
                prompt_tokens,
                output_tokens,
                ..
            } => Some(u64::from(prompt_tokens) + u64::from(output_tokens)),
            _ => None,
        })
        .sum();
    assert_eq!(
        total, 1200,
        "explicit host oracle detects prior charge loss across wake"
    );
    assert!(events.iter().any(|row|matches!(&row.payload.payload,surge_core::EventPayload::RunStarted {config,..} if config.budget.limits.tokens==Some(cap))));
    if cap == 1000 {
        assert!(events.iter().any(|row|matches!(&row.payload.payload,surge_core::EventPayload::RunAborted {reason} if reason.contains("budget"))));
        assert!(!events.iter().any(|row| matches!(
            row.payload.payload,
            surge_core::EventPayload::RunCompleted { .. }
        )));
        assert!(events.iter().any(|row| matches!(
            row.payload.payload,
            surge_core::EventPayload::BudgetExceeded {
                total_tokens: 1200,
                ..
            }
        )));
    } else {
        assert!(events.iter().any(|row| matches!(
            row.payload.payload,
            surge_core::EventPayload::RunCompleted { .. }
        )));
    }
}

fn committed_reentry_engine_config(home: &Path) -> EngineConfig {
    let mut config = engine_config(home, false);
    let mut agents = config
        .agent_registry
        .as_ref()
        .unwrap()
        .list()
        .iter()
        .map(|entry| (entry.id.clone(), entry.to_agent_config()))
        .collect::<std::collections::HashMap<_, _>>();
    let primary = agents.get_mut("quota-a").unwrap();
    let scenario = primary
        .args
        .iter()
        .position(|argument| argument == "--scenario")
        .unwrap()
        + 1;
    primary.args[scenario] = "report_outcome=done".into();
    config.agent_registry = Some(Arc::new(surge_acp::Registry::from_config(agents)));
    config
}

fn committed_reentry_graph() -> surge_core::Graph {
    let mut graph: surge_core::Graph =
        toml::from_str(include_str!("../../../examples/flow_minimal_agent.toml")).unwrap();
    let node = graph.nodes.get_mut(&graph.start).unwrap();
    let surge_core::node::NodeConfig::Agent(agent) = &mut node.config else {
        panic!("agent")
    };
    agent.profile = "quota-primary@1.0".try_into().unwrap();
    node.declared_outcomes[0].edge_kind_hint = surge_core::edge::EdgeKind::Backtrack;
    let mut exceeded = node.declared_outcomes[0].clone();
    exceeded.id = "max_traversals_exceeded".parse().unwrap();
    exceeded.edge_kind_hint = surge_core::edge::EdgeKind::Escalate;
    node.declared_outcomes.push(exceeded);
    let mut finish = node.declared_outcomes[0].clone();
    finish.id = "finish".parse().unwrap();
    finish.edge_kind_hint = surge_core::edge::EdgeKind::Forward;
    node.declared_outcomes.push(finish);
    let mut forward = graph.edges[0].clone();
    forward.id = "structural_forward".parse().unwrap();
    forward.from.outcome = "finish".parse().unwrap();
    let terminal = graph.edges[0].to.clone();
    let edge = &mut graph.edges[0];
    edge.to = graph.start.clone();
    edge.kind = surge_core::edge::EdgeKind::Backtrack;
    edge.policy.max_traversals = Some(1);
    edge.policy.on_max_exceeded = surge_core::edge::ExceededAction::Escalate;
    let mut escape = edge.clone();
    escape.id = "exit_after_one_repeat".parse().unwrap();
    escape.from.outcome = "max_traversals_exceeded".parse().unwrap();
    escape.to = terminal;
    escape.kind = surge_core::edge::EdgeKind::Escalate;
    escape.policy = surge_core::edge::EdgePolicy::default();
    graph.edges.push(escape);
    graph.edges.push(forward);
    graph
}

async fn cold_committed_reentry_probe(first: bool) {
    use surge_core::work_item::{WorkItemCommand, WorkItemRequirements, WorkItemResult};
    let ready = std::path::PathBuf::from(std::env::var_os("SURGE_QUOTA_PARK_READY").unwrap());
    let home_owner = first.then(|| tempfile::tempdir().unwrap());
    let project_owner = first.then(|| tempfile::tempdir().unwrap());
    let proof: Value = if first {
        json!({})
    } else {
        serde_json::from_slice(&std::fs::read(&ready).unwrap()).unwrap()
    };
    let home = home_owner.as_ref().map_or_else(
        || Path::new(proof["home"].as_str().unwrap()).to_owned(),
        |owner| owner.path().to_owned(),
    );
    let project = project_owner.as_ref().map_or_else(
        || Path::new(proof["project"].as_str().unwrap()).to_owned(),
        |owner| owner.path().to_owned(),
    );
    if first {
        initialize_project(&project);
    }
    let storage = Storage::open(&home).await.unwrap();
    let cancel = CancellationToken::new();
    let engine = Arc::new(Engine::new_full(
        Arc::new(AcpBridge::with_defaults().unwrap()),
        storage.clone(),
        Arc::new(WorktreeToolDispatcher::new(project.clone())),
        Arc::new(surge_notify::MultiplexingNotifier::new()),
        None,
        None,
        committed_reentry_engine_config(&home),
    ));
    let socket = home.join("quota.sock");
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
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    if first {
        let created = request(
            &socket,
            serde_json::to_value(WorkItemCommand::Create {
                operation_id: surge_core::id::WorkItemOperationId::new(),
                project: project.clone(),
                title: "Authenticated same-node reentry".into(),
                requirements: WorkItemRequirements::new(
                    "Repeat completed stage once".into(),
                    vec!["No repeat of old provider effect".into()],
                )
                .unwrap(),
            })
            .unwrap(),
        )
        .await;
        assert_eq!(created["method"], "work_item_ok", "{created}");
        let item =
            serde_json::from_value(created["result"]["value"]["item"]["id"].clone()).unwrap();
        let detail = storage.work_items().show(item).unwrap();
        let graph = committed_reentry_graph();
        let start = WorkItemCommand::Start {
            operation_id: surge_core::id::WorkItemOperationId::new(),
            item,
            expected_version: detail.item.version,
            graph: Box::new(graph.clone()),
            quota_recovery: None,
        };
        let frozen = engine
            .freeze_work_item_config(
                &graph,
                surge_orchestrator::engine::EngineRunConfig::default(),
                &detail.item.workspace.path,
            )
            .await
            .unwrap();
        let frozen = serde_json::to_string(&frozen).unwrap();
        let WorkItemResult::Attempt(attempt) = storage
            .work_items()
            .mutate(
                &start,
                None,
                Some(&frozen),
                "host-reentry-oracle",
                chrono::Utc::now().timestamp_millis(),
            )
            .unwrap()
        else {
            panic!("attempt")
        };
        let temporary = ready.with_extension("publishing");
        std::fs::write(&temporary,serde_json::to_vec(&json!({"home":home,"project":project,"item":item,"run":attempt.run,"attempt":attempt,"workspace":detail.item.workspace})).unwrap()).unwrap();
        std::fs::rename(temporary, &ready).unwrap();
        // The first host must terminate at the genuine committed self-route seam.
        let _response = request(&socket, serde_json::to_value(start).unwrap()).await;
        wait_for_terminal_journal(&storage, attempt.run).await;
        cancel.cancel();
        server.await.unwrap().unwrap();
        return;
    }
    let run: surge_core::RunId = serde_json::from_value(proof["run"].clone()).unwrap();
    tokio::time::timeout(
        Duration::from_secs(8),
        wait_for_terminal_journal(&storage, run),
    )
    .await
    .unwrap();
    let inspection = storage.inspect_run(run).await.unwrap();
    let surge_persistence::runs::inspection::RunDatabaseInspection::Present { events } =
        inspection.database
    else {
        panic!("journal")
    };
    assert!(events.iter().any(|row| matches!(
        row.payload.payload,
        surge_core::EventPayload::RunCompleted { .. }
    )));
    for count in [events.iter().filter(|row|matches!(row.payload.payload,surge_core::EventPayload::StageEntered {ref node,..} if node.as_str()=="impl_1")).count(),events.iter().filter(|row|matches!(row.payload.payload,surge_core::EventPayload::QuotaStagePlanned {..})).count(),events.iter().filter(|row|matches!(row.payload.payload,surge_core::EventPayload::StageRouteCommitted {..})).count()] {assert_eq!(count,2);}
    let a = wire(&home, "quota-a");
    assert_eq!(
        a.iter()
            .filter(|row| row["operation"] == "new_session")
            .count(),
        2
    );
    assert_eq!(
        a.iter().filter(|row| row["operation"] == "prompt").count(),
        2
    );
    assert!(wire(&home, "quota-b").is_empty());
    let current = storage.work_items().for_run(run).unwrap().unwrap();
    assert_eq!(serde_json::to_value(current.run).unwrap(), proof["run"]);
    assert_eq!(current.config, proof["attempt"]["config"].as_str().unwrap());
    let item = serde_json::from_value(proof["item"].clone()).unwrap();
    assert_eq!(
        serde_json::to_value(storage.work_items().show(item).unwrap().item.workspace).unwrap(),
        proof["workspace"]
    );
    cancel.cancel();
    server.await.unwrap().unwrap();
    let temporary = ready.with_extension("publishing");
    std::fs::write(&temporary, b"actual cold committed reentry complete").unwrap();
    std::fs::rename(temporary, ready.with_extension("reentered")).unwrap();
}

#[cfg(all(unix, debug_assertions))]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cold_committed_same_node_route_runs_only_the_next_occurrence() {
    let diagnostics = tempfile::tempdir().unwrap();
    let ready = diagnostics.path().join("reentry.json");
    let vars = |phase: &str| {
        vec![
            (
                "SURGE_QUOTA_PARK_PHASE".into(),
                std::ffi::OsString::from(phase),
            ),
            (
                "SURGE_QUOTA_PARK_READY".into(),
                ready.as_os_str().to_os_string(),
            ),
        ]
    };
    let mut initial_vars = vars("reentry");
    initial_vars.push(("SURGE_ROUTE_COMMIT_EXIT".into(), "impl_1".into()));
    let mut first = cold_host::ColdHost::spawn(
        "child_planned_park_restart_probe",
        initial_vars,
        ready.clone(),
        diagnostics.path(),
    )
    .unwrap();
    let proof: Value = match first.wait_ready(Duration::from_secs(12)).await {
        Ok(json) => serde_json::from_str(&json).unwrap(),
        Err(error) => {
            let retained = diagnostics.keep();
            panic!(
                "initial committed reentry probe failed: {error}; diagnostics {}",
                retained.display()
            );
        },
    };
    let exit = first.wait_exit(Duration::from_secs(12)).await.unwrap();
    if exit.code() != Some(99) {
        let retained = diagnostics.keep();
        panic!(
            "must terminate after actual authenticated route commit: {exit}; diagnostics {}",
            retained.display()
        );
    }
    assert!(first.pid().is_none(), "old exact child must be reaped");
    let home = std::path::PathBuf::from(proof["home"].as_str().unwrap());
    let project = std::path::PathBuf::from(proof["project"].as_str().unwrap());
    let run: surge_core::RunId = serde_json::from_value(proof["run"].clone()).unwrap();
    let storage = Storage::open(&home).await.unwrap();
    let inspection = storage.inspect_run(run).await.unwrap();
    let surge_persistence::runs::inspection::RunDatabaseInspection::Present { events } =
        inspection.database
    else {
        panic!("original journal")
    };
    assert_eq!(
        events
            .iter()
            .filter(|row| matches!(
                row.payload.payload,
                surge_core::EventPayload::StageRouteCommitted { .. }
            ))
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|row| matches!(
                row.payload.payload,
                surge_core::EventPayload::StageEntered { .. }
            ))
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|row| matches!(
                row.payload.payload,
                surge_core::EventPayload::QuotaStagePlanned { .. }
            ))
            .count(),
        1
    );
    assert!(!events.iter().any(|row| matches!(
        row.payload.payload,
        surge_core::EventPayload::RunCompleted { .. }
    )));
    let a = wire(&home, "quota-a");
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
    let original = events
        .iter()
        .find_map(|row| match &row.payload.payload {
            surge_core::EventPayload::SessionOpened {
                opened: Some(opened),
                ..
            } => Some(opened.clone()),
            _ => None,
        })
        .unwrap();
    let folded = storage
        .inspect_folded_run(run)
        .await
        .unwrap()
        .database
        .unwrap();
    let surge_core::RunState::Pipeline { cursor, memory, .. } = folded.state else {
        panic!("committed self-route must retain live pipeline")
    };
    assert_eq!(cursor.node.as_str(), "impl_1");
    assert_eq!(memory.committed_stage_outcomes.len(), 1);
    let committed = memory.committed_stage_outcomes.values().next().unwrap();
    assert_eq!(committed.commit.context().session, original.session);
    assert!(
        committed.routed_seq.is_some(),
        "authenticated original outcome must already be consumed by its self-route"
    );
    let reader = storage.open_run_reader(run).await.unwrap();
    let last_seq = events.last().unwrap().seq;
    let (snapshot_seq, blob) = reader
        .latest_snapshot_at_or_before(last_seq)
        .await
        .unwrap()
        .unwrap();
    let snapshot: surge_orchestrator::engine::snapshot::EngineSnapshot =
        serde_json::from_slice(&blob).unwrap();
    assert_eq!(snapshot_seq, last_seq);
    assert_eq!(snapshot.at_seq, last_seq.as_u64());
    assert_eq!(snapshot.stage_boundary_seq, last_seq.as_u64());
    assert_eq!(snapshot.cursor.node, "impl_1");
    assert_eq!(
        snapshot.root_traversal_counts.get("e_impl_to_end"),
        Some(&1)
    );
    drop(reader);
    std::fs::remove_file(home.join("quota.sock")).unwrap();
    drop(storage);
    let mut second = cold_host::ColdHost::spawn(
        "child_planned_park_restart_probe",
        vars("reentry-continue"),
        ready.with_extension("reentered"),
        diagnostics.path(),
    )
    .unwrap();
    if let Err(error) = second.wait_ready(Duration::from_secs(12)).await {
        let retained = diagnostics.keep();
        panic!(
            "legal cold same-node iteration failed: {error}; diagnostics {}",
            retained.display()
        );
    }
    second.stop_and_wait().unwrap();
    let storage = Storage::open(&home).await.unwrap();
    let inspection = storage.inspect_run(run).await.unwrap();
    let surge_persistence::runs::inspection::RunDatabaseInspection::Present { events } =
        inspection.database
    else {
        panic!("restored journal")
    };
    assert_eq!(events.iter().filter(|row|matches!(&row.payload.payload,surge_core::EventPayload::SessionOpened {opened:Some(opened),..} if opened==&original)).count(),1);
    let opened: Vec<_> = events
        .iter()
        .filter_map(|row| match &row.payload.payload {
            surge_core::EventPayload::SessionOpened {
                node,
                opened: Some(opened),
                ..
            } => {
                assert_eq!(node.as_str(), "impl_1");
                Some(opened)
            },
            _ => None,
        })
        .collect();
    assert_eq!(opened.len(), 2);
    assert_ne!(
        opened[0].descriptor.invocation(),
        opened[1].descriptor.invocation()
    );
    assert_ne!(
        opened[0].descriptor.provider_session_id(),
        opened[1].descriptor.provider_session_id()
    );
    assert_ne!(opened[0].session, opened[1].session);
    let plans: Vec<_> = events.iter().filter_map(|row|match &row.payload.payload {
        surge_core::EventPayload::QuotaStagePlanned {node,logical_invocation,stage_entry_seq,..} => {
            assert_eq!(node.as_str(),"impl_1");
            assert!(events.iter().any(|entry|entry.seq.as_u64()==*stage_entry_seq && matches!(&entry.payload.payload,surge_core::EventPayload::StageEntered {node:actual,..} if actual==node)));
            Some((logical_invocation,stage_entry_seq))
        },
        _=>None,
    }).collect();
    assert_eq!(plans.len(), 2);
    assert_ne!(plans[0].0, plans[1].0);
    assert_ne!(plans[0].1, plans[1].1);
    drop(storage);
    std::fs::remove_dir_all(home).unwrap();
    std::fs::remove_dir_all(project).unwrap();
}

fn relative_source_engine_config(home: &Path, all_exhausted: bool) -> EngineConfig {
    let mut config = engine_config(home, all_exhausted);
    let agents = config
        .agent_registry
        .as_ref()
        .unwrap()
        .list()
        .iter()
        .map(|entry| {
            let mut agent = entry.to_agent_config();
            let route = agent.capacity_route.as_mut().unwrap();
            for source in &mut route.auth_sources {
                if let surge_core::config::AuthSource::File { path } = source {
                    *path = std::path::PathBuf::from(".surge-auth").join(&entry.id);
                }
            }
            (entry.id.clone(), agent)
        })
        .collect::<std::collections::HashMap<_, _>>();
    config.agent_registry = Some(Arc::new(surge_acp::Registry::from_config(agents)));
    config
}

fn prepare_relative_source_fixture(workspace: &surge_core::work_item::WorkItemWorkspace) {
    surge_git::task_workspace::prepare(
        workspace,
        surge_git::run_worktree::ReconcilePhase::BeforeExecution,
    )
    .unwrap();
    std::fs::create_dir_all(workspace.path.join(".surge-auth")).unwrap();
    std::fs::create_dir_all(workspace.checkout.join(".surge-auth")).unwrap();
    for runtime in ["quota-a", "quota-b", "quota-c"] {
        std::fs::write(
            workspace.path.join(".surge-auth").join(runtime),
            format!("file-secret-sentinel-{runtime}"),
        )
        .unwrap();
        std::fs::write(
            workspace.checkout.join(".surge-auth").join(runtime),
            format!("different-checkout-secret-{runtime}"),
        )
        .unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn relative_sources_are_frozen_from_actual_launch_worktree_on_normal_start() {
    quota_dispatch_fixture_source(
        true,
        false,
        false,
        None,
        SourceFixtureOptions {
            source_changed: false,
            change_after_warmup: None,
            cross_project: false,
            relative_cwd: true,
            preparation_failure: None,
        },
    )
    .await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn relative_selected_source_change_blocks_provider_effect() {
    quota_dispatch_fixture_source(
        true,
        false,
        false,
        None,
        SourceFixtureOptions {
            source_changed: false,
            change_after_warmup: Some("quota-b"),
            cross_project: false,
            relative_cwd: true,
            preparation_failure: None,
        },
    )
    .await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn relative_skipped_source_change_blocks_provider_effect() {
    quota_dispatch_fixture_source(
        true,
        false,
        false,
        None,
        SourceFixtureOptions {
            source_changed: false,
            change_after_warmup: Some("quota-a"),
            cross_project: false,
            relative_cwd: true,
            preparation_failure: None,
        },
    )
    .await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn relative_post_429_source_change_remains_audit_only() {
    quota_dispatch_fixture_source(
        true,
        false,
        false,
        None,
        SourceFixtureOptions {
            source_changed: true,
            change_after_warmup: None,
            cross_project: false,
            relative_cwd: true,
            preparation_failure: None,
        },
    )
    .await;
}

fn relative_fixture_git_state(path: &Path) -> (Vec<u8>, Option<Vec<u8>>) {
    let head = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(path)
        .output()
        .unwrap();
    assert!(head.status.success());
    let index = std::process::Command::new("git")
        .args(["rev-parse", "--git-path", "index"])
        .current_dir(path)
        .output()
        .unwrap();
    assert!(index.status.success());
    let index_path = path.join(String::from_utf8(index.stdout).unwrap().trim());
    (head.stdout, std::fs::read(index_path).ok())
}

#[derive(Clone, Copy)]
enum RelativePreparationFailure {
    Dirty,
    Foreign,
    MissingPrepared,
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn relative_preparation_preserves_dirty_staged_index_and_head_on_refusal() {
    quota_dispatch_fixture_source(
        true,
        false,
        false,
        None,
        SourceFixtureOptions {
            source_changed: false,
            change_after_warmup: None,
            cross_project: false,
            relative_cwd: true,
            preparation_failure: Some(RelativePreparationFailure::Dirty),
        },
    )
    .await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn relative_preparation_never_adopts_a_foreign_workspace() {
    quota_dispatch_fixture_source(
        true,
        false,
        false,
        None,
        SourceFixtureOptions {
            source_changed: false,
            change_after_warmup: None,
            cross_project: false,
            relative_cwd: true,
            preparation_failure: Some(RelativePreparationFailure::Foreign),
        },
    )
    .await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn relative_preparation_does_not_recreate_a_missing_prepared_workspace() {
    quota_dispatch_fixture_source(
        true,
        false,
        false,
        None,
        SourceFixtureOptions {
            source_changed: false,
            change_after_warmup: None,
            cross_project: false,
            relative_cwd: true,
            preparation_failure: Some(RelativePreparationFailure::MissingPrepared),
        },
    )
    .await;
}

#[cfg(all(unix, debug_assertions))]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn child_start_preparation_worker_probe() {
    if std::env::var_os("SURGE_START_PREPARATION_MANIFEST").is_none() {
        return;
    }
    quota_dispatch_fixture_source(
        true,
        false,
        false,
        None,
        SourceFixtureOptions {
            source_changed: false,
            change_after_warmup: None,
            cross_project: false,
            relative_cwd: true,
            preparation_failure: None,
        },
    )
    .await;
}
#[cfg(all(unix, debug_assertions))]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn configured_start_pre_fingerprint_worker_keeps_other_daemon_controls_responsive() {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    use surge_core::id::{WorkItemId, WorkItemOperationId};
    use surge_core::work_item::{WorkItemCommand, WorkItemRequirements};
    let diagnostics = tempfile::tempdir().unwrap();
    let manifest = diagnostics.path().join("manifest.json");
    let ready = diagnostics.path().join("worker.ready");
    let release = diagnostics.path().join("worker.release");
    let accepted = diagnostics.path().join("accepted.json");
    let operation = WorkItemOperationId::new();
    let item: WorkItemId = operation.as_ulid().to_string().parse().unwrap();
    let vars = vec![
        (
            "SURGE_START_PREPARATION_ACCEPTED".into(),
            accepted.as_os_str().to_owned(),
        ),
        (
            "SURGE_START_PREPARATION_ITEM".into(),
            item.to_string().into(),
        ),
        (
            "SURGE_START_PREPARATION_READY".into(),
            ready.as_os_str().to_owned(),
        ),
        (
            "SURGE_START_PREPARATION_RELEASE".into(),
            release.as_os_str().to_owned(),
        ),
        (
            "SURGE_START_PREPARATION_MANIFEST".into(),
            manifest.as_os_str().to_owned(),
        ),
    ];
    let mut child = cold_host::ColdHost::spawn(
        "child_start_preparation_worker_probe",
        vars,
        manifest.clone(),
        diagnostics.path(),
    )
    .unwrap();
    let proof: Value =
        serde_json::from_str(&child.wait_ready(Duration::from_secs(30)).await.unwrap()).unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if std::fs::read_to_string(&ready).ok().as_deref() == Some(&item.to_string()) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let home = std::path::PathBuf::from(proof["home"].as_str().unwrap());
    let project = std::path::PathBuf::from(proof["project"].as_str().unwrap());
    let socket = std::path::PathBuf::from(proof["socket"].as_str().unwrap());
    let storage = Storage::open(&home).await.unwrap();
    assert!(
        storage
            .work_items()
            .show(item)
            .unwrap()
            .item
            .active_run
            .is_none()
    );
    assert!(!storage.work_items().workspace_prepared(item).unwrap());
    assert!(wire(&home, "quota-a").is_empty());
    assert!(wire(&home, "quota-b").is_empty());
    let create = WorkItemCommand::Create {
        operation_id: WorkItemOperationId::new(),
        project: project.clone(),
        title: "Independent control".into(),
        requirements: WorkItemRequirements::new("Independent".into(), vec!["Responsive".into()])
            .unwrap(),
    };
    let created = tokio::time::timeout(
        Duration::from_secs(2),
        request(&socket, serde_json::to_value(create).unwrap()),
    )
    .await
    .unwrap();
    assert_eq!(created["method"], "work_item_ok", "{created}");
    let other: WorkItemId =
        serde_json::from_value(created["result"]["value"]["item"]["id"].clone()).unwrap();
    let archive = WorkItemCommand::Archive {
        operation_id: WorkItemOperationId::new(),
        item: other,
        expected_version: 1,
    };
    let archived = tokio::time::timeout(
        Duration::from_secs(2),
        request(&socket, serde_json::to_value(archive).unwrap()),
    )
    .await
    .unwrap();
    assert_eq!(archived["method"], "work_item_ok", "{archived}");
    assert!(
        storage
            .work_items()
            .show(other)
            .unwrap()
            .item
            .archived_at_ms
            .is_some()
    );
    assert!(wire(&home, "quota-a").is_empty());
    assert!(wire(&home, "quota-b").is_empty());
    let busy = request(&socket, proof["command"].clone()).await;
    assert_eq!(busy["method"], "error", "{busy}");
    assert_eq!(busy["code"], "work_item_busy", "{busy}");
    let temporary = release.with_extension("publishing");
    let mut release_file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)
        .unwrap();
    release_file.write_all(item.to_string().as_bytes()).unwrap();
    release_file.sync_all().unwrap();
    std::fs::rename(temporary, &release).unwrap();
    let original: Value = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Ok(bytes) = std::fs::read(&accepted) {
                break serde_json::from_slice(&bytes).unwrap();
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let (replay1, replay2) = tokio::join!(
        request(&socket, proof["command"].clone()),
        request(&socket, proof["command"].clone())
    );
    for replay in [replay1, replay2] {
        assert_eq!(replay["method"], "work_item_ok", "{replay}");
        assert_eq!(
            replay["result"]["value"]["run"],
            original["result"]["value"]["run"]
        );
        assert_eq!(
            replay["result"]["value"]["config"],
            original["result"]["value"]["config"]
        );
    }
    assert_eq!(
        storage
            .work_items()
            .scan_attempts(None, 10)
            .unwrap()
            .entries
            .len(),
        1
    );
    std::fs::write(accepted.with_extension("continue"), b"continue fixture").unwrap();
    assert!(
        child
            .wait_exit(Duration::from_secs(30))
            .await
            .unwrap()
            .success()
    );
    assert_eq!(
        wire(&home, "quota-a")
            .iter()
            .filter(|row| row["operation"] == "new_session")
            .count(),
        1
    );
    assert_eq!(
        wire(&home, "quota-b")
            .iter()
            .filter(|row| row["operation"] == "new_session")
            .count(),
        2
    );
    drop(storage);
    std::fs::remove_dir_all(home).unwrap();
    std::fs::remove_dir_all(project).unwrap();
}
