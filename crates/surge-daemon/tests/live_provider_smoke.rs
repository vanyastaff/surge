//! Opt-in real daemon/ACP/MCP journey. Never runs a provider in ordinary CI.
//! Build surge + mock_acp_agent first, then run the ignored controlled test.
//! SURGE_LIVE_CODEX=1 enables exactly one bounded live invocation.
#[path = "support/live_provider_capture.rs"]
mod live_provider_capture;
#[path = "support/runtime_home.rs"]
mod runtime_home_fixture;
use live_provider_capture::{ReadStatus, SmokeCounts, SmokeDiagnostic, SmokeFailure, SmokePhase};
use runtime_home_fixture::FixtureHome;

use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use surge_acp::bridge::AcpBridge;
use surge_core::{RunId, graph::Graph, run_event::EventPayload, stage_tool::StageToolResult};
use surge_daemon::{ServerConfig, admission::AdmissionController, broadcast::BroadcastRegistry};
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
use tokio_util::sync::CancellationToken;

const SENTINEL: &str = "Surge live smoke must not change this file.\n";

#[test]
#[ignore = "requires freshly built surge and mock_acp_agent binaries"]
fn controlled_daemon_mcp_smoke() {
    watchdog(false);
}

#[test]
#[ignore = "explicit live provider invocation; may consume subscription usage"]
fn live_codex_daemon_mcp_smoke() {
    assert_eq!(
        std::env::var("SURGE_LIVE_CODEX").as_deref(),
        Ok("1"),
        "set SURGE_LIVE_CODEX=1 explicitly"
    );
    watchdog(true);
}

fn watchdog(live: bool) {
    if let Some(root) = std::env::var_os("SURGE_SMOKE_CHILD") {
        let home = PathBuf::from(std::env::var_os("SURGE_HOME").unwrap());
        let mut diagnostic = SmokeDiagnostic::default();
        let result = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(journey(
                PathBuf::from(root),
                home.clone(),
                live,
                &mut diagnostic,
            ));
        if result.is_err() && diagnostic.failure.is_none() {
            diagnostic.failure = Some(SmokeFailure::HostFailed);
        }
        let published = live_provider_capture::EvidenceFile::create(&diagnostic_path(&home))
            .and_then(|mut file| file.write_diagnostic(&diagnostic));
        assert!(
            result.is_ok() && published.is_ok(),
            "smoke child failed: {:?}",
            diagnostic.failure
        );
        return;
    }
    let root = tempfile::Builder::new()
        .prefix("surge-live-")
        .tempdir()
        .unwrap();
    let home = FixtureHome::new().unwrap();
    let test = if live {
        "live_codex_daemon_mcp_smoke"
    } else {
        "controlled_daemon_mcp_smoke"
    };
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command
        .args(["--ignored", "--exact", test, "--nocapture"])
        .env("SURGE_SMOKE_CHILD", root.path())
        .env("SURGE_HOME", home.path());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let status = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(watch_owned(command));
    let model = provider_launch(live)
        .map(|launch| launch.model)
        .unwrap_or_else(|_| "requested-model-unavailable".into());
    let result = finish_parent(status, home, root, &model, &mut std::io::stderr());
    assert!(result.is_ok(), "smoke failed: {:?}", result.err());
}

#[derive(Debug, Clone, Copy, serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum WatchStatus {
    Succeeded,
    Failed,
    TimedOut,
    Unconfirmed,
}
fn diagnostic_path(home: &Path) -> PathBuf {
    capture_path(home).with_file_name("smoke-diagnostic.json")
}
#[derive(serde::Serialize)]
struct ParentObservation {
    process: WatchStatus,
    report_status: ReadStatus,
    diagnostic: Option<SmokeDiagnostic>,
    capture_status: ReadStatus,
    requested_model: String,
    offered_exact_observed: Option<bool>,
    selected_exact_observed: Option<bool>,
    provider_session_observed: Option<bool>,
    adapter_metadata_observed: Option<bool>,
    selection_mode: Option<live_provider_capture::SelectionMode>,
    selection_source: Option<live_provider_capture::SelectionMode>,
    prompt_admissions: Option<usize>,
    failure: Option<SmokeFailure>,
    retained_paths: Vec<PathBuf>,
}
fn read_observation(
    status: Result<WatchStatus, SmokeFailure>,
    home: &Path,
    model: &str,
) -> ParentObservation {
    let mut observed = ParentObservation {
        process: status.unwrap_or(WatchStatus::Unconfirmed),
        report_status: ReadStatus::Unconfirmed,
        diagnostic: None,
        capture_status: ReadStatus::Unconfirmed,
        requested_model: model.into(),
        offered_exact_observed: None,
        selected_exact_observed: None,
        provider_session_observed: None,
        adapter_metadata_observed: None,
        selection_mode: None,
        selection_source: None,
        prompt_admissions: None,
        failure: status.err(),
        retained_paths: vec![],
    };
    if status.is_err() {
        return observed;
    }
    let diagnostic = live_provider_capture::read_bounded::<SmokeDiagnostic>(&diagnostic_path(home));
    match diagnostic {
        Ok(report) => {
            observed.report_status = ReadStatus::Valid;
            observed.diagnostic = Some(report);
        },
        Err(status) => {
            observed.report_status = status;
            observed.failure = Some(match status {
                ReadStatus::Oversized => SmokeFailure::DiagnosticOversized,
                ReadStatus::Missing => SmokeFailure::DiagnosticUnavailable,
                _ => SmokeFailure::DiagnosticInvalid,
            });
            return observed;
        },
    }
    let report = observed.diagnostic.clone().unwrap();
    if !report.settled {
        observed.failure = Some(SmokeFailure::CleanupUnconfirmed);
        return observed;
    }
    observed.failure = report.failure.or(match observed.process {
        WatchStatus::Failed => Some(SmokeFailure::ChildFailed),
        WatchStatus::TimedOut => Some(SmokeFailure::WatchdogTimedOut),
        _ => None,
    });
    project_capture(&mut observed, home, model);
    if observed.failure.is_none() && !success_verified(&observed, &report) {
        observed.failure = Some(SmokeFailure::VerificationFailed);
    }
    observed
}
fn project_capture(observed: &mut ParentObservation, home: &Path, model: &str) {
    match live_provider_capture::read_bounded::<live_provider_capture::Capture>(&capture_path(home))
    {
        Ok(capture)
            if capture.rejection.is_none()
                && capture.strict_policy()
                && capture.required_model == model
                && capture.prompts <= 1 =>
        {
            observed.capture_status = ReadStatus::Valid;
            observed.offered_exact_observed = Some(
                capture
                    .offered_models
                    .iter()
                    .any(|offered| offered == model),
            );
            observed.selected_exact_observed =
                Some(capture.selected_model.as_deref() == Some(model));
            observed.provider_session_observed = Some(capture.provider_session.is_some());
            observed.adapter_metadata_observed =
                Some(capture.adapter_name.is_some() && capture.adapter_version.is_some());
            observed.selection_mode = Some(capture.selection_mode);
            observed.selection_source = capture.selection_source;
            observed.prompt_admissions = Some(capture.prompts);
        },
        Ok(_) => {
            observed.capture_status = ReadStatus::Invalid;
        },
        Err(status) => {
            observed.capture_status = status;
        },
    }
}
fn success_verified(observed: &ParentObservation, report: &SmokeDiagnostic) -> bool {
    !(observed.capture_status != ReadStatus::Valid
        || observed.offered_exact_observed != Some(true)
        || observed.selected_exact_observed != Some(true)
        || observed.provider_session_observed != Some(true)
        || observed.adapter_metadata_observed != Some(true)
        || observed.selection_mode != Some(live_provider_capture::SelectionMode::ExplicitSetAck)
        || observed.selection_source != Some(live_provider_capture::SelectionMode::ExplicitSetAck)
        || observed.prompt_admissions != Some(1)
        || !report.binding_verified
        || report.phase != SmokePhase::Completed
        || report.counts.as_ref()
            != Some(&SmokeCounts {
                session_opened: 1,
                human_input_requested: 1,
                human_input_resolved: 1,
                stage_tool_receipt: 2,
                outcome_reported: 1,
                run_completed: 1,
                run_failed: 0,
            }))
}
fn finish_parent(
    status: Result<WatchStatus, SmokeFailure>,
    home: FixtureHome,
    root: tempfile::TempDir,
    model: &str,
    output: &mut impl std::io::Write,
) -> Result<(), SmokeFailure> {
    let mut observed = read_observation(status, home.path(), model);
    let may_close = observed
        .diagnostic
        .as_ref()
        .is_some_and(|report| report.settled)
        && !matches!(observed.process, WatchStatus::Unconfirmed);
    if !may_close {
        observed.retained_paths = vec![home.retain(), root.keep()];
        let _published = serde_json::to_writer(&mut *output, &observed).is_ok()
            && output.write_all(b"\n").is_ok();
        return Err(observed.failure.unwrap_or(SmokeFailure::CleanupUnconfirmed));
    }
    // Close fixtures even if publication fails; original provider failure wins.
    let published =
        serde_json::to_writer(&mut *output, &observed).is_ok() && output.write_all(b"\n").is_ok();
    let closed = home.close().is_ok() & root.close().is_ok();
    if let Some(failure) = observed.failure {
        return Err(failure);
    }
    if !closed {
        return Err(SmokeFailure::FixtureCleanupFailed);
    }
    if !published {
        return Err(SmokeFailure::OutputUnavailable);
    }
    Ok(())
}

