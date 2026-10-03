//! Real engine/ACP permission roundtrip, isolated by a process watchdog.
use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use surge_acp::bridge::*;
use surge_core::{
    RunId, SessionId,
    run_event::{ElevationDecision, EventPayload},
};
use surge_orchestrator::engine::tools::worktree::WorktreeToolDispatcher;
use surge_orchestrator::engine::{Engine, EngineConfig, EngineRunConfig, RunOutcome};
use surge_persistence::runs::Storage;

#[derive(Clone)]
struct Elevate;
impl Sandbox for Elevate {
    fn visibility(&self, _: &str, _: Option<&str>) -> SandboxDecision {
        SandboxDecision::Allow
    }
    fn allows_tool(&self, _: &str, _: Option<&str>) -> SandboxDecision {
        SandboxDecision::Elevate {
            capability: "filesystem_write".into(),
        }
    }
    fn boxed_clone(&self) -> Box<dyn Sandbox> {
        Box::new(self.clone())
    }
}
struct RealBridge {
    bridge: AcpBridge,
    pid_path: PathBuf,
    flags: Vec<String>,
    verification_report: Option<String>,
}
#[async_trait::async_trait]
impl BridgeFacade for RealBridge {
    async fn open_session(
        &self,
        mut config: SessionConfig,
    ) -> Result<surge_core::execution_recovery::OpenedSession, OpenSessionError> {
        config.agent_kind = AgentKind::Custom {
            binary: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!(
                "../../target/debug/mock_acp_agent{}",
                std::env::consts::EXE_SUFFIX
            )),
            args: self
                .flags
                .iter()
                .cloned()
                .chain(["--pid-file".into(), self.pid_path.display().to_string()])
                .collect(),
        };
        config.env.insert(
            "SURGE_TEST_MCP_HELPER_PID".into(),
            self.pid_path
                .with_file_name("mcp.pid")
                .display()
                .to_string(),
        );
        if let Some(report) = &self.verification_report {
            config
                .env
                .insert("SURGE_TEST_VERIFICATION_REPORT".into(), report.clone());
        }
        config.sandbox = Box::new(Elevate);
        self.bridge.open_session(config).await
    }
    async fn send_message(
        &self,
        id: SessionId,
        content: MessageContent,
    ) -> Result<(), SendMessageError> {
        self.bridge.send_message(id, content).await
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

#[test]
fn real_engine_permission_roundtrip() {
    watchdog("real_engine_permission_roundtrip", 0);
}

#[test]
fn real_engine_stop_during_permission() {
    watchdog("real_engine_stop_during_permission", 1);
}

#[test]
fn failed_prompt_never_commits_reported_outcome() {
    watchdog("failed_prompt_never_commits_reported_outcome", 2);
}
#[test]
fn streaming_prompt_cannot_starve_deadline() {
    watchdog("streaming_prompt_cannot_starve_deadline", 3);
}

#[test]
fn failed_prompt_cannot_verify_active_task() {
    watchdog("failed_prompt_cannot_verify_active_task", 4);
}

#[test]
fn successful_prompt_verifies_active_task() {
    watchdog("successful_prompt_verifies_active_task", 5);
}

#[test]
fn conformant_mcp_peer_completes_real_engine() {
    watchdog("conformant_mcp_peer_completes_real_engine", 6);
}

fn watchdog(test: &str, mode: u8) {
    if let Some(root) = std::env::var_os("SURGE_PERMISSION_HELPER") {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(roundtrip(PathBuf::from(root), mode));
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let mut helper = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test, "--nocapture"])
        .env("SURGE_PERMISSION_HELPER", root.path())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(12);
    let status = loop {
        if let Some(status) = helper.try_wait().unwrap() {
            break Some(status);
        }
        if Instant::now() >= deadline {
            helper.kill().unwrap();
            helper.wait().unwrap();
            break None;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    if !status.is_some_and(|status| status.success())
        && let Ok(pid) = std::fs::read_to_string(root.path().join("child.pid"))
    {
        #[cfg(unix)]
        let _ = std::process::Command::new("kill")
            .args(["-KILL", pid.trim()])
            .status();
        #[cfg(windows)]
        let _ = std::process::Command::new("taskkill")
            .args(["/F", "/PID", pid.trim()])
            .status();
    }
    assert!(
        status.is_some_and(|status| status.success()),
        "real ACP permission journey failed or exceeded watchdog"
    );
}

#[test]
fn conformant_mcp_duplicate_is_durable_once() {
    watchdog("conformant_mcp_duplicate_is_durable_once", 7);
}
#[test]
fn conformant_mcp_invalid_candidate_fails_without_hang() {
    watchdog("conformant_mcp_invalid_candidate_fails_without_hang", 8);
}

#[test]
#[cfg(unix)]
fn conformant_mcp_rejected_outcome_gets_repair_turn() {
    watchdog("conformant_mcp_rejected_outcome_gets_repair_turn", 13);
}

#[test]
#[cfg(unix)]
fn conformant_mcp_rejected_outcome_exhausts_retry_budget() {
    watchdog("conformant_mcp_rejected_outcome_exhausts_retry_budget", 14);
}
#[test]
fn conformant_mcp_silent_turn_gets_outcome_reminder() {
    watchdog("conformant_mcp_silent_turn_gets_outcome_reminder", 15);
}

#[test]
fn conformant_mcp_always_silent_agent_fails_bounded() {
    watchdog("conformant_mcp_always_silent_agent_fails_bounded", 16);
}

#[test]
fn conformant_mcp_human_answer_reaches_provider() {
    watchdog("conformant_mcp_human_answer_reaches_provider", 9);
}
#[test]
fn conformant_mcp_stop_cancels_pending_human() {
    watchdog("conformant_mcp_stop_cancels_pending_human", 10);
}
#[test]
fn conformant_mcp_generic_tool_never_dispatches() {
    watchdog("conformant_mcp_generic_tool_never_dispatches", 11);
}

#[test]
fn conformant_mcp_helper_death_removes_pending_human() {
    watchdog("conformant_mcp_helper_death_removes_pending_human", 12);
}

async fn roundtrip(root: PathBuf, mode: u8) {
    let stop = mode == 1;
    let storage = Storage::open(&root).await.unwrap();
    let bridge = Arc::new(RealBridge {
        bridge: AcpBridge::with_defaults().unwrap(),
        pid_path: root.join("child.pid"),
        verification_report: (4..=5).contains(&mode).then(|| {
            serde_json::json!({
            "task_id":"task-under-verification", "outcome":"passed", "summary":"Acceptance checked",
            "checks":[{"command":"acceptance", "result":"passed", "covers":["criterion:1"]}]
        }).to_string()
        }),
        flags: if mode >= 6 {
            let case = match mode {
                7 => "duplicate",
                8 => "invalid",
                9 => "human",
                10 | 12 => "stop",
                11 => "unknown",
                13 => "retry",
                14 => "retry-exhaust",
                15 => "silent-first",
                16 => "silent-always",
                _ => "valid",
            };
            vec!["--stage-mcp".into(), format!("--stage-mcp-case={case}")]
        } else if mode == 5 {
            vec!["--scenario=report_done".into()]
        } else if mode < 2 {
            vec!["--permission".into(), "--scenario=report_done".into()]
        } else if mode == 2 || mode == 4 {
            vec![
                "--scenario=report_done".into(),
                "--error-after-outcome".into(),
            ]
        } else {
            vec!["--stream-forever".into()]
        },
    });
    if (4..=5).contains(&mode) {
        verify_failure(&root, &storage, bridge.clone(), mode == 5).await;
        Arc::try_unwrap(bridge)
            .ok()
            .unwrap()
            .bridge
            .shutdown()
            .await
            .unwrap();
        return;
    }
    // The actual launch recipe must be configured before engine admission,
    // rather than only rewritten by the controlled transport adapter afterward.
    let profiles = root.join("fixture-profiles");
    std::fs::create_dir_all(&profiles).unwrap();
    std::fs::write(
        profiles.join("permission-fixture-1.0.toml"),
        r#"
schema_version=1
[role]
id="permission-fixture"
version="1.0.0"
display_name="Controlled ACP fixture"
category="agents"
description="Actual launch recipe before dispatch"
when_to_use="Tests"
[runtime]
agent_id="permission-fixture"
recommended_model="fixture"
[[outcomes]]
id="done"
description="Done"
edge_kind_hint="forward"
[prompt]
system="Controlled permission journey"
"#,
    )
    .unwrap();
    let binary = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!(
        "../../target/debug/mock_acp_agent{}",
        std::env::consts::EXE_SUFFIX
    ));
    let args: Vec<String> = bridge
        .flags
        .iter()
        .cloned()
        .chain(["--pid-file".into(), bridge.pid_path.display().to_string()])
        .collect();
    let agent =
        serde_json::from_value(serde_json::json!({"command": binary, "args": args})).unwrap();
    let disk_profiles =
        surge_orchestrator::profile_loader::DiskProfileSet::scan(&profiles).unwrap();
    assert_eq!(
        disk_profiles.entries().len(),
        1,
        "controlled profile must parse, not silently fall back to bundled profiles"
    );
    let engine_config = EngineConfig {
        profile_registry: Some(Arc::new(
            surge_orchestrator::profile_loader::ProfileRegistry::new(disk_profiles),
        )),
        agent_registry: Some(Arc::new(surge_acp::Registry::from_config(
            std::collections::HashMap::from([("permission-fixture".into(), agent)]),
        ))),
        ..EngineConfig::default()
    };
    let mut bridge_events = bridge.subscribe();
    let engine = Engine::new(
        bridge.clone(),
        storage.clone(),
        Arc::new(WorktreeToolDispatcher::new(root.clone())),
        engine_config,
    );
    let mut tap = engine.subscribe_tap();
    let id = RunId::new();
    let graph_source = include_str!("../../../examples/flow_elevation_demo.toml");
    let graph_source = if matches!(mode, 9 | 10 | 12) {
        graph_source.replace(
            "elevation_channels = []",
            "elevation_channels = [{ type = \"desktop\", duration = \"persistent\" }]",
        )
    } else {
        graph_source.to_owned()
    };
    let graph_source = if matches!(mode, 13 | 14) {
        graph_source.replace("hooks = []", "hooks = [{ id = 'repair-check', trigger = 'on_outcome', matcher = { outcome = 'done' }, command = 'test -f repair-ready', on_failure = 'reject', timeout_seconds = 2 }]")
            .replace("timeout_seconds = 900", "timeout_seconds = 900\nmax_retries = 1")
    } else {
        graph_source
    };
    let graph_source = graph_source.replace("implementer@1.0", "permission-fixture@1.0");
    let graph = toml::from_str(&graph_source).unwrap();
    let mut run_config = EngineRunConfig::default();
    if mode == 3 {
        run_config.tool_call_loop_guard = Some(surge_core::loop_config::ToolCallLoopGuardConfig {
            node_wall_clock_limit_secs: 1,
            ..Default::default()
        });
    }
    let handle = engine
        .start_run(id, graph, root.clone(), run_config)
        .await
        .unwrap();
    if (2..=3).contains(&mode) {
        if mode == 3 {
            tokio::time::timeout(Duration::from_secs(3), wait_for_deadline(&mut tap))
                .await
                .expect("continuous notifications starved deadline escalation");
        }
        let outcome = tokio::time::timeout(Duration::from_secs(8), handle.await_completion())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(outcome, RunOutcome::Failed { .. }), "{outcome:?}");
        let events = storage
            .open_run_reader(id)
            .await
            .unwrap()
            .read_run_events()
            .await
            .unwrap();
        let kinds: Vec<_> = events
            .iter()
            .map(|event| event.payload.discriminant_str())
            .collect();
        assert!(!kinds.contains(&"OutcomeReported"));
        if mode == 3 {
            assert!(kinds.contains(&"EscalationRequested"));
        } else {
            assert!(
                events
                    .iter()
                    .any(|event| matches!(event.payload, EventPayload::StageToolReceipt { .. })),
                "fixture must durably submit a real MCP candidate before failing"
            );
        }
        drop(engine);
        Arc::try_unwrap(bridge)
            .ok()
            .unwrap()
            .bridge
            .shutdown()
            .await
            .unwrap();
        return;
    }
    if mode >= 6 {
        let mut pending_call = None;
        if matches!(mode, 9 | 10 | 12) {
            let call_id =
                tokio::time::timeout(Duration::from_secs(4), wait_for_human_request(&mut tap))
                    .await
                    .expect("no durable MCP human request");
            assert!(call_id.is_some());
            pending_call = call_id.clone();
            let before = storage
                .open_run_reader(id)
                .await
                .unwrap()
                .read_run_events()
                .await
                .unwrap();
            assert!(!before.iter().any(|event| matches!(
                event.payload,
                EventPayload::OutcomeReported { .. } | EventPayload::StageToolReceipt { .. }
            )));
            if mode == 12 {
                kill_mcp_helper(&root);
            } else if mode == 10 {
                engine
                    .stop_run(id, "MCP human cancellation".into())
                    .await
                    .unwrap();
            } else {
                engine
                    .resolve_human_input(
                        id,
                        call_id,
                        serde_json::json!({"choice":"approved","nested":[1,2]}),
                    )
                    .await
                    .unwrap();
            }
        }
        let outcome = tokio::time::timeout(Duration::from_secs(5), handle.await_completion())
            .await
            .unwrap()
            .unwrap();
        if matches!(mode, 8 | 12 | 14 | 16) {
            assert!(matches!(outcome, RunOutcome::Failed { .. }), "{outcome:?}");
        } else if mode == 10 {
            assert!(matches!(outcome, RunOutcome::Aborted { .. }), "{outcome:?}");
        } else {
            assert!(
                matches!(outcome, RunOutcome::Completed { .. }),
                "conformant MCP peer failed: {outcome:?}"
            );
        }
        let events = storage
            .open_run_reader(id)
            .await
            .unwrap()
            .read_run_events()
            .await
            .unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event.payload, EventPayload::OutcomeReported { .. }))
                .count(),
            usize::from(!matches!(mode, 8 | 10 | 12 | 14 | 16))
        );
        if matches!(mode, 13 | 14) {
            let rejected = events
                .iter()
                .filter(|event| matches!(event.payload, EventPayload::OutcomeRejectedByHook { .. }))
                .count();
            assert_eq!(rejected, if mode == 13 { 1 } else { 2 });
            assert_eq!(
                events
                    .iter()
                    .filter(|event| matches!(event.payload, EventPayload::StageToolReceipt { .. }))
                    .count(),
                2
            );
        }
        if mode == 12 {
            assert!(
                !events
                    .iter()
                    .any(|event| matches!(event.payload, EventPayload::HumanInputResolved { .. }))
            );
            assert!(
                engine
                    .resolve_human_input(id, pending_call, serde_json::json!({"late":true}))
                    .await
                    .is_err(),
                "stale resolver survived helper death"
            );
        }
        let receipts: Vec<_> = events
            .iter()
            .filter(|event| matches!(event.payload, EventPayload::StageToolReceipt { .. }))
            .collect();
        assert_eq!(
            receipts.len(),
            if matches!(mode, 9 | 13 | 14) {
                2
            } else if mode == 8 {
                // One rejected candidate per turn: the first plus two
                // missing-outcome reminder turns.
                3
            } else {
                usize::from(!matches!(mode, 10 | 12 | 16))
            }
        );
        if matches!(mode, 8 | 16) {
            let failure = events.iter().find_map(|event| match &event.payload {
                EventPayload::RunFailed { error } => Some(error.clone()),
                _ => None,
            });
            assert!(
                failure.is_some_and(|error| error.contains("ended 3 turns")),
                "silent agent must fail after the bounded reminders"
            );
        }
        if !matches!(mode, 8 | 10 | 12 | 14 | 16) {
            let candidate = receipts.last().unwrap();
            let final_outcome = events
                .iter()
                .find(|event| matches!(event.payload, EventPayload::OutcomeReported { .. }))
                .unwrap();
            assert!(candidate.seq < final_outcome.seq);
        }
        assert!(
            !events
                .iter()
                .any(|event| matches!(event.payload, EventPayload::ToolCalled { .. }))
        );
        assert!(!root.join("should-not-exist").exists());
        drop(engine);
        Arc::try_unwrap(bridge)
            .ok()
            .unwrap()
            .bridge
            .shutdown()
            .await
            .unwrap();
        return;
    }
    let (session, request_id) = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let BridgeEvent::PermissionRequested {
                session,
                request_id,
                ..
            } = bridge_events.recv().await.unwrap()
            {
                break (session, request_id);
            }
        }
    })
    .await
    .expect("ACP child never requested permission");
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let event = tap.recv().await.unwrap();
            if matches!(
                event.event.payload.payload,
                EventPayload::SandboxElevationRequested { .. }
            ) {
                break;
            }
        }
    })
    .await
    .expect("engine did not persist permission while prompt awaited it");
    if stop {
        engine
            .stop_run(id, "controlled permission stop".into())
            .await
            .unwrap();
    } else {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if engine
                    .resolve_elevation(
                        id,
                        session,
                        request_id.clone(),
                        surge_orchestrator::engine::elevation::EngineElevationDecision {
                            decision: ElevationDecision::Allow,
                            remember: false,
                            option_id: "allow".into(),
                        },
                    )
                    .await
                    .is_ok()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }
    let outcome = tokio::time::timeout(Duration::from_secs(3), handle.await_completion())
        .await
        .unwrap()
        .unwrap();
    if stop {
        assert!(matches!(outcome, RunOutcome::Aborted { .. }), "{outcome:?}");
    } else {
        assert!(
            matches!(outcome, RunOutcome::Completed { .. }),
            "{outcome:?}"
        );
    }
    let events = storage
        .open_run_reader(id)
        .await
        .unwrap()
        .read_run_events()
        .await
        .unwrap();
    let kinds: Vec<_> = events
        .iter()
        .map(|event| event.payload.discriminant_str())
        .collect();
    let requested = kinds
        .iter()
        .position(|kind| *kind == "SandboxElevationRequested")
        .unwrap();
    if stop {
        assert!(kinds.contains(&"RunAborted"));
        assert!(!kinds.contains(&"SandboxElevationDecided"));
        assert!(!kinds.contains(&"SandboxElevationTimedOut"));
        assert!(!kinds.contains(&"OutcomeReported"));
        assert!(
            engine
                .resolve_elevation(
                    id,
                    session,
                    request_id,
                    surge_orchestrator::engine::elevation::EngineElevationDecision {
                        decision: ElevationDecision::Allow,
                        remember: false,
                        option_id: "allow".into(),
                    }
                )
                .await
                .is_err(),
            "stale permission must be removed"
        );
    } else {
        let decided = kinds
            .iter()
            .position(|kind| *kind == "SandboxElevationDecided")
            .unwrap();
        let outcome = kinds
            .iter()
            .position(|kind| *kind == "OutcomeReported")
            .unwrap();
        assert!(requested < decided && decided < outcome);
    }
    drop(engine);
    let real = Arc::try_unwrap(bridge).ok().expect("bridge still owned");
    real.bridge.shutdown().await.unwrap();
    let pid = std::fs::read_to_string(root.join("child.pid"))
        .unwrap()
        .parse()
        .unwrap();
    let tracker = surge_acp::ProcessTracker::new(root.join("tracking")).unwrap();
    tracker.track("child", pid).unwrap();
    assert!(
        !tracker.is_running("child"),
        "child survived verified shutdown"
    );
}

