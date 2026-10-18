mod mock_acp_binary {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../scripts/test-support/mock_acp_binary.rs"
    ));
}
use super::{runtime_home_fixture::FixtureHome, wire_bridge};
use futures::FutureExt as _;
use interprocess::local_socket::tokio::prelude::*;
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use surge_core::{EventPayload, RunEvent, RunId};
use surge_orchestrator::engine::{
    Engine, EngineConfig, facade::LocalEngineFacade, ipc::local_socket_name_from_path,
    tools::worktree::WorktreeToolDispatcher,
};
use surge_persistence::runs::Storage;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio_util::sync::CancellationToken;

const DESCRIPTION: &str =
    include_str!("../../../surge-orchestrator/tests/fixtures/artifacts/valid/description.md");
const ROADMAP: &str =
    include_str!("../../../surge-orchestrator/tests/fixtures/artifacts/valid/roadmap.toml");
const ROADMAP_MD: &str =
    "## Milestones\n1. Complete app.\n## Dependencies\nNone.\n## Risks\nNone.\n";

fn generated_flow(name: &str) -> String {
    concat!(
        include_str!("../../../../examples/flow_terminal_only.toml"),
        "\n[metadata.archetype]\nname = \"single-task\"\n"
    )
    .replace("flow_terminal_only", name)
}

fn flags(home: &Path) -> Vec<String> {
    vec![
        "--stage-mcp".into(),
        "--session-store".into(),
        home.join("provider-sessions.json").display().to_string(),
        "--wire-log".into(),
        home.join("provider-wire.jsonl").display().to_string(),
        "--stage-script".into(),
        home.join("bootstrap-script.json").display().to_string(),
    ]
}

