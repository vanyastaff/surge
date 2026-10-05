//! Persistent task controls use the actual daemon, engine and registry.
#[path = "../../surge-orchestrator/tests/fixtures/mock_bridge.rs"]
mod mock_bridge;
use interprocess::local_socket::tokio::prelude::*;
use serde_json::{Value, json};
use std::{path::Path, sync::Arc, time::Duration};
use surge_orchestrator::engine::facade::LocalEngineFacade;
use surge_orchestrator::engine::ipc::local_socket_name_from_path;
use surge_orchestrator::engine::tools::worktree::WorktreeToolDispatcher;
use surge_orchestrator::engine::{Engine, EngineConfig};
use surge_persistence::runs::Storage;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio_util::sync::CancellationToken;

async fn request(socket: &Path, command: Value) -> Value {
    request_frame(
        socket,
        json!({"method":"work_item", "request_id":1,"command":command}),
    )
    .await
}

async fn request_frame(socket: &Path, envelope: Value) -> Value {
    let stream = LocalSocketStream::connect(local_socket_name_from_path(socket).unwrap())
        .await
        .unwrap();
    let (read, mut write) = stream.split();
    let mut frame = serde_json::to_vec(&envelope).unwrap();
    let _: surge_orchestrator::engine::ipc::DaemonRequest = serde_json::from_slice(&frame).unwrap();
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
    assert!(
        !line.is_empty(),
        "daemon must answer durable task controls rather than close an unrecognized request"
    );
    serde_json::from_str(&line).unwrap()
}

async fn attention_reached(
    bridge: &mock_bridge::MockBridge,
    storage: &Storage,
    run: surge_core::RunId,
) -> bool {
    bridge.last_prompt().await.is_some()
        || storage.work_items().for_run(run).unwrap().unwrap().state
            == surge_core::work_item::WorkItemAttemptState::Attention
}

