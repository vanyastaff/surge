//! Opt-in real daemon/ACP/MCP journey. Never runs a provider in ordinary CI.
//! Build surge + mock_acp_agent first, then run the ignored controlled test.
//! SURGE_LIVE_CODEX=1 enables exactly one bounded live invocation.
#[path = "support/live_provider_capture.rs"]
mod live_provider_capture;
#[path = "support/runtime_home.rs"]
mod runtime_home_fixture;
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
        let result = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(journey(
                PathBuf::from(root),
                PathBuf::from(std::env::var_os("SURGE_HOME").unwrap()),
                live,
            ));
        assert!(
            result.is_ok(),
            "smoke failed: {}",
            result.err().unwrap_or_default()
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
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(watch_owned(command))
        .unwrap();
    let capture: live_provider_capture::Capture =
        serde_json::from_slice(&std::fs::read(capture_path(home.path())).unwrap()).unwrap();
    eprintln!(
        "smoke model metadata: {}",
        serde_json::to_string(&capture).unwrap()
    );
    home.close().unwrap();
}

async fn watch_owned(mut command: std::process::Command) -> Result<(), String> {
    command
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = live_provider_capture::OwnedProcess::spawn(command, true)
        .map_err(|_| "smoke child spawn")?;
    if child.id() == 0 {
        return Err("smoke child identity unavailable".into());
    }
    let (input, mut output, mut error) = child.pipes().map_err(|_| "smoke child pipes")?;
    drop(input);
    let stdout =
        tokio::spawn(async move { tokio::io::copy(&mut output, &mut tokio::io::sink()).await });
    let stderr =
        tokio::spawn(async move { tokio::io::copy(&mut error, &mut tokio::io::sink()).await });
    let result = child.wait(Duration::from_secs(150)).await;
    child
        .terminate()
        .map_err(|_| "smoke child termination unconfirmed")?;
    child
        .wait(Duration::from_secs(10))
        .await
        .map_err(|_| "smoke child settlement unconfirmed")?;
    tokio::time::timeout(Duration::from_secs(10), async {
        stdout
            .await
            .map_err(|_| "smoke stdout task")?
            .map_err(|_| "smoke stdout drain")?;
        stderr
            .await
            .map_err(|_| "smoke stderr task")?
            .map_err(|_| "smoke stderr drain")?;
        Ok::<(), String>(())
    })
    .await
    .map_err(|_| "smoke descendant drain deadline")??;
    if !result
        .map_err(|_| "smoke exceeded 150s watchdog")?
        .success()
    {
        return Err("isolated smoke failed; provider output withheld".into());
    }
    Ok(())
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
            command: Path::new(env!("CARGO_MANIFEST_DIR"))
                .join(format!(
                    "../../target/debug/mock_acp_agent{}",
                    std::env::consts::EXE_SUFFIX
                ))
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
    let bytes = std::fs::read(path).map_err(|_| "provider capture missing")?;
    if bytes.len() > 65536 {
        return Err("provider capture limit".into());
    }
    let capture: live_provider_capture::Capture =
        serde_json::from_slice(&bytes).map_err(|_| "provider capture invalid")?;
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
    if sessions.next().is_some() || capture.provider_session.as_deref() != Some(provider)
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

async fn journey(root: PathBuf, home: PathBuf, live: bool) -> Result<(), String> {
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
        live,
        &launch.model,
        run_id,
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
    // Evaluate checkout evidence before propagating any run or cleanup failure.
    finish_checked(result.and(bridge_result).and(server_result), &pins)?;
    let events = storage
        .open_run_reader(run_id)
        .await
        .map_err(|_| "capture reader")?
        .read_run_events()
        .await
        .map_err(|_| "capture events")?;
    check_provider_capture(&capture_path(&home), &events, &launch.model)?;

    assert_eq!(admission.snapshot().await.active, 0);
    Ok(())
}

async fn exercise(
    socket: PathBuf,
    storage: &Arc<Storage>,
    worktree: &Path,
    nonce: &str,
    live: bool,
    model: &str,
    id: RunId,
) -> Result<(), String> {
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
    let complete = match completion {
        Ok(Ok(RunOutcome::Completed { .. })) => Ok(()),
        Ok(Ok(RunOutcome::Failed { error })) => Err(if live {
            classify_failure(&error)
        } else {
            error
        }),
        _ => Err("provider completion unconfirmed".into()),
    };
    let events = storage
        .open_run_reader(id)
        .await
        .map_err(|_| "reader")?
        .read_run_events()
        .await
        .map_err(|_| "events")?;
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

fn classify_failure(error: &str) -> String {
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
        "provider authentication unavailable; run codex login outside the fixture".into()
    } else if error.contains("timeout") || error.contains("timed out") {
        "provider stage timed out".into()
    } else {
        "provider stage failed (raw provider output withheld)".into()
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