async fn rpc(socket: &Path, frame: Value) -> Value {
    let _: surge_orchestrator::engine::ipc::DaemonRequest =
        serde_json::from_value(frame.clone()).unwrap();
    let stream = LocalSocketStream::connect(local_socket_name_from_path(socket).unwrap())
        .await
        .unwrap();
    let (read, mut write) = stream.split();
    write
        .write_all(format!("{frame}\n").as_bytes())
        .await
        .unwrap();
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

/// Each invocation owns an entirely fresh Storage, Engine, ACP bridge and supervisor.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn child_bootstrap_host_probe() {
    let Some(home) = std::env::var_os("SURGE_TEST_BOOTSTRAP_GATE_HOME") else {
        return;
    };
    let _ = tracing_subscriber::fmt().with_ansi(false).with_env_filter("warn,surge_daemon::bootstrap_supervisor=debug,surge_daemon::admission=debug,surge_daemon::server=info").try_init();
    // Run worktree ownership refuses symlink ancestors, including macOS /var.
    let home = PathBuf::from(home).canonicalize().unwrap();
    let project = PathBuf::from(std::env::var_os("SURGE_TEST_BOOTSTRAP_GATE_PROJECT").unwrap());
    let storage = Storage::open(&home).await.unwrap();
    let config = Arc::new(surge_core::SurgeConfig::load(&project.join("surge.toml")).unwrap());
    let bridge = Arc::new(wire_bridge::WireBridge {
        bridge: surge_acp::bridge::AcpBridge::with_defaults().unwrap(),
        flags: flags(&home),
        session_commit_check: None,
    });
    let wire_config = bridge.engine_config(&home);
    let profiles = Arc::new(surge_orchestrator::profile_loader::ProfileRegistry::new(
        surge_orchestrator::profile_loader::DiskProfileSet::scan(&home.join("profiles")).unwrap(),
    ));
    let agents = Arc::new(surge_acp::Registry::for_run(&config));
    let engine = Arc::new(Engine::new_full(
        bridge.clone(),
        storage.clone(),
        Arc::new(WorktreeToolDispatcher::new(project.clone())),
        Arc::new(surge_notify::MultiplexingNotifier::new()),
        None,
        Some(profiles.clone()),
        EngineConfig {
            agent_registry: Some(agents.clone()),
            memory_store_path: Some(home.join("memory.db")),
            ..wire_config
        },
    ));
    let cancel = CancellationToken::new();
    let admission = Arc::new(surge_daemon::admission::AdmissionController::new(4, 4));
    let broadcast = Arc::new(surge_daemon::broadcast::BroadcastRegistry::new());
    let runtime = surge_daemon::bootstrap_runtime::BootstrapRuntime::new(
        config,
        profiles,
        agents,
        home.join("worktrees"),
        home.join("profiles"),
    )
    .unwrap();
    let supervisor = surge_daemon::bootstrap_supervisor::BootstrapSupervisor::new(
        surge_daemon::bootstrap_supervisor::BootstrapServices {
            engine: engine.clone(),
            facade: Arc::new(LocalEngineFacade::new(engine.clone())),
            storage: storage.clone(),
            runtime: Some(runtime),
            admission: admission.clone(),
            broadcast: broadcast.clone(),
            shutdown: cancel.clone(),
        },
    );
    supervisor.reconcile().await.unwrap();
    let owner = tokio::spawn(supervisor.clone().run());
    let server = tokio::spawn(surge_daemon::run_with_supervisor(
        surge_daemon::ServerConfig {
            socket_path: home.join("bootstrap.sock"),
            max_active: 4,
            max_queue: 4,
        },
        Arc::new(LocalEngineFacade::new(engine.clone())),
        surge_daemon::tracked_run::TrackingContext::new(engine.clone(), storage.clone()),
        broadcast,
        admission,
        cancel.clone(),
        supervisor.clone(),
    ));
    while !home.join("host-stop").exists() {
        assert!(
            !server.is_finished(),
            "bootstrap server exited before parent settlement"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    // Stop scheduling before asking actual run owners to cancel. Their tracked
    // handles are joined by the supervisor below; stop_run alone is best effort.
    cancel.cancel();
    let mut stop_errors = Vec::new();
    for active in engine.snapshot_active_runs().await {
        if let Err(error) = engine
            .stop_run(active.run_id, "fixture owner unwind".into())
            .await
        {
            stop_errors.push(error.to_string());
        }
    }
    server.await.unwrap().unwrap();
    owner.await.unwrap();
    assert!(
        engine.snapshot_active_runs().await.is_empty(),
        "joined supervisor left an actual run owner"
    );
    drop(supervisor);
    drop(engine);
    let bridge = Arc::try_unwrap(bridge)
        .ok()
        .expect("all actual engine owners joined");
    bridge.bridge.shutdown().await.unwrap();
    drop(storage);
    assert!(
        stop_errors.is_empty(),
        "actual run cancellation errors: {stop_errors:?}"
    );
    std::fs::write(
        home.join("host-settled"),
        b"actual engine owners, ACP and storage released",
    )
    .unwrap();
}

fn read_events(home: &Path, run: RunId) -> Vec<RunEvent> {
    let path = home
        .join("runs")
        .join(run.to_string())
        .join("events.sqlite");
    if !path.exists() {
        return Vec::new();
    }
    let conn =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let mut query = match conn.prepare("SELECT seq,timestamp,payload FROM events ORDER BY seq") {
        Ok(query) => query,
        Err(error) if error.to_string().contains("no such table: events") => return Vec::new(),
        Err(error) => panic!("bootstrap journal query: {error}"),
    };
    query
        .query_map([], |row| {
            let seq: u64 = row.get(0)?;
            let time: i64 = row.get(1)?;
            let bytes: Vec<u8> = row.get(2)?;
            Ok(RunEvent {
                run_id: run,
                seq,
                timestamp: chrono::DateTime::from_timestamp_millis(time).unwrap(),
                payload: serde_json::from_slice::<surge_core::VersionedEventPayload>(&bytes)
                    .unwrap()
                    .payload,
            })
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}

struct Host {
    child: std::process::Child,
    home: PathBuf,
    run: Option<RunId>,
    log: PathBuf,
}
impl Host {
    fn spawn(home: &Path, project: &Path, cut: bool, run: Option<RunId>) -> Self {
        Self::spawn_configured(home, project, cut, run, false)
    }
    fn spawn_configured(
        home: &Path,
        project: &Path,
        cut: bool,
        run: Option<RunId>,
        continuation: bool,
    ) -> Self {
        let log = home.join(if cut {
            "first-host.log"
        } else {
            "second-host.log"
        });
        let output = std::fs::File::create(&log).unwrap();
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "bootstrap_gate_cold::child_bootstrap_host_probe",
                "--nocapture",
            ])
            .env("SURGE_TEST_BOOTSTRAP_GATE_HOME", home)
            .env("SURGE_TEST_BOOTSTRAP_GATE_PROJECT", project)
            .env_remove("SURGE_ROUTE_COMMIT_EXIT")
            .stdout(output.try_clone().unwrap())
            .stderr(output);
        if continuation {
            command
                .env(
                    "SURGE_TEST_CONTINUATION_MCP_PROXY",
                    home.join("artifact-proxy.py"),
                )
                .env(
                    "SURGE_TEST_CONTINUATION_PYTHON",
                    implementation_continuation::python(),
                )
                .env(
                    "SURGE_TEST_CONTINUATION_SCRIPT",
                    home.join("bootstrap-script.json"),
                );
        }
        if cut {
            command.env("SURGE_ROUTE_COMMIT_EXIT", "flow_generator@2");
        }
        Self {
            child: command.spawn().unwrap(),
            home: home.into(),
            run,
            log,
        }
    }
    fn diagnostic(&self) -> String {
        let log = std::fs::read_to_string(&self.log).unwrap_or_default();
        let events = self
            .run
            .map(|run| read_events(&self.home, run))
            .unwrap_or_default();
        let state = rusqlite::Connection::open_with_flags(
            self.home.join("db/registry.sqlite"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .ok()
        .and_then(|connection| {
            connection
                .query_row(
                    "SELECT state_json FROM bootstrap_operations LIMIT 1",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .ok()
        });
        format!(
            "home={}; state={state:?}; log={log}; last events={:?}",
            self.home.display(),
            events.iter().rev().take(8).collect::<Vec<_>>()
        )
    }
    fn settle_groups(&self) -> Vec<surge_acp::process_evidence::GroupState> {
        self.run
            .map(|run| read_events(&self.home, run))
            .unwrap_or_default()
            .iter()
            .filter_map(|event| {
                if let EventPayload::SessionOpened {
                    opened: Some(opened),
                    ..
                } = &event.payload
                {
                    opened
                        .execution_writer
                        .as_ref()
                        .and_then(|writer| writer.container())
                        .map(|container| {
                            surge_acp::process_evidence::stop_group(
                                container,
                                Duration::from_secs(1),
                            )
                        })
                } else {
                    None
                }
            })
            .collect()
    }
}
impl Drop for Host {
    fn drop(&mut self) {
        let mut errors = Vec::new();
        let observed = self.child.try_wait();
        let graceful_requested = matches!(&observed, Ok(None));
        let mut exited = match observed {
            Ok(status) => status.is_some(),
            Err(error) => {
                errors.push(format!("initial host observation: {error}"));
                false
            },
        };
        if graceful_requested {
            if let Err(error) = std::fs::write(self.home.join("host-stop"), b"settle actual owners")
            {
                errors.push(format!("host stop marker: {error}"));
            }
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            while std::time::Instant::now() < deadline {
                match self.child.try_wait() {
                    Ok(Some(_)) => {
                        exited = true;
                        break;
                    },
                    Ok(None) => std::thread::sleep(Duration::from_millis(10)),
                    Err(error) => {
                        errors.push(format!("host exit observation: {error}"));
                        break;
                    },
                }
            }
        }
        let forced = !exited;
        if forced && let Err(error) = self.child.kill() {
            errors.push(format!("forced host kill: {error}"));
        }
        let joined = self.child.wait().map_err(|error| error.to_string());
        let groups =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.settle_groups()));
        let (group_evidence, groups_empty) = match groups {
            Ok(states) => {
                let empty = states
                    .iter()
                    .all(|state| *state == surge_acp::process_evidence::GroupState::Empty);
                (
                    format!("authenticated group observations: {states:?}"),
                    Some(empty),
                )
            },
            Err(panic) => {
                let message = panic
                    .downcast_ref::<String>()
                    .map(String::as_str)
                    .or_else(|| panic.downcast_ref::<&str>().copied())
                    .unwrap_or("non-string cleanup panic");
                (
                    format!("authenticated group inspection/settlement failed: {message}"),
                    None,
                )
            },
        };
        let evidence = json!({"graceful_requested":graceful_requested, "forced":forced,
            "joined_success": joined.as_ref().is_ok_and(|status| status.success()),
            "join_observation":format!("{joined:?}"), "groups_empty":groups_empty,
            "group_observations":group_evidence, "cleanup_errors":errors})
        .to_string();
        if let Err(error) = std::fs::write(self.log.with_extension("cleanup.log"), &evidence) {
            eprintln!("cannot retain cleanup evidence: {error}; {evidence}");
        }
    }
}

async fn ready(host: &mut Host) {
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            assert!(
                host.child.try_wait().unwrap().is_none(),
                "{}",
                host.diagnostic()
            );
            if LocalSocketStream::connect(
                local_socket_name_from_path(&host.home.join("bootstrap.sock")).unwrap(),
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
    .unwrap_or_else(|error| panic!("host readiness {error}: {}", host.diagnostic()));
}

async fn gate(host: &mut Host, node_name: &str, index: usize) -> (u64, u64, String) {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            assert!(host.child.try_wait().unwrap().is_none(), "{}", host.diagnostic());
            let events = read_events(&host.home, host.run.unwrap());
            assert!(!events.iter().any(|event| matches!(event.payload, EventPayload::RunRecoveryRequired { .. } | EventPayload::RunFailed { .. })), "real bootstrap refused gate: {}", host.diagnostic());
            if let Some(request) = events.iter().filter(|event| matches!(&event.payload, EventPayload::HumanInputRequested { node, session: None, .. } if node.as_str() == node_name)).nth(index) {
                let EventPayload::HumanInputRequested { call_id: Some(call_id), .. } = &request.payload else { panic!("missing gate identity") };
                let entry = events.iter().rev().find(|event| event.seq < request.seq && matches!(&event.payload, EventPayload::StageEntered { node, .. } if node.as_str() == node_name)).unwrap();
                return (entry.seq, request.seq, call_id.clone());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.unwrap_or_else(|error| panic!("gate {node_name}/{index}: {error}: {}", host.diagnostic()))
}

async fn answer(host: &Host, node: &str, call_id: &str, outcome: &str) -> Value {
    rpc(&host.home.join("bootstrap.sock"), json!({"method":"resolve_gate_input","request_id":19,"run_id":host.run.unwrap(),"node":node,"gate_request_id":surge_core::id::GateRequestId::from_event_call_id(call_id).unwrap(),"response":{"outcome":outcome,"comment":"revise latest flow"}})).await
}

fn git(project: &Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(project)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn prepare(home: &Path, project: &Path) {
    std::fs::create_dir_all(project).unwrap();
    let binary = mock_acp_binary::path();
    let config = json!({"schema_version":1,"default_agent":"wire-fixture","agents":{"wire-fixture":{"command":binary,"args":flags(home)}}});
    std::fs::write(
        project.join("surge.toml"),
        toml::to_string(&config).unwrap(),
    )
    .unwrap();
    git(project, &["init"]);
    git(project, &["config", "user.name", "Gate fixture"]);
    git(
        project,
        &["config", "user.email", "fixture@example.invalid"],
    );
    git(project, &["add", "."]);
    git(project, &["commit", "-m", "isolated bootstrap fixture"]);
    std::fs::create_dir_all(home.join("profiles")).unwrap();
    for mut profile in surge_core::profile::bundled::BundledRegistry::all() {
        if ["description-author", "roadmap-planner", "flow-generator"]
            .contains(&profile.role.id.as_str())
        {
            profile.runtime.agent_id = "wire-fixture".into();
            profile.hooks = Default::default();
            std::fs::write(
                home.join("profiles")
                    .join(format!("{}.toml", profile.role.id)),
                toml::to_string(&profile).unwrap(),
            )
            .unwrap();
        }
    }
    let script = json!([
        {"prompt_contains":"", "outcome":"drafted", "bootstrap_artifacts":{"description.md":DESCRIPTION}},
        {"prompt_contains":"", "outcome":"drafted", "bootstrap_artifacts":{"roadmap.md":ROADMAP_MD,"roadmap.toml":ROADMAP}},
        {"prompt_contains":"", "outcome":"drafted", "bootstrap_artifacts":{"flow.toml":generated_flow("first_candidate")}},
        {"prompt_contains":"revise latest flow", "outcome":"drafted", "bootstrap_artifacts":{"flow.toml":generated_flow("latest_revised_candidate")}}
    ]);
    std::fs::write(
        home.join("bootstrap-script.json"),
        serde_json::to_vec(&script).unwrap(),
    )
    .unwrap();
}

fn assert_author_prefix(events: &[RunEvent]) {
    let accepted: Vec<_> = events
        .iter()
        .filter_map(|event| match &event.payload {
            EventPayload::StageOutcomeCommitted { commit }
                if commit.context().node.as_str() == "flow_generator" =>
            {
                Some(commit.invocation())
            },
            _ => None,
        })
        .collect();
    assert_eq!(accepted.len(), 2, "exactly two genuine author acceptances");
    for invocation in accepted {
        assert_eq!(events.iter().filter(|event| matches!(&event.payload, EventPayload::StageRouteCommitted { invocation: routed, .. } if *routed == invocation)).count(), 1);
    }
    assert_eq!(events.iter().filter(|event| matches!(&event.payload, EventPayload::StageCompleted { node, outcome } if node.as_str()=="flow_generator" && outcome.as_str()=="drafted")).count(), 2);
}

/// Assertion unwind must ask the genuine paused engine owner to settle.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn assertion_unwind_settles_actual_bootstrap_host_and_provider() {
    let home = FixtureHome::new().unwrap();
    let result = std::panic::AssertUnwindSafe(unwind_in_home(&home))
        .catch_unwind()
        .await;
    finish_home(home, result);
}

async fn unwind_in_home(home: &FixtureHome) {
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("project");
    prepare(home.path(), &project);
    let project = project.canonicalize().unwrap();
    let mut run = None;
    let unwound = std::panic::AssertUnwindSafe(async {
        let mut host = Host::spawn(home.path(), &project, false, None);
        ready(&mut host).await;
        let intent = surge_core::bootstrap_operation::BootstrapIntent::new(project, "Exercise fixture ownership".into(), Default::default()).unwrap();
        let accepted = rpc(&home.path().join("bootstrap.sock"), json!({"method":"start_bootstrap","request_id":1,"operation_id":RunId::new(),"intent":intent})).await;
        assert_eq!(accepted["method"], "bootstrap_operation", "{accepted}");
        let planning = serde_json::from_value(accepted["status"]["planning_run"].clone()).unwrap();
        host.run = Some(planning);
        run = Some(planning);
        gate(&mut host, "description_gate", 0).await;
        panic!("intentional fixture assertion unwind");
    }).catch_unwind().await;
    let panic = unwound.expect_err("the fixture must deliberately unwind its active host");
    assert_eq!(
        panic.downcast_ref::<&str>().copied(),
        Some("intentional fixture assertion unwind"),
        "setup failures cannot satisfy the unwind exercise"
    );
    let events = read_events(home.path(), run.expect("actual accepted bootstrap run"));
    let evidence = std::fs::read_to_string(home.path().join("second-host.cleanup.log")).unwrap();
    assert!(
        events
            .iter()
            .any(|event| matches!(&event.payload, EventPayload::RunAborted { .. })),
        "unwind must let the actual engine commit its cancellation before host exit; {evidence}"
    );
    assert!(
        home.path().join("host-settled").exists(),
        "actual owners did not report completed shutdown"
    );
    let cleanup: Value = serde_json::from_str(&evidence).unwrap();
    assert_eq!(cleanup["graceful_requested"], true, "{cleanup}");
    assert_eq!(cleanup["forced"], false, "{cleanup}");
    assert_eq!(cleanup["joined_success"], true, "{cleanup}");
    assert_eq!(cleanup["groups_empty"], true, "{cleanup}");
    assert_eq!(cleanup["cleanup_errors"], json!([]), "{cleanup}");
    assert_eq!(
        cleanup["group_observations"], "authenticated group observations: [Empty]",
        "{cleanup}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn second_flow_author_route_recovers_fresh_gate_and_latest_child_once() {
    let home = FixtureHome::new().unwrap();
    let result = std::panic::AssertUnwindSafe(run_in_home(&home, false))
        .catch_unwind()
        .await;
    finish_home(home, result);
}

/// Additional current-schema projection-rebuild coverage, not a literal old
/// schema fixture or a new baseline RED claim.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cold_owned_replay_preserves_real_accepted_commits_and_raw_prefix() {
    let home = FixtureHome::new().unwrap();
    let result = std::panic::AssertUnwindSafe(run_in_home(&home, true))
        .catch_unwind()
        .await;
    finish_home(home, result);
}

fn finish_home(home: FixtureHome, result: Result<(), Box<dyn std::any::Any + Send>>) {
    match result {
        Ok(()) => home.close().unwrap(),
        Err(panic) => {
            // Host RAII attempted subprocess joining and authenticated group
            // settlement. Its observations are retained beside each host log.
            let retained = home.retain();
            eprintln!(
                "bootstrap cold fixture evidence retained: {}",
                retained.display()
            );
            std::panic::resume_unwind(panic);
        },
    }
}

async fn run_in_home(home: &FixtureHome, distrust_projection: bool) {
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("project");
    prepare(home.path(), &project);
    let project = project.canonicalize().unwrap();
    let mut first = Host::spawn(home.path(), &project, true, None);
    ready(&mut first).await;
    let operation = RunId::new();
    let (run, implementation) =
        submit_bootstrap(&mut first, home.path(), &project, operation).await;
    let first_gate = approve_planning_and_edit_flow(&mut first).await;
    let prefix = wait_for_second_author_cut(&mut first, home.path(), run).await;
    assert_cut_prefix(&prefix, &first_gate);
    assert_cut_provider_settlement(&first);
    drop(first);

    let preserved = if distrust_projection {
        Some(distrust_cut_projection(home.path(), run, &prefix))
    } else {
        None
    };

    let mut second = Host::spawn(home.path(), &project, false, Some(run));
    ready(&mut second).await;
    let fresh = gate(&mut second, "flow_gate", 1).await;
    // Listener publication precedes asynchronous bootstrap reconciliation.
    // The genuine new request establishes that the owned run writer replayed.
    if let Some((cut, captured)) = &preserved {
        assert_preserved(home.path(), run, *cut, captured);
    }
    approve_fresh_flow_gate(&second, &first_gate, &fresh).await;
    await_handoff_and_host_shutdown(&mut second, home.path(), operation).await;
    let events = read_events(home.path(), run);
    assert_author_prefix(&events);
    assert_provider_turns(home.path());
    assert_latest_child(home.path(), implementation);
    assert_parent_authority(&prefix, &events);
    assert!(
        !events
            .iter()
            .any(|event| matches!(event.payload, EventPayload::RunRecoveryRequired { .. }))
    );
    drop(second);
    if let Some((cut, captured)) = &preserved {
        assert_preserved(home.path(), run, *cut, captured);
    }
}

async fn submit_bootstrap(
    first: &mut Host,
    home: &Path,
    project: &Path,
    operation: RunId,
) -> (RunId, RunId) {
    let intent = surge_core::bootstrap_operation::BootstrapIntent::new(
        project.to_path_buf(),
        "Build an isolated fixture app".into(),
        Default::default(),
    )
    .unwrap();
    let accepted = rpc(
        &home.join("bootstrap.sock"),
        json!({"method":"start_bootstrap","request_id":1,"operation_id":operation,"intent":intent}),
    )
    .await;
    assert_eq!(accepted["method"], "bootstrap_operation", "{accepted}");
    let run: RunId = serde_json::from_value(accepted["status"]["planning_run"].clone()).unwrap();
    let implementation: RunId =
        serde_json::from_value(accepted["status"]["implementation_run"].clone()).unwrap();
    first.run = Some(run);
    (run, implementation)
}

async fn approve_planning_and_edit_flow(first: &mut Host) -> (u64, u64, String) {
    for node in ["description_gate", "roadmap_gate"] {
        let (_, _, call) = gate(first, node, 0).await;
        assert_eq!(
            answer(first, node, &call, "approve").await["method"],
            "resolve_human_input_ok"
        );
    }
    let first_gate = gate(first, "flow_gate", 0).await;
    assert_eq!(
        answer(first, "flow_gate", &first_gate.2, "edit").await["method"],
        "resolve_human_input_ok"
    );
    first_gate
}

async fn wait_for_second_author_cut(first: &mut Host, home: &Path, run: RunId) -> Vec<RunEvent> {
    let exit = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if let Some(exit) = first.child.try_wait().unwrap() {
                break exit;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|error| panic!("second author route cut: {error}: {}", first.diagnostic()));
    assert_eq!(exit.code(), Some(99), "{}", first.diagnostic());
    read_events(home, run)
}

fn assert_cut_prefix(prefix: &[RunEvent], first_gate: &(u64, u64, String)) {
    assert_author_prefix(prefix);
    assert_eq!(prefix.iter().filter(|event| matches!(&event.payload, EventPayload::StageEntered { node, .. } if node.as_str()=="flow_gate")).count(), 1, "cut precedes fresh gate execution");
    let first_request = surge_core::id::GateRequestId::from_event_call_id(&first_gate.2).unwrap();
    assert_eq!(prefix.iter().filter(|event| matches!(&event.payload, EventPayload::GateStageRouteCommitted { request, .. } if *request == first_request)).count(), 1);
    assert_eq!(
        prefix
            .iter()
            .filter(|event| matches!(&event.payload, EventPayload::BootstrapEditRequested { .. }))
            .count(),
        1
    );
    assert_eq!(
        prefix
            .iter()
            .filter(|event| matches!(&event.payload, EventPayload::HumanInputResolved { .. }))
            .count(),
        3
    );
    assert_eq!(prefix.iter().filter(|event| matches!(&event.payload, EventPayload::HumanInputResolved { node, call_id: Some(call), response } if node.as_str()=="flow_gate" && call==&first_gate.2 && response["outcome"]=="edit")).count(), 1);
    let candidates: Vec<_> = prefix
        .iter()
        .filter_map(|event| match &event.payload {
            EventPayload::PipelineMaterialized { graph, .. }
                if !graph.nodes.keys().any(|key| key.as_str() == "flow_gate") =>
            {
                Some(graph.metadata.name.as_str())
            },
            _ => None,
        })
        .collect();
    assert_eq!(candidates, ["first_candidate", "latest_revised_candidate"]);
    assert!(!prefix.iter().any(|event| matches!(
        event.payload,
        EventPayload::RunRecoveryRequired { .. }
            | EventPayload::RunCompleted { .. }
            | EventPayload::RunFailed { .. }
    )));
}

fn assert_cut_provider_settlement(first: &Host) {
    let states = first.settle_groups();
    assert_eq!(
        states.len(),
        4,
        "actual provider sessions including both authors"
    );
    assert!(
        states
            .iter()
            .all(|state| *state == surge_acp::process_evidence::GroupState::Empty)
    );
}

fn distrust_cut_projection(home: &Path, run: RunId, prefix: &[RunEvent]) -> (u64, RawPrefix) {
    let cut = prefix.last().unwrap().seq;
    let connection = rusqlite::Connection::open(
        home.join("runs")
            .join(run.to_string())
            .join("events.sqlite"),
    )
    .unwrap();
    let captured = preserved_prefix(&connection, cut);
    assert_eq!(captured[2].len(), 4, "four actual accepted author stages");
    assert_eq!(
        captured[3].len(),
        3,
        "description, roadmap and consumed Flow edit"
    );
    assert!(!captured[1].is_empty(), "real committed route snapshots");
    assert_eq!(projection_version(&connection), 1);
    assert_eq!(
        connection
            .execute(
                "UPDATE projection_semantics SET version=0 WHERE singleton=1",
                []
            )
            .unwrap(),
        1
    );
    assert_eq!(projection_version(&connection), 0);
    (cut, captured)
}

async fn approve_fresh_flow_gate(
    second: &Host,
    first_gate: &(u64, u64, String),
    fresh: &(u64, u64, String),
) {
    assert!(fresh.0 > first_gate.0 && fresh.1 > first_gate.1);
    assert_ne!(fresh.2, first_gate.2);
    assert_eq!(
        answer(second, "flow_gate", &first_gate.2, "approve").await["method"],
        "error"
    );
    assert_eq!(
        answer(second, "roadmap_gate", &fresh.2, "approve").await["method"],
        "error"
    );
    assert_eq!(
        answer(second, "flow_gate", &fresh.2, "approve").await["method"],
        "resolve_human_input_ok"
    );
}

async fn await_handoff_and_host_shutdown(second: &mut Host, home: &Path, operation: RunId) {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let status = rpc(
                &home.join("bootstrap.sock"),
                json!({"method":"bootstrap_status","request_id":3,"operation_id":operation}),
            )
            .await;
            if !status["status"]["result"].is_null() {
                break;
            }
            assert_ne!(
                status["status"]["state"]["state"],
                "needs_attention",
                "{status}: {}",
                second.diagnostic()
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|error| panic!("child handoff: {error}: {}", second.diagnostic()));
    std::fs::write(home.join("host-stop"), b"settle").unwrap();
    let exit = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            if let Some(exit) = second.child.try_wait().unwrap() {
                break exit;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(exit.success(), "{}", second.diagnostic());
}

fn assert_provider_turns(home: &Path) {
    assert_eq!(
        std::fs::read_to_string(home.join("bootstrap-script.cursor")).unwrap(),
        "4"
    );
    let wire: Vec<Value> = std::fs::read_to_string(home.join("provider-wire.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(
        wire.iter()
            .filter(|row| row["operation"] == "prompt")
            .count(),
        4,
        "description, roadmap and exactly two Flow author calls"
    );
}

fn assert_latest_child(home: &Path, implementation: RunId) {
    let child_events = read_events(home, implementation);
    let child_graphs: Vec<_> = child_events
        .iter()
        .filter_map(|event| match &event.payload {
            EventPayload::PipelineMaterialized { graph, .. } => Some(graph),
            _ => None,
        })
        .collect();
    assert_eq!(child_graphs.len(), 1);
    assert_eq!(child_graphs[0].metadata.name, "latest_revised_candidate");
    let expected_child: surge_core::graph::Graph =
        toml::from_str(&generated_flow("latest_revised_candidate")).unwrap();
    assert_eq!(
        child_graphs[0].as_ref(),
        &expected_child,
        "child owns the exact latest generated graph"
    );
    assert_eq!(
        child_events
            .iter()
            .filter(|event| matches!(event.payload, EventPayload::RunStarted { .. }))
            .count(),
        1
    );
    assert!(
        child_events
            .iter()
            .any(|event| matches!(event.payload, EventPayload::RunCompleted { .. }))
    );
}

fn assert_parent_authority(prefix: &[RunEvent], events: &[RunEvent]) {
    let terminal = events
        .iter()
        .position(|event| matches!(event.payload, EventPayload::RunCompleted { .. }))
        .unwrap();
    for observed in [prefix, &events[..terminal]] {
        let surge_core::RunState::Pipeline { memory: folded, .. } =
            surge_core::run_state::fold(observed).unwrap()
        else {
            panic!("actual parent prefix")
        };
        let mut cold = surge_core::RunMemory::default();
        for event in observed {
            cold.apply_event(event).unwrap();
        }
        let expected = surge_core::BundledFlows::by_name_latest("bootstrap")
            .unwrap()
            .graph;
        let membership = |memory: &surge_core::RunMemory| {
            memory
                .verification_graph
                .as_ref()
                .unwrap()
                .nodes
                .keys()
                .map(|key| key.as_str().to_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(membership(&cold), membership(&folded));
        assert_eq!(
            membership(&cold),
            surge_core::escalation::with_default_escalation_gates(&expected)
                .nodes
                .keys()
                .map(|key| key.as_str().to_owned())
                .collect::<Vec<_>>()
        );
        assert!(
            cold.gate_decisions.is_empty(),
            "consumed decisions remain history, not pending authority"
        );
    }
}

type RawPrefix = Vec<Vec<Vec<rusqlite::types::Value>>>;

fn projection_version(connection: &rusqlite::Connection) -> i64 {
    connection
        .query_row(
            "SELECT version FROM projection_semantics WHERE singleton=1",
            [],
            |row| row.get(0),
        )
        .unwrap()
}

fn preserved_prefix(connection: &rusqlite::Connection, cut: u64) -> RawPrefix {
    [
        "SELECT seq,timestamp,kind,payload,schema_version FROM events WHERE seq<=? ORDER BY seq",
        "SELECT at_seq,snapshot,bytes_compressed FROM graph_snapshots WHERE at_seq<=? ORDER BY at_seq",
        "SELECT invocation,committed_seq,node_id,outcome,routed_seq FROM stage_outcome_commits WHERE committed_seq<=? ORDER BY committed_seq",
        "SELECT request_id,stage_entry_seq,committed_seq,node_id,outcome,disposition,routed_seq FROM gate_stage_commits WHERE committed_seq<=? ORDER BY committed_seq",
    ].iter().map(|sql| {
        let mut statement = connection.prepare(sql).unwrap();
        let columns = statement.column_count();
        statement.query_map([cut], |row| (0..columns).map(|column| row.get(column)).collect::<Result<Vec<rusqlite::types::Value>, _>>()).unwrap().collect::<Result<Vec<_>, _>>().unwrap()
    }).collect()
}

fn assert_preserved(home: &Path, run: RunId, cut: u64, captured: &RawPrefix) {
    let connection = rusqlite::Connection::open_with_flags(
        home.join("runs")
            .join(run.to_string())
            .join("events.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    assert_eq!(
        projection_version(&connection),
        1,
        "fresh owned replay certifies the projection epoch"
    );
    assert_eq!(
        &preserved_prefix(&connection, cut),
        captured,
        "raw events, snapshots and every original accepted commit column including routed_seq stay unchanged"
    );
}

#[path = "bootstrap_implementation_continuation.rs"]
mod implementation_continuation;