async fn watch_owned(mut command: std::process::Command) -> Result<WatchStatus, SmokeFailure> {
    command
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = live_provider_capture::OwnedProcess::spawn(command, true)
        .map_err(|_| SmokeFailure::CleanupUnconfirmed)?;
    if child.id() == 0 {
        return Err(SmokeFailure::CleanupUnconfirmed);
    }
    #[cfg(unix)]
    let container = surge_acp::process_evidence::observe_container(
        child.id(),
        surge_core::execution_recovery::process::WriterCoverage::GroupOnly,
    )
    .map_err(|_| SmokeFailure::CleanupUnconfirmed)?;
    #[cfg(windows)]
    let container = child
        .container()
        .map_err(|_| SmokeFailure::CleanupUnconfirmed)?;
    let (input, mut output, mut error) = child
        .pipes()
        .map_err(|_| SmokeFailure::CleanupUnconfirmed)?;
    drop(input);
    let stdout =
        tokio::spawn(async move { tokio::io::copy(&mut output, &mut tokio::io::sink()).await });
    let stderr =
        tokio::spawn(async move { tokio::io::copy(&mut error, &mut tokio::io::sink()).await });
    let result = child.wait(Duration::from_secs(150)).await;
    child
        .terminate()
        .map_err(|_| SmokeFailure::CleanupUnconfirmed)?;
    child
        .wait(Duration::from_secs(10))
        .await
        .map_err(|_| SmokeFailure::CleanupUnconfirmed)?;
    tokio::time::timeout(Duration::from_secs(10), async {
        stdout
            .await
            .map_err(|_| SmokeFailure::CleanupUnconfirmed)?
            .map_err(|_| SmokeFailure::CleanupUnconfirmed)?;
        stderr
            .await
            .map_err(|_| SmokeFailure::CleanupUnconfirmed)?
            .map_err(|_| SmokeFailure::CleanupUnconfirmed)?;
        Ok::<(), SmokeFailure>(())
    })
    .await
    .map_err(|_| SmokeFailure::CleanupUnconfirmed)??;
    tokio::time::timeout(Duration::from_secs(10), async {
        while surge_acp::process_evidence::group_state(&container)
            != surge_acp::process_evidence::GroupState::Empty
        {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .map_err(|_| SmokeFailure::CleanupUnconfirmed)?;
    Ok(match result {
        Ok(status) if status.success() => WatchStatus::Succeeded,
        Ok(_) => WatchStatus::Failed,
        Err(_) => WatchStatus::TimedOut,
    })
}

fn git(path: &Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success(), "temporary Git preparation failed");
}

fn prepare(
    root: &Path,
    home: &Path,
    live: bool,
) -> (PathBuf, surge_core::SurgeConfig, Arc<ProfileRegistry>) {
    let repo = root.join("repository");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::write(repo.join("sentinel.txt"), SENTINEL).unwrap();
    git(&repo, &["init", "-q"]);
    git(
        &repo,
        &[
            "-c",
            "user.name=Smoke",
            "-c",
            "user.email=smoke@example.invalid",
            "add",
            "sentinel.txt",
        ],
    );
    git(
        &repo,
        &[
            "-c",
            "user.name=Smoke",
            "-c",
            "user.email=smoke@example.invalid",
            "commit",
            "-qm",
            "sentinel",
        ],
    );
    let worktree = root.join("worktree");
    git(
        &repo,
        &["worktree", "add", "--detach", worktree.to_str().unwrap()],
    );
    let launch = provider_launch(live).expect("explicit ACP command/args/model are required");
    let proxy = proxy_binary();
    assert!(proxy.is_file(), "build the acp_smoke_proxy example first");
    let capture = capture_path(home);
    let mut proxy_args = vec![
        capture.to_string_lossy().into_owned(),
        launch.model.clone(),
        launch.command,
    ];
    proxy_args.extend(launch.args);
    let mut config = surge_core::SurgeConfig::default();
    let agent: surge_core::config::AgentConfig = serde_json::from_value(serde_json::json!({
        "command":proxy, "args":proxy_args
    }))
    .unwrap();
    config.agents.insert("live-smoke".into(), agent);
    std::fs::write(repo.join("surge.toml"), toml::to_string(&config).unwrap()).unwrap();
    let profiles = home.join("profiles");
    std::fs::create_dir_all(&profiles).unwrap();
    let mut profile: surge_core::profile::Profile = toml::from_str(include_str!(
        "../../surge-core/bundled/profiles/mock-1.0.toml"
    ))
    .unwrap();
    profile.role.id = "live-smoke".parse().unwrap();
    profile.runtime.agent_id = "live-smoke".into();
    profile.runtime.recommended_model = "provider-default".into();
    profile.sandbox.mode = surge_core::sandbox::SandboxMode::ReadOnly;
    profile.prompt.system = "This is a transport smoke test. Do not read, create, edit, or delete files. Do not run shell commands. Call request_human_input with call_id smoke-human and question Return the smoke nonce. Wait for its JSON response. Then call report_stage_outcome with call_id smoke-report, outcome done, summary equal to the returned nonce string, and artifacts_produced []. Do not guess the nonce. Stop after the report tool reply.".into();
    std::fs::write(
        profiles.join("live-smoke.toml"),
        toml::to_string(&profile).unwrap(),
    )
    .unwrap();
    let registry = Arc::new(ProfileRegistry::new(
        DiskProfileSet::scan(&profiles).unwrap(),
    ));
    seed_catalog_baseline(&worktree, &registry, &config);
    git(&repo, &["add", "surge.toml"]);
    git(
        &repo,
        &[
            "-c",
            "user.name=Smoke",
            "-c",
            "user.email=smoke@example.invalid",
            "commit",
            "-qm",
            "configuration baseline",
        ],
    );
    (worktree, config, registry)
}

fn capture_path(home: &Path) -> PathBuf {
    let directory = home.join("daemon");
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        let mut builder = std::fs::DirBuilder::new();
        builder.mode(0o700);
        if !directory.exists() {
            builder.create(&directory).unwrap();
        }
    }
    directory.join("provider-capture.json")
}

struct ProviderLaunch {
    command: String,
    args: Vec<String>,
    model: String,
}
fn provider_launch(live: bool) -> Result<ProviderLaunch, String> {
    if live {
        let command = std::env::var("SURGE_LIVE_ACP_COMMAND")
            .map_err(|_| "missing SURGE_LIVE_ACP_COMMAND")?;
        let args = std::env::var("SURGE_LIVE_ACP_ARGS_JSON")
            .map_err(|_| "missing SURGE_LIVE_ACP_ARGS_JSON")?;
        let model =
            std::env::var("SURGE_LIVE_ACP_MODEL").map_err(|_| "missing SURGE_LIVE_ACP_MODEL")?;
        let args: Vec<String> =
            serde_json::from_str(&args).map_err(|_| "invalid ACP arguments JSON")?;
        live_provider_capture::ProtocolCapture::new(model.clone()).map_err(str::to_owned)?;
        if command.is_empty() || command.contains('\0') || args.iter().any(|arg| arg.contains('\0'))
        {
            return Err("invalid ACP command/arguments".into());
        }
        Ok(ProviderLaunch {
            command,
            args,
            model,
        })
    } else {
        Ok(ProviderLaunch {
            command: proxy_binary()
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .join(format!("mock_acp_agent{}", std::env::consts::EXE_SUFFIX))
                .to_string_lossy()
                .into_owned(),
            args: vec![
                "--stage-mcp".into(),
                "--stage-mcp-case=smoke".into(),
                "--config-options".into(),
                "--ack-config-choice".into(),
            ],
            model: "opus".into(),
        })
    }
}
fn proxy_binary() -> PathBuf {
    std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("examples")
        .join(format!("acp_smoke_proxy{}", std::env::consts::EXE_SUFFIX))
}
fn check_provider_capture(
    path: &Path,
    events: &[surge_core::RunEvent],
    model: &str,
) -> Result<(), String> {
    let capture: live_provider_capture::Capture = live_provider_capture::read_bounded(path)
        .map_err(|_| "provider capture missing, invalid or oversized")?;
    let mut sessions = events.iter().filter_map(|event| {
        if let EventPayload::SessionOpened {
            session,
            opened: Some(opened),
            ..
        } = &event.payload
        {
            Some((*session, opened.descriptor.provider_session_id().as_str()))
        } else {
            None
        }
    });
    let Some((session, provider)) = sessions.next() else {
        return Err("provider session proof missing".into());
    };
    if capture.rejection.is_some() || !capture.strict_policy() || sessions.next().is_some() || capture.provider_session.as_deref() != Some(provider)
        || capture.required_model != model || capture.selected_model.as_deref() != Some(model)
        || !capture.offered_models.iter().any(|offered| offered == model) || capture.prompts != 1
        || capture.selection_mode != live_provider_capture::SelectionMode::ExplicitSetAck
        || capture.selection_source != Some(live_provider_capture::SelectionMode::ExplicitSetAck)
        || capture.adapter_name.is_none() || capture.adapter_version.is_none()
        || events.iter().any(|event| matches!(&event.payload, EventPayload::StageToolReceipt { receipt } if receipt.context.session != session)) {
        return Err("exact-model same-session proof failed".into());
    }
    eprintln!("smoke provider model confirmed: {model}; one authenticated session/prompt");
    Ok(())
}

fn seed_catalog_baseline(
    worktree: &Path,
    registry: &ProfileRegistry,
    config: &surge_core::SurgeConfig,
) {
    let agents = surge_acp::Registry::for_run(config);
    let mut unavailable = std::collections::BTreeMap::new();
    for runtime in surge_orchestrator::profile_loader::catalog_runtimes(registry, Some(&agents)) {
        if let Some(entry) = agents.find_normalized(&runtime)
            && let Err(error) = surge_acp::agent_env::resolve(&entry.id, &entry.env)
        {
            unavailable.insert(runtime, format!("not configured: {error}"));
        }
    }
    // Engine always seeds this catalog; commit its exact deterministic contents
    // before the provider starts so the checkout oracle allows no new files.
    std::fs::create_dir(worktree.join(".surge")).unwrap();
    std::fs::write(
        worktree.join(".surge/profile_catalog.md"),
        surge_orchestrator::profile_loader::render_profile_catalog_with(
            registry,
            Some(&agents),
            &unavailable,
        ),
    )
    .unwrap();
    git(worktree, &["add", ".surge/profile_catalog.md"]);
    git(
        worktree,
        &[
            "-c",
            "user.name=Smoke",
            "-c",
            "user.email=smoke@example.invalid",
            "commit",
            "-qm",
            "catalog baseline",
        ],
    );
}

fn graph(model: &str) -> Graph {
    let mut graph: Graph =
        toml::from_str(include_str!("../../../examples/flow_minimal_agent.toml")).unwrap();
    let surge_core::node::NodeConfig::Agent(agent) = &mut graph
        .nodes
        .values_mut()
        .find(|node| matches!(node.config, surge_core::node::NodeConfig::Agent(_)))
        .unwrap()
        .config
    else {
        unreachable!()
    };
    agent.profile = "live-smoke@1.0".parse().unwrap();
    *agent = agent.with_runtime_override("live-smoke", Some(model));
    // The transport smoke is fully specified by its system prompt, without
    // the example workflow's required initial-prompt artifact.
    agent.bindings.clear();
    agent.approvals_override = Some(
        serde_json::from_value(serde_json::json!({
            "policy":"on-request", "elevation":true,
            "elevation_channels":[{"type":"desktop","duration":"persistent"}]
        }))
        .unwrap(),
    );
    agent.limits.timeout_seconds = 60;
    agent.limits.max_retries = 0;
    agent.limits.max_tokens = 4096;
    graph
}

async fn connect(socket: PathBuf) -> Result<DaemonEngineFacade, String> {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(client) = DaemonEngineFacade::connect(socket.clone()).await {
                return client;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .map_err(|_| "daemon connection timeout".into())
}

async fn journey(
    root: PathBuf,
    home: PathBuf,
    live: bool,
    diagnostic: &mut SmokeDiagnostic,
) -> Result<(), String> {
    let nonce = RunId::new().to_string();
    let (worktree, config, profiles) = prepare(&root, &home, live);
    let pins = [
        CheckoutPin::capture(root.join("repository")),
        CheckoutPin::capture(worktree.clone()),
    ];
    let storage = Storage::open(&home).await.map_err(|_| "storage open")?;
    let bridge = Arc::new(AcpBridge::with_defaults().map_err(|_| "bridge open")?);
    let engine = Arc::new(Engine::new_full(
        bridge.clone(),
        storage.clone(),
        Arc::new(WorktreeToolDispatcher::new(worktree.clone())),
        Arc::new(surge_notify::MultiplexingNotifier::new()),
        None,
        Some(profiles),
        EngineConfig {
            agent_registry: Some(Arc::new(surge_acp::Registry::for_run(&config))),
            ..Default::default()
        },
    ));
    let shutdown = CancellationToken::new();
    let admission = Arc::new(AdmissionController::new(1, 0));
    let socket = root.join("smoke.sock");
    let server = tokio::spawn(surge_daemon::run_runs_only(
        ServerConfig {
            socket_path: socket.clone(),
            max_active: 1,
            max_queue: 0,
        },
        Arc::new(LocalEngineFacade::new(engine.clone())),
        surge_daemon::tracked_run::TrackingContext::new(engine.clone(), storage.clone()),
        Arc::new(BroadcastRegistry::new()),
        admission.clone(),
        shutdown.clone(),
    ));
    let launch = provider_launch(live)?;
    let run_id = RunId::new();
    let result = exercise(
        socket,
        &storage,
        &worktree,
        &nonce,
        &launch.model,
        run_id,
        diagnostic,
    )
    .await;
    for active in engine.snapshot_active_runs().await {
        let _ = engine.stop_run(active.run_id, "smoke cleanup".into()).await;
    }
    shutdown.cancel();
    let server_result = tokio::time::timeout(Duration::from_secs(10), server).await;
    drop(engine);
    // Tasks spawned by the server and run tracking release their engine (and
    // through it the bridge) after shutdown is observed, not synchronously.
    let released = tokio::time::timeout(Duration::from_secs(10), async {
        while Arc::strong_count(&bridge) > 1 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    if released.is_err() {
        eprintln!(
            "bridge still has {} owners 10s after shutdown",
            Arc::strong_count(&bridge)
        );
    }
    let bridge_result = match Arc::try_unwrap(bridge) {
        Ok(bridge) => bridge
            .shutdown()
            .await
            .map_err(|_| "provider cleanup unconfirmed".to_owned()),
        Err(_) => Err("bridge ownership remained after cleanup".to_owned()),
    };
    let server_result = server_result
        .map_err(|_| "server cleanup timeout".to_owned())
        .and_then(|joined| joined.map_err(|_| "server task failed".to_owned()))
        .and_then(|served| served.map_err(|_| "server failed".to_owned()));
    diagnostic.settled = bridge_result.is_ok() && server_result.is_ok();
    if !diagnostic.settled && diagnostic.failure.is_none() {
        diagnostic.failure = Some(SmokeFailure::CleanupUnconfirmed);
    }
    // Observe final event kinds independently of primary failure and cleanup results.
    if let Ok(reader) = storage.open_run_reader(run_id).await
        && let Ok(events) = reader.read_run_events().await
    {
        diagnostic.counts = Some(diagnostic_counts(&events));
    }
    finish_checked(result.and(bridge_result).and(server_result), &pins)?;
    let events = storage
        .open_run_reader(run_id)
        .await
        .map_err(|_| "capture reader")?
        .read_run_events()
        .await
        .map_err(|_| "capture events")?;
    check_provider_capture(&capture_path(&home), &events, &launch.model)
        .inspect_err(|_| diagnostic.failure = Some(SmokeFailure::VerificationFailed))?;
    diagnostic.binding_verified = true;
    diagnostic.phase = SmokePhase::Completed;
    assert_eq!(admission.snapshot().await.active, 0);
    Ok(())
}

async fn exercise(
    socket: PathBuf,
    storage: &Arc<Storage>,
    worktree: &Path,
    nonce: &str,
    model: &str,
    id: RunId,
    diagnostic: &mut SmokeDiagnostic,
) -> Result<(), String> {
    diagnostic.phase = SmokePhase::Running;
    let client = connect(socket).await?;
    let config = EngineRunConfig {
        budget: surge_core::budget::BudgetGuard {
            limits: surge_core::budget::BudgetLimits {
                tokens: Some(8192),
                ..Default::default()
            },
            ..Default::default()
        },
        ..Default::default()
    };
    let mut handle = client
        .start_run(id, graph(model), worktree.to_path_buf(), config)
        .await
        .map_err(|_| "start rejected")?;
    let observed = tokio::time::timeout(
        Duration::from_secs(90),
        observe(&client, id, &mut handle.events, nonce),
    )
    .await;
    if !matches!(&observed, Ok(Ok(()))) {
        let _ = client.stop_run(id, "smoke cleanup".into()).await;
    }
    let completion = tokio::time::timeout(Duration::from_secs(10), handle.await_completion()).await;
    let complete: Result<(), String> = match completion {
        Ok(Ok(RunOutcome::Completed { .. })) => Ok(()),
        Ok(Ok(RunOutcome::Failed { error })) => {
            diagnostic.failure = Some(classify_failure(&error));
            Err("provider failed; diagnostic is typed".into())
        },
        _ => Err("provider completion unconfirmed".into()),
    };
    let events = storage
        .open_run_reader(id)
        .await
        .map_err(|_| "reader")?
        .read_run_events()
        .await
        .map_err(|_| "events")?;
    diagnostic.phase = SmokePhase::Completion;
    diagnostic.counts = Some(diagnostic_counts(&events));
    // Structural progress only: never print provider text, prompts, auth, or tool values.
    for kind in [
        "SessionOpened",
        "HumanInputRequested",
        "HumanInputResolved",
        "StageToolReceipt",
        "OutcomeReported",
        "RunCompleted",
        "RunFailed",
    ] {
        let count = events
            .iter()
            .filter(|event| event.payload.discriminant_str() == kind)
            .count();
        eprintln!("smoke durable {kind}: {count}");
    }
    complete?;
    observed.map_err(|_| "stream deadline")??;
    check_evidence(
        &events
            .iter()
            .map(|event| &event.payload)
            .collect::<Vec<_>>(),
        id,
        nonce,
    )
}

fn diagnostic_counts(events: &[surge_core::RunEvent]) -> SmokeCounts {
    let count = |kind| {
        events
            .iter()
            .filter(|event| event.payload.discriminant_str() == kind)
            .count() as u64
    };
    SmokeCounts {
        session_opened: count("SessionOpened"),
        human_input_requested: count("HumanInputRequested"),
        human_input_resolved: count("HumanInputResolved"),
        stage_tool_receipt: count("StageToolReceipt"),
        outcome_reported: count("OutcomeReported"),
        run_completed: count("RunCompleted"),
        run_failed: count("RunFailed"),
    }
}

fn classify_failure(error: &str) -> SmokeFailure {
    let error = error.to_ascii_lowercase();
    if [
        "unauthorized",
        "authentication",
        "not logged",
        "login",
        "api key",
    ]
    .iter()
    .any(|word| error.contains(word))
    {
        SmokeFailure::ProviderAuthenticationUnavailable
    } else if error.contains("timeout") || error.contains("timed out") {
        SmokeFailure::ProviderTimedOut
    } else if error.contains("does not offer model") {
        SmokeFailure::ProviderModelUnavailable
    } else {
        SmokeFailure::ProviderFailed
    }
}

async fn observe(
    client: &DaemonEngineFacade,
    id: RunId,
    events: &mut tokio::sync::broadcast::Receiver<EngineRunEvent>,
    nonce: &str,
) -> Result<(), String> {
    loop {
        match events.recv().await.map_err(|_| "stream closed or lagged")? {
            EngineRunEvent::Persisted { payload, .. } => {
                if let EventPayload::HumanInputRequested {
                    call_id: Some(call_id),
                    ..
                } = *payload
                {
                    client
                        .resolve_human_input(id, Some(call_id), serde_json::json!({"nonce":nonce}))
                        .await
                        .map_err(|_| "human reply rejected")?;
                }
            },
            EngineRunEvent::Terminal { .. } => return Ok(()),
            EngineRunEvent::StreamError { .. } => return Err("unconfirmed stream".into()),
            _ => {},
        }
    }
}

fn check_evidence(events: &[&EventPayload], run_id: RunId, nonce: &str) -> Result<(), String> {
    check_event_counts(events)?;
    let mut human = None;
    let mut resolved = false;
    let mut human_receipt = false;
    let mut report = false;
    let mut committed_outcome = false;
    let mut completed = 0;
    for event in events {
        match event {
            EventPayload::HumanInputRequested {
                call_id: Some(id), ..
            } => human = Some(id),
            EventPayload::HumanInputResolved {
                call_id: Some(id),
                response,
                ..
            } => resolved = human == Some(id) && response == &serde_json::json!({"nonce":nonce}),
            EventPayload::StageToolReceipt { receipt } if receipt.context.run == run_id => {
                match &receipt.result {
                    StageToolResult::HumanResponse { response } => {
                        human_receipt = resolved && response == &serde_json::json!({"nonce":nonce})
                    },
                    StageToolResult::OutcomeCandidate { candidate } => {
                        report = human_receipt
                            && candidate.summary == nonce
                            && candidate.outcome.as_str() == "done"
                    },
                    _ => {},
                }
            },
            EventPayload::OutcomeReported {
                outcome, summary, ..
            } => committed_outcome = report && outcome.as_str() == "done" && summary == nonce,
            EventPayload::RunCompleted { .. } if committed_outcome => completed += 1,
            _ => {},
        }
    }
    if human.is_some() && resolved && human_receipt && report && completed == 1 {
        Ok(())
    } else {
        Err("missing ordered authenticated human/nonce/report evidence".into())
    }
}

fn check_event_counts(events: &[&EventPayload]) -> Result<(), String> {
    for (kind, expected) in [
        ("SessionOpened", 1),
        ("HumanInputRequested", 1),
        ("HumanInputResolved", 1),
        ("StageToolReceipt", 2),
        ("OutcomeReported", 1),
        ("RunCompleted", 1),
        ("RunFailed", 0),
        ("RunAborted", 0),
        ("RunParked", 0),
        ("StageFailed", 0),
        ("HumanInputTimedOut", 0),
    ] {
        if events
            .iter()
            .filter(|event| event.discriminant_str() == kind)
            .count()
            != expected
        {
            return Err(format!("unexpected durable event count: {kind}"));
        }
    }
    Ok(())
}

struct CheckoutPin {
    path: PathBuf,
    head: Vec<u8>,
}
impl CheckoutPin {
    fn capture(path: PathBuf) -> Self {
        let head = git_output(&path, &["rev-parse", "HEAD"]).unwrap();
        Self { path, head }
    }
    fn check(&self) -> Result<(), String> {
        if git_output(&self.path, &["rev-parse", "HEAD"])? != self.head {
            return Err("checkout integrity failed".into());
        }
        let pinned = std::str::from_utf8(&self.head)
            .map_err(|_| "checkout integrity failed")?
            .trim();
        git_output(&self.path, &["diff", "--exit-code", pinned, "--"])?;
        if !git_output(
            &self.path,
            &[
                "status",
                "--porcelain",
                "--untracked-files=all",
                "--ignored=matching",
            ],
        )?
        .is_empty()
        {
            return Err("checkout integrity failed".into());
        }
        Ok(())
    }
}
fn git_output(path: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .map_err(|_| "checkout integrity failed")?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err("checkout integrity failed".into())
    }
}
fn finish_checked(result: Result<(), String>, pins: &[CheckoutPin]) -> Result<(), String> {
    for pin in pins {
        pin.check()?;
    }
    result
}

#[test]
fn checkout_oracle_rejects_committed_mutation_after_success_and_failure() {
    for target in 0..2 {
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().join("repository");
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-q"]);
        std::fs::write(repo.join("sentinel.txt"), SENTINEL).unwrap();
        git(&repo, &["add", "."]);
        git(
            &repo,
            &[
                "-c",
                "user.name=Smoke",
                "-c",
                "user.email=smoke@example.invalid",
                "commit",
                "-qm",
                "base",
            ],
        );
        let worktree = root.path().join("worktree");
        git(
            &repo,
            &["worktree", "add", "--detach", worktree.to_str().unwrap()],
        );
        let pins = [CheckoutPin::capture(repo), CheckoutPin::capture(worktree)];
        assert!(finish_checked(Ok(()), &pins).is_ok());
        let directory = &pins[target].path;
        std::fs::write(directory.join("provider-change.txt"), "unapproved").unwrap();
        git(directory, &["add", "."]);
        git(
            directory,
            &[
                "-c",
                "user.name=Smoke",
                "-c",
                "user.email=smoke@example.invalid",
                "commit",
                "-qm",
                "provider mutation",
            ],
        );
        assert_ne!(
            git_output(directory, &["rev-parse", "HEAD"]).unwrap(),
            pins[target].head
        );
        for outcome in [Ok(()), Err("provider failed".into())] {
            assert_eq!(
                finish_checked(outcome, &pins),
                Err("checkout integrity failed".into())
            );
        }
    }
}

fn valid_evidence(run: RunId, nonce: &str) -> Vec<EventPayload> {
    use surge_core::{
        ContentHash, SessionId,
        id::StageGenerationId,
        stage_tool::{StageOutcomeCandidate, StageToolContext, StageToolReceipt},
    };
    let node: surge_core::keys::NodeKey = "smoke".parse().unwrap();
    let session = SessionId::new();
    let context = StageToolContext {
        run,
        node: node.clone(),
        session,
        generation: StageGenerationId::new(),
    };
    let receipt = |id: &str, result| EventPayload::StageToolReceipt {
        receipt: StageToolReceipt {
            context: context.clone(),
            call_id: id.to_owned().try_into().unwrap(),
            arguments_hash: ContentHash::compute(b"fixture"),
            result,
        },
    };
    vec![
        EventPayload::SessionOpened {
            handoff: None,
            opened: None,
            node: node.clone(),
            session,
            agent: "smoke".into(),
            agent_id: Some("smoke".into()),
        },
        EventPayload::HumanInputRequested {
            node: node.clone(),
            session: Some(session),
            call_id: Some("human".into()),
            prompt: "nonce?".into(),
            schema: None,
        },
        EventPayload::HumanInputResolved {
            node: node.clone(),
            call_id: Some("human".into()),
            response: serde_json::json!({"nonce":nonce}),
        },
        receipt(
            "human",
            StageToolResult::HumanResponse {
                response: serde_json::json!({"nonce":nonce}),
            },
        ),
        receipt(
            "report",
            StageToolResult::OutcomeCandidate {
                candidate: StageOutcomeCandidate {
                    outcome: "done".parse().unwrap(),
                    summary: nonce.into(),
                    artifacts_produced: vec![],

                    verification_report: None,
                },
            },
        ),
        EventPayload::OutcomeReported {
            node,
            outcome: "done".parse().unwrap(),
            summary: nonce.into(),
        },
        EventPayload::RunCompleted {
            terminal_node: "end".parse().unwrap(),
        },
    ]
}

#[test]
fn evidence_oracle_rejects_extra_receipts_outcomes_terminals_and_missing_session() {
    let run = RunId::new();
    let good = valid_evidence(run, "independent-nonce");
    let check = |events: &[EventPayload]| {
        check_evidence(&events.iter().collect::<Vec<_>>(), run, "independent-nonce")
    };
    assert!(check(&good).is_ok());
    let mut incorrectly_accepted = Vec::new();
    for index in [0, 1, 2, 3, 4, 5, 6] {
        let mut altered = good.clone();
        altered.insert(0, good[index].clone());
        if check(&altered).is_ok() {
            incorrectly_accepted.push(index);
        }
    }
    let mut failed = good.clone();
    failed.push(EventPayload::RunFailed {
        error: "failure after success".into(),
    });
    assert!(check(&failed).is_err(), "extra failure was accepted");
    assert!(check(&good[1..]).is_err(), "missing session was accepted");
    assert!(
        incorrectly_accepted.is_empty(),
        "duplicate event indexes accepted: {incorrectly_accepted:?}"
    );
}

#[tokio::test]
#[ignore = "requires built acp_smoke_proxy example"]
async fn proxy_cancellation_settles_observed_descendant_and_owned_container() {
    use surge_acp::process_evidence::{self, GroupState};
    #[cfg(unix)]
    use surge_core::execution_recovery::process::WriterCoverage;
    let root = tempfile::tempdir().unwrap();
    let marker = root.path().join("descendant.pid");
    let home = FixtureHome::new().unwrap();
    let binary = proxy_binary();
    assert!(binary.is_file());
    let mut command = std::process::Command::new(&binary);
    command
        .arg(capture_path(home.path()))
        .arg("gpt-6.1-sol")
        .arg(&binary)
        .arg("--fixture-provider")
        .arg(&marker)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut owner = live_provider_capture::OwnedProcess::spawn(command, true).unwrap();
    let (input, mut output, mut error) = owner.pipes().unwrap();
    let stdout =
        tokio::spawn(async move { tokio::io::copy(&mut output, &mut tokio::io::sink()).await });
    let stderr =
        tokio::spawn(async move { tokio::io::copy(&mut error, &mut tokio::io::sink()).await });
    let descendant = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(text) = std::fs::read_to_string(&marker)
                && let Ok(pid) = text.parse::<u32>()
                && let Ok((identity, _)) = process_evidence::observe(pid)
            {
                break identity;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    #[cfg(unix)]
    let container =
        process_evidence::observe_container(owner.id(), WriterCoverage::GroupOnly).unwrap();
    #[cfg(windows)]
    let container = owner.container().unwrap();
    // Close the actual client transport, then exercise retaining owner cleanup.
    drop(input);
    tokio::time::sleep(Duration::from_millis(50)).await;
    owner.terminate().unwrap();
    owner.wait(Duration::from_secs(5)).await.unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let same_descendant = process_evidence::observe(descendant.pid())
                .is_ok_and(|(identity, _)| identity == descendant);
            if !same_descendant && process_evidence::group_state(&container) == GroupState::Empty {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("observed descendant/container did not settle");
    tokio::time::timeout(Duration::from_secs(2), async {
        stdout.await.unwrap().unwrap();
        stderr.await.unwrap().unwrap();
    })
    .await
    .unwrap();
}

struct NativeWire {
    home: FixtureHome,
    root: tempfile::TempDir,
    marker: PathBuf,
    owner: live_provider_capture::OwnedProcess,
    input: Option<tokio::process::ChildStdin>,
    output: tokio::io::BufReader<tokio::process::ChildStdout>,
    stderr: tokio::task::JoinHandle<std::io::Result<Vec<u8>>>,
    received_expected: Vec<String>,
    provider_identity: Option<surge_core::execution_recovery::process::ProcessIdentity>,
}
impl NativeWire {
    fn start(case: &str, initial: bool) -> Self {
        let home = FixtureHome::new().unwrap();
        let root = tempfile::tempdir().unwrap();
        let marker = root.path().join("received-frames");
        let binary = proxy_binary();
        let mut command = std::process::Command::new(&binary);
        if case == "native-publication-failure" {
            command.arg("--fixture-rejection-publication-failure");
        }
        command
            .arg("--native-multi-turn")
            .arg(capture_path(home.path()));
        if initial {
            command.arg("--initial-provider");
        }
        command
            .arg("gpt-6.1-sol")
            .arg(&binary)
            .arg("--fixture-wire")
            .arg(case)
            .arg(&marker)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let mut owner = live_provider_capture::OwnedProcess::spawn(command, true).unwrap();
        let (input, output, error) = owner.pipes().unwrap();
        let stderr = tokio::spawn(async move {
            use tokio::io::AsyncReadExt;
            let mut bytes = Vec::new();
            error.take(65537).read_to_end(&mut bytes).await?;
            assert!(bytes.len() <= 65536);
            Ok(bytes)
        });
        Self {
            home,
            root,
            marker,
            owner,
            input: Some(input),
            output: tokio::io::BufReader::new(output),
            stderr,
            received_expected: vec![],
            provider_identity: None,
        }
    }
    async fn provider_ready(&mut self) {
        if self.provider_identity.is_some() {
            return;
        }
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if let Ok(pid) = std::fs::read_to_string(self.marker.with_extension("pid"))
                    && let Ok(pid) = pid.parse()
                    && let Ok((identity, _)) = surge_acp::process_evidence::observe(pid)
                {
                    self.provider_identity = Some(identity);
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }
    async fn send(&mut self, frame: &serde_json::Value, forwarded: bool) {
        self.provider_ready().await;
        let mut bytes = serde_json::to_vec(frame).unwrap();
        if forwarded {
            self.received_expected.push(format!(
                "{} {}",
                frame["method"].as_str().unwrap_or("reply"),
                surge_core::ContentHash::compute(&bytes)
            ));
        }
        bytes.push(b'\n');
        live_provider_capture::forward(self.input.as_mut().unwrap(), &bytes)
            .await
            .unwrap();
    }
    async fn message(&mut self) -> serde_json::Value {
        use tokio::io::AsyncBufReadExt;
        let mut reply = String::new();
        tokio::time::timeout(Duration::from_secs(3), self.output.read_line(&mut reply))
            .await
            .unwrap()
            .unwrap();
        serde_json::from_str(&reply).expect("actual forwarded reply missing")
    }
    async fn reply(&mut self, id: u64) -> serde_json::Value {
        let response = self.message().await;
        assert_eq!(response["id"], id);
        response
    }
    async fn wait_received(&self, expected: usize) {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if std::fs::read_to_string(&self.marker)
                    .unwrap_or_default()
                    .lines()
                    .count()
                    == expected
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }
    async fn handshake(&mut self, steps: usize) {
        for frame in wire_handshake().into_iter().take(steps) {
            self.send(&frame, true).await;
            self.reply(frame["id"].as_u64().unwrap()).await;
        }
    }
    async fn finish(self) -> live_provider_capture::Capture {
        self.finish_publication(false).await
    }
    async fn finish_publication(mut self, unavailable: bool) -> live_provider_capture::Capture {
        use tokio::io::AsyncReadExt;
        let mut tail = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), self.output.read_to_end(&mut tail))
            .await
            .unwrap()
            .unwrap();
        assert!(tail.is_empty(), "rejected provider frame reached client");
        drop(self.input);
        assert!(
            !self
                .owner
                .wait(Duration::from_secs(5))
                .await
                .unwrap()
                .success()
        );
        self.owner.terminate().unwrap();
        self.owner.wait(Duration::from_secs(5)).await.unwrap();
        let stderr = tokio::time::timeout(Duration::from_secs(2), self.stderr)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(
            stderr,
            if unavailable {
                b"capture_unavailable\n".to_vec()
            } else {
                vec![]
            }
        );
        if let Some(identity) = &self.provider_identity {
            assert!(
                !surge_acp::process_evidence::observe(identity.pid())
                    .is_ok_and(|(current, _)| &current == identity),
                "observed provider writer survived proxy settlement"
            );
        }
        let actual = std::fs::read_to_string(&self.marker).unwrap_or_default();
        assert_eq!(
            actual.lines().collect::<Vec<_>>(),
            self.received_expected,
            "independent provider received a rejected or changed frame"
        );
        let capture = live_provider_capture::read_bounded::<live_provider_capture::Capture>(
            &capture_path(self.home.path()),
        )
        .unwrap();
        assert_eq!(capture.rejection.is_none(), unavailable);
        self.home.close().unwrap();
        self.root.close().unwrap();
        capture
    }
}
fn wire_handshake() -> [serde_json::Value; 3] {
    [
        serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}),
        serde_json::json!({"jsonrpc":"2.0","id":2,"method":"session/new","params":{}}),
        serde_json::json!({"jsonrpc":"2.0","id":3,"method":"session/set_config_option","params":{"sessionId":"fixture-session","configId":"model","value":"gpt-6.1-sol"}}),
    ]
}
fn wire_prompt(id: u64) -> serde_json::Value {
    serde_json::json!({"jsonrpc":"2.0","id":id,"method":"session/prompt","params":{"sessionId":"fixture-session","prompt":[{"type":"text","text":"PROMPT_SENTINEL"}]}})
}
#[tokio::test]
#[ignore = "requires built acp_smoke_proxy example"]
async fn proxy_native_incomplete_eof_and_failed_rejection_publication_settle_children() {
    let mut wire = NativeWire::start("native-hold", false);
    wire.handshake(3).await;
    wire.send(&wire_prompt(10), true).await;
    wire.wait_received(4).await;
    wire.input.take();
    let capture = wire.finish().await;
    assert_eq!(
        capture.rejection.unwrap().code,
        live_provider_capture::Rejection::IncompletePrompt
    );
    assert!(capture.prompt_pending);
    let mut wire = NativeWire::start("native-publication-failure", false);
    wire.handshake(3).await;
    wire.send(&wire_prompt(1), false).await;
    let capture = wire.finish_publication(true).await;
    assert_eq!(capture.prompts, 0); // Last successfully published snapshot, not a claimed durable rejection.
}
#[tokio::test]
#[ignore = "requires built acp_smoke_proxy example"]
async fn proxy_native_pending_config_collision_and_provider_namespace_do_not_complete_prompts() {
    let mut wire = NativeWire::start("native-hold-config", false);
    wire.handshake(2).await;
    wire.send(&wire_handshake()[2], true).await;
    wire.wait_received(3).await;
    wire.send(&wire_prompt(3), false).await;
    let capture = wire.finish().await;
    assert_eq!(capture.prompts, 0);
    let mut wire = NativeWire::start("native-provider-request", false);
    wire.handshake(3).await;
    wire.send(&wire_prompt(10), true).await;
    let request = wire.reply(10).await;
    assert_eq!(request["method"], "session/request_permission");
    wire.send(
        &serde_json::json!({"jsonrpc":"2.0","id":10,"result":{"outcome":"cancelled"}}),
        true,
    )
    .await;
    wire.wait_received(5).await;
    wire.input.take();
    let capture = wire.finish().await;
    assert!(capture.prompt_pending);
    assert_eq!(capture.successful_responses, 0);
    assert_eq!(
        capture.rejection.unwrap().code,
        live_provider_capture::Rejection::IncompletePrompt
    );
}
#[tokio::test]
#[ignore = "requires built acp_smoke_proxy example"]
async fn proxy_native_full_same_model_updates_during_prompt_remain_transparent() {
    let mut wire = NativeWire::start("native-full-update", false);
    wire.handshake(3).await;
    for id in 10..12 {
        wire.send(&wire_prompt(id), true).await;
        let update = wire.message().await;
        assert_eq!(update["method"], "session/update");
        assert_eq!(
            update["params"]["update"]["configOptions"]
                .as_array()
                .unwrap()
                .len(),
            4
        );
        assert_eq!(
            update["params"]["update"]["configOptions"][0]["currentValue"],
            "gpt-6.1-sol"
        );
        let result = wire.reply(id).await;
        assert_eq!(result["result"]["stopReason"], "end_turn");
    }
    wire.send(&wire_prompt(1), false).await;
    let capture = wire.finish().await;
    assert_eq!(capture.prompts, 2);
    assert_eq!(capture.successful_responses, 2);
}
#[tokio::test]
#[ignore = "requires built acp_smoke_proxy example"]
async fn proxy_native_pending_handshake_and_lifetime_request_budgets_fail_before_forwarding() {
    for (case, steps, held, bad) in [
        (
            "native-hold-init",
            0,
            wire_handshake()[0].clone(),
            wire_handshake()[1].clone(),
        ),
        (
            "native-hold-init",
            0,
            wire_handshake()[0].clone(),
            serde_json::json!({"id":30,"method":"initialize","params":{}}),
        ),
        (
            "native-hold-new",
            1,
            wire_handshake()[1].clone(),
            serde_json::json!({"id":30,"method":"session/new","params":{}}),
        ),
    ] {
        let mut wire = NativeWire::start(case, false);
        wire.handshake(steps).await;
        wire.send(&held, true).await;
        wire.wait_received(steps + 1).await;
        wire.send(&bad, false).await;
        let capture = wire.finish().await;
        assert_eq!(capture.prompts, 0);
    }
    let mut wire = NativeWire::start("native", false);
    wire.handshake(3).await;
    for id in 4..65 {
        wire.send(&serde_json::json!({"id":id,"method":"session/set_config_option","params":{"sessionId":"fixture-session","configId":"thought","value":"high"}}),true).await;
        wire.reply(id).await;
    }
    wire.send(&wire_prompt(65), false).await;
    let capture = wire.finish().await;
    assert_eq!(
        capture.rejection.unwrap().code,
        live_provider_capture::Rejection::RequestLimit
    );
    let mut wire = NativeWire::start("native-hold-config", false);
    wire.handshake(2).await;
    for id in 3..19 {
        wire.send(&serde_json::json!({"id":id,"method":"session/set_config_option","params":{"sessionId":"fixture-session","configId":"thought","value":"high"}}),true).await;
    }
    wire.wait_received(18).await;
    wire.send(&wire_prompt(19), false).await;
    let capture = wire.finish().await;
    assert_eq!(
        capture.rejection.unwrap().code,
        live_provider_capture::Rejection::RequestLimit
    );
}
#[tokio::test]
#[ignore = "requires built acp_smoke_proxy example"]
async fn proxy_native_six_turns_forward_unchanged_and_seventh_is_not_forwarded() {
    for (case, initial, steps) in [
        ("native", false, 3),
        ("native-initial", true, 2),
        ("native-error-response", false, 3),
    ] {
        let mut wire = NativeWire::start(case, initial);
        wire.handshake(steps).await;
        for id in 10..16 {
            wire.send(&wire_prompt(id), true).await;
            let actual = wire.reply(id).await;
            if case == "native-error-response" {
                assert_eq!(actual["error"]["message"], "ERROR_SENTINEL");
            } else {
                assert_eq!(
                    actual,
                    serde_json::json!({"jsonrpc":"2.0","id":id,"result":{"stopReason":"end_turn"}})
                );
            }
        }
        wire.send(&wire_prompt(16), false).await;
        let capture = wire.finish().await;
        assert_eq!(capture.prompts, 6);
        assert!(!capture.prompt_pending);
        assert_eq!(capture.successful_responses + capture.error_responses, 6);
        assert_eq!(
            capture.rejection.unwrap().code,
            live_provider_capture::Rejection::PromptLimitExhausted
        );
    }
}
#[tokio::test]
#[ignore = "requires built acp_smoke_proxy example"]
async fn proxy_native_client_replays_openings_and_unknown_methods_never_reach_provider() {
    for case in [
        "replay-init",
        "replay-new",
        "replay-config",
        "replay-prompt-as-config",
        "repeat-init",
        "repeat-new",
        "load",
        "resume",
        "fork",
        "unknown",
        "wrong-session",
        "overlap",
        "mode-during",
    ] {
        let mut wire = NativeWire::start(
            if matches!(case, "overlap" | "mode-during") {
                "native-hold"
            } else {
                "native"
            },
            false,
        );
        wire.handshake(3).await;
        let mut bad = wire_prompt(30);
        match case {
            "replay-init" => bad["id"] = serde_json::json!(1),
            "replay-new" => bad["id"] = serde_json::json!(2),
            "replay-config" => bad["id"] = serde_json::json!(3),
            "replay-prompt-as-config" => {
                wire.send(&wire_prompt(10), true).await;
                wire.reply(10).await;
                bad = wire_handshake()[2].clone();
                bad["id"] = serde_json::json!(10);
            },
            "repeat-init" => bad = serde_json::json!({"id":30,"method":"initialize","params":{}}),
            "repeat-new" => bad = serde_json::json!({"id":30,"method":"session/new","params":{}}),
            "load" | "resume" | "fork" | "unknown" => {
                bad["method"] = serde_json::json!(format!("session/{case}"))
            },
            "wrong-session" => bad["params"]["sessionId"] = serde_json::json!("foreign-session"),
            "overlap" | "mode-during" => {
                wire.send(&wire_prompt(10), true).await;
                // The fixture confirms actual receipt independently of oracle counts.
                wire.wait_received(4).await;
                if case == "mode-during" {
                    bad = serde_json::json!({"id":30,"method":"session/set_mode","params":{"sessionId":"fixture-session","modeId":"code"}});
                }
            },
            _ => unreachable!(),
        }
        wire.send(&bad, false).await;
        wire.finish().await;
    }
    let mut wire = NativeWire::start("native", false);
    wire.send(&wire_handshake()[1], false).await;
    wire.finish().await;
}
#[tokio::test]
#[ignore = "requires built acp_smoke_proxy example"]
async fn proxy_native_bad_model_ack_updates_duplicate_replies_and_malformed_response_are_not_forwarded()
 {
    for kind in ["removed", "malformed", "ambiguous", "identity"] {
        let mut wire = NativeWire::start(&format!("native-ack-{kind}"), false);
        wire.handshake(2).await;
        wire.send(&wire_handshake()[2], true).await;
        let capture = wire.finish().await;
        assert_eq!(capture.prompts, 0);
        let mut wire = NativeWire::start(&format!("native-update-{kind}"), false);
        wire.handshake(3).await;
        wire.send(&wire_prompt(10), true).await;
        let capture = wire.finish().await;
        assert!(capture.prompt_pending);
        assert_eq!(capture.successful_responses, 0);
    }
    for (method, steps) in [
        ("initialize", 1),
        ("session/new", 2),
        ("session/set_config_option", 3),
    ] {
        let mut wire = NativeWire::start(&format!("native-duplicate-{method}"), false);
        wire.handshake(steps).await;
        wire.finish().await;
    }
    let mut wire = NativeWire::start("native-malformed-response", false);
    wire.handshake(3).await;
    wire.send(&wire_prompt(10), true).await;
    let capture = wire.finish().await;
    assert_eq!(capture.successful_responses, 0);
}

#[tokio::test]
#[ignore = "requires built acp_smoke_proxy example"]
async fn proxy_transport_rejects_wrong_ack_and_missing_model_before_actual_prompt() {
    use tokio::io::{AsyncBufReadExt, BufReader};
    for case in ["wrong-ack", "missing-model"] {
        let home = FixtureHome::new().unwrap();
        let root = tempfile::tempdir().unwrap();
        let marker = root.path().join("provider-prompt");
        let binary = proxy_binary();
        let mut command = std::process::Command::new(&binary);
        command
            .arg(capture_path(home.path()))
            .arg("gpt-6.1-sol")
            .arg(&binary)
            .arg("--fixture-wire")
            .arg(case)
            .arg(&marker)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let mut owner = live_provider_capture::OwnedProcess::spawn(command, true).unwrap();
        let (mut input, output, mut error) = owner.pipes().unwrap();
        let stderr =
            tokio::spawn(async move { tokio::io::copy(&mut error, &mut tokio::io::sink()).await });
        let mut output = BufReader::new(output);
        for frame in [
            serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}),
            serde_json::json!({"jsonrpc":"2.0","id":2,"method":"session/new","params":{}}),
            serde_json::json!({"jsonrpc":"2.0","id":3,"method":"session/set_config_option","params":{"sessionId":"fixture-session","configId":"model","value":"gpt-6.1-sol"}}),
        ] {
            let mut bytes = serde_json::to_vec(&frame).unwrap();
            bytes.push(b'\n');
            live_provider_capture::forward(&mut input, &bytes)
                .await
                .unwrap();
            let mut reply = String::new();
            tokio::time::timeout(Duration::from_secs(3), output.read_line(&mut reply))
                .await
                .unwrap()
                .unwrap();
            let response: serde_json::Value =
                serde_json::from_str(&reply).expect("actual provider handshake response missing");
            assert_eq!(response["id"], frame["id"]);
        }
        live_provider_capture::forward(&mut input, b"{\"jsonrpc\":\"2.0\",\"id\":4,\"method\":\"session/prompt\",\"params\":{\"sessionId\":\"fixture-session\",\"prompt\":[]}}\n").await.unwrap();
        drop(input);
        assert!(!owner.wait(Duration::from_secs(5)).await.unwrap().success());
        owner.terminate().unwrap();
        owner.wait(Duration::from_secs(5)).await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            tokio::io::copy(&mut output, &mut tokio::io::sink())
                .await
                .unwrap();
            stderr.await.unwrap().unwrap();
        })
        .await
        .unwrap();
        assert!(!marker.exists(), "provider received a forbidden prompt");
        let capture: live_provider_capture::Capture =
            serde_json::from_slice(&std::fs::read(capture_path(home.path())).unwrap()).unwrap();
        assert_eq!(capture.prompts, 0);
        home.close().unwrap();
    }
}