async fn wait_for_attention(
    bridge: &mock_bridge::MockBridge,
    storage: &Storage,
    run: surge_core::RunId,
) -> Result<(), tokio::time::error::Elapsed> {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if attention_reached(bridge, storage, run).await {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
}

async fn wait_for_event_count(conn: &rusqlite::Connection, kind: &str, expected: u64) {
    let query = format!("SELECT COUNT(*) FROM events WHERE kind='{kind}'");
    loop {
        if conn
            .query_row(&query, [], |row| row.get::<_, u64>(0))
            .unwrap()
            >= expected
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn wait_for_human_input(
    conn: &rusqlite::Connection,
    storage: &Storage,
    run: surge_core::RunId,
    index: usize,
) -> (u64, Vec<u8>) {
    loop {
        let rows: Vec<(u64, Vec<u8>)> = conn
            .prepare("SELECT seq,payload FROM events WHERE kind='HumanInputRequested' ORDER BY seq")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        if let Some(row) = rows.get(index) {
            return row.clone();
        }
        if !storage
            .work_items()
            .for_run(run)
            .unwrap()
            .unwrap()
            .state
            .is_active()
        {
            return (0, Vec::new());
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn wait_for_active_run(conn: &rusqlite::Connection, engine: &Engine, run: surge_core::RunId) {
    wait_for_event_count(conn, "HumanInputRequested", 1).await;
    loop {
        if engine
            .snapshot_active_runs()
            .await
            .iter()
            .any(|active| active.run_id == run)
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn daemon_gate_answer_requires_durable_acceptance_before_success() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let (storage, attempt, _, start) =
        reserve_fixture_with_gate(home.path(), project.path(), true, true).await;
    let bridge = Arc::new(wire_bridge::WireBridge {
        bridge: surge_acp::bridge::AcpBridge::with_defaults().unwrap(),
        flags: commit_wire_flags(home.path()),
        session_commit_check: None,
    });
    let (engine, cancel, server, socket) =
        cold_wire_host(home.path(), project.path(), storage.clone(), bridge).await;
    request(&socket, serde_json::to_value(start).unwrap()).await;
    let path = home
        .path()
        .join("runs")
        .join(attempt.run.to_string())
        .join("events.sqlite");
    let conn = rusqlite::Connection::open(path).unwrap();
    let original = tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            let rows: Vec<Vec<u8>> = conn
                .prepare("SELECT payload FROM events WHERE kind='HumanInputRequested'")
                .unwrap()
                .query_map([], |row| row.get(0))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap();
            if let Some(payload) = rows.first() {
                break serde_json::from_slice::<surge_core::VersionedEventPayload>(payload)
                    .unwrap();
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let surge_core::EventPayload::HumanInputRequested {
        node,
        call_id: Some(call_id),
        ..
    } = original.payload()
    else {
        panic!("host gate identity missing")
    };
    let command = surge_orchestrator::engine::ipc::DaemonRequest::ResolveGateInput {
        request_id: 9,
        run_id: attempt.run,
        node: node.clone(),
        gate_request_id: surge_core::id::GateRequestId::from_event_call_id(call_id).unwrap(),
        response: json!({"outcome":"approve","comment":"durable answer"}),
    };
    conn.execute_batch("CREATE TRIGGER fixture_reject_answer BEFORE INSERT ON events WHEN NEW.kind='HumanInputResolved' BEGIN SELECT RAISE(ABORT,'fixture rejects durable answer'); END;").unwrap();
    let rejected = request_frame(&socket, serde_json::to_value(&command).unwrap()).await;
    if rejected["method"] != "error" {
        let _ = engine
            .stop_run(
                attempt.run,
                "fixture cleanup after unsafe answer acknowledgement".into(),
            )
            .await;
        cancel.cancel();
        server.await.unwrap().unwrap();
        panic!("daemon acknowledged an answer whose durable append was rejected: {rejected}");
    }
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM events WHERE kind='HumanInputResolved'",
            [],
            |row| row.get::<_, u64>(0)
        )
        .unwrap(),
        0
    );
    conn.execute_batch("DROP TRIGGER fixture_reject_answer")
        .unwrap();
    assert_eq!(
        request_frame(&socket, serde_json::to_value(&command).unwrap()).await["method"],
        "resolve_human_input_ok"
    );
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM events WHERE kind='HumanInputResolved'",
            [],
            |row| row.get::<_, u64>(0)
        )
        .unwrap(),
        1,
        "application-visible success must already have its exact durable answer"
    );
    tokio::time::timeout(Duration::from_secs(8), async {
        while storage
            .work_items()
            .for_run(attempt.run)
            .unwrap()
            .unwrap()
            .state
            != surge_core::work_item::WorkItemAttemptState::Completed
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    cancel.cancel();
    server.await.unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn task_created_over_daemon_survives_restart_and_runs_pinned_requirements() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    for args in [
        vec!["init"],
        vec!["config", "user.name", "Fixture"],
        vec!["config", "user.email", "fixture@example.com"],
        vec!["commit", "--allow-empty", "-m", "base"],
    ] {
        assert!(
            std::process::Command::new("git")
                .args(args)
                .current_dir(project.path())
                .output()
                .unwrap()
                .status
                .success()
        );
    }
    let storage = Storage::open(home.path()).await.unwrap();
    let bridge = Arc::new(mock_bridge::MockBridge::new());
    let engine = Arc::new(Engine::new(
        bridge.clone(),
        storage.clone(),
        Arc::new(WorktreeToolDispatcher::new(project.path().into())),
        EngineConfig::default(),
    ));
    let socket = home.path().join("task.sock");
    let cancel = CancellationToken::new();
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
    for _ in 0..100 {
        if socket.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let created = request(&socket,json!({"action":"create","operation_id":surge_core::RunId::new(),"project":project.path(),"title":"Durable task","requirements":{"text":"Preserve accepted requirements","criteria":["Requirement survives restart"]}})).await;
    assert_eq!(
        created["method"], "work_item_ok",
        "daemon must create a durable task: {created}"
    );
    let item = created["result"]["value"]["item"]["id"].clone();
    let shown = request(&socket, json!({"action":"show","item":item})).await;
    assert_eq!(
        shown["result"]["value"]["revision"]["requirements"]["text"],
        "Preserve accepted requirements"
    );
    cancel.cancel();
    server.await.unwrap().unwrap();
    drop(engine);
    drop(storage);
    // Reopen the registry and create a genuinely new daemon/engine owner.
    let storage = Storage::open(home.path()).await.unwrap();
    let engine = Arc::new(Engine::new(
        bridge.clone(),
        storage.clone(),
        Arc::new(WorktreeToolDispatcher::new(project.path().into())),
        EngineConfig::default(),
    ));
    let cancel = CancellationToken::new();
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
    // The old socket is unlinked by the new listener; wait for a successful typed query.
    tokio::time::sleep(Duration::from_millis(50)).await;
    let shown = request(&socket, json!({"action":"show","item":item})).await;
    assert_eq!(
        shown["result"]["value"]["revision"]["requirements"]["text"],
        "Preserve accepted requirements"
    );
    let version = shown["result"]["value"]["item"]["version"]
        .as_u64()
        .unwrap();
    let graph: surge_core::Graph =
        toml::from_str(include_str!("../../../examples/flow_minimal_agent.toml")).unwrap();
    let start = json!({"action":"start","operation_id":surge_core::id::WorkItemOperationId::new(),"item":item,"expected_version":version,"graph":graph,"quota_recovery":{"stages":[]}});
    let started = request(&socket, start.clone()).await;
    assert_eq!(started["method"], "work_item_ok", "{started}");
    let run: surge_core::RunId =
        serde_json::from_value(started["result"]["value"]["run"].clone()).unwrap();
    let attempt = storage.work_items().for_run(run).unwrap().unwrap();
    let frozen: surge_orchestrator::engine::EngineRunConfig =
        serde_json::from_str(&attempt.config).unwrap();
    assert!(frozen.quota_recovery.stages().is_empty());
    bridge.wait_for_subscribe_count(1).await;
    let prompt = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Some(prompt) = bridge.last_prompt().await {
                break prompt;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let hash = shown["result"]["value"]["revision"]["hash"]
        .as_str()
        .unwrap();
    assert!(
        prompt.contains("Preserve accepted requirements"),
        "{prompt}"
    );
    assert!(prompt.contains("Requirement survives restart"), "{prompt}");
    assert!(
        prompt.contains("revision 1") && prompt.contains(hash),
        "{prompt}"
    );
    let replay = request(&socket, start).await;
    assert_eq!(
        replay["result"]["value"]["run"],
        started["result"]["value"]["run"]
    );
    let inspected = storage.inspect_run(run).await.unwrap();
    let surge_persistence::runs::inspection::RunDatabaseInspection::Present { events } =
        inspected.database
    else {
        panic!("journal")
    };
    assert!(matches!(
        events[0].payload.payload,
        surge_core::EventPayload::RunStarted { .. }
    ));
    assert!(events.iter().any(|event|matches!(&event.payload.payload,surge_core::EventPayload::WorkItemAttemptBound{context} if context.requirements().unwrap().text()=="Preserve accepted requirements")));
    engine
        .stop_run(run, "acceptance cleanup".into())
        .await
        .unwrap();
    cancel.cancel();
    server.await.unwrap().unwrap();
}

async fn reserve_fixture(
    home: &Path,
    project: &Path,
    agent: bool,
) -> (
    Arc<Storage>,
    surge_core::work_item::WorkItemAttempt,
    surge_core::work_item::WorkItemWorkspace,
    surge_core::work_item::WorkItemCommand,
) {
    reserve_fixture_with_gate(home, project, agent, false).await
}

async fn reserve_fixture_with_gate(
    home: &Path,
    project: &Path,
    agent: bool,
    successor_gate: bool,
) -> (
    Arc<Storage>,
    surge_core::work_item::WorkItemAttempt,
    surge_core::work_item::WorkItemWorkspace,
    surge_core::work_item::WorkItemCommand,
) {
    reserve_fixture_graph(home, project, agent, successor_gate, false).await
}

async fn reserve_fixture_graph(
    home: &Path,
    project: &Path,
    agent: bool,
    successor_gate: bool,
    loop_gate: bool,
) -> (
    Arc<Storage>,
    surge_core::work_item::WorkItemAttempt,
    surge_core::work_item::WorkItemWorkspace,
    surge_core::work_item::WorkItemCommand,
) {
    reserve_fixture_graph_timeout(home, project, agent, successor_gate, loop_gate, None).await
}

async fn reserve_fixture_graph_timeout(
    home: &Path,
    project: &Path,
    agent: bool,
    successor_gate: bool,
    loop_gate: bool,
    timeout_seconds: Option<u32>,
) -> (
    Arc<Storage>,
    surge_core::work_item::WorkItemAttempt,
    surge_core::work_item::WorkItemWorkspace,
    surge_core::work_item::WorkItemCommand,
) {
    use surge_core::{Graph, RunId, id::WorkItemOperationId, work_item::*};
    for args in [
        vec!["init"],
        vec!["config", "user.name", "Fixture"],
        vec!["config", "user.email", "fixture@example.com"],
        vec!["commit", "--allow-empty", "-m", "base"],
    ] {
        assert!(
            std::process::Command::new("git")
                .args(args)
                .current_dir(project)
                .output()
                .unwrap()
                .status
                .success()
        );
    }
    let storage = Storage::open(home).await.unwrap();
    let workspace =
        surge_git::task_workspace::plan(project, &home.join("work-items/workspaces"), RunId::new())
            .unwrap();
    let requirements = WorkItemRequirements::new(
        "Cold accepted context".into(),
        vec!["Same reserved run".into()],
    )
    .unwrap();
    let create = WorkItemCommand::Create {
        operation_id: WorkItemOperationId::new(),
        project: project.into(),
        title: "Cold".into(),
        requirements,
    };
    let WorkItemResult::Detail(created) = storage
        .work_items()
        .mutate(&create, Some(&workspace), None, "human", 1)
        .unwrap()
    else {
        panic!("detail")
    };
    let mut graph: Graph = toml::from_str(if agent {
        include_str!("../../../examples/flow_minimal_agent.toml")
    } else {
        include_str!("../../../examples/flow_terminal_only.toml")
    })
    .unwrap();
    if successor_gate {
        let end = surge_core::NodeKey::try_from("end").unwrap();
        let finish = surge_core::NodeKey::try_from("finish").unwrap();
        let mut terminal = graph.nodes[&end].clone();
        terminal.id = finish.clone();
        graph.nodes.insert(finish.clone(), terminal);
        let template: Graph =
            toml::from_str(include_str!("../../../examples/flow_minimal_agent.toml")).unwrap();
        let mut declared = template.nodes[&template.start].declared_outcomes[0].clone();
        declared.id = surge_core::OutcomeKey::try_from("approve").unwrap();
        let gate = graph.nodes.get_mut(&end).unwrap();
        gate.declared_outcomes = vec![declared];
        gate.config = serde_json::from_value(json!({"node_kind":"human_gate","delivery_channels":[],"summary":{"title":"Continue checkpoint","body":"Operator decision retained"},"options":[{"outcome":"approve","label":"Approve"}],"allow_freetext":true})).unwrap();
        if let Some(timeout) = timeout_seconds {
            let surge_core::NodeConfig::HumanGate(config) = &mut gate.config else {
                panic!("gate");
            };
            config.timeout_seconds = Some(timeout);
        }
        let mut edge = template.edges[0].clone();
        edge.id = surge_core::keys::EdgeKey::try_from("e_gate_to_finish").unwrap();
        edge.from.node = end;
        edge.from.outcome = surge_core::OutcomeKey::try_from("approve").unwrap();
        edge.to = finish;
        graph.edges.push(edge);
        if loop_gate {
            let gate = graph
                .nodes
                .get_mut(&surge_core::NodeKey::try_from("end").unwrap())
                .unwrap();
            let mut edit = gate.declared_outcomes[0].clone();
            edit.id = surge_core::OutcomeKey::try_from("edit").unwrap();
            gate.declared_outcomes.push(edit);
            let surge_core::NodeConfig::HumanGate(config) = &mut gate.config else {
                panic!("gate")
            };
            let mut option = config.options[0].clone();
            option.outcome = surge_core::OutcomeKey::try_from("edit").unwrap();
            option.label = "Edit".into();
            config.options.push(option);
            let mut edge = graph.edges.last().unwrap().clone();
            edge.id = surge_core::keys::EdgeKey::try_from("gate_edit_revisit").unwrap();
            edge.from.outcome = surge_core::OutcomeKey::try_from("edit").unwrap();
            edge.to = edge.from.node.clone();
            edge.kind = surge_core::edge::EdgeKind::Backtrack;
            graph.edges.push(edge);
        }
    }
    let start = WorkItemCommand::Start {
        operation_id: WorkItemOperationId::new(),
        item: created.item.id,
        expected_version: 1,
        graph: Box::new(graph),
        quota_recovery: None,
    };
    let config =
        serde_json::to_string(&surge_orchestrator::engine::EngineRunConfig::default()).unwrap();
    let WorkItemResult::Attempt(attempt) = storage
        .work_items()
        .mutate(&start, None, Some(&config), "host", 2)
        .unwrap()
    else {
        panic!("attempt")
    };
    (storage, *attempt, workspace, start)
}
async fn cold_host(
    home: &Path,
    project: &Path,
    storage: Arc<Storage>,
    bridge: Arc<dyn surge_acp::bridge::BridgeFacade>,
) -> (
    Arc<Engine>,
    CancellationToken,
    tokio::task::JoinHandle<Result<(), surge_daemon::DaemonError>>,
    std::path::PathBuf,
) {
    cold_host_with_config(home, project, storage, bridge, EngineConfig::default()).await
}

async fn cold_wire_host(
    home: &Path,
    project: &Path,
    storage: Arc<Storage>,
    bridge: Arc<wire_bridge::WireBridge>,
) -> (
    Arc<Engine>,
    CancellationToken,
    tokio::task::JoinHandle<Result<(), surge_daemon::DaemonError>>,
    std::path::PathBuf,
) {
    let config = bridge.engine_config(home);
    cold_host_with_config(home, project, storage, bridge, config).await
}

async fn cold_host_with_config(
    home: &Path,
    project: &Path,
    storage: Arc<Storage>,
    bridge: Arc<dyn surge_acp::bridge::BridgeFacade>,
    config: EngineConfig,
) -> (
    Arc<Engine>,
    CancellationToken,
    tokio::task::JoinHandle<Result<(), surge_daemon::DaemonError>>,
    std::path::PathBuf,
) {
    let engine = Arc::new(Engine::new(
        bridge,
        storage.clone(),
        Arc::new(WorktreeToolDispatcher::new(project.into())),
        config,
    ));
    let cancel = CancellationToken::new();
    let socket = home.join("cold.sock");
    let server = tokio::spawn(surge_daemon::run_runs_only(
        surge_daemon::ServerConfig {
            socket_path: socket.clone(),
            max_active: 8,
            max_queue: 2,
        },
        Arc::new(LocalEngineFacade::new(engine.clone())),
        surge_daemon::tracked_run::TrackingContext::new(engine.clone(), storage),
        Arc::new(surge_daemon::broadcast::BroadcastRegistry::new()),
        Arc::new(surge_daemon::admission::AdmissionController::new(8, 2)),
        cancel.clone(),
    ));
    for _ in 0..100 {
        if socket.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    (engine, cancel, server, socket)
}
async fn write_owned_startup(
    storage: &Arc<Storage>,
    attempt: &surge_core::work_item::WorkItemAttempt,
    workspace: &surge_core::work_item::WorkItemWorkspace,
    binding: bool,
    prompt_artifact: bool,
) {
    use surge_core::{
        ContentHash, EventPayload as E, VersionedEventPayload as V, approvals::ApprovalPolicy,
        run_event::RunConfig, sandbox::SandboxMode, work_item::WorkItemContext,
    };
    let claim = storage.work_items().claim(attempt.run).unwrap();
    surge_git::task_workspace::prepare(
        workspace,
        surge_git::run_worktree::ReconcilePhase::BeforeExecution,
    )
    .unwrap();
    storage
        .work_items()
        .mark_workspace_prepared(&claim)
        .unwrap();
    let requirements = storage
        .work_items()
        .requirements(attempt)
        .unwrap()
        .origin
        .requirements()
        .unwrap()
        .clone();
    let context = WorkItemContext::new(attempt.binding.clone(), requirements).unwrap();
    let artifact = surge_persistence::artifacts::ArtifactStore::new(storage.home().join("runs"))
        .put(
            attempt.run,
            "accepted_requirements",
            &serde_json::to_vec(context.requirements().unwrap()).unwrap(),
        )
        .await
        .unwrap();
    let writer = storage
        .create_run(attempt.run, &workspace.path, None)
        .await
        .unwrap();
    let mut events = vec![
        E::RunStarted {
            pipeline_template: None,
            project_path: workspace.path.clone(),
            initial_prompt: context.prompt(),
            config: RunConfig {
                bootstrap_edit_loop_cap: Some(
                    serde_json::from_str::<surge_orchestrator::engine::EngineRunConfig>(
                        &attempt.config,
                    )
                    .unwrap()
                    .bootstrap
                    .edit_loop_cap,
                ),
                sandbox_default: SandboxMode::WorkspaceWrite,
                approval_default: ApprovalPolicy::OnRequest,
                auto_pr: false,
                mcp_servers: vec![],
                budget: Default::default(),
            },
        },
        E::PipelineMaterialized {
            graph: attempt.graph.clone(),
            graph_hash: ContentHash::compute(&serde_json::to_vec(&attempt.graph).unwrap()),
        },
        E::ArtifactProduced {
            node: "task_requirements".parse().unwrap(),
            artifact: artifact.hash,
            path: artifact.path,
            name: "accepted_requirements".into(),
            source_path: None,
        },
    ];
    let prompt = context.prompt();
    std::fs::create_dir_all(workspace.path.join(".surge")).unwrap();
    std::fs::write(workspace.path.join(".surge/user_prompt.txt"), &prompt).unwrap();
    if prompt_artifact {
        events.push(E::ArtifactProduced {
            node: "initial_prompt_seed".parse().unwrap(),
            artifact: ContentHash::compute(prompt.as_bytes()),
            path: ".surge/user_prompt.txt".into(),
            name: "user_prompt".into(),
            source_path: None,
        });
    }
    if binding {
        events.push(E::WorkItemAttemptBound { context });
    }
    writer
        .append_events(events.into_iter().map(V::new).collect())
        .await
        .unwrap();
    writer.close().await.unwrap();
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cold_daemon_resumes_committed_startup_with_same_run_and_accepted_acp_context() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let (storage, attempt, workspace, _start) =
        reserve_fixture(home.path(), project.path(), true).await;
    write_owned_startup(&storage, &attempt, &workspace, true, true).await;
    let bridge = Arc::new(mock_bridge::MockBridge::new());
    let (engine, cancel, server, _socket) =
        cold_host(home.path(), project.path(), storage.clone(), bridge.clone()).await;
    let observed = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if bridge.last_prompt().await.is_some() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await;
    assert!(
        observed.is_ok(),
        "cold resume diagnostic: {:?}; journal: {:?}",
        storage.work_items().for_run(attempt.run).unwrap(),
        storage.inspect_run(attempt.run).await.unwrap()
    );

    let prompt = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Some(value) = bridge.last_prompt().await {
                break value;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(
        prompt.contains("Cold accepted context")
            && prompt.contains("revision 1")
            && prompt.contains(&attempt.binding.requirements_hash.to_string()),
        "{prompt}"
    );
    assert_eq!(
        storage.work_items().show(attempt.item).unwrap().usage.runs,
        1
    );
    let inspected = storage.inspect_run(attempt.run).await.unwrap();
    let surge_persistence::runs::inspection::RunDatabaseInspection::Present { events } =
        inspected.database
    else {
        panic!("history")
    };
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(
                event.payload.payload,
                surge_core::EventPayload::RunStarted { .. }
            ))
            .count(),
        1
    );
    assert_eq!(
        storage
            .work_items()
            .for_run(attempt.run)
            .unwrap()
            .unwrap()
            .state,
        surge_core::work_item::WorkItemAttemptState::Launched
    );
    engine
        .stop_run(attempt.run, "fixture shutdown".into())
        .await
        .unwrap();
    cancel.cancel();
    server.await.unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn legacy_unbound_cap_resumes_generic_task_but_current_missing_cap_retains_attention() {
    for version in [13, 12] {
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let (storage, attempt, workspace, _) =
            reserve_fixture(home.path(), project.path(), true).await;
        write_owned_startup(&storage, &attempt, &workspace, true, true).await;
        let path = home
            .path()
            .join("runs")
            .join(attempt.run.to_string())
            .join("events.sqlite");
        let conn = rusqlite::Connection::open(path).unwrap();
        conn.execute_batch("DROP TRIGGER trg_events_no_update")
            .unwrap();
        let rows: Vec<(u64, Vec<u8>)> = conn
            .prepare("SELECT seq,payload FROM events ORDER BY seq")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        for (seq, bytes) in rows {
            let mut payload: surge_core::VersionedEventPayload =
                serde_json::from_slice(&bytes).unwrap();
            payload.schema_version = version;
            if let surge_core::EventPayload::RunStarted { config, .. } = &mut payload.payload {
                config.bootstrap_edit_loop_cap = None;
            }
            conn.execute(
                "UPDATE events SET payload=?,schema_version=? WHERE seq=?",
                rusqlite::params![serde_json::to_vec(&payload).unwrap(), version, seq],
            )
            .unwrap();
        }
        conn.execute_batch("CREATE TRIGGER trg_events_no_update BEFORE UPDATE ON events BEGIN SELECT RAISE(ABORT, 'events are append-only'); END;").unwrap();
        let bridge = Arc::new(mock_bridge::MockBridge::new());
        let (engine, cancel, server, _) =
            cold_host(home.path(), project.path(), storage.clone(), bridge.clone()).await;
        let observed = wait_for_attention(&bridge, &storage, attempt.run).await;
        assert!(
            observed.is_ok(),
            "version {version}: {:?}",
            storage.work_items().for_run(attempt.run).unwrap()
        );
        let prompted = bridge.last_prompt().await.is_some();
        if prompted {
            engine
                .stop_run(attempt.run, "fixture cleanup".into())
                .await
                .unwrap();
        }
        cancel.cancel();
        server.await.unwrap().unwrap();
        assert_eq!(
            prompted,
            version == 12,
            "legacy generic task must remain executable; current origin must retain immutable cap authority"
        );
        if !prompted {
            let current = storage.work_items().for_run(attempt.run).unwrap().unwrap();
            assert_eq!(
                current.state,
                surge_core::work_item::WorkItemAttemptState::Attention
            );
            assert_eq!(current.binding, attempt.binding);
        }
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cold_daemon_keeps_missing_binding_in_attention_and_never_dispatches() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let (storage, attempt, workspace, _start) =
        reserve_fixture(home.path(), project.path(), true).await;
    write_owned_startup(&storage, &attempt, &workspace, false, true).await;
    let bridge = Arc::new(mock_bridge::MockBridge::new());
    let (_engine, cancel, server, _) =
        cold_host(home.path(), project.path(), storage.clone(), bridge.clone()).await;
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if storage
                .work_items()
                .for_run(attempt.run)
                .unwrap()
                .unwrap()
                .state
                == surge_core::work_item::WorkItemAttemptState::Attention
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(bridge.last_prompt().await.is_none());
    assert_eq!(
        storage
            .work_items()
            .show(attempt.item)
            .unwrap()
            .item
            .active_run,
        Some(attempt.run)
    );
    assert!(workspace.path.exists());
    cancel.cancel();
    server.await.unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lost_reservation_empty_journal_and_provision_before_ack_retry_same_run_once() {
    for boundary in 0..3 {
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let (storage, attempt, workspace, start) =
            reserve_fixture(home.path(), project.path(), false).await;
        if boundary == 1 {
            let writer = storage
                .create_run(attempt.run, &workspace.path, None)
                .await
                .unwrap();
            writer.close().await.unwrap();
        }
        if boundary == 2 {
            surge_git::task_workspace::prepare(
                &workspace,
                surge_git::run_worktree::ReconcilePhase::BeforeExecution,
            )
            .unwrap();
        }
        let bridge = Arc::new(mock_bridge::MockBridge::new());
        let (_engine, cancel, server, socket) =
            cold_host(home.path(), project.path(), storage.clone(), bridge).await;
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert_eq!(
            storage
                .work_items()
                .for_run(attempt.run)
                .unwrap()
                .unwrap()
                .state,
            surge_core::work_item::WorkItemAttemptState::Reserved,
            "unlaunched reservation must require explicit retry"
        );
        let command = serde_json::to_value(&start).unwrap();
        let (first, concurrent) = tokio::join!(
            request(&socket, command.clone()),
            request(&socket, command.clone())
        );
        for response in [&first, &concurrent] {
            if response["method"] == "work_item_ok" {
                assert_eq!(
                    response["result"]["value"]["run"],
                    serde_json::to_value(attempt.run).unwrap()
                );
            } else {
                assert!(
                    response["message"].as_str().unwrap().contains("claimed"),
                    "{response}"
                );
            }
        }
        let replay = request(&socket, command).await;
        assert_eq!(
            replay["result"]["value"]["run"],
            serde_json::to_value(attempt.run).unwrap(),
            "{replay}"
        );
        let inspected = storage.inspect_run(attempt.run).await.unwrap();
        let surge_persistence::runs::inspection::RunDatabaseInspection::Present { events } =
            inspected.database
        else {
            panic!("startup")
        };
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(
                    event.payload.payload,
                    surge_core::EventPayload::RunStarted { .. }
                ))
                .count(),
            1
        );
        assert_eq!(
            storage.work_items().show(attempt.item).unwrap().usage.runs,
            1
        );
        cancel.cancel();
        server.await.unwrap().unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn partial_task_startup_missing_initial_prompt_is_attention_without_dispatch() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let (storage, attempt, workspace, _start) =
        reserve_fixture(home.path(), project.path(), true).await;
    write_owned_startup(&storage, &attempt, &workspace, true, false).await;
    let bridge = Arc::new(mock_bridge::MockBridge::new());
    let (_engine, cancel, server, _) =
        cold_host(home.path(), project.path(), storage.clone(), bridge.clone()).await;
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if storage
                .work_items()
                .for_run(attempt.run)
                .unwrap()
                .unwrap()
                .state
                == surge_core::work_item::WorkItemAttemptState::Attention
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(bridge.last_prompt().await.is_none());
    assert!(
        storage
            .work_items()
            .for_run(attempt.run)
            .unwrap()
            .unwrap()
            .diagnostic
            .unwrap()
            .contains("user_prompt")
    );
    assert_eq!(
        storage
            .work_items()
            .show(attempt.item)
            .unwrap()
            .item
            .active_run,
        Some(attempt.run)
    );
    cancel.cancel();
    server.await.unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn disconnected_start_caller_does_not_drop_durable_attempt_or_active_run() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let (storage, attempt, _workspace, start) =
        reserve_fixture(home.path(), project.path(), true).await;
    let bridge = Arc::new(mock_bridge::MockBridge::new());
    let (engine, cancel, server, socket) =
        cold_host(home.path(), project.path(), storage.clone(), bridge.clone()).await;
    let mut stream = LocalSocketStream::connect(local_socket_name_from_path(&socket).unwrap())
        .await
        .unwrap();
    let mut frame =
        serde_json::to_vec(&json!({"method":"work_item","request_id":9,"command":start})).unwrap();
    frame.push(b'\n');
    stream.write_all(&frame).await.unwrap();
    drop(stream);
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if bridge.last_prompt().await.is_some() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        storage
            .work_items()
            .show(attempt.item)
            .unwrap()
            .item
            .active_run,
        Some(attempt.run)
    );
    assert!(
        engine
            .snapshot_active_runs()
            .await
            .iter()
            .any(|entry| entry.run_id == attempt.run)
    );
    assert_eq!(
        storage.work_items().show(attempt.item).unwrap().usage.runs,
        1
    );
    engine
        .stop_run(attempt.run, "fixture cleanup".into())
        .await
        .unwrap();
    cancel.cancel();
    server.await.unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn durable_terminal_settlement_validates_frozen_requirements_before_releasing_ownership() {
    for corrupt in [false, true] {
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let (storage, attempt, workspace, _start) =
            reserve_fixture(home.path(), project.path(), true).await;
        write_owned_startup(&storage, &attempt, &workspace, true, true).await;
        let writer = storage.open_run_writer(attempt.run).await.unwrap();
        writer
            .append_event(surge_core::VersionedEventPayload::new(
                surge_core::EventPayload::RunFailed {
                    error: "Execution failed after committed startup".into(),
                },
            ))
            .await
            .unwrap();
        writer.close().await.unwrap();
        if corrupt {
            let inspection = storage.inspect_folded_run(attempt.run).await.unwrap();
            let path = inspection
                .database
                .unwrap()
                .startup
                .into_iter()
                .find_map(|row| match row.payload.payload {
                    surge_core::EventPayload::ArtifactProduced { name, path, .. }
                        if name == "accepted_requirements" =>
                    {
                        Some(path)
                    },
                    _ => None,
                })
                .unwrap();
            std::fs::write(path, b"tampered immutable requirement artifact").unwrap();
        }
        let bridge = Arc::new(mock_bridge::MockBridge::new());
        let (_engine, cancel, server, _) =
            cold_host(home.path(), project.path(), storage.clone(), bridge.clone()).await;
        let expected = if corrupt {
            surge_core::work_item::WorkItemAttemptState::Attention
        } else {
            surge_core::work_item::WorkItemAttemptState::Failed
        };
        wait_for_attempt_state(&storage, attempt.run, expected).await;
        assert!(bridge.last_prompt().await.is_none());
        assert_eq!(
            storage
                .work_items()
                .show(attempt.item)
                .unwrap()
                .item
                .active_run,
            if corrupt { Some(attempt.run) } else { None }
        );
        assert!(workspace.path.exists());
        cancel.cancel();
        server.await.unwrap().unwrap();
    }
}

async fn wait_for_attempt_state(
    storage: &Storage,
    run: surge_core::RunId,
    expected: surge_core::work_item::WorkItemAttemptState,
) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while storage.work_items().for_run(run).unwrap().unwrap().state != expected {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

async fn wait_for_attempt_transition(
    storage: &Storage,
    run: surge_core::RunId,
) -> surge_core::work_item::WorkItemAttempt {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let row = storage.work_items().for_run(run).unwrap().unwrap();
            if row.state != surge_core::work_item::WorkItemAttemptState::Reserved {
                return row;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap()
}

/// Deliberately damage only an isolated fixture journal, retaining valid payloads.
async fn assert_malformed_owned_startup_is_attention(kind: &str) {
    use surge_core::{EventPayload as E, VersionedEventPayload as V};
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let (storage, attempt, workspace, _) = reserve_fixture(home.path(), project.path(), true).await;
    write_owned_startup(&storage, &attempt, &workspace, true, true).await;
    let inspection = storage.inspect_run(attempt.run).await.unwrap();
    let surge_persistence::runs::inspection::RunDatabaseInspection::Present { events } =
        inspection.database
    else {
        panic!("fixture history")
    };
    let mut payloads: Vec<V> = events.into_iter().map(|event| event.payload).collect();
    match kind {
        "graph" => {
            let E::PipelineMaterialized { graph, .. } = &mut payloads[1].payload else {
                panic!("graph")
            };
            **graph =
                toml::from_str(include_str!("../../../examples/flow_terminal_only.toml")).unwrap();
        },
        "binding-before-terminal" | "binding-after-terminal" => {
            let binding = payloads.pop().unwrap();
            payloads.push(V::new(E::StageEntered {
                node: attempt.graph.start.clone(),
                attempt: 1,
            }));
            if kind == "binding-after-terminal" {
                payloads.push(V::new(E::RunFailed {
                    error: "Execution occurred before ownership binding".into(),
                }));
            }
            payloads.push(binding);
            if kind == "binding-before-terminal" {
                payloads.push(V::new(E::RunFailed {
                    error: "Lifecycle result follows the late binding".into(),
                }));
            }
        },
        "requirements" | "prompt" => {
            let name = if kind == "requirements" {
                "accepted_requirements"
            } else {
                "user_prompt"
            };
            let index = payloads.iter().position(|row| matches!(&row.payload,E::ArtifactProduced { name: found, .. } if found == name)).unwrap();
            let artifact = payloads.remove(index);
            payloads.push(V::new(E::StageEntered {
                node: attempt.graph.start.clone(),
                attempt: 1,
            }));
            payloads.push(artifact);
        },
        _ => panic!("unknown fixture case"),
    }
    let mut connection = rusqlite::Connection::open(
        home.path()
            .join("runs")
            .join(attempt.run.to_string())
            .join("events.sqlite"),
    )
    .unwrap();
    // Corruption simulation removes fixture-only guards; production remains append-only.
    connection
        .execute_batch("DROP TRIGGER trg_events_no_update; DROP TRIGGER trg_events_no_delete;")
        .unwrap();
    let transaction = connection.transaction().unwrap();
    transaction.execute("DELETE FROM events", []).unwrap();
    for (index, payload) in payloads.iter().enumerate() {
        transaction
            .execute(
                "INSERT INTO events(seq,timestamp,kind,payload,schema_version) VALUES(?,?,?,?,?)",
                rusqlite::params![
                    index + 1,
                    1,
                    payload.payload.discriminant_str(),
                    serde_json::to_vec(payload).unwrap(),
                    payload.schema_version
                ],
            )
            .unwrap();
    }
    transaction.commit().unwrap();
    drop(connection);
    let bridge = Arc::new(mock_bridge::MockBridge::new());
    let (engine, cancel, server, _) =
        cold_host(home.path(), project.path(), storage.clone(), bridge.clone()).await;
    let observed = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let value = storage.work_items().for_run(attempt.run).unwrap().unwrap();
            if value.state != surge_core::work_item::WorkItemAttemptState::Reserved {
                break value;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    // Clean up even the unsafe pre-fix dispatch, so the RED does not leak a run.
    if engine
        .snapshot_active_runs()
        .await
        .iter()
        .any(|entry| entry.run_id == attempt.run)
    {
        engine
            .stop_run(attempt.run, "fixture cleanup".into())
            .await
            .unwrap();
    }
    cancel.cancel();
    server.await.unwrap().unwrap();
    assert_eq!(
        observed.state,
        surge_core::work_item::WorkItemAttemptState::Attention,
        "malformed {kind} startup must not authorize recovery: {observed:?}"
    );
    assert!(
        bridge.last_prompt().await.is_none(),
        "malformed startup dispatched an agent"
    );
    assert_eq!(
        storage
            .work_items()
            .show(attempt.item)
            .unwrap()
            .item
            .active_run,
        Some(attempt.run)
    );
    assert!(workspace.path.exists());
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn corrupt_owned_startup_graph_payload_cannot_reuse_declared_hash() {
    assert_malformed_owned_startup_is_attention("graph").await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn corrupt_owned_startup_late_binding_before_terminal_is_attention() {
    assert_malformed_owned_startup_is_attention("binding-before-terminal").await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn corrupt_owned_startup_late_binding_after_terminal_keeps_ownership() {
    assert_malformed_owned_startup_is_attention("binding-after-terminal").await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn corrupt_owned_startup_late_requirements_artifact_is_attention() {
    assert_malformed_owned_startup_is_attention("requirements").await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn corrupt_owned_startup_late_prompt_artifact_is_attention() {
    assert_malformed_owned_startup_is_attention("prompt").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn execution_owner_prevents_second_live_host_from_replaying_agent_turn() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let (storage, attempt, _, start) = reserve_fixture(home.path(), project.path(), true).await;
    let first_bridge = Arc::new(mock_bridge::MockBridge::new());
    let (first, first_cancel, first_server, first_socket) = cold_host(
        home.path(),
        project.path(),
        storage.clone(),
        first_bridge.clone(),
    )
    .await;
    request(&first_socket, serde_json::to_value(start).unwrap()).await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while first_bridge.last_prompt().await.is_none() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let second_bridge = Arc::new(mock_bridge::MockBridge::new());
    let (second, second_cancel, second_server, _) = cold_host(
        home.path(),
        project.path(),
        storage.clone(),
        second_bridge.clone(),
    )
    .await;
    tokio::time::sleep(Duration::from_millis(250)).await;
    let duplicate = second_bridge.last_prompt().await.is_some();
    let live_state = storage
        .work_items()
        .for_run(attempt.run)
        .unwrap()
        .unwrap()
        .state;
    if second
        .snapshot_active_runs()
        .await
        .iter()
        .any(|entry| entry.run_id == attempt.run)
    {
        second
            .stop_run(attempt.run, "fixture cleanup".into())
            .await
            .unwrap();
    }
    first
        .stop_run(attempt.run, "fixture cleanup".into())
        .await
        .unwrap();
    first_cancel.cancel();
    second_cancel.cancel();
    first_server.await.unwrap().unwrap();
    second_server.await.unwrap().unwrap();
    assert!(
        !duplicate,
        "a distinct live Engine resumed an already executing task after start returned"
    );
    assert_eq!(
        live_state,
        surge_core::work_item::WorkItemAttemptState::Launched,
        "another host displaced healthy live execution ownership"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ambiguous_owned_terminal_history_never_releases_task_ownership() {
    for duplicate in [false, true] {
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let (storage, attempt, workspace, _) =
            reserve_fixture(home.path(), project.path(), true).await;
        write_owned_startup(&storage, &attempt, &workspace, true, true).await;
        let writer = storage.open_run_writer(attempt.run).await.unwrap();
        let completed = surge_core::EventPayload::RunCompleted {
            terminal_node: "end".parse().unwrap(),
        };
        writer
            .append_events(vec![
                surge_core::VersionedEventPayload::new(completed.clone()),
                surge_core::VersionedEventPayload::new(if duplicate {
                    completed
                } else {
                    surge_core::EventPayload::RunFailed {
                        error: "Conflicting definitive evidence".into(),
                    }
                }),
            ])
            .await
            .unwrap();
        writer.close().await.unwrap();
        let bridge = Arc::new(mock_bridge::MockBridge::new());
        let (_, cancel, server, _) =
            cold_host(home.path(), project.path(), storage.clone(), bridge.clone()).await;
        let observed = wait_for_attempt_transition(&storage, attempt.run).await;
        cancel.cancel();
        server.await.unwrap().unwrap();
        assert_eq!(
            observed.state,
            surge_core::work_item::WorkItemAttemptState::Attention,
            "ambiguous definitive terminal evidence was trusted"
        );
        assert_eq!(
            storage
                .work_items()
                .show(attempt.item)
                .unwrap()
                .item
                .active_run,
            Some(attempt.run)
        );
        assert!(bridge.last_prompt().await.is_none());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn task_pr_cannot_attach_another_repository_or_unknown_remote() {
    use surge_core::{
        id::WorkItemOperationId,
        work_item::{WorkItemCommand as C, WorkItemPr},
    };
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let (storage, attempt, _, _) = reserve_fixture(home.path(), project.path(), false).await;
    let (_, cancel, server, socket) = cold_host(
        home.path(),
        project.path(),
        storage.clone(),
        Arc::new(mock_bridge::MockBridge::new()),
    )
    .await;
    let attach = |repository: &str, number: u64| C::AttachPr {
        operation_id: WorkItemOperationId::new(),
        item: attempt.item,
        expected_version: storage
            .work_items()
            .show(attempt.item)
            .unwrap()
            .item
            .version,
        pr: WorkItemPr {
            provider: "github".into(),
            repository: repository.into(),
            number,
            url: format!("https://github.com/{repository}/pull/{number}"),
        },
    };
    let unknown = request(
        &socket,
        serde_json::to_value(attach("fixture/repo", 31)).unwrap(),
    )
    .await;
    cancel.cancel();
    server.await.unwrap().unwrap();
    assert_ne!(
        unknown["method"], "work_item_ok",
        "unknown project remote accepted PR: {unknown}"
    );
    assert!(
        storage
            .work_items()
            .show(attempt.item)
            .unwrap()
            .pr
            .is_none()
    );
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn task_pr_normalized_remote_mapping_replay_and_rejection_preserve_association() {
    use surge_core::{
        id::WorkItemOperationId,
        work_item::{WorkItemCommand as C, WorkItemPr},
    };
    for origin in [
        "git@github.com:fixture/repo.git",
        "https://github.com/fixture/repo.git",
        "ssh://git@github.com/fixture/repo.git",
        "https://github.com/fork/repo.git",
    ] {
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let (storage, attempt, _, _) = reserve_fixture(home.path(), project.path(), false).await;
        assert!(
            std::process::Command::new("git")
                .args(["remote", "add", "origin", origin])
                .current_dir(project.path())
                .output()
                .unwrap()
                .status
                .success()
        );
        if origin.contains("fork/") {
            assert!(
                std::process::Command::new("git")
                    .args([
                        "remote",
                        "add",
                        "upstream",
                        "git@github.com:fixture/repo.git"
                    ])
                    .current_dir(project.path())
                    .output()
                    .unwrap()
                    .status
                    .success()
            );
        }
        let (_, cancel, server, socket) = cold_host(
            home.path(),
            project.path(),
            storage.clone(),
            Arc::new(mock_bridge::MockBridge::new()),
        )
        .await;
        let attach = |repository: &str, number: u64| C::AttachPr {
            operation_id: WorkItemOperationId::new(),
            item: attempt.item,
            expected_version: storage
                .work_items()
                .show(attempt.item)
                .unwrap()
                .item
                .version,
            pr: WorkItemPr {
                provider: "github".into(),
                repository: repository.into(),
                number,
                url: format!("https://github.com/{repository}/pull/{number}"),
            },
        };
        let wrong = request(
            &socket,
            serde_json::to_value(attach("other/repository", 31)).unwrap(),
        )
        .await;
        assert_ne!(
            wrong["method"], "work_item_ok",
            "foreign PR accepted: {wrong}"
        );
        let valid = attach("fixture/repo", 32);
        let accepted = request(&socket, serde_json::to_value(&valid).unwrap()).await;
        assert_eq!(accepted["method"], "work_item_ok", "{accepted}");
        for name in ["origin", "upstream"] {
            let _ = std::process::Command::new("git")
                .args(["remote", "remove", name])
                .current_dir(project.path())
                .output()
                .unwrap();
        }
        let replay = request(&socket, serde_json::to_value(valid).unwrap()).await;
        assert_eq!(accepted, replay);
        let rejected = request(
            &socket,
            serde_json::to_value(attach("other/repository", 33)).unwrap(),
        )
        .await;
        assert_ne!(rejected["method"], "work_item_ok");
        assert_eq!(
            storage
                .work_items()
                .show(attempt.item)
                .unwrap()
                .pr
                .unwrap()
                .number,
            32
        );
        cancel.cancel();
        server.await.unwrap().unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_tracking_error_with_conflicting_terminals_retains_task_ownership() {
    use surge_core::{EventPayload as E, VersionedEventPayload as V};
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let (storage, attempt, _, start) = reserve_fixture(home.path(), project.path(), true).await;
    let bridge = Arc::new(mock_bridge::MockBridge::new());
    let (engine, cancel, server, socket) =
        cold_host(home.path(), project.path(), storage.clone(), bridge.clone()).await;
    request(&socket, serde_json::to_value(start).unwrap()).await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while bridge.last_prompt().await.is_none() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let mut conn = rusqlite::Connection::open(
        home.path()
            .join("runs")
            .join(attempt.run.to_string())
            .join("events.sqlite"),
    )
    .unwrap();
    let tx = conn.transaction().unwrap();
    let last: u64 = tx
        .query_row("SELECT MAX(seq) FROM events", [], |row| row.get(0))
        .unwrap();
    for (offset, payload) in [
        E::RunCompleted {
            terminal_node: "end".parse().unwrap(),
        },
        E::RunFailed {
            error: "Conflicting completion evidence".into(),
        },
    ]
    .into_iter()
    .enumerate()
    {
        let wrapped = V::new(payload);
        tx.execute(
            "INSERT INTO events(seq,timestamp,kind,payload,schema_version) VALUES(?,?,?,?,?)",
            rusqlite::params![
                last + offset as u64 + 1,
                1,
                wrapped.payload.discriminant_str(),
                serde_json::to_vec(&wrapped).unwrap(),
                wrapped.schema_version
            ],
        )
        .unwrap();
    }
    tx.commit().unwrap();
    drop(conn);
    engine
        .stop_run(attempt.run, "finish tracking validation".into())
        .await
        .unwrap();
    let observed = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let value = storage.work_items().for_run(attempt.run).unwrap().unwrap();
            if value.state != surge_core::work_item::WorkItemAttemptState::Launched {
                break value;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    cancel.cancel();
    server.await.unwrap().unwrap();
    assert_eq!(
        observed.state,
        surge_core::work_item::WorkItemAttemptState::Attention,
        "failed tracking confirmation was settled as a successful definitive terminal"
    );
    assert_eq!(
        storage
            .work_items()
            .show(attempt.item)
            .unwrap()
            .item
            .active_run,
        Some(attempt.run)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn child_execution_host_probe() {
    let Some(home) = std::env::var_os("SURGE_TEST_EXECUTION_HOME") else {
        return;
    };
    let home = std::path::PathBuf::from(home);
    let project =
        std::path::PathBuf::from(std::env::var_os("SURGE_TEST_EXECUTION_PROJECT").unwrap());
    let storage = Storage::open(&home).await.unwrap();
    let bridge = Arc::new(mock_bridge::MockBridge::new());
    let (_engine, _cancel, _server, socket) =
        cold_host(&home, &project, storage, bridge.clone()).await;
    let start: Value =
        serde_json::from_slice(&std::fs::read(home.join("child-start.json")).unwrap()).unwrap();
    let response = request(&socket, start).await;
    assert_eq!(response["method"], "work_item_ok", "{response}");
    tokio::time::timeout(Duration::from_secs(5), async {
        while bridge.last_prompt().await.is_none() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    std::fs::write(home.join("child-execution-ready"), b"ready").unwrap();
    std::future::pending::<()>().await;
}

fn commit_wire_flags(home: &Path) -> Vec<String> {
    vec![
        "--stage-mcp".into(),
        "--session-store".into(),
        home.join("commit-provider-sessions.json")
            .display()
            .to_string(),
        "--wire-log".into(),
        home.join("commit-provider-wire.jsonl")
            .display()
            .to_string(),
        "--pid-file".into(),
        home.join("commit-provider.pid").display().to_string(),
    ]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn child_committed_outcome_host_probe() {
    let Some(home) = std::env::var_os("SURGE_TEST_COMMIT_HOME") else {
        return;
    };
    let home = std::path::PathBuf::from(home);
    let project = std::path::PathBuf::from(std::env::var_os("SURGE_TEST_COMMIT_PROJECT").unwrap());
    let storage = Storage::open(&home).await.unwrap();
    let bridge = Arc::new(wire_bridge::WireBridge {
        bridge: surge_acp::bridge::AcpBridge::with_defaults().unwrap(),
        flags: commit_wire_flags(&home),
        session_commit_check: None,
    });
    let (_engine, _cancel, _server, socket) =
        cold_wire_host(&home, &project, storage, bridge).await;
    let start: Value =
        serde_json::from_slice(&std::fs::read(home.join("child-start.json")).unwrap()).unwrap();
    let response = request(&socket, start).await;
    assert_eq!(response["method"], "work_item_ok", "{response}");
    std::future::pending::<()>().await;
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn continued_accepted_route_survives_rejected_control_ack_without_second_route() {
    committed_continue_fixture(None, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn killed_continue_owner_after_route_before_journal_ack_routes_once() {
    committed_continue_fixture(Some(false), false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn killed_continue_owner_after_journal_ack_before_registry_ack_routes_once() {
    committed_continue_fixture(Some(true), false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn killed_continue_ack_reconciles_nonterminal_control_before_waiting_for_operator() {
    committed_continue_fixture(Some(true), true).await;
}

// This fixture owns both subprocesses, including when an assertion unwinds.
struct ContinueFixtureChild(std::process::Child);

impl std::ops::Deref for ContinueFixtureChild {
    type Target = std::process::Child;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for ContinueFixtureChild {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Drop for ContinueFixtureChild {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

struct ContinueFixtureDirectory(Option<tempfile::TempDir>);

impl ContinueFixtureDirectory {
    fn new() -> Self {
        Self(Some(tempfile::tempdir().unwrap()))
    }

    fn path(&self) -> &std::path::Path {
        self.0.as_ref().unwrap().path()
    }
}

impl Drop for ContinueFixtureDirectory {
    fn drop(&mut self) {
        if std::thread::panicking()
            && let Some(directory) = self.0.take()
        {
            // Logs and durable journals remain isolated here for failure triage.
            eprintln!(
                "retained Continue fixture diagnostics: {}",
                directory.keep().display()
            );
        }
    }
}

fn continue_fixture_prefix(
    conn: &rusqlite::Connection,
) -> Result<Vec<(u64, String)>, rusqlite::Error> {
    let mut statement = conn.prepare("SELECT seq, kind FROM events ORDER BY seq LIMIT 128")?;
    statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect()
}

async fn wait_for_continue_confirmation(
    storage: &Arc<Storage>,
    run: surge_core::RunId,
    generation: u64,
    operation: surge_core::id::WorkItemOperationId,
    attempt_generation: u64,
) {
    // Engine activity precedes the supervisor's durable Continue acknowledgement.
    loop {
        let confirmed = storage
            .work_items()
            .execution_control(run)
            .unwrap()
            .is_some_and(|control| {
                control.state == surge_core::execution_recovery::ExecutionControlState::Executing
                    && control.generation == generation
                    && control.operation == operation
                    && control.attempt_generation == attempt_generation
            });
        if confirmed {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn committed_continue_fixture(crash_ack: Option<bool>, successor_gate: bool) {
    use surge_core::id::WorkItemOperationId;
    use surge_core::work_item::WorkItemCommand;
    let home = ContinueFixtureDirectory::new();
    let project = ContinueFixtureDirectory::new();
    let (storage, attempt, _, start) =
        reserve_fixture_with_gate(home.path(), project.path(), true, successor_gate).await;
    std::fs::write(
        home.path().join("child-start.json"),
        serde_json::to_vec(&start).unwrap(),
    )
    .unwrap();
    let barrier = home.path().join("control-close-barrier");
    let child_log = home.path().join("control-child.log");
    let child_output = std::fs::File::create(&child_log).unwrap();
    let mut child = ContinueFixtureChild(
        std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "child_committed_outcome_host_probe",
                "--nocapture",
            ])
            .env("SURGE_TEST_COMMIT_HOME", home.path())
            .env("SURGE_TEST_COMMIT_PROJECT", project.path())
            .env("SURGE_TEST_CLOSE_BARRIER", &barrier)
            .stdout(child_output.try_clone().unwrap())
            .stderr(child_output)
            .spawn()
            .unwrap(),
    );
    let ready = tokio::time::timeout(Duration::from_secs(8), async {
        while !barrier.with_extension("ready").exists() {
            if let Some(status) = child.try_wait().unwrap() {
                panic!(
                    "owned child exited {status}; retained log={}",
                    child_log.display()
                );
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    if ready.is_err() {
        let _ = child.kill();
        let _ = child.wait();
        panic!(
            "owned child did not reach close barrier; retained log={}",
            child_log.display()
        );
    }
    let command = WorkItemCommand::Suspend {
        operation_id: WorkItemOperationId::new(),
        item: attempt.item,
        expected_version: storage
            .work_items()
            .show(attempt.item)
            .unwrap()
            .item
            .version,
    };
    let socket = home.path().join("cold.sock");
    let request_socket = socket.clone();
    let pending = tokio::spawn(async move {
        request(&request_socket, serde_json::to_value(command).unwrap()).await
    });
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            if storage
                .work_items()
                .execution_control(attempt.run)
                .unwrap()
                .is_some_and(|control| {
                    control.state
                        == surge_core::execution_recovery::ExecutionControlState::SuspendRequested
                })
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    std::fs::write(&barrier, b"release").unwrap();
    assert_eq!(pending.await.unwrap()["method"], "work_item_ok");
    let paused = storage
        .work_items()
        .execution_control(attempt.run)
        .unwrap()
        .unwrap();
    assert_eq!(
        paused.state,
        surge_core::execution_recovery::ExecutionControlState::Suspended
    );
    assert!(matches!(
        paused.fence.as_ref().unwrap().pending_stage,
        surge_core::execution_recovery::PendingStagePhase::CommittedOutcomeAwaitingRoute {
            invocation: Some(_),
            ..
        }
    ));
    let _ = child.kill();
    let _ = child.wait();
    let path = home
        .path()
        .join("runs")
        .join(attempt.run.to_string())
        .join("events.sqlite");
    let conn = rusqlite::Connection::open(&path).unwrap();
    if let Some(after_journal_ack) = crash_ack {
        // The fixture's SQLite computation exposes the real committed route /
        // uncommitted continuation interval; assertions observe journal facts.
        conn.execute_batch("CREATE TRIGGER fixture_ack_interval BEFORE INSERT ON events WHEN NEW.kind='RunContinued' BEGIN SELECT sum(x) FROM (WITH RECURSIVE delay(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM delay WHERE x<5000000) SELECT x FROM delay); END;").unwrap();
        let continue_operation = WorkItemOperationId::new();
        let continue_command = WorkItemCommand::Continue {
            operation_id: continue_operation,
            item: attempt.item,
            expected_version: storage
                .work_items()
                .show(attempt.item)
                .unwrap()
                .item
                .version,
            new_session: false,
        };
        std::fs::write(
            home.path().join("child-start.json"),
            serde_json::to_vec(&continue_command).unwrap(),
        )
        .unwrap();
        let stale_socket = home.path().join("cold.sock");
        if stale_socket.exists() {
            std::fs::remove_file(&stale_socket).unwrap();
        }
        let diagnostic = std::fs::File::create(home.path().join("continue-child.log")).unwrap();
        let mut owner = ContinueFixtureChild(
            std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "child_committed_outcome_host_probe",
                    "--nocapture",
                ])
                .env("SURGE_TEST_COMMIT_HOME", home.path())
                .env("SURGE_TEST_COMMIT_PROJECT", project.path())
                .stdout(diagnostic.try_clone().unwrap())
                .stderr(diagnostic)
                .spawn()
                .unwrap(),
        );
        let cut = tokio::time::timeout(Duration::from_secs(8), async {
            loop {
                let routes: u64 = conn
                    .query_row(
                        "SELECT COUNT(*) FROM events WHERE kind='StageRouteCommitted'",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap();
                let acknowledgements: u64 = conn
                    .query_row(
                        "SELECT COUNT(*) FROM events WHERE kind='RunContinued'",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap();
                if routes == 1 {
                    assert_eq!(
                        acknowledgements, 0,
                        "fixture failed to expose the pre-ack crash interval"
                    );
                    assert!(owner.try_wait().unwrap().is_none());
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        let registry_barrier = if cut.is_ok() && after_journal_ack {
            let registry = rusqlite::Connection::open(storage.registry_db_path()).unwrap();
            registry.execute_batch("BEGIN IMMEDIATE").unwrap();
            let continued = tokio::time::timeout(
                Duration::from_secs(8),
                wait_for_event_count(&conn, "RunContinued", 1),
            )
            .await;
            assert!(
                continued.is_ok(),
                "Continue acknowledgement timed out; journal prefix={:?}",
                continue_fixture_prefix(&conn)
            );
            if successor_gate {
                let gate = tokio::time::timeout(
                    Duration::from_secs(8),
                    wait_for_event_count(&conn, "HumanInputRequested", 1),
                )
                .await;
                assert!(
                    gate.is_ok(),
                    "Continue successor gate timed out; journal prefix={:?}",
                    continue_fixture_prefix(&conn)
                );
            }
            Some(registry)
        } else {
            None
        };
        let _ = owner.kill();
        let _ = owner.wait();
        drop(registry_barrier);
        assert!(
            cut.is_ok(),
            "Continue owner did not reach its real post-route/pre-ack journal cut; retained log={}; journal prefix={:?}",
            home.path().join("continue-child.log").display(),
            continue_fixture_prefix(&conn)
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM events WHERE kind='RunContinued'",
                [],
                |row| row.get::<_, u64>(0)
            )
            .unwrap(),
            u64::from(after_journal_ack)
        );
        conn.execute_batch("DROP TRIGGER fixture_ack_interval")
            .unwrap();
        let bridge = Arc::new(wire_bridge::WireBridge {
            bridge: surge_acp::bridge::AcpBridge::with_defaults().unwrap(),
            flags: commit_wire_flags(home.path()),
            session_commit_check: None,
        });
        let (recovered_engine, cancel, server, socket) =
            cold_wire_host(home.path(), project.path(), storage.clone(), bridge).await;
        if successor_gate {
            tokio::time::timeout(Duration::from_secs(8), async {
                wait_for_active_run(&conn, &recovered_engine, attempt.run).await;
                wait_for_continue_confirmation(
                    &storage,
                    attempt.run,
                    paused.generation + 1,
                    continue_operation,
                    attempt.binding.generation,
                )
                .await;
            })
            .await
            .unwrap();
            let control = storage
                .work_items()
                .execution_control(attempt.run)
                .unwrap()
                .unwrap();
            assert_eq!(
                control.state,
                surge_core::execution_recovery::ExecutionControlState::Executing,
                "durable nonterminal continuation must reconcile registry authorization"
            );
            assert_eq!(control.generation, paused.generation + 1);
            assert_eq!(
                conn.query_row(
                    "SELECT COUNT(*) FROM events WHERE kind='RunContinued'",
                    [],
                    |row| row.get::<_, u64>(0)
                )
                .unwrap(),
                1
            );
            assert_eq!(
                conn.query_row(
                    "SELECT COUNT(*) FROM events WHERE kind='StageRouteCommitted'",
                    [],
                    |row| row.get::<_, u64>(0)
                )
                .unwrap(),
                1
            );
            assert_eq!(conn.query_row("SELECT COUNT(*) FROM events WHERE kind IN ('RunCompleted','RunFailed','RunAborted')", [], |row| row.get::<_,u64>(0)).unwrap(), 0);
            let original: Vec<u8> = conn.query_row("SELECT payload FROM events WHERE kind='HumanInputRequested' ORDER BY seq LIMIT 1", [], |row|row.get(0)).unwrap();
            let original: surge_core::VersionedEventPayload =
                serde_json::from_slice(&original).unwrap();
            let surge_core::EventPayload::HumanInputRequested {
                node,
                call_id: Some(call_id),
                ..
            } = original.payload()
            else {
                panic!("exact original gate identity absent")
            };
            let request_id = surge_core::id::GateRequestId::from_event_call_id(call_id).unwrap();
            tokio::time::timeout(Duration::from_secs(8), async {
                loop {
                    assert_eq!(conn.query_row("SELECT COUNT(*) FROM events WHERE kind='HumanInputRequested'", [], |row|row.get::<_,u64>(0)).unwrap(), 1, "recovery must reuse the original decision request rather than emit a replacement");
                    match recovered_engine.resolve_gate_input(attempt.run, node.clone(), request_id, json!({"outcome":"approve"})).await {
                        Ok(()) => break,
                        Err(surge_orchestrator::engine::EngineError::StaleGateRequest) => tokio::time::sleep(Duration::from_millis(10)).await,
                        Err(error) => panic!("original gate identity cannot resolve: {error}"),
                    }
                }
            }).await.unwrap();
            assert_eq!(
                request(&socket, serde_json::to_value(&continue_command).unwrap()).await["method"],
                "work_item_ok"
            );
            tokio::time::timeout(Duration::from_secs(8), async {
                while storage
                    .work_items()
                    .for_run(attempt.run)
                    .unwrap()
                    .unwrap()
                    .state
                    != surge_core::work_item::WorkItemAttemptState::Completed
                {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
            assert_eq!(
                conn.query_row(
                    "SELECT COUNT(*) FROM events WHERE kind='HumanInputRequested'",
                    [],
                    |row| row.get::<_, u64>(0)
                )
                .unwrap(),
                1
            );
            assert_eq!(
                conn.query_row(
                    "SELECT COUNT(*) FROM events WHERE kind='HumanInputResolved'",
                    [],
                    |row| row.get::<_, u64>(0)
                )
                .unwrap(),
                1
            );
            let wire: Vec<Value> =
                std::fs::read_to_string(home.path().join("commit-provider-wire.jsonl"))
                    .unwrap()
                    .lines()
                    .map(|line| serde_json::from_str(line).unwrap())
                    .collect();
            assert_eq!(
                wire.iter()
                    .filter(|row| row["operation"] == "prompt")
                    .count(),
                1
            );
            assert_eq!(
                wire.iter()
                    .filter(|row| row["operation"] == "new_session")
                    .count(),
                1
            );
            assert!(
                !wire.iter().any(|row| row["operation"] == "resume_session"
                    || row["operation"] == "load_session")
            );
            cancel.cancel();
            server.await.unwrap().unwrap();
            return;
        }
        tokio::time::timeout(Duration::from_secs(8), async {
            while storage
                .work_items()
                .for_run(attempt.run)
                .unwrap()
                .unwrap()
                .state
                != surge_core::work_item::WorkItemAttemptState::Completed
            {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            request(&socket, serde_json::to_value(&continue_command).unwrap()).await["method"],
            "work_item_ok",
            "original operation must replay without another launch"
        );
        cancel.cancel();
        server.await.unwrap().unwrap();
        for kind in [
            "StageOutcomeCommitted",
            "StageRouteCommitted",
            "EdgeTraversed",
            "StageCompleted",
            "RunContinued",
            "RunCompleted",
        ] {
            assert_eq!(
                conn.query_row("SELECT COUNT(*) FROM events WHERE kind=?", [kind], |row| {
                    row.get::<_, u64>(0)
                })
                .unwrap(),
                1,
                "{kind}"
            );
        }
        let wire: Vec<Value> =
            std::fs::read_to_string(home.path().join("commit-provider-wire.jsonl"))
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
        assert_eq!(
            wire.iter()
                .filter(|row| row["operation"] == "prompt")
                .count(),
            1
        );
        assert_eq!(
            wire.iter()
                .filter(|row| row["operation"] == "new_session")
                .count(),
            1
        );
        assert!(
            !wire
                .iter()
                .any(|row| row["operation"] == "resume_session"
                    || row["operation"] == "load_session")
        );
        return;
    }
    conn.execute_batch("CREATE TRIGGER fixture_no_continue BEFORE INSERT ON events WHEN NEW.kind='RunContinued' BEGIN SELECT RAISE(ABORT,'fixture rejected control acknowledgement'); END;").unwrap();
    // The original child was killed and reaped above. Its stale socket must
    // not satisfy the new host's filesystem readiness check before it binds.
    let stale_socket = home.path().join("cold.sock");
    if stale_socket.exists() {
        std::fs::remove_file(&stale_socket).unwrap();
    }
    let bridge = Arc::new(wire_bridge::WireBridge {
        bridge: surge_acp::bridge::AcpBridge::with_defaults().unwrap(),
        flags: commit_wire_flags(home.path()),
        session_commit_check: None,
    });
    let (_engine, cancel, server, socket) =
        cold_wire_host(home.path(), project.path(), storage.clone(), bridge).await;
    let continue_command = || WorkItemCommand::Continue {
        operation_id: WorkItemOperationId::new(),
        item: attempt.item,
        expected_version: storage
            .work_items()
            .show(attempt.item)
            .unwrap()
            .item
            .version,
        new_session: false,
    };
    assert_eq!(
        request(&socket, serde_json::to_value(continue_command()).unwrap()).await["method"],
        "work_item_ok"
    );
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            if storage
                .work_items()
                .execution_control(attempt.run)
                .unwrap()
                .is_some_and(|control| {
                    control.state
                        == surge_core::execution_recovery::ExecutionControlState::Attention
                })
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM events WHERE kind='StageRouteCommitted'",
            [],
            |row| row.get::<_, u64>(0)
        )
        .unwrap(),
        1
    );
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM events WHERE kind IN ('RunCompleted','RunFailed','RunAborted')",
            [],
            |row| row.get::<_, u64>(0)
        )
        .unwrap(),
        0
    );
    conn.execute_batch("DROP TRIGGER fixture_no_continue")
        .unwrap();
    assert_eq!(
        request(&socket, serde_json::to_value(continue_command()).unwrap()).await["method"],
        "work_item_ok"
    );
    tokio::time::timeout(Duration::from_secs(8), async {
        while storage
            .work_items()
            .for_run(attempt.run)
            .unwrap()
            .unwrap()
            .state
            != surge_core::work_item::WorkItemAttemptState::Completed
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    cancel.cancel();
    server.await.unwrap().unwrap();
    for kind in [
        "StageOutcomeCommitted",
        "StageRouteCommitted",
        "EdgeTraversed",
        "StageCompleted",
        "RunContinued",
        "RunCompleted",
    ] {
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM events WHERE kind=?", [kind], |row| {
                row.get::<_, u64>(0)
            })
            .unwrap(),
            1,
            "{kind}"
        );
    }
    let wire: Vec<Value> = std::fs::read_to_string(home.path().join("commit-provider-wire.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(
        wire.iter()
            .filter(|row| row["operation"] == "prompt")
            .count(),
        1
    );
    assert_eq!(
        wire.iter()
            .filter(|row| row["operation"] == "new_session")
            .count(),
        1
    );
    assert!(
        !wire
            .iter()
            .any(|row| row["operation"] == "resume_session" || row["operation"] == "load_session")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cold_committed_outcome_does_not_repeat_a_real_provider_turn() {
    cold_committed_fixture(false, None).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cold_consumed_route_before_registry_ack_does_not_route_twice() {
    cold_committed_fixture(true, None).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cold_consumed_route_missing_checkpoint_keeps_attention_without_provider_dispatch() {
    cold_committed_fixture(true, Some("missing")).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cold_consumed_route_corrupt_checkpoint_keeps_attention_without_provider_dispatch() {
    cold_committed_fixture(true, Some("corrupt")).await;
}

async fn cold_committed_fixture(after_route: bool, checkpoint_damage: Option<&str>) {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let (storage, attempt, _, start) = reserve_fixture(home.path(), project.path(), true).await;
    std::fs::write(
        home.path().join("child-start.json"),
        serde_json::to_vec(&start).unwrap(),
    )
    .unwrap();
    let barrier = home.path().join("close-barrier");
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "child_committed_outcome_host_probe",
            "--nocapture",
        ])
        .env("SURGE_TEST_COMMIT_HOME", home.path())
        .env("SURGE_TEST_COMMIT_PROJECT", project.path())
        .env("SURGE_TEST_CLOSE_BARRIER", &barrier)
        .stdout(std::process::Stdio::null())
        // Inherited so nextest shows a child panic; the probe emits no tracing output.
        .stderr(std::process::Stdio::inherit())
        .spawn()
        .unwrap();
    let ready = tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            if !barrier.with_extension("ready").exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
                continue;
            }
            let inspected = storage.inspect_run(attempt.run).await.unwrap();
            let surge_persistence::runs::inspection::RunDatabaseInspection::Present { events } =
                inspected.database
            else {
                tokio::time::sleep(Duration::from_millis(10)).await;
                continue;
            };
            if events
                .iter()
                .filter(|event| {
                    matches!(
                        event.payload.payload(),
                        surge_core::EventPayload::StageOutcomeCommitted { .. }
                    )
                })
                .count()
                == 1
            {
                break events;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    let registry_barrier = if after_route && ready.is_ok() {
        // Fixture-only SQLite writer lock prevents the registry settlement while
        // allowing the independent per-run accepted route transaction to commit.
        let conn = rusqlite::Connection::open(storage.registry_db_path()).unwrap();
        conn.execute_batch("BEGIN IMMEDIATE").unwrap();
        if checkpoint_damage.is_some() {
            let events_conn = rusqlite::Connection::open(
                home.path()
                    .join("runs")
                    .join(attempt.run.to_string())
                    .join("events.sqlite"),
            )
            .unwrap();
            events_conn.execute_batch("CREATE TRIGGER fixture_no_terminal BEFORE INSERT ON events WHEN NEW.kind IN ('RunCompleted','RunFailed','RunAborted') BEGIN SELECT RAISE(ABORT,'fixture crash boundary before terminal'); END;").unwrap();
        }
        std::fs::write(&barrier, b"release").unwrap();
        // The route commit follows the best-effort capacity clear, which spends the
        // full 5 s registry busy_timeout against this fixture lock before failing.
        tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                let inspected = storage.inspect_run(attempt.run).await.unwrap();
                let surge_persistence::runs::inspection::RunDatabaseInspection::Present { events } =
                    inspected.database
                else {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    continue;
                };
                let route_committed = events.iter().any(|event| {
                    matches!(
                        event.payload.payload(),
                        surge_core::EventPayload::StageRouteCommitted { .. }
                    )
                });
                let run_completed = events.iter().any(|event| {
                    matches!(
                        event.payload.payload(),
                        surge_core::EventPayload::RunCompleted { .. }
                    )
                });
                if route_committed && (checkpoint_damage.is_some() || run_completed) {
                    assert_eq!(
                        events
                            .iter()
                            .filter(|event| matches!(
                                event.payload.payload(),
                                surge_core::EventPayload::StageRouteCommitted { .. }
                            ))
                            .count(),
                        1
                    );
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("route did not commit before registry acknowledgement");
        Some(conn)
    } else {
        None
    };
    let _ = child.kill();
    let _ = child.wait();
    drop(registry_barrier);
    if let Some(damage) = checkpoint_damage {
        let conn = rusqlite::Connection::open(
            home.path()
                .join("runs")
                .join(attempt.run.to_string())
                .join("events.sqlite"),
        )
        .unwrap();
        conn.execute_batch("DROP TRIGGER fixture_no_terminal")
            .unwrap();
        if damage == "missing" {
            conn.execute("DELETE FROM graph_snapshots", []).unwrap();
        } else {
            conn.execute("UPDATE graph_snapshots SET snapshot=x'00'", [])
                .unwrap();
        }
    }
    let events =
        ready.expect("real ACP turn did not reach its durable post-commit/pre-route close barrier");
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(
                event.payload.payload(),
                surge_core::EventPayload::StageOutcomeCommitted { .. }
            ))
            .count(),
        1
    );
    assert!(!events.iter().any(|event| matches!(
        event.payload.payload(),
        surge_core::EventPayload::StageCompleted { .. }
            | surge_core::EventPayload::RunSuspended { .. }
    )));
    // Only the fixture-owned actual process is killed; this is not production containment evidence.
    let pid = std::fs::read_to_string(home.path().join("commit-provider.pid")).unwrap();
    let identity = events
        .iter()
        .find_map(|event| match event.payload.payload() {
            surge_core::EventPayload::SessionOpened {
                opened: Some(opened),
                ..
            } => opened
                .execution_writer
                .as_ref()
                .and_then(|writer| writer.container())
                .map(|container| container.identity()),
            _ => None,
        })
        .unwrap();
    assert_eq!(identity.pid(), pid.trim().parse::<u32>().unwrap());
    if matches!(surge_acp::process_evidence::observe(identity.pid()), Ok((current, _)) if &current == identity)
    {
        let _ = std::process::Command::new("kill")
            .args(["-KILL", pid.trim()])
            .status();
    }
    let bridge = Arc::new(wire_bridge::WireBridge {
        bridge: surge_acp::bridge::AcpBridge::with_defaults().unwrap(),
        flags: commit_wire_flags(home.path()),
        session_commit_check: None,
    });
    let (_engine, cancel, server, _) =
        cold_wire_host(home.path(), project.path(), storage.clone(), bridge).await;
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            let state = storage
                .work_items()
                .for_run(attempt.run)
                .unwrap()
                .unwrap()
                .state;
            if !state.is_active() || state == surge_core::work_item::WorkItemAttemptState::Attention
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    cancel.cancel();
    server.await.unwrap().unwrap();
    let wire: Vec<Value> = std::fs::read_to_string(home.path().join("commit-provider-wire.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(
        wire.iter()
            .filter(|row| row["operation"] == "prompt")
            .count(),
        1,
        "accepted durable outcome must not replay a provider turn"
    );
    assert_eq!(
        wire.iter()
            .filter(|row| row["operation"] == "new_session")
            .count(),
        1,
        "cold accepted phase must not establish a replacement session"
    );
    assert!(
        !wire
            .iter()
            .any(|row| row["operation"] == "resume_session" || row["operation"] == "load_session")
    );
    let inspected = storage.inspect_run(attempt.run).await.unwrap();
    let surge_persistence::runs::inspection::RunDatabaseInspection::Present { events } =
        inspected.database
    else {
        panic!("fixture journal absent")
    };
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(
                event.payload.payload(),
                surge_core::EventPayload::StageOutcomeCommitted { .. }
            ))
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(
                event.payload.payload(),
                surge_core::EventPayload::EdgeTraversed { .. }
            ))
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(
                event.payload.payload(),
                surge_core::EventPayload::StageCompleted { .. }
            ))
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(
                event.payload.payload(),
                surge_core::EventPayload::StageRouteCommitted { .. }
            ))
            .count(),
        1
    );
    if checkpoint_damage.is_some() {
        assert!(!events.iter().any(|event| matches!(
            event.payload.payload(),
            surge_core::EventPayload::RunCompleted { .. }
                | surge_core::EventPayload::RunFailed { .. }
                | surge_core::EventPayload::RunAborted { .. }
        )));
        let row = storage.work_items().for_run(attempt.run).unwrap().unwrap();
        assert_eq!(
            row.state,
            surge_core::work_item::WorkItemAttemptState::Attention
        );
        assert_eq!(row.run, attempt.run);
        return;
    }
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(
                event.payload.payload(),
                surge_core::EventPayload::RunCompleted { .. }
            ))
            .count(),
        1
    );
    assert!(!events.iter().any(|event| matches!(
        event.payload.payload(),
        surge_core::EventPayload::RunAborted { .. } | surge_core::EventPayload::RunFailed { .. }
    )));
    assert_eq!(
        storage
            .work_items()
            .for_run(attempt.run)
            .unwrap()
            .unwrap()
            .state,
        surge_core::work_item::WorkItemAttemptState::Completed
    );
    assert_eq!(
        storage.work_items().show(attempt.item).unwrap().usage.runs,
        1
    );
    let reader = storage.open_run_reader(attempt.run).await.unwrap();
    let prefix = reader.current_seq().await.unwrap();
    let checkpoint = reader
        .latest_snapshot_at_or_before(prefix)
        .await
        .unwrap()
        .expect("routed phase must have a checkpoint");
    let writer = storage.open_run_writer(attempt.run).await.unwrap();
    writer.rebuild_views().await.unwrap();
    writer.close().await.unwrap();
    assert_eq!(
        reader.latest_snapshot_at_or_before(prefix).await.unwrap(),
        Some(checkpoint),
        "projection rebuild must preserve the authoritative execution checkpoint"
    );
    let conn = rusqlite::Connection::open(
        home.path()
            .join("runs")
            .join(attempt.run.to_string())
            .join("events.sqlite"),
    )
    .unwrap();
    let (accepted, consumed): (u64, u64) = conn
        .query_row(
            "SELECT COUNT(*),COUNT(routed_seq) FROM stage_outcome_commits",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        (accepted, consumed),
        (1, 1),
        "projection rebuild must retain exact consumed invocation identity"
    );
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn killed_execution_owner_releases_os_lock_and_same_attempt_recovers() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let (storage, attempt, _, start) = reserve_fixture(home.path(), project.path(), true).await;
    std::fs::write(
        home.path().join("child-start.json"),
        serde_json::to_vec(&start).unwrap(),
    )
    .unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "child_execution_host_probe", "--nocapture"])
        .env("SURGE_TEST_EXECUTION_HOME", home.path())
        .env("SURGE_TEST_EXECUTION_PROJECT", project.path())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let ready = tokio::time::timeout(Duration::from_secs(8), async {
        while !home.path().join("child-execution-ready").exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    let held = storage.work_items().claim(attempt.run).is_err();
    let _ = child.kill();
    let _ = child.wait();
    assert!(ready.is_ok(), "child failed to reach first ACP turn");
    assert!(held, "live execution did not retain launch ownership");
    let bridge = Arc::new(mock_bridge::MockBridge::new());
    let (engine, cancel, server, _) =
        cold_host(home.path(), project.path(), storage.clone(), bridge.clone()).await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while bridge.last_prompt().await.is_none() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        storage
            .work_items()
            .show(attempt.item)
            .unwrap()
            .item
            .active_run,
        Some(attempt.run)
    );
    assert_eq!(
        storage.work_items().show(attempt.item).unwrap().usage.runs,
        1
    );
    engine
        .stop_run(attempt.run, "fixture cleanup".into())
        .await
        .unwrap();
    cancel.cancel();
    server.await.unwrap().unwrap();
}

/// Actual ACP subprocess opening is the anchor for provider-session continuity.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn task_provider_identity_is_durable_before_first_real_acp_prompt() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let (storage, attempt, workspace, start) =
        reserve_fixture(home.path(), project.path(), true).await;
    let bridge = Arc::new(wire_bridge::WireBridge {
        session_commit_check: Some((storage.clone(), attempt.run)),
        bridge: surge_acp::bridge::AcpBridge::with_defaults().unwrap(),
        flags: vec![],
    });
    let (engine, cancel, server, socket) =
        cold_wire_host(home.path(), project.path(), storage.clone(), bridge.clone()).await;
    let accepted = request(&socket, serde_json::to_value(start).unwrap()).await;
    assert_eq!(accepted["method"], "work_item_ok");
    let observed = tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            let inspection = storage.inspect_run(attempt.run).await.unwrap();
            let surge_persistence::runs::inspection::RunDatabaseInspection::Present { events } =
                inspection.database
            else {
                tokio::time::sleep(Duration::from_millis(10)).await;
                continue;
            };
            if events.iter().any(|row| {
                matches!(
                    row.payload.payload,
                    surge_core::EventPayload::RunCompleted { .. }
                )
            }) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    std::fs::write(workspace.path.join("retained-untracked"), "operator work").unwrap();
    let inspection = storage.inspect_run(attempt.run).await.unwrap();
    let surge_persistence::runs::inspection::RunDatabaseInspection::Present { events } =
        inspection.database
    else {
        panic!("journal missing")
    };
    let encoded =
        serde_json::to_value(events.iter().map(|row| &row.payload).collect::<Vec<_>>()).unwrap();
    let _ = engine
        .stop_run(attempt.run, "pre-implementation fixture cleanup".into())
        .await;
    cancel.cancel();
    server.await.unwrap().unwrap();
    drop(engine);
    drop(bridge);
    assert!(
        observed.is_ok(),
        "fixture did not reach actual first prompt: {encoded}"
    );
    assert!(
        encoded.to_string().contains("stage_tool_receipt"),
        "fixture must execute the authenticated report over actual ACP"
    );
    let establishment = events
        .iter()
        .find(|row| {
            matches!(
                row.payload.payload,
                surge_core::EventPayload::SessionEstablishmentRequested { .. }
            )
        })
        .unwrap()
        .seq;
    let opened = events
        .iter()
        .find(|row| {
            matches!(
                row.payload.payload,
                surge_core::EventPayload::SessionOpened {
                    opened: Some(_),
                    ..
                }
            )
        })
        .unwrap()
        .seq;
    let receipt = events
        .iter()
        .find(|row| {
            matches!(
                row.payload.payload,
                surge_core::EventPayload::StageToolReceipt { .. }
            )
        })
        .unwrap()
        .seq;
    assert!(
        establishment < opened && opened < receipt,
        "durable metadata must precede authenticated first turn effects"
    );
    assert!(
        encoded.to_string().contains("provider_session_id"),
        "real ACP session opened and prompted, but the task journal lacks its provider continuation identity: {encoded}"
    );
    assert_eq!(
        std::fs::read_to_string(workspace.path.join("retained-untracked")).unwrap(),
        "operator work"
    );
}

#[path = "fixtures/wire_bridge.rs"]
mod wire_bridge;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn task_suspend_continue_uses_saved_provider_on_real_acp_wire() {
    suspend_continue_wire("both", false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn task_continue_without_provider_restore_capability_preserves_attention() {
    suspend_continue_wire("none", false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn manual_suspend_supersedes_reserved_continue_and_historical_replay_cannot_dispatch() {
    suspend_continue_wire("both", true).await;
}

async fn suspend_continue_wire(restored_capabilities: &str, manual_wins: bool) {
    use surge_core::{EventPayload, id::WorkItemOperationId, work_item::WorkItemCommand};
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let (storage, attempt, workspace, start) =
        reserve_fixture(home.path(), project.path(), true).await;
    let marker = home.path().join("prompt.marker");
    let capability_policy = home.path().join("provider-capabilities.txt");
    std::fs::write(&capability_policy, "both").unwrap();
    let session_store = home.path().join("provider-sessions.json");
    let wire_log = home.path().join("provider-wire.jsonl");
    let flags = vec![
        "--session-capability-policy-file".into(),
        capability_policy.display().to_string(),
        "--stall-prompt".into(),
        "--prompt-file".into(),
        marker.display().to_string(),
        "--session-store".into(),
        session_store.display().to_string(),
        "--wire-log".into(),
        wire_log.display().to_string(),
    ];
    let bridge = Arc::new(wire_bridge::WireBridge {
        session_commit_check: None,
        bridge: surge_acp::bridge::AcpBridge::with_defaults().unwrap(),
        flags: flags.clone(),
    });
    let (engine, cancel, server, socket) =
        cold_wire_host(home.path(), project.path(), storage.clone(), bridge).await;
    request(&socket, serde_json::to_value(start).unwrap()).await;
    tokio::time::timeout(Duration::from_secs(8), async {
        while !marker.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    std::fs::write(
        workspace.path.join("retained-untracked"),
        "preserve on pause",
    )
    .unwrap();
    let command = json!({"action":"suspend","operation_id":surge_core::RunId::new(),"item":attempt.item,"expected_version":storage.work_items().show(attempt.item).unwrap().item.version});
    let stream = LocalSocketStream::connect(local_socket_name_from_path(&socket).unwrap())
        .await
        .unwrap();
    let (read, mut write) = stream.split();
    let mut frame =
        serde_json::to_vec(&json!({"method":"work_item","request_id":2,"command":command}))
            .unwrap();
    frame.push(b'\n');
    write.write_all(&frame).await.unwrap();
    let mut line = String::new();
    let reply = tokio::time::timeout(
        Duration::from_secs(8),
        BufReader::new(read).read_line(&mut line),
    )
    .await;
    cancel.cancel();
    server.await.unwrap().unwrap();
    assert!(
        reply.is_ok() && !line.is_empty(),
        "actual daemon must acknowledge resumable suspension after real ACP prompt; response={line:?}"
    );
    let response: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(response["method"], "work_item_ok", "{response}");
    let control = storage
        .work_items()
        .execution_control(attempt.run)
        .unwrap()
        .unwrap();
    assert_eq!(
        control.state,
        surge_core::execution_recovery::ExecutionControlState::Suspended,
        "attempt={:?}; journal={:?}",
        storage.work_items().for_run(attempt.run).unwrap(),
        storage.inspect_run(attempt.run).await.unwrap()
    );
    let inspected = storage.inspect_run(attempt.run).await.unwrap();
    let surge_persistence::runs::inspection::RunDatabaseInspection::Present { events } =
        inspected.database
    else {
        panic!("journal")
    };
    assert!(events.iter().any(|row|matches!(&row.payload.payload,EventPayload::RunSuspended { fence } if fence.cleanup_confirmed && fence.control_generation==control.generation)));
    assert!(
        !events
            .iter()
            .any(|row| matches!(row.payload.payload, EventPayload::RunAborted { .. }))
    );
    assert_eq!(
        std::fs::read_to_string(workspace.path.join("retained-untracked")).unwrap(),
        "preserve on pause"
    );
    drop(engine);
    std::fs::write(&capability_policy, restored_capabilities).unwrap();
    let reopened = Storage::open(home.path()).await.unwrap();
    let retained_claim = if manual_wins {
        Some(reopened.work_items().claim(attempt.run).unwrap())
    } else {
        None
    };
    let bridge = Arc::new(wire_bridge::WireBridge {
        session_commit_check: None,
        bridge: surge_acp::bridge::AcpBridge::with_defaults().unwrap(),
        flags: flags.clone(),
    });
    let (engine, cancel, server, socket) =
        cold_wire_host(home.path(), project.path(), reopened.clone(), bridge).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        engine.snapshot_active_runs().await.is_empty(),
        "manual pause must remain inert after restart"
    );
    if manual_wins {
        let original_continue = WorkItemCommand::Continue {
            operation_id: WorkItemOperationId::new(),
            item: attempt.item,
            expected_version: reopened
                .work_items()
                .show(attempt.item)
                .unwrap()
                .item
                .version,
            new_session: false,
        };
        let reserved = reopened
            .work_items()
            .mutate(
                &original_continue,
                None,
                None,
                "operator",
                chrono::Utc::now().timestamp_millis(),
            )
            .unwrap();
        let surge_core::work_item::WorkItemResult::Control(reserved) = reserved else {
            panic!("continuation receipt missing")
        };
        assert_eq!(
            reserved.state,
            surge_core::execution_recovery::ExecutionControlState::ContinueReserved
        );
        let wire_before = std::fs::read_to_string(&wire_log).unwrap();
        let manual = WorkItemCommand::Suspend {
            operation_id: WorkItemOperationId::new(),
            item: attempt.item,
            expected_version: reopened
                .work_items()
                .show(attempt.item)
                .unwrap()
                .item
                .version,
        };
        let reply = request(&socket, serde_json::to_value(manual).unwrap()).await;
        cancel.cancel();
        server.await.unwrap().unwrap();
        assert_eq!(
            reply["method"], "work_item_ok",
            "manual pause must supersede a reserved, undispatched Continue: {reply}"
        );
        let latest = reopened
            .work_items()
            .execution_control(attempt.run)
            .unwrap()
            .unwrap();
        assert!(latest.generation > reserved.generation);
        assert_eq!(
            latest.state,
            surge_core::execution_recovery::ExecutionControlState::SuspendRequested
        );
        drop(retained_claim);
        let registry = rusqlite::Connection::open(reopened.registry_db_path()).unwrap();
        let claim_before: String = registry
            .query_row(
                "SELECT claim_token FROM work_item_attempts WHERE run=?",
                [attempt.run.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        let bridge = Arc::new(wire_bridge::WireBridge {
            session_commit_check: None,
            bridge: surge_acp::bridge::AcpBridge::with_defaults().unwrap(),
            flags,
        });
        let (engine, cancel, server, socket) =
            cold_wire_host(home.path(), project.path(), reopened.clone(), bridge).await;
        let replay = request(&socket, serde_json::to_value(&original_continue).unwrap()).await;
        assert_eq!(
            replay["method"], "work_item_ok",
            "historical admission stays replayable: {replay}"
        );
        let original_receipt = reopened.work_items().replay(&original_continue).unwrap();
        assert!(
            matches!(original_receipt, Some(surge_core::work_item::WorkItemResult::Control(ref control)) if control.generation==reserved.generation && control.operation==reserved.operation)
        );
        assert_eq!(
            reopened
                .work_items()
                .execution_control(attempt.run)
                .unwrap()
                .unwrap(),
            latest
        );
        assert!(engine.snapshot_active_runs().await.is_empty());
        assert_eq!(
            std::fs::read_to_string(&wire_log).unwrap(),
            wire_before,
            "old Continue must not open or prompt any provider"
        );
        let claim_after: String = registry
            .query_row(
                "SELECT claim_token FROM work_item_attempts WHERE run=?",
                [attempt.run.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            claim_after, claim_before,
            "historical Continue must not even claim a launch after manual control wins"
        );
        assert_eq!(
            std::fs::read_to_string(workspace.path.join("retained-untracked")).unwrap(),
            "preserve on pause"
        );
        cancel.cancel();
        server.await.unwrap().unwrap();
        return;
    }
    let continued = request(
        &socket,
        serde_json::to_value(WorkItemCommand::Continue {
            operation_id: WorkItemOperationId::new(),
            item: attempt.item,
            expected_version: reopened
                .work_items()
                .show(attempt.item)
                .unwrap()
                .item
                .version,
            new_session: false,
        })
        .unwrap(),
    )
    .await;
    assert_eq!(continued["method"], "work_item_ok", "{continued}");
    let restored = reopened
        .work_items()
        .execution_control(attempt.run)
        .unwrap()
        .unwrap();
    if restored_capabilities == "none" {
        assert_eq!(
            restored.state,
            surge_core::execution_recovery::ExecutionControlState::Attention
        );
        let attempt_state = reopened.work_items().for_run(attempt.run).unwrap().unwrap();
        assert_eq!(
            attempt_state.state,
            surge_core::work_item::WorkItemAttemptState::Attention
        );
        assert_eq!(
            reopened
                .work_items()
                .show(attempt.item)
                .unwrap()
                .item
                .active_run,
            Some(attempt.run)
        );
        let history = reopened.inspect_run(attempt.run).await.unwrap();
        let surge_persistence::runs::inspection::RunDatabaseInspection::Present { events } =
            history.database
        else {
            panic!("journal")
        };
        assert!(events.iter().any(|row|matches!(&row.payload.payload,EventPayload::RunRecoveryRequired { diagnostic,.. } if diagnostic.contains("neither session/resume nor session/load"))));
        assert!(!events.iter().any(|row| matches!(
            row.payload.payload,
            EventPayload::RunFailed { .. } | EventPayload::RunAborted { .. }
        )));
        let entries: Vec<Value> = std::fs::read_to_string(&wire_log)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(
            entries
                .iter()
                .filter(|entry| entry["operation"] == "new_session")
                .count(),
            1
        );
        assert_eq!(
            entries
                .iter()
                .filter(|entry| entry["operation"] == "prompt")
                .count(),
            1
        );
        assert_eq!(
            std::fs::read_to_string(workspace.path.join("retained-untracked")).unwrap(),
            "preserve on pause"
        );
        cancel.cancel();
        server.await.unwrap().unwrap();
        return;
    }
    assert_eq!(
        restored.state,
        surge_core::execution_recovery::ExecutionControlState::Executing,
        "attempt={:?}; journal={:?}",
        reopened.work_items().for_run(attempt.run).unwrap(),
        reopened.inspect_run(attempt.run).await.unwrap()
    );
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            let wire = std::fs::read_to_string(&wire_log).unwrap_or_default();
            let prompts = wire
                .lines()
                .filter_map(|line| serde_json::from_str::<Value>(line).ok())
                .filter(|entry| entry["operation"] == "prompt")
                .count();
            if prompts == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let entries: Vec<Value> = std::fs::read_to_string(&wire_log)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(
        entries
            .iter()
            .filter(|entry| entry["operation"] == "new_session")
            .count(),
        1,
        "Continue must not create a replacement provider session"
    );
    assert_eq!(
        entries
            .iter()
            .filter(|entry| entry["operation"] == "resume_session")
            .count(),
        1
    );
    let prompts: Vec<_> = entries
        .iter()
        .filter(|entry| entry["operation"] == "prompt")
        .collect();
    assert_eq!(
        prompts[0]["request"]["sessionId"],
        prompts[1]["request"]["sessionId"]
    );
    assert_eq!(
        reopened
            .work_items()
            .for_run(attempt.run)
            .unwrap()
            .unwrap()
            .binding,
        attempt.binding
    );
    assert_eq!(
        std::fs::read_to_string(workspace.path.join("retained-untracked")).unwrap(),
        "preserve on pause"
    );
    engine
        .stop_run(
            attempt.run,
            "fixture cleanup after continuity assertions".into(),
        )
        .await
        .unwrap();
    cancel.cancel();
    server.await.unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn accepted_gate_answer_receipt_survives_completion_and_restart() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let (storage, attempt, _, start) =
        reserve_fixture_with_gate(home.path(), project.path(), true, true).await;
    let bridge = Arc::new(wire_bridge::WireBridge {
        bridge: surge_acp::bridge::AcpBridge::with_defaults().unwrap(),
        flags: commit_wire_flags(home.path()),
        session_commit_check: None,
    });
    let (engine, cancel, server, socket) =
        cold_wire_host(home.path(), project.path(), storage.clone(), bridge).await;
    assert_eq!(
        request(&socket, serde_json::to_value(start).unwrap()).await["method"],
        "work_item_ok"
    );
    let path = home
        .path()
        .join("runs")
        .join(attempt.run.to_string())
        .join("events.sqlite");
    let conn = rusqlite::Connection::open(path).unwrap();
    let original = tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            let rows: Vec<Vec<u8>> = conn
                .prepare("SELECT payload FROM events WHERE kind='HumanInputRequested'")
                .unwrap()
                .query_map([], |row| row.get(0))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap();
            if let Some(payload) = rows.first() {
                break serde_json::from_slice::<surge_core::VersionedEventPayload>(payload)
                    .unwrap();
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let surge_core::EventPayload::HumanInputRequested {
        node,
        call_id: Some(call_id),
        ..
    } = original.payload()
    else {
        panic!("host gate identity missing")
    };
    let command = surge_orchestrator::engine::ipc::DaemonRequest::ResolveGateInput {
        request_id: 9,
        run_id: attempt.run,
        node: node.clone(),
        gate_request_id: surge_core::id::GateRequestId::from_event_call_id(call_id).unwrap(),
        response: json!({"outcome":"approve","comment":"durable answer"}),
    };
    conn.execute_batch("CREATE TRIGGER fixture_reject_answer BEFORE INSERT ON events WHEN NEW.kind='HumanInputResolved' BEGIN SELECT RAISE(ABORT,'fixture rejects durable answer'); END;").unwrap();
    let rejected = request_frame(&socket, serde_json::to_value(&command).unwrap()).await;
    if rejected["method"] != "error" {
        let _ = engine
            .stop_run(
                attempt.run,
                "fixture cleanup after unsafe answer acknowledgement".into(),
            )
            .await;
        cancel.cancel();
        server.await.unwrap().unwrap();
        panic!("daemon acknowledged an answer whose durable append was rejected: {rejected}");
    }
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM events WHERE kind='HumanInputResolved'",
            [],
            |row| row.get::<_, u64>(0)
        )
        .unwrap(),
        0
    );
    conn.execute_batch("DROP TRIGGER fixture_reject_answer")
        .unwrap();
    let accepted = request_frame(&socket, serde_json::to_value(&command).unwrap()).await;
    assert_eq!(accepted["method"], "resolve_human_input_ok", "{accepted}");
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM events WHERE kind='HumanInputResolved'",
            [],
            |row| row.get::<_, u64>(0)
        )
        .unwrap(),
        1,
        "application-visible success must already have its exact durable answer"
    );
    tokio::time::timeout(Duration::from_secs(8), async {
        while storage
            .work_items()
            .for_run(attempt.run)
            .unwrap()
            .unwrap()
            .state
            != surge_core::work_item::WorkItemAttemptState::Completed
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let completed_replay = request_frame(&socket, serde_json::to_value(&command).unwrap()).await;
    assert_eq!(
        completed_replay["method"], "resolve_human_input_ok",
        "identical accepted answer remains replayable after completion: {completed_replay}"
    );
    let mut conflicting = serde_json::to_value(&command).unwrap();
    conflicting["response"] = json!({"outcome":"approve","comment":"different answer"});
    assert_eq!(request_frame(&socket, conflicting).await["method"], "error");
    cancel.cancel();
    server.await.unwrap().unwrap();
    drop(engine);
    let bridge = Arc::new(wire_bridge::WireBridge {
        bridge: surge_acp::bridge::AcpBridge::with_defaults().unwrap(),
        flags: commit_wire_flags(home.path()),
        session_commit_check: None,
    });
    let (_engine, cancel, server, socket) =
        cold_wire_host(home.path(), project.path(), storage.clone(), bridge).await;
    assert_eq!(
        request_frame(&socket, serde_json::to_value(&command).unwrap()).await["method"],
        "resolve_human_input_ok",
        "durable receipt survives a new engine owner"
    );
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM events WHERE kind='HumanInputResolved'",
            [],
            |row| row.get::<_, u64>(0)
        )
        .unwrap(),
        1
    );
    cancel.cancel();
    server.await.unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn accepted_gate_answer_survives_owner_death_before_outcome_delivery() {
    gate_answer_crash_fixture(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn committed_gate_effects_survive_owner_death_before_route_without_repetition() {
    gate_answer_crash_fixture(true).await;
}

async fn gate_answer_crash_fixture(after_effects: bool) {
    use rusqlite::OptionalExtension;
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let (storage, attempt, _, start) =
        reserve_fixture_with_gate(home.path(), project.path(), true, true).await;
    std::fs::write(
        home.path().join("child-start.json"),
        serde_json::to_vec(&start).unwrap(),
    )
    .unwrap();
    let diagnostic = std::fs::File::create(home.path().join("gate-child.log")).unwrap();
    let mut owner = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "child_committed_outcome_host_probe",
            "--nocapture",
        ])
        .env("SURGE_TEST_COMMIT_HOME", home.path())
        .env("SURGE_TEST_COMMIT_PROJECT", project.path())
        .stdout(diagnostic.try_clone().unwrap())
        .stderr(diagnostic)
        .spawn()
        .unwrap();
    let path = home
        .path()
        .join("runs")
        .join(attempt.run.to_string())
        .join("events.sqlite");
    let original = tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            assert!(
                owner.try_wait().unwrap().is_none(),
                "{}",
                std::fs::read_to_string(home.path().join("gate-child.log")).unwrap()
            );
            if path.exists() {
                let conn = rusqlite::Connection::open(&path).unwrap();
                if let Ok(Some(bytes)) = conn
                    .query_row(
                        "SELECT payload FROM events WHERE kind='HumanInputRequested' LIMIT 1",
                        [],
                        |row| row.get::<_, Vec<u8>>(0),
                    )
                    .optional()
                {
                    break serde_json::from_slice::<surge_core::VersionedEventPayload>(&bytes)
                        .unwrap();
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let surge_core::EventPayload::HumanInputRequested {
        node,
        call_id: Some(call_id),
        ..
    } = original.payload()
    else {
        panic!("original request missing")
    };
    let conn = rusqlite::Connection::open(&path).unwrap();
    let barrier_kind = if after_effects {
        "StageCompleted"
    } else {
        "OutcomeReported"
    };
    conn.execute_batch(&format!("CREATE TRIGGER fixture_gate_delivery_interval BEFORE INSERT ON events WHEN NEW.kind='{barrier_kind}' BEGIN SELECT sum(x) FROM (WITH RECURSIVE delay(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM delay WHERE x<5000000) SELECT x FROM delay); END;")).unwrap();
    let command = surge_orchestrator::engine::ipc::DaemonRequest::ResolveGateInput {
        request_id: 12,
        run_id: attempt.run,
        node: node.clone(),
        gate_request_id: surge_core::id::GateRequestId::from_event_call_id(call_id).unwrap(),
        response: json!({"outcome":"approve","comment":"accepted before crash"}),
    };
    let acknowledged = request_frame(
        &home.path().join("cold.sock"),
        serde_json::to_value(&command).unwrap(),
    )
    .await;
    assert_eq!(
        acknowledged["method"], "resolve_human_input_ok",
        "{acknowledged}"
    );
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM events WHERE kind='HumanInputResolved'",
            [],
            |row| row.get::<_, u64>(0)
        )
        .unwrap(),
        1
    );
    if after_effects {
        tokio::time::timeout(Duration::from_secs(8), async {
            while conn
                .query_row(
                    "SELECT COUNT(*) FROM events WHERE kind='OutcomeReported'",
                    [],
                    |row| row.get::<_, u64>(0),
                )
                .unwrap()
                != 2
            {
                assert!(owner.try_wait().unwrap().is_none());
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM events WHERE kind='StageCompleted'",
                [],
                |row| row.get::<_, u64>(0)
            )
            .unwrap(),
            1,
            "gate effects must commit before its route at the crash cut"
        );
    } else {
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM events WHERE kind='OutcomeReported'",
                [],
                |row| row.get::<_, u64>(0)
            )
            .unwrap(),
            1,
            "only the earlier agent outcome may exist at the crash cut"
        );
    }
    owner.kill().unwrap();
    owner.wait().unwrap();
    if after_effects {
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM events WHERE kind='OutcomeReported'",
                [],
                |row| row.get::<_, u64>(0)
            )
            .unwrap(),
            2,
            "owned host died after committed gate effects"
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM events WHERE kind='StageCompleted'",
                [],
                |row| row.get::<_, u64>(0)
            )
            .unwrap(),
            1,
            "owned host died before gate route committed"
        );
    }
    conn.execute_batch("DROP TRIGGER fixture_gate_delivery_interval")
        .unwrap();
    std::fs::remove_file(home.path().join("cold.sock")).unwrap();
    let bridge = Arc::new(wire_bridge::WireBridge {
        bridge: surge_acp::bridge::AcpBridge::with_defaults().unwrap(),
        flags: commit_wire_flags(home.path()),
        session_commit_check: None,
    });
    let (_engine, cancel, server, socket) =
        cold_wire_host(home.path(), project.path(), storage.clone(), bridge).await;
    tokio::time::timeout(Duration::from_secs(8), async {
        while storage
            .work_items()
            .for_run(attempt.run)
            .unwrap()
            .unwrap()
            .state
            != surge_core::work_item::WorkItemAttemptState::Completed
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        request_frame(&socket, serde_json::to_value(&command).unwrap()).await["method"],
        "resolve_human_input_ok"
    );
    for (kind, expected) in [
        ("HumanInputRequested", 1),
        ("HumanInputResolved", 1),
        ("OutcomeReported", 2),
        ("StageCompleted", 2),
        ("EdgeTraversed", 2),
        ("RunCompleted", 1),
    ] {
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM events WHERE kind=?", [kind], |row| {
                row.get::<_, u64>(0)
            })
            .unwrap(),
            expected,
            "{kind}"
        );
    }
    let wire = std::fs::read_to_string(home.path().join("commit-provider-wire.jsonl")).unwrap();
    let entries: Vec<Value> = wire
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(
        entries
            .iter()
            .filter(|entry| entry["operation"] == "new_session")
            .count(),
        1
    );
    assert_eq!(
        entries
            .iter()
            .filter(|entry| entry["operation"] == "prompt")
            .count(),
        1
    );
    assert!(!entries.iter().any(|entry| matches!(
        entry["operation"].as_str(),
        Some("load_session" | "resume_session")
    )));
    cancel.cancel();
    server.await.unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pre_admission_task_conflict_is_typed_and_has_no_operation_receipt() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let (storage, attempt, _, _) = reserve_fixture(home.path(), project.path(), false).await;
    let claim = storage.work_items().claim(attempt.run).unwrap();
    let (_engine, cancel, server, socket) = cold_host(
        home.path(),
        project.path(),
        storage.clone(),
        Arc::new(mock_bridge::MockBridge::new()),
    )
    .await;
    let detail = storage.work_items().show(attempt.item).unwrap();
    let operation = surge_core::id::WorkItemOperationId::new();
    let command = surge_core::work_item::WorkItemCommand::Edit {
        operation_id: operation,
        item: attempt.item,
        expected_version: detail.item.version + 1,
        expected_revision: detail.item.accepted_revision,
        requirements: detail.revision.origin.requirements().unwrap().clone(),
    };
    let response = request(&socket, serde_json::to_value(command).unwrap()).await;
    cancel.cancel();
    server.await.unwrap().unwrap();
    drop(claim);
    let registry = rusqlite::Connection::open(storage.registry_db_path()).unwrap();
    assert_eq!(
        registry
            .query_row(
                "SELECT COUNT(*) FROM work_item_operations WHERE operation=?",
                [operation.to_string()],
                |row| row.get::<_, u64>(0)
            )
            .unwrap(),
        0
    );
    assert_eq!(
        storage
            .work_items()
            .show(attempt.item)
            .unwrap()
            .item
            .version,
        detail.item.version
    );
    assert_eq!(response["method"], "error", "{response}");
    assert_eq!(
        response["code"], "work_item_conflict",
        "a definitive pre-admission conflict must not be indistinguishable from uncertain execution: {response}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn admitted_suspend_with_no_local_actor_returns_pending_original_operation() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let (storage, attempt, _, _) = reserve_fixture(home.path(), project.path(), false).await;
    let claim = storage.work_items().claim(attempt.run).unwrap();
    let (_engine, cancel, server, socket) = cold_host(
        home.path(),
        project.path(),
        storage.clone(),
        Arc::new(mock_bridge::MockBridge::new()),
    )
    .await;
    let operation = surge_core::id::WorkItemOperationId::new();
    let command = surge_core::work_item::WorkItemCommand::Suspend {
        operation_id: operation,
        item: attempt.item,
        expected_version: storage
            .work_items()
            .show(attempt.item)
            .unwrap()
            .item
            .version,
    };
    let response = request(&socket, serde_json::to_value(&command).unwrap()).await;
    let admitted = storage
        .work_items()
        .execution_control(attempt.run)
        .unwrap()
        .unwrap();
    assert_eq!(admitted.operation, operation);
    assert_eq!(
        admitted.state,
        surge_core::execution_recovery::ExecutionControlState::SuspendRequested
    );
    assert!(admitted.fence.is_none());
    if response["method"] != "work_item_ok" {
        cancel.cancel();
        server.await.unwrap().unwrap();
        drop(claim);
        panic!("a durable control intent was misreported as definitive rejection: {response}");
    }
    let control = storage
        .work_items()
        .execution_control(attempt.run)
        .unwrap()
        .unwrap();
    assert_eq!(control.operation, operation);
    assert_eq!(
        control.state,
        surge_core::execution_recovery::ExecutionControlState::SuspendRequested
    );
    assert!(
        control.fence.is_none(),
        "accepted pending is not confirmed cleanup"
    );
    assert!(
        control.diagnostic.is_some(),
        "accepted pending retains its failure diagnostic"
    );
    let replay = request(&socket, serde_json::to_value(command).unwrap()).await;
    assert_eq!(replay["method"], "work_item_ok", "{replay}");
    assert_eq!(
        storage
            .work_items()
            .execution_control(attempt.run)
            .unwrap()
            .unwrap()
            .generation,
        control.generation
    );
    cancel.cancel();
    server.await.unwrap().unwrap();
    drop(claim);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pure_human_gate_suspend_is_not_abort_and_reuses_original_decision() {
    pure_gate_suspension_fixture(false, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn forged_suspended_human_gate_request_cannot_authorize_cold_continue() {
    pure_gate_suspension_fixture(true, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn suspended_original_gate_answer_is_durable_before_continue() {
    pure_gate_suspension_fixture(false, true).await;
}

async fn pure_gate_suspension_fixture(forged: bool, answer_while_suspended: bool) {
    pure_gate_suspension_fixture_deadline(forged, answer_while_suspended, false, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn expired_suspended_gate_answer_cannot_reset_original_deadline() {
    pure_gate_suspension_fixture_deadline(false, false, true, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn completed_gate_route_kind_must_match_accepted_graph() {
    pure_gate_suspension_fixture_deadline(false, false, false, true).await;
}

async fn pure_gate_suspension_fixture_deadline(
    forged: bool,
    answer_while_suspended: bool,
    expired: bool,
    forged_route: bool,
) {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let (storage, attempt, _, start) = reserve_fixture_graph_timeout(
        home.path(),
        project.path(),
        false,
        true,
        false,
        expired.then_some(1),
    )
    .await;
    let (_, cancel, server, socket) = cold_host(
        home.path(),
        project.path(),
        storage.clone(),
        Arc::new(mock_bridge::MockBridge::new()),
    )
    .await;
    assert_eq!(
        request(&socket, serde_json::to_value(start).unwrap()).await["method"],
        "work_item_ok"
    );
    let conn = rusqlite::Connection::open(
        home.path()
            .join("runs")
            .join(attempt.run.to_string())
            .join("events.sqlite"),
    )
    .unwrap();
    let original = tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            if let Ok(bytes) = conn.query_row(
                "SELECT payload FROM events WHERE kind='HumanInputRequested' LIMIT 1",
                [],
                |row| row.get::<_, Vec<u8>>(0),
            ) {
                break serde_json::from_slice::<surge_core::VersionedEventPayload>(&bytes).unwrap();
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let suspend = surge_core::work_item::WorkItemCommand::Suspend {
        operation_id: surge_core::id::WorkItemOperationId::new(),
        item: attempt.item,
        expected_version: storage
            .work_items()
            .show(attempt.item)
            .unwrap()
            .item
            .version,
    };
    let response = request(&socket, serde_json::to_value(suspend).unwrap()).await;
    cancel.cancel();
    server.await.unwrap().unwrap();
    assert_eq!(response["method"], "work_item_ok", "{response}");
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM events WHERE kind='RunAborted'",
            [],
            |row| row.get::<_, u64>(0)
        )
        .unwrap(),
        0,
        "Suspend must preserve the unanswered gate, not abort it"
    );
    assert_eq!(
        storage
            .work_items()
            .for_run(attempt.run)
            .unwrap()
            .unwrap()
            .state,
        surge_core::work_item::WorkItemAttemptState::Suspended
    );
    if forged {
        assert!(
            storage.inspect_folded_run(attempt.run).await.is_ok(),
            "valid identical history must be accepted"
        );
        let (seq, bytes): (u64, Vec<u8>) = conn
            .query_row(
                "SELECT seq,payload FROM events WHERE kind='RunSuspended'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        let mut payload: surge_core::VersionedEventPayload =
            serde_json::from_slice(&bytes).unwrap();
        let surge_core::EventPayload::RunSuspended { fence } = &mut payload.payload else {
            panic!("suspension missing")
        };
        let surge_core::execution_recovery::PendingStagePhase::WaitingHumanGate {
            request: fenced_request,
            ..
        } = &mut fence.pending_stage
        else {
            panic!("nonprovider gate phase missing")
        };
        *fenced_request = surge_core::id::GateRequestId::new();
        conn.execute_batch("DROP TRIGGER trg_events_no_update")
            .unwrap();
        conn.execute(
            "UPDATE events SET payload=? WHERE seq=?",
            rusqlite::params![serde_json::to_vec(&payload).unwrap(), seq],
        )
        .unwrap();
        let error = storage
            .inspect_folded_run(attempt.run)
            .await
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("waiting human gate contradicts its original decision"),
            "{error}"
        );
        let (_, cancel, server, socket) = cold_host(
            home.path(),
            project.path(),
            storage.clone(),
            Arc::new(mock_bridge::MockBridge::new()),
        )
        .await;
        let command = surge_core::work_item::WorkItemCommand::Continue {
            operation_id: surge_core::id::WorkItemOperationId::new(),
            item: attempt.item,
            expected_version: storage
                .work_items()
                .show(attempt.item)
                .unwrap()
                .item
                .version,
            new_session: false,
        };
        let response = request(&socket, serde_json::to_value(command).unwrap()).await;
        assert_eq!(response["method"], "work_item_ok", "{response}");
        assert_eq!(
            response["result"]["value"]["state"], "attention",
            "{response}"
        );
        tokio::time::timeout(Duration::from_secs(8), async {
            while storage
                .work_items()
                .for_run(attempt.run)
                .unwrap()
                .unwrap()
                .state
                != surge_core::work_item::WorkItemAttemptState::Attention
            {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM events WHERE kind='RunContinued'",
                [],
                |row| row.get::<_, u64>(0)
            )
            .unwrap(),
            0
        );
        assert_eq!(
            storage
                .work_items()
                .show(attempt.item)
                .unwrap()
                .item
                .active_run,
            Some(attempt.run)
        );
        cancel.cancel();
        server.await.unwrap().unwrap();
        return;
    }
    if socket.exists() {
        std::fs::remove_file(&socket).unwrap();
    }
    let (_, cancel, server, socket) = cold_host(
        home.path(),
        project.path(),
        storage.clone(),
        Arc::new(mock_bridge::MockBridge::new()),
    )
    .await;
    if expired {
        let surge_core::EventPayload::HumanInputRequested {
            node,
            call_id: Some(call_id),
            ..
        } = original.payload()
        else {
            panic!("original gate")
        };
        tokio::time::sleep(Duration::from_millis(1100)).await;
        let answer = surge_orchestrator::engine::ipc::DaemonRequest::ResolveGateInput {
            request_id: 44,
            run_id: attempt.run,
            node: node.clone(),
            gate_request_id: surge_core::id::GateRequestId::from_event_call_id(call_id).unwrap(),
            response: json!({"outcome":"approve"}),
        };
        assert_ne!(
            request_frame(&socket, serde_json::to_value(answer).unwrap()).await["method"],
            "resolve_human_input_ok"
        );
        for kind in [
            "HumanInputResolved",
            "HumanInputTimedOut",
            "GateStageOutcomeCommitted",
            "GateStageRouteCommitted",
            "StageCompleted",
            "RunContinued",
        ] {
            assert_eq!(
                conn.query_row("SELECT COUNT(*) FROM events WHERE kind=?1", [kind], |row| {
                    row.get::<_, u64>(0)
                })
                .unwrap(),
                0,
                "expired paused answer must remain inert: {kind}"
            );
        }
        assert_eq!(
            storage
                .work_items()
                .for_run(attempt.run)
                .unwrap()
                .unwrap()
                .state,
            surge_core::work_item::WorkItemAttemptState::Suspended
        );
        assert_eq!(
            storage
                .work_items()
                .show(attempt.item)
                .unwrap()
                .item
                .active_run,
            Some(attempt.run)
        );
        cancel.cancel();
        server.await.unwrap().unwrap();
        return;
    }
    if answer_while_suspended {
        let surge_core::EventPayload::HumanInputRequested {
            node,
            call_id: Some(call_id),
            ..
        } = original.payload()
        else {
            panic!("original gate")
        };
        for invalid_response in [
            json!({"outcome":""}),
            json!({"outcome":"approve","comment":42}),
        ] {
            let invalid = surge_orchestrator::engine::ipc::DaemonRequest::ResolveGateInput {
                request_id: 40,
                run_id: attempt.run,
                node: node.clone(),
                gate_request_id: surge_core::id::GateRequestId::from_event_call_id(call_id)
                    .unwrap(),
                response: invalid_response,
            };
            assert_ne!(
                request_frame(&socket, serde_json::to_value(invalid).unwrap()).await["method"],
                "resolve_human_input_ok"
            );
            assert_eq!(
                conn.query_row(
                    "SELECT COUNT(*) FROM events WHERE kind='HumanInputResolved'",
                    [],
                    |row| row.get::<_, u64>(0)
                )
                .unwrap(),
                0
            );
        }
        let answer = surge_orchestrator::engine::ipc::DaemonRequest::ResolveGateInput {
            request_id: 41,
            run_id: attempt.run,
            node: node.clone(),
            gate_request_id: surge_core::id::GateRequestId::from_event_call_id(call_id).unwrap(),
            response: json!({"outcome":"approve","comment":"same gate"}),
        };
        let response = request_frame(&socket, serde_json::to_value(answer).unwrap()).await;
        if response["method"] != "resolve_human_input_ok" {
            cancel.cancel();
            server.await.unwrap().unwrap();
            panic!("original operator answer must remain actionable while suspended: {response}");
        }
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM events WHERE kind='HumanInputResolved'",
                [],
                |row| row.get::<_, u64>(0)
            )
            .unwrap(),
            1,
            "success already has durable answer before Continue"
        );
        let conflicting = surge_orchestrator::engine::ipc::DaemonRequest::ResolveGateInput {
            request_id: 43,
            run_id: attempt.run,
            node: node.clone(),
            gate_request_id: surge_core::id::GateRequestId::from_event_call_id(call_id).unwrap(),
            response: json!({"outcome":"approve","comment":"different answer"}),
        };
        assert_ne!(
            request_frame(&socket, serde_json::to_value(conflicting).unwrap()).await["method"],
            "resolve_human_input_ok"
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM events WHERE kind='HumanInputResolved'",
                [],
                |row| row.get::<_, u64>(0)
            )
            .unwrap(),
            1
        );
        for kind in [
            "GateStageOutcomeCommitted",
            "GateStageRouteCommitted",
            "StageCompleted",
            "EdgeTraversed",
            "RunContinued",
        ] {
            assert_eq!(
                conn.query_row("SELECT COUNT(*) FROM events WHERE kind=?1", [kind], |row| {
                    row.get::<_, u64>(0)
                })
                .unwrap(),
                0,
                "answer must not execute or route while suspended: {kind}"
            );
        }
    }
    let continue_command = surge_core::work_item::WorkItemCommand::Continue {
        operation_id: surge_core::id::WorkItemOperationId::new(),
        item: attempt.item,
        new_session: false,
        expected_version: storage
            .work_items()
            .show(attempt.item)
            .unwrap()
            .item
            .version,
    };
    assert_eq!(
        request(&socket, serde_json::to_value(continue_command).unwrap()).await["method"],
        "work_item_ok"
    );
    let surge_core::EventPayload::HumanInputRequested {
        node,
        call_id: Some(call_id),
        ..
    } = original.payload()
    else {
        panic!("original gate identity missing")
    };
    let answer = surge_orchestrator::engine::ipc::DaemonRequest::ResolveGateInput {
        request_id: 42,
        run_id: attempt.run,
        node: node.clone(),
        gate_request_id: surge_core::id::GateRequestId::from_event_call_id(call_id).unwrap(),
        response: json!({"outcome":"approve","comment":"same gate"}),
    };
    assert_eq!(
        request_frame(&socket, serde_json::to_value(answer).unwrap()).await["method"],
        "resolve_human_input_ok"
    );
    tokio::time::timeout(Duration::from_secs(8), async {
        while storage
            .work_items()
            .for_run(attempt.run)
            .unwrap()
            .unwrap()
            .state
            != surge_core::work_item::WorkItemAttemptState::Completed
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM events WHERE kind='HumanInputRequested'",
            [],
            |row| row.get::<_, u64>(0)
        )
        .unwrap(),
        1
    );
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM events WHERE kind='SessionOpened'",
            [],
            |row| row.get::<_, u64>(0)
        )
        .unwrap(),
        0
    );
    if forged_route {
        storage.inspect_folded_run(attempt.run).await.unwrap();
        let (seq, bytes) = conn
            .query_row(
                "SELECT seq,payload FROM events WHERE kind='EdgeTraversed' LIMIT 1",
                [],
                |row| Ok((row.get::<_, u64>(0)?, row.get::<_, Vec<u8>>(1)?)),
            )
            .unwrap();
        let mut payload: surge_core::VersionedEventPayload =
            serde_json::from_slice(&bytes).unwrap();
        let surge_core::EventPayload::EdgeTraversed { kind, .. } = &mut payload.payload else {
            panic!("actual route")
        };
        assert_eq!(*kind, surge_core::edge::EdgeKind::Forward);
        *kind = surge_core::edge::EdgeKind::Backtrack;
        conn.execute_batch("DROP TRIGGER trg_events_no_update")
            .unwrap();
        conn.execute(
            "UPDATE events SET payload=? WHERE seq=?",
            rusqlite::params![serde_json::to_vec(&payload).unwrap(), seq],
        )
        .unwrap();
        let result = storage.inspect_folded_run(attempt.run).await;
        cancel.cancel();
        server.await.unwrap().unwrap();
        assert!(
            matches!(result,Err(error) if error.to_string().contains("routing edge contradicts accepted graph and traversal policy")),
            "self-consistent route identity must not authorize a forged route kind"
        );
        return;
    }
    cancel.cancel();
    server.await.unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn revisited_human_gate_requires_new_occurrence_and_new_decision() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let (storage, attempt, _, start) =
        reserve_fixture_graph(home.path(), project.path(), false, true, true).await;
    let (_, cancel, server, socket) = cold_host(
        home.path(),
        project.path(),
        storage.clone(),
        Arc::new(mock_bridge::MockBridge::new()),
    )
    .await;
    assert_eq!(
        request(&socket, serde_json::to_value(start).unwrap()).await["method"],
        "work_item_ok"
    );
    let conn = rusqlite::Connection::open(
        home.path()
            .join("runs")
            .join(attempt.run.to_string())
            .join("events.sqlite"),
    )
    .unwrap();
    let mut identities = Vec::new();
    for (index, outcome) in ["edit", "approve"].into_iter().enumerate() {
        let observed = tokio::time::timeout(
            Duration::from_secs(5),
            wait_for_human_input(&conn, &storage, attempt.run, index),
        )
        .await;
        if observed.as_ref().is_err() || observed.as_ref().is_ok_and(|(seq, _)| *seq == 0) {
            cancel.cancel();
            server.await.unwrap().unwrap();
            panic!(
                "visit {index} must produce a fresh actionable decision; state={:?}",
                storage.work_items().for_run(attempt.run).unwrap()
            );
        }
        let (seq, bytes) = observed.unwrap();
        let payload: surge_core::VersionedEventPayload = serde_json::from_slice(&bytes).unwrap();
        let surge_core::EventPayload::HumanInputRequested {
            node,
            call_id: Some(call_id),
            ..
        } = payload.payload()
        else {
            panic!("request")
        };
        let id = surge_core::id::GateRequestId::from_event_call_id(call_id).unwrap();
        identities.push((seq, id));
        let answer = surge_orchestrator::engine::ipc::DaemonRequest::ResolveGateInput {
            request_id: index as u64 + 90,
            run_id: attempt.run,
            node: node.clone(),
            gate_request_id: id,
            response: json!({"outcome":outcome,"comment":"exact visit"}),
        };
        assert_eq!(
            request_frame(&socket, serde_json::to_value(answer).unwrap()).await["method"],
            "resolve_human_input_ok"
        );
    }
    assert_ne!(identities[0].1, identities[1].1);
    assert!(identities[1].0 > identities[0].0);
    tokio::time::timeout(Duration::from_secs(5), async {
        while storage
            .work_items()
            .for_run(attempt.run)
            .unwrap()
            .unwrap()
            .state
            != surge_core::work_item::WorkItemAttemptState::Completed
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM events WHERE kind='HumanInputRequested'",
            [],
            |row| row.get::<_, u64>(0)
        )
        .unwrap(),
        2
    );
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM events WHERE kind='HumanInputResolved'",
            [],
            |row| row.get::<_, u64>(0)
        )
        .unwrap(),
        2
    );
    cancel.cancel();
    server.await.unwrap().unwrap();
}
