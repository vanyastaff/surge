//! Actual owned-Flow IPC and cold-host MCP oracles. Author evidence, not acceptance.
//!
//! The populated cold twin deliberately requires production restoration. It must
//! not manufacture writer closure or upgrade GroupOnly coverage to make it pass.
#[path = "support/cold_host.rs"]
mod cold_host;
#[path = "support/runtime_home.rs"]
mod runtime_home_fixture;
use runtime_home_fixture::FixtureHome;

use interprocess::local_socket::tokio::prelude::*;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    ffi::OsString,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use surge_core::{
    EventPayload, Graph, RunId,
    id::WorkItemOperationId,
    mcp_config::{McpServerRef, McpTransportConfig},
    work_item::OwnedFlowReceipt,
};
use surge_orchestrator::{
    engine::{
        Engine, EngineConfig, EngineRunConfig,
        facade::LocalEngineFacade,
        owned_flow::{
            FlowInput, McpSelection, OwnedFlowRunConfig, OwnedFlowStart, WorkspaceRequest,
        },
    },
    profile_loader::{DiskProfileSet, ProfileRegistry},
};
use surge_persistence::runs::{Storage, inspection::RunDatabaseInspection};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio_util::sync::CancellationToken;
use tracing_subscriber::prelude::*;

struct CatalogBridge {
    inner: Arc<dyn surge_acp::bridge::BridgeFacade>,
    capture: PathBuf,
}

#[async_trait::async_trait]
impl surge_acp::bridge::BridgeFacade for CatalogBridge {
    async fn open_session(
        &self,
        config: surge_acp::bridge::SessionConfig,
    ) -> Result<surge_core::execution_recovery::OpenedSession, surge_acp::bridge::OpenSessionError>
    {
        use std::io::Write;
        let tools: Vec<&str> = config.tools.iter().map(|tool| tool.name.as_str()).collect();
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.capture)
            .unwrap();
        writeln!(file, "{}", json!({"tools":tools})).unwrap();
        self.inner.open_session(config).await
    }
    async fn send_message(
        &self,
        session: surge_core::SessionId,
        content: surge_acp::bridge::MessageContent,
    ) -> Result<(), surge_acp::bridge::SendMessageError> {
        self.inner.send_message(session, content).await
    }
    async fn session_state(
        &self,
        session: surge_core::SessionId,
    ) -> Result<surge_acp::bridge::SessionState, surge_acp::bridge::BridgeError> {
        self.inner.session_state(session).await
    }
    async fn close_session(
        &self,
        session: surge_core::SessionId,
    ) -> Result<(), surge_acp::bridge::CloseSessionError> {
        self.inner.close_session(session).await
    }
    async fn reply_to_tool(
        &self,
        session: surge_core::SessionId,
        call_id: String,
        payload: surge_acp::bridge::ToolResultPayload,
    ) -> Result<(), surge_acp::bridge::ReplyToToolError> {
        self.inner.reply_to_tool(session, call_id, payload).await
    }
    async fn reply_to_permission(
        &self,
        session: surge_core::SessionId,
        request_id: String,
        response: surge_acp::bridge::RequestPermissionResponse,
    ) -> Result<(), surge_acp::bridge::ReplyToPermissionError> {
        self.inner
            .reply_to_permission(session, request_id, response)
            .await
    }
    fn subscribe(&self) -> tokio::sync::broadcast::Receiver<surge_acp::bridge::BridgeEvent> {
        self.inner.subscribe()
    }
}

const CHILD: &str = r#"
import json, os, sys
def record(value):
    with open(os.environ['RECORDER'], 'a', encoding='utf-8') as f:
        f.write(json.dumps(value, ensure_ascii=True) + '\n')
record({'kind':'spawn','exe':os.path.realpath(sys.executable),'args':sys.argv,
        'value':os.environ['DECLARED_PRIVATE'],'cwd':os.getcwd(),'pid':os.getpid()})
for line in sys.stdin:
    request=json.loads(line)
    record({'kind':'request','method':request.get('method'),'params':request.get('params')})
    if 'id' not in request: continue
    method=request['method']
    if method=='initialize':
        result={'protocolVersion':'2024-11-05','capabilities':{'tools':{}},'serverInfo':{'name':'controlled-mcp','version':'1'}}
    elif method=='tools/list':
        result={'tools':[{'name':name,'inputSchema':{'type':'object'}} for name in ['echo','forbidden']]}
    elif method=='ping': result={}
    else: result={'content':[{'type':'text','text':'controlled functional result'}]}
    print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':result}), flush=True)
record({'kind':'eof','pid':os.getpid()})
"#;

fn python() -> PathBuf {
    if cfg!(windows) {
        PathBuf::from("python")
    } else {
        PathBuf::from("/usr/bin/python3")
    }
}

fn server(root: &Path, name: &str, recorder: &str) -> McpServerRef {
    McpServerRef::new(
        name.into(),
        McpTransportConfig::stdio(
            python(),
            vec![
                root.join("mcp.py").to_string_lossy().into_owned(),
                "ordered-first".into(),
                "ordered-second-私有Ω".into(),
                "quote\"slash\\line\nnext".into(),
            ],
            HashMap::from([
                (
                    "RECORDER".into(),
                    root.join(recorder).to_string_lossy().into_owned(),
                ),
                (
                    "DECLARED_PRIVATE".into(),
                    "owned-flow-private-environment-Ω".into(),
                ),
            ]),
        ),
        Some(vec!["echo".into()]),
        // Per-RPC deadline only: the handshake (with interpreter startup) has
        // its own startup deadline (unset here, so max(30s, call_timeout)).
        // 1s keeps the positive (0.1s) catalog page well inside on hosted runners.
        Duration::from_secs(1),
        false,
    )
    .with_sandbox(Some(surge_core::sandbox::SandboxMode::WorkspaceWrite))
}

fn graph() -> Graph {
    let mut graph: Graph =
        toml::from_str(include_str!("../../../examples/flow_minimal_agent.toml")).unwrap();
    let node = graph.nodes.get_mut(&graph.start).unwrap();
    let mut config = serde_json::to_value(&node.config).unwrap();
    config["profile"] = json!("mcp-primary@1.0");
    // Nonempty even for Empty: otherwise the engine would never consult fallback.
    config["tool_overrides"] = json!({"mcp_add":["oracle","denied"]});
    node.config = serde_json::from_value(config).unwrap();
    graph
}

fn engine_config(home: &Path, stall_prompt: bool) -> EngineConfig {
    let profiles = home.join("profiles");
    std::fs::create_dir_all(&profiles).unwrap();
    let binary = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
        "../../target/debug/mock_acp_agent{}",
        std::env::consts::EXE_SUFFIX
    ));
    assert!(binary.is_file(), "build mock_acp_agent prerequisite");
    let mut agents = HashMap::new();
    for (role, runtime) in [("mcp-primary", "quota-a"), ("mcp-fallback", "quota-b")] {
        let profile = profiles.join(format!("{role}-1.0.toml"));
        if !profile.exists() {
            std::fs::write(
                &profile,
                format!(
                    r#"schema_version=1
[role]
id="{role}"
version="1.0.0"
display_name="MCP recovery oracle"
category="agents"
description="Controlled actual ACP"
when_to_use="Tests"
[runtime]
agent_id="{runtime}"
recommended_model="sonnet"
[[outcomes]]
id="done"
description="Success"
edge_kind_hint="forward"
[prompt]
system="Report done using the supplied stage outcome tool."
"#
                ),
            )
            .unwrap();
        }
        let auth = home.join(format!("{runtime}-auth"));
        if !auth.exists() {
            std::fs::write(&auth, format!("declared-provider-source-{runtime}")).unwrap();
        }
        let mut flags: Vec<String> = vec![
            "--scenario".into(),
            "prompt_error=429_retry_after".into(),
            "--prompt-error-once".into(),
            home.join(format!("{runtime}-failed-once"))
                .display()
                .to_string(),
            "--session-store".into(),
            home.join(format!("{runtime}-sessions.json"))
                .display()
                .to_string(),
            "--wire-log".into(),
            home.join(format!("{runtime}-wire.jsonl"))
                .display()
                .to_string(),
            "--pid-file".into(),
            home.join(format!("{runtime}-provider.pid"))
                .display()
                .to_string(),
            "--stage-mcp".into(),
            "--config-options".into(),
        ];
        if stall_prompt {
            flags.push("--stall-prompt".into());
        }
        let config: surge_core::config::AgentConfig = serde_json::from_value(json!({
            "command":binary,"args":flags,"env":{"SURGE_MCP_RECOVERY_AUTH":format!("declared-provider-env-{runtime}")},
            "capacity_route":{"provider_family":"mcp-recovery-fixture","configured_route":runtime,
                "auth_sources":[{"kind":"env","target_key":"SURGE_MCP_RECOVERY_AUTH"},{"kind":"file","path":auth}],
                "completeness":"complete_configured_sources"}
        })).unwrap();
        agents.insert(runtime.to_owned(), config);
    }
    EngineConfig {
        profile_registry: Some(Arc::new(ProfileRegistry::new(
            DiskProfileSet::scan(&profiles).unwrap(),
        ))),
        agent_registry: Some(Arc::new(surge_acp::Registry::from_config(agents))),
        capacity: surge_core::capacity::CapacityPolicy {
            blind_backoff: Some(Duration::from_secs(1)),
            jitter_max: Duration::ZERO,
            rotation: surge_core::capacity::RotationPolicy::Candidate {
                profile: "mcp-fallback@1.0".into(),
            },
        },
        ..EngineConfig::default()
    }
}