#[tokio::test]
#[ignore = "requires built acp_smoke_proxy example"]
async fn proxy_transport_bounds_unterminated_provider_frames_during_read() {
    use tokio::io::AsyncReadExt;
    let home = FixtureHome::new().unwrap();
    let binary = proxy_binary();
    let mut command = std::process::Command::new(&binary);
    command
        .arg(capture_path(home.path()))
        .arg("gpt-6.1-sol")
        .arg(&binary)
        .arg("--fixture-overflow")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut owner = live_provider_capture::OwnedProcess::spawn(command, true).unwrap();
    let (input, mut output, mut error) = owner.pipes().unwrap();
    let stderr =
        tokio::spawn(async move { tokio::io::copy(&mut error, &mut tokio::io::sink()).await });
    let mut forwarded = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), output.read_to_end(&mut forwarded))
        .await
        .unwrap()
        .unwrap();
    assert!(
        forwarded.is_empty(),
        "an oversized partial frame reached the client"
    );
    assert!(!owner.wait(Duration::from_secs(3)).await.unwrap().success());
    drop(input);
    owner.terminate().unwrap();
    owner.wait(Duration::from_secs(3)).await.unwrap();
    stderr.await.unwrap().unwrap();
    home.close().unwrap();
}

#[tokio::test]
#[ignore = "requires built acp_smoke_proxy example"]
async fn outer_failed_child_publishes_typed_diagnostic_before_propagation() {
    let home = FixtureHome::new().unwrap();
    let root = tempfile::tempdir().unwrap();
    let home_path = home.path().to_owned();
    let root_path = root.path().to_owned();
    let mut command = std::process::Command::new(proxy_binary());
    command
        .arg("--fixture-diagnostic-failure")
        .arg(diagnostic_path(home.path()));
    let observed = watch_owned(command).await;
    let mut output = Vec::new();
    let result = finish_parent(observed, home, root, "gpt-6.1-sol", &mut output);
    assert!(result.is_err());
    let text = String::from_utf8(output).unwrap();
    assert!(
        text.contains("provider_failed"),
        "settled child diagnostic was lost"
    );
    assert!(text.contains("run_failed"));
    for sentinel in [
        "AUTH_SENTINEL",
        "PROMPT_SENTINEL",
        "TOOL_SENTINEL",
        "NONCE_SENTINEL",
        "RAW_ERROR_SENTINEL",
    ] {
        assert!(!text.contains(sentinel));
    }
    assert!(!home_path.exists());
    assert!(!root_path.exists());
}