async fn verify_failure(
    root: &std::path::Path,
    storage: &Arc<Storage>,
    bridge: Arc<RealBridge>,
    success: bool,
) {
    use surge_core::{
        agent_config::AgentConfig,
        node::{LedgerEffect, OutcomeDecl},
        sandbox::{SandboxConfig, SandboxMode},
    };
    use surge_orchestrator::engine::{
        hooks::HookExecutor,
        stage::agent::{AgentStageParams, execute_agent_stage},
    };
    let id = RunId::new();
    let workspace = tempfile::tempdir().unwrap();
    let worktree = workspace.path();
    for args in [
        vec!["init"],
        vec!["config", "user.name", "Fixture"],
        vec!["config", "user.email", "fixture@example.com"],
        vec!["commit", "--allow-empty", "-m", "fixture"],
    ] {
        assert!(
            std::process::Command::new("git")
                .args(args)
                .current_dir(worktree)
                .output()
                .unwrap()
                .status
                .success()
        );
    }
    let writer = storage.create_run(id, worktree, None).await.unwrap();
    let artifacts = surge_persistence::artifacts::ArtifactStore::new(root.join("runs"));
    let item: toml::Value = toml::from_str("id='task-under-verification'\ntitle='Acceptance'\nacceptance_criteria=['Acceptance works']\n").unwrap();
    let frames = [surge_orchestrator::engine::frames::Frame::Loop(
        surge_orchestrator::engine::frames::LoopFrame {
            loop_node: "tasks".parse().unwrap(),
            config: surge_core::loop_config::LoopConfig {
                iterates_over: surge_core::loop_config::IterableSource::Static(vec![item.clone()]),
                body: "body".parse().unwrap(),
                iteration_var_name: "task".into(),
                exit_condition: surge_core::loop_config::ExitCondition::AllItems,
                on_iteration_failure: Default::default(),
                parallelism: Default::default(),
                gate_after_each: false,
            },
            items: vec![item],
            current_index: 0,
            attempts_remaining: 0,
            return_to: "end".parse().unwrap(),
            traversal_counts: Default::default(),
        },
    )];
    let config = AgentConfig {
        profile: surge_core::ProfileKey::try_from("implementer@1.0").unwrap(),
        sandbox_override: Some(SandboxConfig {
            mode: SandboxMode::ReadOnly,
            ..Default::default()
        }),
        prompt_overrides: None,
        tool_overrides: None,
        approvals_override: None,
        bindings: vec![],
        rules_overrides: None,
        limits: Default::default(),
        hooks: vec![],
        custom_fields: Default::default(),
    };
    let outcomes = [OutcomeDecl {
        id: surge_core::OutcomeKey::try_from("done").unwrap(),
        description: "verified".into(),
        edge_kind_hint: surge_core::edge::EdgeKind::Forward,
        is_terminal: false,
        ledger_effect: LedgerEffect::Verified,
    }];
    let node = surge_core::NodeKey::try_from("verify").unwrap();
    let memory = surge_core::run_state::RunMemory::default();
    let dispatcher: Arc<dyn surge_orchestrator::engine::tools::ToolDispatcher> =
        Arc::new(WorktreeToolDispatcher::new(worktree.into()));
    let bridge: Arc<dyn BridgeFacade> = bridge;
    let result = execute_agent_stage(AgentStageParams {
        quota_opening: None,
        quota_cycle: None,
        quota_owner: None,
        continuation: None,
        frames: &frames,
        cancel: tokio_util::sync::CancellationToken::new(),
        node: &node,
        attempt: 1,
        steers: vec![],
        agent_config: &config,
        bound_skills: &[],
        declared_outcomes: &outcomes,
        bridge: &bridge,
        writer: &writer,
        artifact_store: &artifacts,
        worktree_path: worktree,
        tool_dispatcher: &dispatcher,
        run_memory: &memory,
        run_id: id,
        tool_resolutions: &Arc::new(tokio::sync::Mutex::new(Default::default())),
        human_input_timeout: Duration::from_secs(2),
        mcp_registry: None,
        mcp_servers: vec![],
        tool_call_loop_guard: Default::default(),
        output_spill: Default::default(),
        profile_registry: None,
        agent_registry: None,
        hook_executor: &HookExecutor::new(),
        pending_elevations: surge_orchestrator::engine::elevation::PendingElevations::new(),
        active_task_id: Some("task-under-verification".into()),
    })
    .await;
    assert_eq!(result.is_ok(), success, "{result:?}");
    let events = storage
        .open_run_reader(id)
        .await
        .unwrap()
        .read_run_events()
        .await
        .unwrap();
    assert_eq!(
        events
            .iter()
            .any(|event| matches!(event.payload, EventPayload::TaskVerified { .. })),
        success
    );
    assert_eq!(
        events
            .iter()
            .any(|event| matches!(event.payload, EventPayload::OutcomeReported { .. })),
        success
    );
}