async fn request(socket: &Path, body: Value) -> Value {
    let stream = LocalSocketStream::connect(
        surge_orchestrator::engine::ipc::local_socket_name_from_path(socket).unwrap(),
    )
    .await
    .unwrap();
    let (read, mut write) = stream.split();
    let mut frame = serde_json::to_vec(&body).unwrap();
    frame.push(b'\n');
    write.write_all(&frame).await.unwrap();
    let mut line = String::new();
    tokio::time::timeout(
        Duration::from_secs(15),
        BufReader::new(read).read_line(&mut line),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(!line.is_empty(), "real daemon closed without a reply");
    serde_json::from_str(&line).unwrap()
}

fn initial_mcp_evidence_ready(
    phase: &str,
    events: &[surge_persistence::runs::ReadEvent],
    storage: &Storage,
    run: RunId,
    has_cycle: bool,
) -> bool {
    if phase == "populated-unconfirmed" {
        events.iter().any(|event| {
            matches!(
                event.payload.payload,
                EventPayload::RunRecoveryRequired { .. }
            )
        }) && storage.work_items().for_run(run).unwrap().unwrap().state
            == surge_core::work_item::WorkItemAttemptState::Attention
    } else {
        has_cycle
            && events.iter().any(|event| {
                matches!(event.payload.payload,
            EventPayload::RunSuspended { ref fence } if fence.cleanup_confirmed)
            })
    }
}

fn records(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .split_inclusive('\n')
        .filter(|line| line.ends_with('\n'))
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn records_only_reads_committed_newline_records() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("record.jsonl");
    std::fs::write(&path, b"{\"kind\":\"spawn\"}\n{\"kind\":\"requ").unwrap();
    assert_eq!(records(&path), vec![json!({"kind":"spawn"})]);
    std::fs::write(&path, b"{\"kind\":\"spawn\"}\n{\"kind\":\"request\"}\n").unwrap();
    assert_eq!(
        records(&path),
        vec![json!({"kind":"spawn"}), json!({"kind":"request"})]
    );
    std::fs::write(&path, b"{\"kind\":\"corrupt}\n").unwrap();
    assert!(std::panic::catch_unwind(|| records(&path)).is_err());
}

fn assert_private_absent(public: &str) {
    for sentinel in [
        "ordered-second-私有Ω",
        "quote\"slash\\line\nnext",
        "owned-flow-private-environment-Ω",
    ] {
        assert!(
            !public.contains(sentinel),
            "private transport escaped public observation"
        );
        let escaped = serde_json::to_string(sentinel).unwrap();
        assert!(
            !public.contains(&escaped[1..escaped.len() - 1]),
            "JSON-escaped transport escaped public observation"
        );
        let ascii: String = escaped[1..escaped.len() - 1]
            .chars()
            .map(|character| {
                if character.is_ascii() {
                    character.to_string()
                } else {
                    format!("\\u{:04x}", u32::from(character))
                }
            })
            .collect();
        assert!(
            !public.contains(&ascii),
            "ASCII JSON transport escaped public observation"
        );
    }
}

fn assert_host_diagnostics_opaque(root: &Path) {
    let mut captures = 0;
    for entry in std::fs::read_dir(root).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name();
        if name.to_string_lossy().starts_with("cold-host-stdout-")
            || name.to_string_lossy().starts_with("cold-host-stderr-")
        {
            let bytes = std::fs::read(entry.path()).unwrap();
            assert_private_absent(&String::from_utf8_lossy(&bytes));
            captures += 1;
        }
    }
    assert!(captures >= 2, "actual host stdout/stderr captures missing");
}

fn publish(path: &Path, value: &Value) {
    let temporary = path.with_extension("publishing");
    std::fs::write(&temporary, serde_json::to_vec(value).unwrap()).unwrap();
    std::fs::rename(temporary, path).unwrap();
}