fn remove_retained_fixture_paths(output: &[u8]) {
    let projection: serde_json::Value = serde_json::from_slice(output).unwrap();
    for path in projection["retained_paths"].as_array().unwrap() {
        std::fs::remove_dir_all(path.as_str().unwrap()).unwrap();
    }
}

#[tokio::test]
#[ignore = "requires built acp_smoke_proxy example"]
async fn outer_child_cleanup_uncertainty_preserves_primary_failure_and_retains_fixtures() {
    let home = FixtureHome::new().unwrap();
    let root = tempfile::tempdir().unwrap();
    let home_path = home.path().to_owned();
    let root_path = root.path().to_owned();
    let mut command = std::process::Command::new(proxy_binary());
    command
        .arg("--fixture-diagnostic-unconfirmed")
        .arg(diagnostic_path(home.path()));
    let observed = watch_owned(command).await;
    let mut output = Vec::new();
    assert_eq!(
        finish_parent(observed, home, root, "gpt-6.1-sol", &mut output),
        Err(SmokeFailure::CleanupUnconfirmed)
    );
    let projection: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(projection["diagnostic"]["failure"], "provider_failed");
    assert_eq!(projection["diagnostic"]["settled"], false);
    assert_eq!(projection["capture_status"], "unconfirmed");
    assert!(home_path.exists() && root_path.exists());
    // This controlled child had no provider; its outer domain is already empty.
    remove_retained_fixture_paths(&output);
}