async fn wait_for_human_request(
    tap: &mut tokio::sync::broadcast::Receiver<surge_orchestrator::engine::event_tap::RunEventTap>,
) -> Option<String> {
    loop {
        let event = tap.recv().await.unwrap();
        if let EventPayload::HumanInputRequested { call_id, .. } = event.event.payload.payload {
            return call_id;
        }
    }
}

async fn wait_for_deadline(
    tap: &mut tokio::sync::broadcast::Receiver<surge_orchestrator::engine::event_tap::RunEventTap>,
) {
    loop {
        match tap.recv().await {
            Ok(event)
                if matches!(
                    event.event.payload.payload,
                    EventPayload::EscalationRequested { .. }
                ) =>
            {
                return;
            },
            Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {},
            Err(error) => panic!("deadline event stream closed: {error}"),
        }
    }
}

struct SpoofedBroadcast {
    events: tokio::sync::broadcast::Sender<BridgeEvent>,
}
#[async_trait::async_trait]
impl BridgeFacade for SpoofedBroadcast {
    async fn open_session(
        &self,
        config: SessionConfig,
    ) -> Result<surge_core::execution_recovery::OpenedSession, OpenSessionError> {
        use surge_core::execution_recovery::{
            OpenedSession, ProviderSessionDescriptor, ProviderSessionId, SessionOpenMode,
        };
        let session = config
            .stage_mcp
            .expect("Engine must prepare authenticated MCP")
            .session;
        self.events
            .send(BridgeEvent::HumanInputRequested {
                session,
                call_id: "spoof".into(),
                question: "unauthenticated question".into(),
                context: None,
            })
            .unwrap();
        self.events
            .send(BridgeEvent::OutcomeReported {
                session,
                outcome: surge_core::OutcomeKey::try_from("done").unwrap(),
                summary: "spoof".into(),
                artifacts_produced: vec![],

                verification_report: None,
            })
            .unwrap();
        let mut opened = OpenedSession::new(
            SessionId::new(),
            ProviderSessionDescriptor::new(
                ProviderSessionId::new("spoofed-broadcast".into()).unwrap(),
                config.invocation,
                config.runtime,
                surge_core::ContentHash::compute(format!("{:?}", config.agent_kind).as_bytes()),
                config.working_dir,
                Default::default(),
            )
            .unwrap(),
            SessionOpenMode::New,
        )
        .unwrap();
        opened.execution_writer = Some(
            surge_core::execution_recovery::process::ExecutionWriterObservation::new(
                config.writer_id,
                None,
            )
            .unwrap(),
        );
        Ok(opened)
    }
    async fn send_message(&self, _: SessionId, _: MessageContent) -> Result<(), SendMessageError> {
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
        panic!("display notification reached tool reply")
    }
    async fn reply_to_permission(
        &self,
        _: SessionId,
        _: String,
        _: RequestPermissionResponse,
    ) -> Result<(), ReplyToPermissionError> {
        panic!("unexpected permission")
    }
    fn subscribe(&self) -> tokio::sync::broadcast::Receiver<BridgeEvent> {
        self.events.subscribe()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_session_ignores_injected_legacy_broadcast_authority() {
    let root = tempfile::tempdir().unwrap();
    let storage = Storage::open(root.path()).await.unwrap();
    let (events, _) = tokio::sync::broadcast::channel(16);
    let engine = Engine::new(
        Arc::new(SpoofedBroadcast { events }),
        storage.clone(),
        Arc::new(WorktreeToolDispatcher::new(root.path().into())),
        EngineConfig::default(),
    );
    let graph = toml::from_str(include_str!("../../../examples/flow_elevation_demo.toml")).unwrap();
    let id = RunId::new();
    let handle = engine
        .start_run(id, graph, root.path().into(), EngineRunConfig::default())
        .await
        .unwrap();
    let outcome = tokio::time::timeout(Duration::from_secs(3), handle.await_completion())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(outcome, RunOutcome::Failed { .. }), "{outcome:?}");
    let events = storage
        .open_run_reader(id)
        .await
        .unwrap()
        .read_run_events()
        .await
        .unwrap();
    assert!(!events.iter().any(|event| matches!(
        event.payload,
        EventPayload::OutcomeReported { .. }
            | EventPayload::HumanInputRequested { .. }
            | EventPayload::StageToolReceipt { .. }
    )));
}

fn kill_mcp_helper(root: &std::path::Path) {
    let pid = std::fs::read_to_string(root.join("mcp.pid")).unwrap();
    #[cfg(unix)]
    let status = std::process::Command::new("kill")
        .args(["-KILL", pid.trim()])
        .status()
        .unwrap();
    #[cfg(windows)]
    let status = std::process::Command::new("taskkill")
        .args(["/F", "/PID", pid.trim()])
        .status()
        .unwrap();
    assert!(status.success(), "controlled helper was not killed");
}
