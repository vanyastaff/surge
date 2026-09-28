//! Opt-in real daemon/ACP/MCP journey. Never runs a provider in ordinary CI.
//! Build surge + mock_acp_agent first, then run the ignored controlled test.
//! SURGE_LIVE_CODEX=1 enables exactly one bounded live invocation.
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
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

const CODEX: &str = "/Users/vanyastafford/.npm/_npx/e3854e347c184741/node_modules/@zed-industries/codex-acp-darwin-arm64/bin/codex-acp";
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
            .block_on(journey(PathBuf::from(root), live));
        assert!(
            result.is_ok(),
            "smoke failed: {}",
            result.err().unwrap_or_default()
        );
        return;
    }
    let root = tempfile::Builder::new()
        .prefix("surge-live-")
        .tempdir_in("/tmp")
        .unwrap();
    let test = if live {
        "live_codex_daemon_mcp_smoke"
    } else {
        "controlled_daemon_mcp_smoke"
    };
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command
        .args(["--ignored", "--exact", test, "--nocapture"])
        .env("SURGE_SMOKE_CHILD", root.path())
        .env("SURGE_HOME", root.path().join("home"));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command.spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(150);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            if !status.success() {
                #[cfg(unix)]
                {
                    let group = nix::unistd::Pid::from_raw(i32::try_from(child.id()).unwrap());
                    let _ = nix::sys::signal::killpg(group, nix::sys::signal::Signal::SIGKILL);
                }
            }
            assert!(
                status.success(),
                "isolated smoke failed (provider output is not printed)"
            );
            break;
        }
        if Instant::now() >= deadline {
            #[cfg(unix)]
            {
                let group = nix::unistd::Pid::from_raw(i32::try_from(child.id()).unwrap());
                let _ = nix::sys::signal::killpg(group, nix::sys::signal::Signal::SIGKILL);
            }
            let _ = child.kill();
            let _ = child.wait();
            panic!("smoke exceeded 150s watchdog; isolated process group reaped");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
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

fn prepare(root: &Path, live: bool) -> (PathBuf, surge_core::SurgeConfig, Arc<ProfileRegistry>) {
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
    let binary = if live {
        PathBuf::from(CODEX)
    } else {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/mock_acp_agent")
    };
    assert!(binary.is_file(), "required local ACP binary is absent");
    let mut config = surge_core::SurgeConfig::default();
    let agent: surge_core::config::AgentConfig = serde_json::from_value(serde_json::json!({
        "command":binary, "args":if live { vec![] } else { vec!["--stage-mcp", "--stage-mcp-case=smoke"] }
    })).unwrap();
    config.agents.insert("live-smoke".into(), agent);
    std::fs::write(repo.join("surge.toml"), toml::to_string(&config).unwrap()).unwrap();
    let profiles = root.join("home/profiles");
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
    seed_catalog_baseline(&worktree, &registry);
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

fn seed_catalog_baseline(worktree: &Path, registry: &ProfileRegistry) {
    // Engine always seeds this catalog; commit its exact deterministic contents
    // before the provider starts so the checkout oracle allows no new files.
    std::fs::create_dir(worktree.join(".surge")).unwrap();
    std::fs::write(
        worktree.join(".surge/profile_catalog.md"),
        surge_orchestrator::profile_loader::render_profile_catalog(registry),
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

fn graph() -> Graph {
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

async fn journey(root: PathBuf, live: bool) -> Result<(), String> {
    let nonce = RunId::new().to_string();
    let (worktree, config, profiles) = prepare(&root, live);
    let pins = [
        CheckoutPin::capture(root.join("repository")),
        CheckoutPin::capture(worktree.clone()),
    ];
    let storage = Storage::open(root.join("home"))
        .await
        .map_err(|_| "storage open")?;
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
    let result = exercise(socket, &storage, &worktree, &nonce, live).await;
    for active in engine.snapshot_active_runs().await {
        let _ = engine.stop_run(active.run_id, "smoke cleanup".into()).await;
    }
    shutdown.cancel();
    let server_result = tokio::time::timeout(Duration::from_secs(10), server).await;
    drop(engine);
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
    assert_eq!(admission.snapshot().await.active, 0);
    Ok(())
}

async fn exercise(
    socket: PathBuf,
    storage: &Arc<Storage>,
    worktree: &Path,
    nonce: &str,
    live: bool,
) -> Result<(), String> {
    let client = connect(socket).await?;
    let id = RunId::new();
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
        .start_run(id, graph(), worktree.to_path_buf(), config)
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