#[tokio::test]
#[ignore = "requires built acp_smoke_proxy example"]
async fn outer_unconfirmed_owner_never_inspects_artifacts_or_removes_live_writer_paths() {
    let home = FixtureHome::new().unwrap();
    let root = tempfile::tempdir().unwrap();
    let home_path = home.path().to_owned();
    let root_path = root.path().to_owned();
    let report = SmokeDiagnostic {
        phase: SmokePhase::Completed,
        failure: None,
        settled: true,
        binding_verified: true,
        counts: Some(SmokeCounts {
            run_failed: 999,
            ..Default::default()
        }),
    };
    let mut file =
        live_provider_capture::EvidenceFile::create(&diagnostic_path(home.path())).unwrap();
    file.write_diagnostic(&report).unwrap();
    drop(file);
    let mut command = std::process::Command::new(proxy_binary());
    command
        .arg("--fixture-writer")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut writer = live_provider_capture::OwnedProcess::spawn(command, true).unwrap();
    let (input, mut output_pipe, mut error_pipe) = writer.pipes().unwrap();
    let stdout =
        tokio::spawn(
            async move { tokio::io::copy(&mut output_pipe, &mut tokio::io::sink()).await },
        );
    let stderr =
        tokio::spawn(async move { tokio::io::copy(&mut error_pipe, &mut tokio::io::sink()).await });
    assert!(writer.try_wait().unwrap().is_none());
    let mut output = Vec::new();
    assert_eq!(
        finish_parent(
            Err(SmokeFailure::CleanupUnconfirmed),
            home,
            root,
            "gpt-6.1-sol",
            &mut output
        ),
        Err(SmokeFailure::CleanupUnconfirmed)
    );
    let projection: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(projection["report_status"], "unconfirmed");
    assert!(projection["diagnostic"].is_null());
    assert_eq!(projection["capture_status"], "unconfirmed");
    assert!(!String::from_utf8(output.clone()).unwrap().contains("999"));
    assert!(home_path.exists() && root_path.exists());
    assert!(writer.try_wait().unwrap().is_none());
    drop(input);
    writer.terminate().unwrap();
    writer.wait(Duration::from_secs(5)).await.unwrap();
    stdout.await.unwrap().unwrap();
    stderr.await.unwrap().unwrap();
    remove_retained_fixture_paths(&output);
}