async fn wait_file(path: &Path) {
    tokio::time::timeout(Duration::from_secs(30), async {
        while !path.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

async fn wait_mcp_catalog(path: &Path) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !records(path)
            .iter()
            .any(|row| row["kind"] == "request" && row["method"] == "tools/list")
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

#[test]
fn child_owned_flow_mcp_probe() {
    let Some(root) = std::env::var_os("SURGE_OWNED_MCP_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    let home = PathBuf::from(std::env::var_os("SURGE_OWNED_MCP_HOME").unwrap());
    let phase = std::env::var("SURGE_OWNED_MCP_PHASE").unwrap();
    struct TerminalChildren;
    impl Drop for TerminalChildren {
        fn drop(&mut self) {
            surge_mcp::shutdown_children_and_join();
        }
    }
    // Declared before the runtime: unwind drops async owners before joining children.
    let _terminal_children = TerminalChildren;
    tracing_subscriber::registry()
        .with(tracing_subscriber::filter::filter_fn(|metadata| {
            surge_mcp::diagnostics::permits_target(metadata.target())
        }))
        .with(tracing_subscriber::EnvFilter::new("trace"))
        .with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(std::io::stderr),
        )
        .init();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(host_probe(&root, &home, &phase));
    drop(runtime);
    if phase == "refusal-reclaim-missing" {
        publish(
            &root.join("runtime-consumed.ready"),
            &json!({"consumed":true}),
        );
        surge_persistence::work_items::join_owned_flow_refusal_owners();
        publish(&root.join("reclaimed.ready"), &json!({"delivered":true}));
    }
}

/// The MCP child leaves a descendant in its own process group (ADR-0021 refusal case).
const LINGERING_DESCENDANT: &str = r#"
if os.fork() == 0:
    devnull = os.open(os.devnull, os.O_RDWR)
    for fd in (0, 1, 2):
        os.dup2(devnull, fd)
    with open(os.environ['RECORDER'] + '.descendant', 'a') as f:
        f.write(str(os.getpid()) + '\n')
    import time
    time.sleep(600)
    os._exit(0)
for line in sys.stdin:"#;

/// Kills lingering descendants recorded by [`LINGERING_DESCENDANT`], even on panic.
#[cfg(unix)]
struct DescendantReaper(PathBuf);
#[cfg(unix)]
impl Drop for DescendantReaper {
    fn drop(&mut self) {
        for pid in std::fs::read_to_string(&self.0).unwrap_or_default().lines() {
            if let Ok(pid) = pid.trim().parse::<i32>() {
                let _ = nix::sys::signal::kill(
                    nix::unistd::Pid::from_raw(pid),
                    nix::sys::signal::Signal::SIGKILL,
                );
            }
        }
    }
}

async fn host_probe(root: &Path, home: &Path, phase: &str) {
    let project = root.join("project");
    if phase == "populated-unconfirmed" {
        let script = CHILD.replacen("for line in sys.stdin:", LINGERING_DESCENDANT, 1);
        std::fs::write(root.join("mcp.py"), script).unwrap();
    }
    if phase.starts_with("legacy-deadline") {
        let delay = if phase == "legacy-deadline-positive" {
            "0.1"
        } else {
            "1.5"
        };
        let script = CHILD.replace(
            "    elif method=='tools/list':",
            &format!("    elif method=='tools/list':\n        import time; time.sleep({delay})"),
        );
        std::fs::write(root.join("mcp.py"), script).unwrap();
    }
    let storage = Storage::open(home).await.unwrap();
    if matches!(phase, "refusal-reclaim" | "refusal-reclaim-missing") {
        assert_eq!(storage.resume_pending_owned_flow_refusals().unwrap(), 1);
        surge_persistence::work_items::close_owned_flow_refusal_admission();
        if phase == "refusal-reclaim-missing" {
            publish(
                &root.join("reclaim-pending.ready"),
                &json!({"pending":true}),
            );
            return;
        }
        surge_persistence::work_items::join_owned_flow_refusal_owners();
        assert_eq!(storage.resume_pending_owned_flow_refusals().unwrap(), 0);
        publish(&root.join("reclaimed.ready"), &json!({"delivered":true}));
        return;
    }
    let cancel = CancellationToken::new();
    let mut global = server(
        root,
        "oracle",
        if phase.ends_with("-resume") {
            "legacy-resumed.jsonl"
        } else if phase.starts_with("legacy") {
            "legacy.jsonl"
        } else {
            "replacement.jsonl"
        },
    );
    if phase.contains("tools-none") {
        global.allowed_tools = None;
    } else if phase.contains("tools-empty") {
        global.allowed_tools = Some(Vec::new());
    }
    let global_denied = server(
        root,
        "denied",
        if phase.starts_with("legacy") {
            "legacy-denied.jsonl"
        } else {
            "replacement-denied.jsonl"
        },
    )
    .with_sandbox(Some(surge_core::sandbox::SandboxMode::ReadOnly));
    let mut config = engine_config(home, phase.ends_with("-start"));
    if phase.starts_with("legacy") {
        config.capacity.rotation = surge_core::capacity::RotationPolicy::Disabled;
    }
    let engine = Arc::new(Engine::new_full(
        Arc::new(CatalogBridge {
            inner: Arc::new(surge_acp::bridge::AcpBridge::with_defaults().unwrap()),
            capture: root.join(format!("{phase}-stage-catalog.jsonl")),
        }),
        storage.clone(),
        Arc::new(
            surge_orchestrator::engine::tools::worktree::WorktreeToolDispatcher::new(
                project.clone(),
            ),
        ),
        Arc::new(surge_notify::MultiplexingNotifier::new()),
        Some(Arc::new(surge_mcp::McpRegistry::from_config(
            &[global, global_denied],
            Some(&project),
        ))),
        None,
        config,
    ));
    let socket = home.join("owned-mcp.sock");
    let admission = Arc::new(surge_daemon::admission::AdmissionController::new(2, 4));
    let broadcast = Arc::new(surge_daemon::broadcast::BroadcastRegistry::new());
    let tracking = surge_daemon::tracked_run::TrackingContext::new(engine.clone(), storage.clone());
    let facade = Arc::new(LocalEngineFacade::new(engine.clone()));
    let task = tokio::spawn(surge_daemon::run_runs_only(
        surge_daemon::ServerConfig {
            socket_path: socket.clone(),
            max_active: 2,
            max_queue: 4,
        },
        facade.clone(),
        tracking.clone(),
        broadcast.clone(),
        admission.clone(),
        cancel.clone(),
    ));
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let name =
                surge_orchestrator::engine::ipc::local_socket_name_from_path(&socket).unwrap();
            if LocalSocketStream::connect(name).await.is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    if phase.ends_with("-resume") {
        let run: RunId =
            serde_json::from_slice(&std::fs::read(root.join("legacy-run.json")).unwrap()).unwrap();
        let response = request(
            &socket,
            json!({"method":"resume_run","request_id":2,
            "run_id":run,"worktree_path":project}),
        )
        .await;
        assert_eq!(response["method"], "resume_run_ok");
        wait_mcp_catalog(&root.join("legacy-resumed.jsonl")).await;
        wait_file(&root.join(format!("{phase}-stage-catalog.jsonl"))).await;
        assert!(!root.join("legacy-denied.jsonl").exists());
        publish(&root.join("legacy-resumed.ready"), &json!({"run":run}));
        wait_file(&root.join("legacy-resumed.release")).await;
    } else if phase.starts_with("legacy") {
        let run = RunId::new();
        let run_config = EngineRunConfig {
            initial_prompt: "Legacy MCP exposure positive control".into(),
            ..EngineRunConfig::default()
        };
        let message = json!({"method":"start_run","request_id":1,"run_id":run,
            "graph":legacy_policy_graph(phase),"worktree_path":project,"run_config":run_config});
        let _: surge_orchestrator::engine::ipc::DaemonRequest =
            serde_json::from_value(message.clone()).unwrap();
        let started = request(&socket, message).await;
        assert_ne!(
            started["method"], "error",
            "legacy positive control: {started}"
        );
        wait_file(&root.join("legacy.jsonl")).await;
        wait_mcp_catalog(&root.join("legacy.jsonl")).await;
        wait_file(&root.join(format!("{phase}-stage-catalog.jsonl"))).await;
        if phase.ends_with("-start") {
            wait_provider_prompt(home).await;
            publish(
                &root.join("legacy-run.json"),
                &serde_json::to_value(run).unwrap(),
            );
        }
        publish(&root.join("legacy.ready"), &json!({"legacy_spawned":true}));
        wait_file(&root.join("legacy.release")).await;
    } else if matches!(phase, "wake" | "refuse" | "refuse-state" | "refuse-held") {
        let proof: Value =
            serde_json::from_slice(&std::fs::read(root.join("park.ready")).unwrap()).unwrap();
        let receipt: OwnedFlowReceipt = serde_json::from_value(proof["receipt"].clone()).unwrap();
        let current =
            serde_json::to_value(storage.work_items().for_run(receipt.run).unwrap().unwrap())
                .unwrap();
        for field in ["run", "item", "binding", "graph", "config"] {
            assert!(
                current[field] == proof["attempt"][field],
                "immutable accepted attempt changed"
            );
        }
        if phase == "wake" {
            assert!(
                serde_json::to_value(storage.work_items().execution_control(receipt.run).unwrap())
                    .unwrap()
                    == proof["control"],
                "original cold control changed"
            );
        }
        let held_writer = if phase == "refuse-held" {
            Some(storage.open_run_writer(receipt.run).await.unwrap())
        } else {
            None
        };
        let scheduler = surge_daemon::wake_scheduler::WakeScheduler {
            storage: storage.clone(),
            facade,
            admission,
            broadcast,
            tracking,
            clock: Arc::new(surge_persistence::runs::MockClock::new(
                proof["due"].as_i64().unwrap(),
            )),
            notifier: Arc::new(surge_notify::MultiplexingNotifier::new()),
            blind_park_limit: 100,
            poll_interval: if matches!(phase, "refuse-state" | "refuse-held") {
                Duration::from_secs(3600)
            } else {
                Duration::from_millis(10)
            },
        };
        let waking = tokio::spawn(scheduler.run(cancel.clone()));
        if phase == "wake" {
            wait_completed(&storage, receipt.run).await;
        } else if phase == "refuse-state" {
            tokio::time::sleep(Duration::from_millis(250)).await;
        } else {
            wait_attention(&storage, receipt.run).await;
        }
        cancel.cancel();
        waking.await.unwrap();
        let first = storage.work_items().for_run(receipt.run).unwrap().unwrap();
        let first_control = storage.work_items().execution_control(receipt.run).unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        let later = storage.work_items().for_run(receipt.run).unwrap().unwrap();
        let later_control = storage.work_items().execution_control(receipt.run).unwrap();
        publish(
            &root.join("wake.ready"),
            &json!({"run":receipt.run,"phase":phase,"first":first,"first_control":first_control,"later":later,"later_control":later_control}),
        );
        if let Some(writer) = held_writer {
            wait_file(&root.join("held.release")).await;
            writer.close().await.unwrap();
            wait_actual_refusal_ack(&home.join("db/registry.sqlite")).await;
            surge_persistence::work_items::close_owned_flow_refusal_admission();
            surge_persistence::work_items::join_owned_flow_refusal_owners();
            publish(
                &root.join("held-delivered.ready"),
                &json!({"delivered":true}),
            );
        }
    } else {
        let mcp = match phase {
            "explicit-empty" => McpSelection::Explicit(Vec::new()),
            "default-empty" => McpSelection::HostDefault,
            "populated" | "populated-unconfirmed" => McpSelection::Explicit(vec![
                server(root, "oracle", "original.jsonl"),
                server(root, "denied", "denied.jsonl")
                    .with_sandbox(Some(surge_core::sandbox::SandboxMode::ReadOnly)),
            ]),
            _ => panic!("unknown controlled phase"),
        };
        let input = OwnedFlowStart {
            operation_id: WorkItemOperationId::new(),
            source_project: project,
            input: FlowInput::Inline {
                graph: Box::new(graph()),
            },
            config: OwnedFlowRunConfig {
                mcp,
                initial_prompt: "raw initial prompt\n".into(),
                ..OwnedFlowRunConfig::default()
            },
            workspace: WorkspaceRequest::Managed,
        };
        let response = request(
            &socket,
            json!({"method":"owned_flow_start","request_id":1,"request":input}),
        )
        .await;
        assert_eq!(response["method"], "owned_flow_started", "{response}");
        let receipt: OwnedFlowReceipt =
            serde_json::from_value(response["receipt"].clone()).unwrap();
        let cycle = tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                let cycles = storage
                    .work_items()
                    .due_recovery_wakes_for_run(receipt.run, i64::MAX, 10)
                    .unwrap();
                let history = match storage.inspect_run(receipt.run).await {
                    Ok(history) => history,
                    Err(surge_persistence::runs::StorageError::Sqlite(
                        rusqlite::Error::SqliteFailure(_, Some(message)),
                    )) if message == "no such table: events" => {
                        // The accepted receipt precedes the first startup journal migration.
                        tokio::time::sleep(Duration::from_millis(10)).await;
                        continue;
                    },
                    Err(_) => panic!("unexpected initial journal inspection error"),
                };
                if let RunDatabaseInspection::Present { events } = history.database
                    && initial_mcp_evidence_ready(
                        phase,
                        &events,
                        &storage,
                        receipt.run,
                        !cycles.is_empty(),
                    )
                {
                    break (phase != "populated-unconfirmed")
                        .then_some(cycles.into_iter().next())
                        .flatten();
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        publish(
            &root.join("park.ready"),
            &json!({"receipt":receipt,
            "attempt":storage.work_items().for_run(receipt.run).unwrap().unwrap(),
            "control":storage.work_items().execution_control(receipt.run).unwrap(),
            "workspace":storage.work_items().show(receipt.item).unwrap().item.workspace,
            "manifest":storage.work_items().owned_flow_manifest(receipt.run).unwrap().unwrap(),
            "due":cycle.map_or(i64::MAX, |cycle| cycle.wake.unwrap().due_at_ms())}),
        );
        wait_file(&root.join("park.release")).await;
    }
    cancel.cancel();
    task.await.unwrap().unwrap();
}

struct RetainedFixture {
    home: FixtureHome,
    directory: tempfile::TempDir,
}
impl RetainedFixture {
    fn path(&self) -> &Path {
        self.directory.path()
    }
    fn home(&self) -> &Path {
        self.home.path()
    }
    fn close(self) {
        self.home.close().unwrap();
        self.directory.close().unwrap();
    }
}

fn fixture() -> RetainedFixture {
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(root.path().join("mcp.py"), CHILD).unwrap();
    std::fs::write(project.join("tracked.txt"), b"retained source").unwrap();
    std::fs::write(project.join("surge.toml"), b"mcp_servers = []\n").unwrap();
    let repo = git2::Repository::init(&project).unwrap();
    let mut index = repo.index().unwrap();
    for name in ["tracked.txt", "surge.toml"] {
        index.add_path(Path::new(name)).unwrap();
    }
    index.write().unwrap();
    let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
    let sig = git2::Signature::now("MCP oracle", "oracle@example.com").unwrap();
    repo.commit(Some("HEAD"), &sig, &sig, "fixed source", &tree, &[])
        .unwrap();
    RetainedFixture {
        home: FixtureHome::new().unwrap(),
        directory: root,
    }
}

async fn phase(root: &Path, home: &Path, mode: &str, ready: &str) -> cold_host::ColdHost {
    let mut host = cold_host::ColdHost::spawn(
        "child_owned_flow_mcp_probe",
        [
            (
                OsString::from("SURGE_OWNED_MCP_ROOT"),
                root.as_os_str().to_owned(),
            ),
            (
                OsString::from("SURGE_OWNED_MCP_HOME"),
                home.as_os_str().to_owned(),
            ),
            (
                OsString::from("SURGE_OWNED_MCP_PHASE"),
                OsString::from(mode),
            ),
        ],
        root.join(ready),
        root,
    )
    .unwrap();
    let result = host.wait_ready(Duration::from_secs(40)).await;
    if let Err(error) = result {
        let (stdout, stderr) = host.diagnostics_paths();
        panic!(
            "{mode} child readiness failed: {error}; stdout {}; stderr {}",
            stdout.display(),
            stderr.display()
        );
    }
    host
}

async fn finish(root: &Path, mut host: cold_host::ColdHost, release: &str) {
    std::fs::write(root.join(release), b"finish exact owned host").unwrap();
    assert!(
        host.wait_exit(Duration::from_secs(35))
            .await
            .unwrap()
            .success()
    );
    assert_eq!(host.pid(), None);
    assert_host_diagnostics_opaque(root);
}

fn proof(root: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(root.join("park.ready")).unwrap()).unwrap()
}

async fn legacy_control() {
    let root = fixture();
    {
        let host = phase(root.path(), root.home(), "legacy", "legacy.ready").await;
        let observed = records(&root.path().join("legacy.jsonl"));
        assert!(
            !root.path().join("legacy-denied.jsonl").exists(),
            "configured global ReadOnly server started before policy admission"
        );
        finish(root.path(), host, "legacy.release").await;
        assert!(observed.iter().any(|row| row["kind"] == "spawn"));
        assert!(
            observed.iter().any(|row| row["method"] == "tools/list"),
            "same mcp_add must actually expose fallback"
        );
    }
    root.close();
}

async fn start_park(root: &Path, home: &Path, mode: &str) -> Value {
    let host = phase(root, home, mode, "park.ready").await;
    let accepted = proof(root);
    finish(root, host, "park.release").await;
    assert!(
        !home.join("owned-mcp.sock").exists(),
        "first host must remove its owned socket"
    );
    accepted
}

async fn wait_provider_prompt(home: &Path) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !records(&home.join("quota-a-wire.jsonl"))
            .iter()
            .any(|row| row["operation"] == "prompt")
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

async fn wait_completed(storage: &Arc<Storage>, run: RunId) {
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let completed = match storage.inspect_run(run).await.unwrap().database {
                RunDatabaseInspection::Present { events } => events.iter().any(|event| {
                    matches!(event.payload.payload, EventPayload::RunCompleted { .. })
                }),
                _ => false,
            };
            if completed {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

async fn wait_attention(storage: &Arc<Storage>, run: RunId) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let attempt = storage.work_items().for_run(run).unwrap().unwrap();
            let control = storage.work_items().execution_control(run).unwrap();
            if attempt.state == surge_core::work_item::WorkItemAttemptState::Attention
                || control.as_ref().is_some_and(|value| {
                    value.state == surge_core::execution_recovery::ExecutionControlState::Attention
                })
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

async fn cold_wake(root: &Path, home: &Path, expected: &Value, mode: &str) {
    // Poison present project defaults AND engine-global fallback after capture.
    let config = surge_core::SurgeConfig {
        mcp_servers: vec![server(root, "oracle", "changed-default.jsonl")],
        ..surge_core::SurgeConfig::default()
    };
    std::fs::write(
        root.join("project/surge.toml"),
        toml::to_string(&config).unwrap(),
    )
    .unwrap();
    let mut host = phase(root, home, mode, "wake.ready").await;
    assert!(
        host.wait_exit(Duration::from_secs(35))
            .await
            .unwrap()
            .success()
    );
    let storage = Storage::open(home).await.unwrap();
    let receipt: OwnedFlowReceipt = serde_json::from_value(expected["receipt"].clone()).unwrap();
    assert_eq!(
        storage
            .work_items()
            .scan_attempts(None, 10)
            .unwrap()
            .entries
            .len(),
        1
    );
    assert_eq!(
        serde_json::to_value(
            storage
                .work_items()
                .show(receipt.item)
                .unwrap()
                .item
                .workspace
        )
        .unwrap(),
        expected["workspace"]
    );
    let RunDatabaseInspection::Present { events } =
        storage.inspect_run(receipt.run).await.unwrap().database
    else {
        panic!("original journal")
    };
    for kind in [
        "RunStarted",
        "PipelineMaterialized",
        "WorkItemAttemptBound",
        "OwnedFlowInputsBound",
    ] {
        assert_eq!(
            events
                .iter()
                .filter(|event| event.payload.payload().discriminant_str() == kind)
                .count(),
            1,
            "startup duplicated: {kind}"
        );
    }
    assert!(!root.join("replacement.jsonl").exists());
    assert!(!root.join("changed-default.jsonl").exists());
    assert_host_diagnostics_opaque(root);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn explicit_empty_survives_real_cold_wake_and_changed_global_defaults() {
    legacy_control().await;
    let root = fixture();
    {
        let accepted = start_park(root.path(), root.home(), "explicit-empty").await;
        assert_eq!(accepted["manifest"]["mcp"]["selection"], "explicit");
        cold_wake(root.path(), root.home(), &accepted, "wake").await;
        assert!(!root.home().join("work-items/private-flow-inputs").exists());
        assert!(!root.path().join("original.jsonl").exists());
    }
    root.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn host_default_empty_survives_real_cold_wake_and_changed_global_defaults() {
    legacy_control().await;
    let root = fixture();
    {
        let accepted = start_park(root.path(), root.home(), "default-empty").await;
        assert_eq!(accepted["manifest"]["mcp"]["selection"], "host_default");
        cold_wake(root.path(), root.home(), &accepted, "wake").await;
        assert!(!root.home().join("work-items/private-flow-inputs").exists());
        assert!(!root.path().join("original.jsonl").exists());
    }
    root.close();
}

fn assert_stage_whitelist(root: &Path, phase: &str) {
    let opened = records(&root.join(format!("{phase}-stage-catalog.jsonl")));
    assert!(
        !opened.is_empty(),
        "actual SessionConfig catalog capture missing"
    );
    for opening in opened {
        let tools = opening["tools"].as_array().unwrap();
        assert!(
            tools.iter().any(|tool| tool == "echo"),
            "allowed MCP route omitted"
        );
        assert!(
            !tools.iter().any(|tool| tool == "forbidden"),
            "configured whitelist was lost"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn legacy_global_policy_references_survive_actual_start_and_resume() {
    let root = fixture();
    {
        let started = phase(
            root.path(),
            root.home(),
            "legacy-resume-start",
            "legacy.ready",
        )
        .await;
        let before = records(&root.path().join("legacy.jsonl"));
        assert!(
            before
                .iter()
                .any(|row| row["kind"] == "request" && row["method"] == "tools/list")
        );
        assert!(!root.path().join("legacy-denied.jsonl").exists());
        finish(root.path(), started, "legacy.release").await;
        assert_stage_whitelist(root.path(), "legacy-resume-start");
        let run: RunId =
            serde_json::from_slice(&std::fs::read(root.path().join("legacy-run.json")).unwrap())
                .unwrap();
        let storage = Storage::open(root.home()).await.unwrap();
        let RunDatabaseInspection::Present { events } =
            storage.inspect_run(run).await.unwrap().database
        else {
            panic!("real nonterminal legacy journal missing");
        };
        assert!(
            !events.iter().any(|event| matches!(
                event.payload.payload(),
                EventPayload::RunCompleted { .. }
                    | EventPayload::RunFailed { .. }
                    | EventPayload::RunAborted { .. }
            )),
            "legacy resume counterfactual became terminal before owner loss"
        );
        let resumed = phase(
            root.path(),
            root.home(),
            "legacy-resume",
            "legacy-resumed.ready",
        )
        .await;
        let after = records(&root.path().join("legacy-resumed.jsonl"));
        assert!(
            after
                .iter()
                .any(|row| row["kind"] == "request" && row["method"] == "tools/list")
        );
        assert!(!root.path().join("legacy-denied.jsonl").exists());
        finish(root.path(), resumed, "legacy-resumed.release").await;
        assert_stage_whitelist(root.path(), "legacy-resume");
    }
    root.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn legacy_engine_catalog_enforces_actual_configured_deadline() {
    for (mode, succeeds) in [
        ("legacy-deadline-positive", true),
        ("legacy-deadline-negative", false),
    ] {
        let root = fixture();
        {
            let host = phase(root.path(), root.home(), mode, "legacy.ready").await;
            finish(root.path(), host, "legacy.release").await;
            let opened = records(&root.path().join(format!("{mode}-stage-catalog.jsonl")));
            assert!(!opened.is_empty());
            for opening in opened {
                let tools = opening["tools"].as_array().unwrap();
                assert_eq!(
                    tools.iter().any(|tool| tool == "echo"),
                    succeeds,
                    "actual engine catalog did not honor accepted server deadline"
                );
                assert!(!tools.iter().any(|tool| tool == "forbidden"));
            }
            assert!(
                records(&root.path().join("legacy.jsonl"))
                    .iter()
                    .any(|row| row["kind"] == "request" && row["method"] == "tools/list")
            );
            assert!(!root.path().join("legacy-denied.jsonl").exists());
        }
        root.close();
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn verify_actual_transport(root: &Path, accepted: &Value) {
    let rows = records(&root.join("original.jsonl"));
    let spawned: Vec<_> = rows.iter().filter(|row| row["kind"] == "spawn").collect();
    assert!(!spawned.is_empty(), "actual owned child must have spawned");
    let probe = std::process::Command::new(python())
        .args([
            "-c",
            "import os,sys; print(os.path.realpath(sys.executable))",
        ])
        .output()
        .unwrap();
    assert!(probe.status.success());
    let executable = String::from_utf8(probe.stdout).unwrap();
    let expected = server(root, "oracle", "original.jsonl");
    let McpTransportConfig::Stdio { args, .. } = expected.transport else {
        panic!("stdio")
    };
    let workspace = std::fs::canonicalize(accepted["workspace"]["path"].as_str().unwrap()).unwrap();
    for row in spawned {
        assert!(
            row["exe"].as_str() == Some(executable.trim()),
            "actual executable identity altered"
        );
        assert!(
            row["args"] == json!(args),
            "actual ordered arguments altered"
        );
        assert!(
            row["value"] == "owned-flow-private-environment-Ω",
            "actual declared environment altered"
        );
        assert!(
            row["cwd"].as_str() == workspace.to_str(),
            "actual working directory altered"
        );
    }
    assert!(rows.iter().any(|row| row["method"] == "initialize"));
    assert!(rows.iter().any(|row| row["method"] == "tools/list"));
    assert!(
        !root.join("denied.jsonl").exists(),
        "ReadOnly server spawned"
    );
    assert_eq!(accepted["manifest"]["mcp"]["entries"][1]["name"], "denied");
    assert_eq!(
        accepted["manifest"]["mcp"]["entries"][1]["sandbox"],
        "read-only"
    );
    assert!(
        rows.iter().any(|row| row["kind"] == "eof"),
        "run teardown must reach actual child EOF"
    );
    assert_eq!(
        accepted["manifest"]["mcp"]["entries"][0]["allowed_tools"],
        json!(["echo"])
    );
    assert_eq!(
        accepted["manifest"]["mcp"]["entries"][0]["call_timeout"],
        "1s"
    );
    assert_eq!(
        accepted["manifest"]["mcp"]["entries"][0]["restart_on_crash"],
        false
    );
    assert_eq!(
        accepted["manifest"]["mcp"]["entries"][0]["sandbox"],
        "workspace-write"
    );
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn populated_initial_transport_is_exact_and_public_snapshot_opaque() {
    let root = fixture();
    {
        let accepted = start_park(root.path(), root.home(), "populated").await;
        verify_actual_transport(root.path(), &accepted);
        let storage = Storage::open(root.home()).await.unwrap();
        let receipt: OwnedFlowReceipt =
            serde_json::from_value(accepted["receipt"].clone()).unwrap();
        let public = format!(
            "{} {} {:?}",
            accepted,
            serde_json::to_string(&storage.work_items().for_run(receipt.run).unwrap()).unwrap(),
            storage.inspect_run(receipt.run).await.unwrap()
        );
        assert_private_absent(&public);
    }
    root.close();
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn populated_cold_valid_twin_keeps_original_transport_and_writer_history() {
    let root = fixture();
    {
        let accepted = start_park(root.path(), root.home(), "populated").await;
        let before = records(&root.path().join("original.jsonl")).len();
        cold_wake(root.path(), root.home(), &accepted, "wake").await;
        verify_actual_transport(root.path(), &accepted);
        let after = records(&root.path().join("original.jsonl"));
        assert!(
            after[before..].iter().any(|row| row["kind"] == "spawn"),
            "cold twin must execute original MCP"
        );
        // Actual original MCP writer must have production-owned cleanup evidence.
        let storage = Storage::open(root.home()).await.unwrap();
        let receipt: OwnedFlowReceipt =
            serde_json::from_value(accepted["receipt"].clone()).unwrap();
        let RunDatabaseInspection::Present { events } =
            storage.inspect_run(receipt.run).await.unwrap().database
        else {
            panic!("original writer journal")
        };
        // ADR-0021 records cleanup for writers of the phase before the suspension
        // when the run resumes; the resumed phase's own writers end with the run.
        let suspended_at = events
            .iter()
            .find(|row| matches!(row.payload.payload, EventPayload::RunSuspended { .. }))
            .map(|row| row.seq)
            .expect("populated park must suspend");
        for event in events.iter().filter(|row| row.seq < suspended_at) {
            if let EventPayload::ExecutionWriterIntent { intent } = &event.payload.payload
                && matches!(&intent.kind,surge_core::execution_recovery::process::ExecutionWriterKind::HostTool { call_id } if call_id.starts_with("mcp-child:"))
            {
                // ADR-0021: an empty group is recorded as best-effort cleanup, never as closure.
                assert!(events.iter().any(|row|matches!(row.payload.payload,
                EventPayload::ExecutionWriterClosed { writer } | EventPayload::ExecutionWriterGroupStopped { writer } if writer==intent.writer)),"MCP cold twin has no persisted cleanup record");
            }
        }
    }
    root.close();
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[derive(Clone, Copy, Debug)]
enum Corruption {
    MissingKey,
    CorruptKey,
    MissingObject,
    ChangedObject,
    OtherServerObject,
    OtherPurposeEnvelope,
    ManifestSandbox,
    ManifestBinding,
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn assert_manifest_update_rejected(
    connection: &rusqlite::Connection,
    run: RunId,
    altered: &str,
) -> String {
    let original: String = connection
        .query_row(
            "SELECT inputs_manifest FROM owned_flow_operations WHERE run=?",
            [run.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    let result = connection.execute(
        "UPDATE owned_flow_operations SET inputs_manifest=? WHERE run=?",
        rusqlite::params![altered, run.to_string()],
    );
    assert!(
        matches!(result, Err(rusqlite::Error::SqliteFailure(code, Some(message)))
        if code.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_TRIGGER
            && message == "owned flow first snapshot is immutable"),
        "ordinary snapshot update did not hit exact production trigger refusal"
    );
    let unchanged: String = connection
        .query_row(
            "SELECT inputs_manifest FROM owned_flow_operations WHERE run=?",
            [run.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        unchanged == original,
        "rejected update changed immutable raw snapshot"
    );
    original
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unresolved_mcp_domain_requires_attention_and_blocks_actual_cold_dispatch() {
    let root = fixture();
    {
        let _reaper = DescendantReaper(root.path().join("original.jsonl.descendant"));
        let accepted = start_park(root.path(), root.home(), "populated-unconfirmed").await;
        verify_actual_transport(root.path(), &accepted);
        let receipt: OwnedFlowReceipt =
            serde_json::from_value(accepted["receipt"].clone()).unwrap();
        let storage = Storage::open(root.home()).await.unwrap();
        let RunDatabaseInspection::Present { events } =
            storage.inspect_run(receipt.run).await.unwrap().database
        else {
            panic!("actual MCP journal is missing");
        };
        assert!(events.iter().any(|event| matches!(&event.payload.payload,
        EventPayload::ExecutionWriterEstablished { container, .. }
        if container.coverage() == surge_core::execution_recovery::process::WriterCoverage::GroupOnly)));
        assert!(
            events.iter().any(|event| matches!(
                &event.payload.payload,
                EventPayload::RunRecoveryRequired { .. }
            )),
            "direct MCP settlement falsely authorized the whole writer domain"
        );
        assert!(
            !events.iter().any(|event| matches!(&event.payload.payload,
        EventPayload::RunSuspended { fence } if fence.cleanup_confirmed)),
            "unconfirmed MCP effects received a confirmed cleanup fence"
        );
        drop(storage);
        let before = records(&root.path().join("original.jsonl"));
        let provider_before = records(&root.home().join("quota-a-wire.jsonl"));
        cold_wake(root.path(), root.home(), &accepted, "refuse").await;
        assert_eq!(
            records(&root.path().join("original.jsonl")),
            before,
            "unresolved prior MCP domain dispatched another child"
        );
        assert_eq!(
            records(&root.home().join("quota-a-wire.jsonl")),
            provider_before,
            "unresolved prior MCP domain opened or prompted another provider session"
        );
        let storage = Storage::open(root.home()).await.unwrap();
        assert_eq!(
            storage
                .work_items()
                .for_run(receipt.run)
                .unwrap()
                .unwrap()
                .state,
            surge_core::work_item::WorkItemAttemptState::Attention
        );
    }
    root.close();
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ordinary_accepted_manifest_updates_preserve_exact_first_snapshot() {
    let root = fixture();
    {
        let accepted = start_park(root.path(), root.home(), "populated").await;
        let receipt: OwnedFlowReceipt =
            serde_json::from_value(accepted["receipt"].clone()).unwrap();
        let mut altered = accepted["manifest"].clone();
        altered["mcp"]["entries"][0]["sandbox"] = json!("full-access");
        let connection =
            rusqlite::Connection::open(root.home().join("db/registry.sqlite")).unwrap();
        assert_manifest_update_rejected(
            &connection,
            receipt.run,
            &serde_json::to_string(&altered).unwrap(),
        );
    }
    root.close();
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn corrupt(home: &Path, accepted: &Value, fault: Corruption) {
    let private = home.join("work-items/private-flow-inputs");
    let manifest = &accepted["manifest"];
    let object = |reference: &Value| private.join(format!("{}.json", reference.as_str().unwrap()));
    let first = object(&manifest["mcp"]["entries"][0]["object_ref"]);
    match fault {
        Corruption::MissingKey => std::fs::remove_file(private.join("host-key-v1")).unwrap(),
        Corruption::CorruptKey => {
            std::fs::write(private.join("host-key-v1"), b"corrupt existing key").unwrap()
        },
        Corruption::MissingObject => std::fs::remove_file(first).unwrap(),
        Corruption::ChangedObject => {
            let mut value: Value = serde_json::from_slice(&std::fs::read(&first).unwrap()).unwrap();
            value["server"]["transport"]["env"]["DECLARED_PRIVATE"] =
                json!("changed private transport");
            std::fs::write(first, serde_json::to_vec(&value).unwrap()).unwrap();
        },
        Corruption::OtherServerObject => {
            let other = object(&manifest["mcp"]["entries"][1]["object_ref"]);
            std::fs::write(first, std::fs::read(other).unwrap()).unwrap();
        },
        Corruption::OtherPurposeEnvelope => {
            let envelope = object(&manifest["mcp"]["envelope_ref"]);
            std::fs::write(envelope, std::fs::read(first).unwrap()).unwrap();
        },
        Corruption::ManifestSandbox | Corruption::ManifestBinding => {
            let receipt: OwnedFlowReceipt =
                serde_json::from_value(accepted["receipt"].clone()).unwrap();
            let mut altered = manifest.clone();
            if matches!(fault, Corruption::ManifestSandbox) {
                altered["mcp"]["entries"][0]["sandbox"] = json!("full-access");
                altered["mcp"]["entries"][0]["allowed_tools"] = json!(["forbidden"]);
            } else {
                altered["workspace_owner"] = json!(RunId::new());
            }
            let mut connection =
                rusqlite::Connection::open(home.join("db/registry.sqlite")).unwrap();
            // Hosts were actually waited/reaped by start_park. Exclusive SQLite
            // ownership proves no live DB writer; this is not domain quiescence.
            let transaction = connection
                .transaction_with_behavior(rusqlite::TransactionBehavior::Exclusive)
                .unwrap();
            let altered = serde_json::to_string(&altered).unwrap();
            let original = assert_manifest_update_rejected(&transaction, receipt.run, &altered);
            // TEST ONLY: simulate damaged saved data, not an ordinary accepted update.
            transaction
                .execute("DROP TRIGGER owned_flow_first_identity_immutable", [])
                .unwrap();
            assert_eq!(
                transaction
                    .execute(
                        "UPDATE owned_flow_operations SET inputs_manifest=? WHERE run=?",
                        rusqlite::params![&altered, receipt.run.to_string()],
                    )
                    .unwrap(),
                1
            );
            let mutated: String = transaction
                .query_row(
                    "SELECT inputs_manifest FROM owned_flow_operations WHERE run=?",
                    [receipt.run.to_string()],
                    |row| row.get(0),
                )
                .unwrap();
            assert!(
                mutated == altered,
                "offline corruption did not change exact original row"
            );
            assert_ne!(
                surge_core::ContentHash::compute(original.as_bytes()),
                surge_core::ContentHash::compute(mutated.as_bytes())
            );
            transaction.commit().unwrap();
        },
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
async fn corruption_refuses_before_any_new_actual_effect(fault: Corruption) {
    let root = fixture();
    {
        let accepted = start_park(root.path(), root.home(), "populated").await;
        verify_actual_transport(root.path(), &accepted);
        let observed = [
            root.path().join("original.jsonl"),
            root.path().join("denied.jsonl"),
            root.home().join("quota-a-wire.jsonl"),
            root.home().join("quota-b-wire.jsonl"),
        ];
        let before: Vec<_> = observed.iter().map(|name| records(name)).collect();
        corrupt(root.home(), &accepted, fault);
        cold_wake(root.path(), root.home(), &accepted, "refuse").await;
        for (name, previous) in observed.iter().zip(before) {
            assert!(
                records(name) == previous,
                "{fault:?} allowed actual effect in {name:?}"
            );
        }
        if matches!(fault, Corruption::MissingKey) {
            assert!(
                !root
                    .home()
                    .join("work-items/private-flow-inputs/host-key-v1")
                    .exists(),
                "hydration regenerated missing key"
            );
        }
    }
    root.close();
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_and_corrupt_private_inputs_refuse_real_cold_writers() {
    for fault in [
        Corruption::MissingKey,
        Corruption::CorruptKey,
        Corruption::MissingObject,
        Corruption::ChangedObject,
    ] {
        corruption_refuses_before_any_new_actual_effect(fault).await;
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn transplanted_objects_and_changed_manifest_refuse_real_cold_writers() {
    for fault in [
        Corruption::OtherServerObject,
        Corruption::OtherPurposeEnvelope,
        Corruption::ManifestSandbox,
        Corruption::ManifestBinding,
    ] {
        corruption_refuses_before_any_new_actual_effect(fault).await;
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn manifest_binding_refusal_must_persist_attention_after_actual_wake() {
    let root = fixture();
    {
        let accepted = start_park(root.path(), root.home(), "populated").await;
        verify_actual_transport(root.path(), &accepted);
        let receipt: OwnedFlowReceipt =
            serde_json::from_value(accepted["receipt"].clone()).unwrap();
        let observed = [
            root.path().join("original.jsonl"),
            root.path().join("denied.jsonl"),
            root.home().join("quota-a-wire.jsonl"),
            root.home().join("quota-b-wire.jsonl"),
        ];
        let before: Vec<_> = observed.iter().map(|name| records(name)).collect();
        let connection =
            rusqlite::Connection::open(root.home().join("db/registry.sqlite")).unwrap();
        let old_token: String = connection
            .query_row(
                "SELECT claim_token FROM work_item_attempts WHERE run=?",
                [receipt.run.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        let lineage: (String, u64, u64, String, u64) = connection.query_row(
        "SELECT q.invocation,q.cycle_generation,q.revision,json_extract(q.wake,'$.identity'),(SELECT MAX(generation) FROM work_item_execution_controls WHERE run=q.run) FROM work_item_quota_cycles q WHERE q.run=? AND q.closed=0 AND q.wake_at_ms IS NOT NULL",
        [receipt.run.to_string()], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?))).unwrap();
        let prewake_due_count: u64 = connection.query_row("SELECT COUNT(*) FROM work_item_quota_cycles WHERE run=? AND closed=0 AND wake_at_ms IS NOT NULL", [receipt.run.to_string()], |row| row.get(0)).unwrap();
        assert_eq!(prewake_due_count, 1);
        drop(connection);
        corrupt(root.home(), &accepted, Corruption::ManifestBinding);
        let mut host = phase(root.path(), root.home(), "refuse-state", "wake.ready").await;
        assert!(
            host.wait_exit(Duration::from_secs(10))
                .await
                .unwrap()
                .success()
        );
        let output: Value =
            serde_json::from_slice(&std::fs::read(root.path().join("wake.ready")).unwrap())
                .unwrap();
        assert!(
            output["first"] == output["later"]
                && output["first_control"] == output["later_control"],
            "refusal state did not persist after scheduler stopped"
        );
        for (name, previous) in observed.iter().zip(before) {
            assert!(
                records(name) == previous,
                "refused wake emitted a physical effect"
            );
        }
        let connection =
            rusqlite::Connection::open(root.home().join("db/registry.sqlite")).unwrap();
        let (key, hash, body, delivered): (String, String, String, u64) = connection.query_row(
        "SELECT r.key,r.hash,r.body,o.delivered_event_seq FROM owned_flow_wake_refusals r JOIN owned_flow_wake_refusal_outbox o ON o.key=r.key AND o.hash=r.hash WHERE r.run=?",
        [receipt.run.to_string()], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))).unwrap();
        let refused: surge_core::work_item::OwnedFlowWakeRefusalReceipt =
            serde_json::from_str(&body).unwrap();
        assert_eq!(refused.run(), receipt.run);
        assert_eq!(refused.operation(), receipt.operation_id);
        assert_eq!(refused.binding(), &receipt.binding);
        assert_eq!(
            refused.lineage().invocation,
            lineage
                .0
                .parse::<surge_core::id::StageInvocationId>()
                .unwrap()
        );
        assert_eq!(refused.lineage().cycle_generation, lineage.1);
        assert_eq!(refused.lineage().source_revision, lineage.2);
        assert_eq!(refused.lineage().wake_identity, lineage.3);
        assert_eq!(refused.lineage().control_generation, lineage.4);
        let refusal_count: u64 = connection
            .query_row(
                "SELECT COUNT(*) FROM owned_flow_wake_refusals WHERE run=?",
                [receipt.run.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(refusal_count, 1);

        // Ordinary SQL cannot rewrite either immutable history or an acknowledged
        // occurrence. Check the actual stored rows again after every rejected write.
        for sql in [
            "UPDATE owned_flow_wake_refusals SET body='{}' WHERE key=?",
            "DELETE FROM owned_flow_wake_refusals WHERE key=?",
            "UPDATE owned_flow_wake_refusal_outbox SET delivered_event_seq=NULL WHERE key=?",
            "UPDATE owned_flow_wake_refusal_outbox SET delivered_event_seq=delivered_event_seq+1 WHERE key=?",
            "UPDATE owned_flow_wake_refusal_outbox SET key=key||'changed' WHERE key=?",
            "UPDATE owned_flow_wake_refusal_outbox SET hash=hash||'changed' WHERE key=?",
            "DELETE FROM owned_flow_wake_refusal_outbox WHERE key=?",
        ] {
            let error = connection.execute(sql, [&key]).unwrap_err();
            assert!(matches!(error, rusqlite::Error::SqliteFailure(ref code, _)
            if code.code == rusqlite::ErrorCode::ConstraintViolation));
            let unchanged: (String, String, u64) = connection.query_row(
            "SELECT r.hash,r.body,o.delivered_event_seq FROM owned_flow_wake_refusals r JOIN owned_flow_wake_refusal_outbox o ON o.key=r.key WHERE r.key=?",
            [&key], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).unwrap();
            assert_eq!(unchanged, (hash.clone(), body.clone(), delivered));
        }

        assert_eq!(
            refused.reason(),
            surge_core::work_item::OwnedFlowWakeRefusalReason::InputsAssociationMismatch
        );
        assert_eq!(key, refused.hash().unwrap().to_string());
        assert_eq!(key, hash);
        assert_eq!(
            output["later"]["diagnostic"],
            "owned_flow_inputs_association_mismatch"
        );
        assert_eq!(
            output["later_control"]["diagnostic"],
            "owned_flow_inputs_association_mismatch"
        );
        let events = rusqlite::Connection::open(
            root.home()
                .join("runs")
                .join(receipt.run.to_string())
                .join("events.sqlite"),
        )
        .unwrap();
        let (kind, payload): (String, Vec<u8>) = events
            .query_row(
                "SELECT kind,payload FROM events WHERE seq=?",
                [delivered],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(kind, "OwnedFlowWakeRefused");
        let payload: surge_core::VersionedEventPayload = serde_json::from_slice(&payload).unwrap();
        assert!(
            matches!(payload.payload, EventPayload::OwnedFlowWakeRefused { receipt: stored } if stored.as_ref() == &refused)
        );
        let token: String = connection
            .query_row(
                "SELECT claim_token FROM work_item_attempts WHERE run=?",
                [receipt.run.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            token == old_token,
            "failed association changed original claim token"
        );
        assert_eq!(
            output["later"]["state"], "attention",
            "permanent association refusal must durably enter Attention"
        );
        assert_eq!(output["later_control"]["state"], "attention");
    }
    root.close();
}

fn legacy_policy_graph(phase: &str) -> Graph {
    let mut graph = graph();
    if phase.starts_with("legacy-policy-") {
        let node = graph.nodes.get_mut(&graph.start).unwrap();
        let mut config = serde_json::to_value(&node.config).unwrap();
        let sandbox = surge_core::sandbox::SandboxConfig {
            mode: surge_core::sandbox::SandboxMode::ReadOnly,
            ..surge_core::sandbox::SandboxConfig::default()
        };
        config["sandbox_override"] = serde_json::to_value(sandbox).unwrap();
        node.config = serde_json::from_value(config).unwrap();
    }
    graph
}

fn assert_optional_route_catalog(root: &Path, phase: &str, visible: bool) {
    let captured = records(&root.join(format!("{phase}-stage-catalog.jsonl")));
    assert!(!captured.is_empty(), "actual forwarded catalog missing");
    for row in captured {
        let tools = row["tools"].as_array().unwrap();
        assert_eq!(tools.iter().any(|name| name == "echo"), visible);
        assert_eq!(tools.iter().any(|name| name == "forbidden"), visible);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn optional_whitelists_and_server_override_survive_actual_start_and_resume() {
    for (policy, visible) in [("none", true), ("empty", false)] {
        let root = fixture();
        {
            let started_phase = format!("legacy-policy-tools-{policy}-start");
            let resumed_phase = format!("legacy-policy-tools-{policy}-resume");
            let started = phase(root.path(), root.home(), &started_phase, "legacy.ready").await;
            finish(root.path(), started, "legacy.release").await;
            assert_optional_route_catalog(root.path(), &started_phase, visible);
            assert!(!root.path().join("legacy-denied.jsonl").exists());
            assert!(
                records(&root.path().join("legacy.jsonl"))
                    .iter()
                    .any(|row| row["method"] == "tools/list"),
                "server override did not admit actual catalog from ReadOnly node"
            );
            let run: RunId = serde_json::from_slice(
                &std::fs::read(root.path().join("legacy-run.json")).unwrap(),
            )
            .unwrap();
            let storage = Storage::open(root.home()).await.unwrap();
            let before = storage
                .inspect_folded_run(run)
                .await
                .unwrap()
                .database
                .unwrap()
                .startup;
            std::fs::remove_file(root.path().join("legacy.ready")).unwrap();
            std::fs::remove_file(root.path().join("legacy.release")).unwrap();
            let resumed = phase(
                root.path(),
                root.home(),
                &resumed_phase,
                "legacy-resumed.ready",
            )
            .await;
            finish(root.path(), resumed, "legacy-resumed.release").await;
            assert_optional_route_catalog(root.path(), &resumed_phase, visible);
            assert!(!root.path().join("legacy-denied.jsonl").exists());
            assert!(
                records(&root.path().join("legacy-resumed.jsonl"))
                    .iter()
                    .any(|row| row["method"] == "tools/list"),
                "resume lost server override or resolved global refs"
            );
            let after = storage
                .inspect_folded_run(run)
                .await
                .unwrap()
                .database
                .unwrap()
                .startup;
            assert!(
                startup_rows(&before) == startup_rows(&after),
                "actual historical startup changed across policy resume"
            );
        }
        root.close();
    }
}

fn startup_rows(events: &[surge_persistence::runs::reader::ReadEvent]) -> Vec<Value> {
    events
        .iter()
        .map(|event| {
            json!({
                "seq":event.seq,
                "timestamp_ms":event.timestamp_ms,
                "kind":event.kind,
                "payload":event.payload,
            })
        })
        .collect()
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn published_refusal_retains_original_os_lease_without_caller_guard_until_real_delivery() {
    let root = fixture();
    {
        let accepted = start_park(root.path(), root.home(), "populated").await;
        verify_actual_transport(root.path(), &accepted);
        let receipt: OwnedFlowReceipt =
            serde_json::from_value(accepted["receipt"].clone()).unwrap();
        corrupt(root.home(), &accepted, Corruption::ManifestBinding);
        let mut host = phase(root.path(), root.home(), "refuse-held", "wake.ready").await;
        let lock_path = root
            .home()
            .join("work-items/preparation-locks")
            .join(format!(
                "flow-launch-v1-{}-{}.lock",
                receipt.operation_id, receipt.run
            ));
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&lock_path)
            .unwrap();
        let actual_second_process_busy =
            matches!(file.try_lock(), Err(std::fs::TryLockError::WouldBlock));
        let connection =
            rusqlite::Connection::open(root.home().join("db/registry.sqlite")).unwrap();
        let pending: Option<u64> = connection
            .query_row(
                "SELECT delivered_event_seq FROM owned_flow_wake_refusal_outbox",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let state: String = connection
            .query_row(
                "SELECT state FROM work_item_attempts WHERE run=?",
                [receipt.run.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        drop(connection);
        std::fs::write(
            root.path().join("held.release"),
            b"release original writer actor",
        )
        .unwrap();
        wait_file(&root.path().join("held-delivered.ready")).await;
        assert!(
            host.wait_exit(Duration::from_secs(10))
                .await
                .unwrap()
                .success()
        );
        let real_release = file.try_lock().is_ok();
        assert!(
            actual_second_process_busy,
            "published refusal lost the original real launch lock"
        );
        assert!(
            pending.is_none(),
            "blocked actual normal writer was bypassed by informational delivery"
        );
        assert_eq!(state, "attention");
        assert!(
            real_release,
            "actual delivered-and-joined refusal kept a stale launch lease"
        );
        let connection =
            rusqlite::Connection::open(root.home().join("db/registry.sqlite")).unwrap();
        let delivered: u64 = connection
            .query_row(
                "SELECT delivered_event_seq FROM owned_flow_wake_refusal_outbox",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let journal = rusqlite::Connection::open(
            root.home()
                .join("runs")
                .join(receipt.run.to_string())
                .join("events.sqlite"),
        )
        .unwrap();
        let kind: String = journal
            .query_row("SELECT kind FROM events WHERE seq=?", [delivered], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(kind, "OwnedFlowWakeRefused");
    }
    root.close();
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cold_refusal_reclaims_actual_event_before_ack_without_new_claim_or_effect() {
    for missing_journal in [false, true] {
        let root = fixture();
        {
            let accepted = start_park(root.path(), root.home(), "populated").await;
            verify_actual_transport(root.path(), &accepted);
            let receipt: OwnedFlowReceipt =
                serde_json::from_value(accepted["receipt"].clone()).unwrap();
            let observed = [
                root.path().join("original.jsonl"),
                root.path().join("denied.jsonl"),
                root.home().join("quota-a-wire.jsonl"),
                root.home().join("quota-b-wire.jsonl"),
            ];
            let before: Vec<_> = observed.iter().map(|name| records(name)).collect();
            let registry_path = root.home().join("db/registry.sqlite");
            let mut connection = rusqlite::Connection::open(&registry_path).unwrap();
            let immutable: (String, String, String) = connection.query_row(
        "SELECT source_snapshot,config_snapshot,receipt FROM owned_flow_operations WHERE run=?",
        [receipt.run.to_string()], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).unwrap();
            let token: String = connection
                .query_row(
                    "SELECT claim_token FROM work_item_attempts WHERE run=?",
                    [receipt.run.to_string()],
                    |row| row.get(0),
                )
                .unwrap();
            corrupt(root.home(), &accepted, Corruption::ManifestBinding);
            let mut host = phase(root.path(), root.home(), "refuse-held", "wake.ready").await;
            let transaction = connection
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .unwrap();
            let pending: Option<u64> = transaction
                .query_row(
                    "SELECT delivered_event_seq FROM owned_flow_wake_refusal_outbox",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert!(pending.is_none());
            let lock_path = root
                .home()
                .join("work-items/preparation-locks")
                .join(format!(
                    "flow-launch-v1-{}-{}.lock",
                    receipt.operation_id, receipt.run
                ));
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(lock_path)
                .unwrap();
            assert!(matches!(
                file.try_lock(),
                Err(std::fs::TryLockError::WouldBlock)
            ));
            std::fs::write(
                root.path().join("held.release"),
                b"allow actual journal append, registry ACK blocked",
            )
            .unwrap();
            let journal_path = root
                .home()
                .join("runs")
                .join(receipt.run.to_string())
                .join("events.sqlite");
            let (original_seq, original_payload) = wait_actual_refusal_event(&journal_path).await;
            let payload: surge_core::VersionedEventPayload =
                serde_json::from_slice(&original_payload).unwrap();
            assert!(
                matches!(payload.payload, EventPayload::OwnedFlowWakeRefused { receipt: stored } if stored.run() == receipt.run && stored.binding() == &receipt.binding)
            );
            let ack: Option<u64> = transaction
                .query_row(
                    "SELECT delivered_event_seq FROM owned_flow_wake_refusal_outbox",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert!(
                ack.is_none(),
                "ACK crossed the actual held registry transaction"
            );
            let original_pid = host.pid().unwrap();
            nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(i32::try_from(original_pid).unwrap()),
                None,
            )
            .unwrap();
            host.stop_and_wait().unwrap(); // Kill and reap the exact owner after the actual event commit.
            transaction.rollback().unwrap();
            let saved_journal = journal_path.with_extension("retained-existing");
            if missing_journal {
                std::fs::rename(&journal_path, &saved_journal).unwrap();
            }
            let mut cold = if missing_journal {
                phase(
                    root.path(),
                    root.home(),
                    "refusal-reclaim-missing",
                    "reclaim-pending.ready",
                )
                .await
            } else {
                phase(
                    root.path(),
                    root.home(),
                    "refusal-reclaim",
                    "reclaimed.ready",
                )
                .await
            };
            if missing_journal {
                wait_file(&root.path().join("runtime-consumed.ready")).await;
                tokio::time::sleep(Duration::from_millis(300)).await;
                assert!(
                    !journal_path.exists(),
                    "informational delivery created a missing journal"
                );
                assert!(
                    !root.path().join("reclaimed.ready").exists(),
                    "terminal barrier reported completion without a journal"
                );
                assert!(
                    matches!(file.try_lock(), Err(std::fs::TryLockError::WouldBlock)),
                    "runtime drop lost original pending refusal ownership"
                );
                nix::sys::signal::kill(
                    nix::unistd::Pid::from_raw(i32::try_from(cold.pid().unwrap()).unwrap()),
                    None,
                )
                .unwrap();
                let still_pending: Option<u64> = connection
                    .query_row(
                        "SELECT delivered_event_seq FROM owned_flow_wake_refusal_outbox",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert!(still_pending.is_none());
                std::fs::rename(&saved_journal, &journal_path).unwrap();
                wait_file(&root.path().join("reclaimed.ready")).await;
            }
            assert!(
                cold.wait_exit(Duration::from_secs(10))
                    .await
                    .unwrap()
                    .success()
            );
            let (delivered, count): (u64, u64) = connection.query_row(
        "SELECT delivered_event_seq,(SELECT COUNT(*) FROM owned_flow_wake_refusals) FROM owned_flow_wake_refusal_outbox", [], |row| Ok((row.get(0)?, row.get(1)?))).unwrap();
            assert_eq!(delivered, original_seq);
            assert_eq!(count, 1);
            let current_token: String = connection
                .query_row(
                    "SELECT claim_token FROM work_item_attempts WHERE run=?",
                    [receipt.run.to_string()],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(current_token, token);
            let current: (String, String, String) = connection.query_row(
        "SELECT source_snapshot,config_snapshot,receipt FROM owned_flow_operations WHERE run=?",
        [receipt.run.to_string()], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).unwrap();
            assert_eq!(current, immutable);
            let journal = rusqlite::Connection::open(journal_path).unwrap();
            let event_count: u64 = journal
                .query_row(
                    "SELECT COUNT(*) FROM events WHERE kind='OwnedFlowWakeRefused'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            let current_payload: Vec<u8> = journal
                .query_row(
                    "SELECT payload FROM events WHERE seq=?",
                    [original_seq],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(event_count, 1);
            assert_eq!(current_payload, original_payload);
            for (name, previous) in observed.iter().zip(before) {
                assert!(
                    records(name) == previous,
                    "cold informational reclaim emitted physical work"
                );
            }
            assert!(
                file.try_lock().is_ok(),
                "cold delivered/joined owner retained stale original lease"
            );
        }
        root.close();
    }
}

async fn wait_actual_refusal_ack(path: &Path) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let connection = rusqlite::Connection::open(path).unwrap();
            let sequence: Option<u64> = connection
                .query_row(
                    "SELECT delivered_event_seq FROM owned_flow_wake_refusal_outbox",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            if sequence.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
async fn wait_actual_refusal_event(path: &Path) -> (u64, Vec<u8>) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let journal = rusqlite::Connection::open(path).unwrap();
            let mut statement = journal
                .prepare("SELECT seq,payload FROM events WHERE kind='OwnedFlowWakeRefused'")
                .unwrap();
            let mut rows = statement.query([]).unwrap();
            if let Some(row) = rows.next().unwrap() {
                let occurrence = (row.get(0).unwrap(), row.get(1).unwrap());
                assert!(rows.next().unwrap().is_none());
                break occurrence;
            }
            drop(rows);
            drop(statement);
            drop(journal);
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}