#[tokio::test]
#[ignore = "requires built acp_smoke_proxy example"]
async fn outer_missing_invalid_and_oversized_diagnostics_are_typed_and_never_echo_bytes() {
    for (bytes, expected) in [
        (None, SmokeFailure::DiagnosticUnavailable),
        (
            Some(b"{\"raw_error\":\"AUTH_SENTINEL\"}".to_vec()),
            SmokeFailure::DiagnosticInvalid,
        ),
        (Some(vec![b'x'; 65537]), SmokeFailure::DiagnosticOversized),
    ] {
        let home = FixtureHome::new().unwrap();
        let root = tempfile::tempdir().unwrap();
        if let Some(bytes) = bytes {
            let mut file =
                live_provider_capture::EvidenceFile::create(&diagnostic_path(home.path())).unwrap();
            file.write_diagnostic_fixture(&bytes).unwrap();
        }
        let mut command = std::process::Command::new(proxy_binary());
        command.arg("--fixture-raw-failure");
        let observed = watch_owned(command).await;
        let mut output = Vec::new();
        assert_eq!(
            finish_parent(observed, home, root, "gpt-6.1-sol", &mut output),
            Err(expected)
        );
        assert!(
            !String::from_utf8(output.clone())
                .unwrap()
                .contains("AUTH_SENTINEL")
        );
        remove_retained_fixture_paths(&output);
    }
}

fn completed_parent_fixture(
    mutate: impl FnOnce(&mut serde_json::Value),
) -> Result<(), SmokeFailure> {
    use live_provider_capture::{EvidenceFile, ProtocolCapture, SelectionMode};
    let home = FixtureHome::new().unwrap();
    let root = tempfile::tempdir().unwrap();
    let mut capture = ProtocolCapture::new("gpt-6.1-sol".into()).unwrap().snapshot;
    capture.adapter_name = Some("fixture-adapter".into());
    capture.adapter_version = Some("1.0.0".into());
    capture.provider_session = Some("fixture-session".into());
    capture.offered_models = vec!["gpt-6.1-sol".into()];
    capture.selected_model = Some("gpt-6.1-sol".into());
    capture.selection_source = Some(SelectionMode::ExplicitSetAck);
    capture.prompts = 1;
    let mut capture = serde_json::to_value(capture).unwrap();
    mutate(&mut capture);
    let mut file = EvidenceFile::create(&capture_path(home.path())).unwrap();
    file.write_diagnostic_fixture(&serde_json::to_vec(&capture).unwrap())
        .unwrap();
    drop(file);
    let report = SmokeDiagnostic {
        phase: SmokePhase::Completed,
        failure: None,
        settled: true,
        binding_verified: true,
        counts: Some(SmokeCounts {
            session_opened: 1,
            human_input_requested: 1,
            human_input_resolved: 1,
            stage_tool_receipt: 2,
            outcome_reported: 1,
            run_completed: 1,
            run_failed: 0,
        }),
    };
    let mut file = EvidenceFile::create(&diagnostic_path(home.path())).unwrap();
    file.write_diagnostic(&report).unwrap();
    drop(file);
    finish_parent(
        Ok(WatchStatus::Succeeded),
        home,
        root,
        "gpt-6.1-sol",
        &mut Vec::new(),
    )
}

#[test]
fn parent_completed_capture_requires_provider_session() {
    assert_eq!(
        completed_parent_fixture(|capture| capture["provider_session"] = serde_json::Value::Null),
        Err(SmokeFailure::VerificationFailed)
    );
}
#[test]
fn parent_completed_capture_requires_adapter_metadata() {
    for field in ["adapter_name", "adapter_version"] {
        assert_eq!(
            completed_parent_fixture(|capture| capture[field] = serde_json::Value::Null),
            Err(SmokeFailure::VerificationFailed)
        );
    }
}
#[test]
fn parent_completed_capture_requires_strict_selection_mode() {
    assert_eq!(
        completed_parent_fixture(|capture| capture["selection_mode"] = "initial_provider".into()),
        Err(SmokeFailure::VerificationFailed)
    );
}
#[test]
fn parent_completed_capture_rejects_missing_selection_mode() {
    assert_eq!(
        completed_parent_fixture(|capture| {
            capture.as_object_mut().unwrap().remove("selection_mode");
        }),
        Err(SmokeFailure::VerificationFailed)
    );
}
#[test]
fn parent_complete_strict_capture_remains_accepted() {
    assert_eq!(completed_parent_fixture(|_| {}), Ok(()));
}

#[test]
fn strict_parent_rejects_native_policy_even_with_one_admission() {
    assert!(
        completed_parent_fixture(|capture| {
            capture["prompt_policy"] = "native_six".into();
            capture["prompt_limit"] = 6.into();
        })
        .is_err()
    );
}

#[test]
fn strict_child_consumer_rejects_native_policy_with_one_prompt() {
    use surge_core::{
        ContentHash, RunEvent, SessionId,
        execution_recovery::{
            OpenedSession, ProviderSessionDescriptor, ProviderSessionId, SessionOpenMode,
            SessionRestoreCapabilities,
        },
        id::StageInvocationId,
    };
    let home = FixtureHome::new().unwrap();
    let path = capture_path(home.path());
    let mut capture = live_provider_capture::ProtocolCapture::new("gpt-6.1-sol".into())
        .unwrap()
        .snapshot;
    capture.adapter_name = Some("adapter".into());
    capture.adapter_version = Some("1".into());
    capture.provider_session = Some("fixture-session".into());
    capture.selected_model = Some("gpt-6.1-sol".into());
    capture.offered_models = vec!["gpt-6.1-sol".into()];
    capture.prompts = 1;
    capture.selection_source = Some(live_provider_capture::SelectionMode::ExplicitSetAck);
    let descriptor = ProviderSessionDescriptor::new(
        ProviderSessionId::new("fixture-session".into()).unwrap(),
        StageInvocationId::new(),
        "codex-acp".into(),
        ContentHash::compute(b"launch"),
        home.path().to_owned(),
        SessionRestoreCapabilities::default(),
    )
    .unwrap();
    let session = SessionId::new();
    let opened = OpenedSession::new(session, descriptor, SessionOpenMode::New).unwrap();
    let events = [RunEvent {
        run_id: RunId::new(),
        seq: 1,
        timestamp: chrono::Utc::now(),
        payload: EventPayload::SessionOpened {
            handoff: None,
            opened: Some(opened),
            node: "smoke".parse().unwrap(),
            session,
            agent: "codex-acp".into(),
            agent_id: Some("codex-acp".into()),
        },
    }];
    let mut file = live_provider_capture::EvidenceFile::create(&path).unwrap();
    file.write(&capture).unwrap();
    assert!(check_provider_capture(&path, &events, "gpt-6.1-sol").is_ok());
    capture.prompt_policy = live_provider_capture::PromptPolicy::NativeSix;
    capture.prompt_limit = 6;
    file.write(&capture).unwrap();
    assert!(check_provider_capture(&path, &events, "gpt-6.1-sol").is_err());
    drop(file);
    home.close().unwrap();
}

#[test]
fn strict_parent_rejects_published_protocol_rejection() {
    assert!(
        completed_parent_fixture(|capture| {
            capture["rejection"] =
                serde_json::json!({"code":"malformed_frame","direction":"provider","turn":1});
        })
        .is_err()
    );
}
