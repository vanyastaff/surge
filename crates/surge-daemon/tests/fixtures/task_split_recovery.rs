//! Cold process recovery of a real automatically derived TaskSplit planner.
use super::*;
use futures::FutureExt as _;
use std::collections::BTreeMap;
use surge_core::EventPayload;
use surge_core::agent_config::AgentConfig;
use surge_core::edge::{Edge, EdgeKind, EdgePolicy, ExceededAction, PortRef};
use surge_core::graph::{Graph, GraphMetadata, SCHEMA_VERSION};
use surge_core::keys::{EdgeKey, NodeKey, OutcomeKey, ProfileKey};
use surge_core::node::{Node, NodeConfig, OutcomeDecl, Position};
use surge_core::terminal_config::{TerminalConfig, TerminalKind};
fn outcome(id: &str, kind: EdgeKind) -> OutcomeDecl {
    OutcomeDecl {
        id: OutcomeKey::try_from(id).unwrap(),
        description: id.into(),
        edge_kind_hint: kind,
        is_terminal: false,
        ledger_effect: if id == "done" {
            surge_core::node::LedgerEffect::ReadyForVerification
        } else {
            Default::default()
        },
    }
}

fn agent(id: &str, outcomes: Vec<OutcomeDecl>) -> Node {
    Node {
        id: NodeKey::try_from(id).unwrap(),
        position: Position::default(),
        declared_outcomes: outcomes,
        config: NodeConfig::Agent(AgentConfig {
            profile: ProfileKey::try_from("implementer@1.0").unwrap(),
            prompt_overrides: None,
            tool_overrides: None,
            sandbox_override: None,
            approvals_override: None,
            bindings: vec![],
            rules_overrides: None,
            limits: surge_core::agent_config::NodeLimits::default(),
            hooks: vec![],
            custom_fields: BTreeMap::new(),
        }),
    }
}

fn edge(id: &str, from: &str, outcome: &str, to: &str, kind: EdgeKind) -> Edge {
    Edge {
        id: EdgeKey::try_from(id).unwrap(),
        from: PortRef {
            node: NodeKey::try_from(from).unwrap(),
            outcome: OutcomeKey::try_from(outcome).unwrap(),
        },
        to: NodeKey::try_from(to).unwrap(),
        kind,
        policy: EdgePolicy::default(),
    }
}

/// implement --done--> verify --passed--> end; verify --failed--> implement
/// (backtrack, at most once, escalate when exhausted, no declared route).
fn retry_loop_graph() -> Graph {
    let mut nodes = BTreeMap::new();
    for node in [
        agent("implement", vec![outcome("done", EdgeKind::Forward)]),
        agent(
            "verify",
            vec![
                outcome("passed", EdgeKind::Forward),
                outcome("failed", EdgeKind::Backtrack),
            ],
        ),
        Node {
            id: NodeKey::try_from("end").unwrap(),
            position: Position::default(),
            declared_outcomes: vec![],
            config: NodeConfig::Terminal(TerminalConfig {
                kind: TerminalKind::Success,
                message: None,
            }),
        },
    ] {
        nodes.insert(node.id.clone(), node);
    }
    let mut retry = edge(
        "e_retry",
        "verify",
        "failed",
        "implement",
        EdgeKind::Backtrack,
    );
    retry.policy.max_traversals = Some(1);
    retry.policy.on_max_exceeded = ExceededAction::Escalate;
    Graph {
        schema_version: SCHEMA_VERSION,
        metadata: GraphMetadata::new("escalation-gate-harness", chrono::Utc::now()),
        start: NodeKey::try_from("implement").unwrap(),
        nodes,
        edges: vec![
            edge("e_done", "implement", "done", "verify", EdgeKind::Forward),
            edge("e_passed", "verify", "passed", "end", EdgeKind::Forward),
            retry,
        ],
        subgraphs: BTreeMap::new(),
    }
}

fn task_loop_graph() -> Graph {
    use surge_core::graph::Subgraph;
    use surge_core::keys::SubgraphKey;
    use surge_core::loop_config::{
        ExitCondition, FailurePolicy, IterableSource, LoopConfig, ParallelismMode,
    };
    let item = |text: &str| -> toml::Value { toml::from_str(text).unwrap() };
    let body_key = SubgraphKey::try_from("task_body").unwrap();
    let mut body = retry_loop_graph();
    body.nodes.remove(&NodeKey::try_from("end").unwrap());
    body.nodes.insert(
        NodeKey::try_from("task_end").unwrap(),
        Node {
            id: NodeKey::try_from("task_end").unwrap(),
            position: Position::default(),
            declared_outcomes: vec![],
            config: NodeConfig::Terminal(TerminalConfig {
                kind: TerminalKind::Success,
                message: None,
            }),
        },
    );
    for edge in &mut body.edges {
        if edge.to.as_str() == "end" {
            edge.to = NodeKey::try_from("task_end").unwrap();
        }
    }
    let mut nodes = BTreeMap::new();
    nodes.insert(
        NodeKey::try_from("task_loop").unwrap(),
        Node {
            id: NodeKey::try_from("task_loop").unwrap(),
            position: Position::default(),
            declared_outcomes: vec![outcome("completed", EdgeKind::Forward)],
            config: NodeConfig::Loop(LoopConfig {
                iterates_over: IterableSource::Static(vec![
                    item("id = 'login'\ntitle = 'Login form'\nacceptance_criteria = ['Valid users sign in', 'Empty passwords are rejected']"),
                    item("id = 'logout'\ntitle = 'Logout'"),
                ]),
                body: body_key.clone(),
                iteration_var_name: "task".into(),
                exit_condition: ExitCondition::AllItems,
                on_iteration_failure: FailurePolicy::Abort,
                parallelism: ParallelismMode::Sequential,
                gate_after_each: false,
            }),
        },
    );
    nodes.insert(
        NodeKey::try_from("end").unwrap(),
        Node {
            id: NodeKey::try_from("end").unwrap(),
            position: Position::default(),
            declared_outcomes: vec![],
            config: NodeConfig::Terminal(TerminalConfig {
                kind: TerminalKind::Success,
                message: None,
            }),
        },
    );
    Graph {
        schema_version: SCHEMA_VERSION,
        metadata: GraphMetadata::new("task-split-harness", chrono::Utc::now()),
        start: NodeKey::try_from("task_loop").unwrap(),
        nodes,
        edges: vec![edge(
            "e_loop_done",
            "task_loop",
            "completed",
            "end",
            EdgeKind::Forward,
        )],
        subgraphs: BTreeMap::from([(
            body_key,
            Subgraph {
                start: body.start,
                nodes: body.nodes,
                edges: body.edges,
            },
        )]),
    }
}

const SPLIT_TASKS: &str = r#"schema_version = 1

[[tasks]]
id = "login-a"
title = "Sign in valid users"
acceptance_criteria = ["Valid users sign in"]

[[tasks]]
id = "login-b"
title = "Reject empty passwords"
description = "The part the verifier rejected."
acceptance_criteria = ["Empty passwords are rejected"]
"#;

fn flags(home: &Path) -> Vec<String> {
    let mut flags = commit_wire_flags(home);
    flags.extend([
        "--stage-script".into(),
        home.join("split-script.json").display().to_string(),
    ]);
    flags
}

fn config(home: &Path, bridge: &wire_bridge::WireBridge) -> EngineConfig {
    use surge_orchestrator::profile_loader::{DiskProfileSet, ProfileRegistry};
    let mut config = bridge.engine_config(home);
    let profiles = home.join("wire-fixture-profiles");
    let splitter = include_str!("../../../surge-core/bundled/profiles/task-splitter-1.0.toml")
        .replace("agent_id = \"claude-code\"", "agent_id = \"wire-fixture\"");
    std::fs::write(profiles.join("task-splitter-1.0.toml"), splitter).unwrap();
    config.profile_registry = Some(Arc::new(ProfileRegistry::new(
        DiskProfileSet::scan(&profiles).unwrap(),
    )));
    config
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn child_split_host_probe() {
    let Some(home) = std::env::var_os("SURGE_TEST_SPLIT_HOME") else {
        return;
    };
    let home = std::path::PathBuf::from(home);
    let project = std::path::PathBuf::from(std::env::var_os("SURGE_TEST_SPLIT_PROJECT").unwrap());
    let storage = Storage::open(&home).await.unwrap();
    let start: surge_core::work_item::WorkItemCommand =
        serde_json::from_slice(&std::fs::read(home.join("child-start.json")).unwrap()).unwrap();
    let run: surge_core::RunId = std::fs::read_to_string(home.join("child-run.txt"))
        .unwrap()
        .parse()
        .unwrap();
    let bridge = Arc::new(wire_bridge::WireBridge {
        bridge: surge_acp::bridge::AcpBridge::with_defaults().unwrap(),
        flags: flags(&home),
        session_commit_check: Some((storage.clone(), run)),
    });
    let config = config(&home, &bridge);
    let (_engine, _cancel, _server, socket) =
        cold_host_with_config(&home, &project, storage, bridge, config).await;
    let response = request(&socket, serde_json::to_value(start).unwrap()).await;
    assert_eq!(response["method"], "work_item_ok", "{response}");
    std::future::pending::<()>().await;
}

/// Stops only fixture-owned processes authenticated by actual SessionOpened records,
/// including every historical provider session if a test assertion unwinds.
struct OwnedSplitHost {
    child: ContinueFixtureChild,
    journal: std::path::PathBuf,
    settled: bool,
}
impl OwnedSplitHost {
    fn stop(&mut self) -> Vec<surge_acp::process_evidence::GroupState> {
        if self.settled {
            return Vec::new();
        }
        let mut states = Vec::new();
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Ok(connection) = rusqlite::Connection::open_with_flags(
            &self.journal,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        ) && let Ok(mut query) =
            connection.prepare("SELECT payload FROM events WHERE kind='SessionOpened'")
            && let Ok(rows) = query.query_map([], |row| row.get::<_, Vec<u8>>(0))
        {
            for bytes in rows.flatten() {
                let Ok(event) = serde_json::from_slice::<surge_core::VersionedEventPayload>(&bytes)
                else {
                    continue;
                };
                if let EventPayload::SessionOpened {
                    opened: Some(opened),
                    ..
                } = event.payload
                    && let Some(container) = opened
                        .execution_writer
                        .as_ref()
                        .and_then(|writer| writer.container())
                {
                    states.push(surge_acp::process_evidence::stop_group(
                        container,
                        Duration::from_secs(1),
                    ));
                }
            }
        }
        self.settled = states.len() == 7
            && states
                .iter()
                .all(|state| *state == surge_acp::process_evidence::GroupState::Empty);
        states
    }
}
impl Drop for OwnedSplitHost {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

#[derive(Clone, Copy)]
enum Cut {
    Accepted,
    Routed,
}
#[derive(Clone, Copy)]
enum Damage {
    None,
    MutableSource,
    MissingBlob,
    CorruptBlob,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn accepted_split_recovers_children_once_with_fresh_retry_budget() {
    run_fixture(Cut::Accepted, Damage::None).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn routed_split_checkpoint_owns_children_without_artifact() {
    run_fixture(Cut::Routed, Damage::MissingBlob).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn accepted_split_uses_captured_blob_after_source_changes() {
    run_fixture(Cut::Accepted, Damage::MutableSource).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn accepted_split_missing_blob_stays_attention_without_dispatch() {
    run_fixture(Cut::Accepted, Damage::MissingBlob).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn accepted_split_corrupt_blob_stays_attention_without_dispatch() {
    run_fixture(Cut::Accepted, Damage::CorruptBlob).await;
}

async fn journal(
    storage: &Arc<Storage>,
    run: surge_core::RunId,
) -> Vec<surge_persistence::runs::ReadEvent> {
    let inspected = match storage.inspect_run(run).await {
        Ok(inspected) => inspected,
        Err(error) if error.to_string().contains("no such table: events") => return Vec::new(),
        Err(error) => panic!("split journal inspection: {error}"),
    };
    let surge_persistence::runs::inspection::RunDatabaseInspection::Present { events } =
        inspected.database
    else {
        return Vec::new();
    };
    events
}

async fn run_fixture(cut: Cut, damage: Damage) {
    let home = FixtureHome::new().unwrap();
    run_in_home(&home, cut, damage).await;
    home.close().unwrap();
}

async fn run_in_home(home: &FixtureHome, cut: Cut, damage: Damage) {
    let project = tempfile::tempdir().unwrap();
    let (storage, attempt, workspace, start) = reserve_fixture_with_graph(
        home.path(),
        project.path(),
        true,
        false,
        false,
        None,
        Some(task_loop_graph()),
    )
    .await;
    std::fs::write(
        home.path().join("child-start.json"),
        serde_json::to_vec(&start).unwrap(),
    )
    .unwrap();
    std::fs::write(home.path().join("child-run.txt"), attempt.run.to_string()).unwrap();
    let script: Vec<Value> = [
        ("Login form", "done"),
        ("Login form", "failed"),
        ("Login form", "done"),
        ("Login form", "failed"),
        ("Login form", "done"),
        ("Login form", "failed"),
        ("Task Splitter", "split"),
        ("Sign in valid users", "done"),
        ("Sign in valid users", "failed"),
        ("Sign in valid users", "done"),
        ("Sign in valid users", "passed"),
        ("Reject empty passwords", "done"),
        ("Reject empty passwords", "passed"),
        ("Logout", "done"),
        ("Logout", "passed"),
    ]
    .into_iter()
    .enumerate()
    .map(|(index, (prompt, outcome))| {
        let mut turn = json!({"prompt_contains":prompt, "outcome":outcome});
        if index == 6 {
            turn["split_artifact"] = json!(SPLIT_TASKS);
        }
        turn
    })
    .collect();
    std::fs::write(
        home.path().join("split-script.json"),
        serde_json::to_vec(&script).unwrap(),
    )
    .unwrap();
    let barrier = home.path().join("split-close");
    let output = std::fs::File::create(home.path().join("split-child.log")).unwrap();
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "task_split_recovery::child_split_host_probe",
            "--nocapture",
        ])
        .env("SURGE_TEST_SPLIT_HOME", home.path())
        .env("SURGE_TEST_SPLIT_PROJECT", project.path())
        .env("SURGE_TEST_CLOSE_BARRIER", &barrier)
        .env("SURGE_TEST_CLOSE_NODE", "verify_split")
        .stdout(output.try_clone().unwrap())
        .stderr(output);
    if matches!(cut, Cut::Routed) {
        command.env("SURGE_ROUTE_COMMIT_EXIT", "verify_split");
    }
    let mut owner = OwnedSplitHost {
        child: ContinueFixtureChild(command.spawn().unwrap()),
        settled: false,
        journal: home
            .path()
            .join("runs")
            .join(attempt.run.to_string())
            .join("events.sqlite"),
    };
    let accepted = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            assert!(
                owner.child.try_wait().unwrap().is_none(),
                "child exited: {}",
                bounded_child_log(&home.path().join("split-child.log"))
            );
            if barrier.with_extension("ready").exists() {
                // The selected close barrier follows durable acceptance. Read a
                // fresh transaction only after observing that ordering marker.
                break journal(&storage, attempt.run).await;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|error| {
        panic!(
            "actual planner close cut: {error}; child={}; wire={}; trace={}; cursor={:?}; home={}",
            bounded_child_log(&home.path().join("split-child.log")),
            bounded_child_log(&home.path().join("commit-provider-wire.jsonl")),
            bounded_child_log(&home.path().join("cold.trace.log")),
            std::fs::read_to_string(home.path().join("split-script.cursor")),
            home.path().display()
        )
    });
    let accepted_envelopes = accepted
        .iter()
        .map(|event| event.to_run_event(attempt.run).unwrap())
        .collect::<Vec<_>>();
    let before_view =
        surge_core::task_execution::TaskExecutionView::compile(attempt.run, &accepted_envelopes)
            .unwrap();
    assert_eq!(
        before_view
            .tasks
            .iter()
            .map(|task| task.task_id.as_str())
            .collect::<Vec<_>>(),
        ["login", "login-a", "login-b"]
    );
    assert!(
        before_view.tasks[1..]
            .iter()
            .all(|task| task.started.is_none() && task.split.is_none())
    );
    let child_anchor = before_view.tasks[1].id;
    let before_planner = before_view
        .stages
        .iter()
        .find(|stage| stage.node.as_str() == "verify_split")
        .unwrap();
    assert_eq!(before_planner.sessions.len(), 1);
    assert_eq!(before_planner.accepted.len(), 1);
    assert!(before_planner.accepted[0].routed.is_none());
    let planner_acceptance = before_planner.accepted[0].seq;

    let commit = accepted
        .iter()
        .find_map(|event| match event.payload.payload() {
            EventPayload::StageOutcomeCommitted { commit }
                if commit.context().node.as_str() == "verify_split" =>
            {
                Some(commit.clone())
            },
            _ => None,
        })
        .expect("actual selected planner acceptance");
    assert_eq!(commit.outcome().as_str(), "split");
    assert_eq!(accepted.iter().filter(|event| matches!(event.payload.payload(), EventPayload::StageOutcomeCommitted { commit } if commit.context().node.as_str()=="verify_split")).count(), 1);
    assert!(
        !accepted
            .iter()
            .any(|event| matches!(event.payload.payload(), EventPayload::TaskSplit { .. }))
    );
    let blob = accepted
        .iter()
        .find_map(|event| match event.payload.payload() {
            EventPayload::ArtifactProduced {
                node, path, name, ..
            } if node.as_str() == "verify_split" && name == "discovered-tasks" => {
                Some(path.clone())
            },
            _ => None,
        })
        .expect("captured split artifact");
    if matches!(cut, Cut::Routed) {
        std::fs::write(&barrier, b"release").unwrap();
        let exit = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let Some(exit) = owner.child.try_wait().unwrap() {
                    break exit;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(exit.code(), Some(99));
        let events = journal(&storage, attempt.run).await;
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event.payload.payload(), EventPayload::TaskSplit { .. }))
                .count(),
            1
        );
        assert!(events.iter().any(|event| matches!(event.payload.payload(), EventPayload::StageRouteCommitted { invocation, .. } if *invocation == commit.invocation())));
    }
    let (owner, stopped) = tokio::task::spawn_blocking(move || {
        let stopped = owner.stop();
        (owner, stopped)
    })
    .await
    .unwrap();
    assert_eq!(
        stopped.len(),
        7,
        "all actual pre-cut provider sessions owned"
    );
    assert!(
        stopped
            .iter()
            .all(|state| *state == surge_acp::process_evidence::GroupState::Empty),
        "actual provider cleanup: {stopped:?}"
    );
    match damage {
        Damage::None => {},
        Damage::MutableSource => std::fs::write(
            workspace.path.join("discovered-tasks.toml"),
            SPLIT_TASKS.replace("login-a", "attacker-task"),
        )
        .unwrap(),
        Damage::MissingBlob => std::fs::remove_file(&blob).unwrap(),
        Damage::CorruptBlob => std::fs::write(&blob, b"corrupt captured split").unwrap(),
    }
    drop(storage);
    let storage = Storage::open(home.path()).await.unwrap();
    let bridge = Arc::new(wire_bridge::WireBridge {
        bridge: surge_acp::bridge::AcpBridge::with_defaults().unwrap(),
        flags: flags(home.path()),
        session_commit_check: Some((storage.clone(), attempt.run)),
    });
    let config = config(home.path(), &bridge);
    let (engine, cancel, server, _) = cold_host_with_config(
        home.path(),
        project.path(),
        storage.clone(),
        bridge.clone(),
        config,
    )
    .await;
    // Observation failures must not detach the live recovery daemon or leave
    // its provider writers running while the fixture home unwinds.
    let recovery_started = std::time::Instant::now();
    let observed = std::panic::AssertUnwindSafe(async {
        let settled = tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                let state = storage
                    .work_items()
                    .for_run(attempt.run)
                    .unwrap()
                    .unwrap()
                    .state;
                if !state.is_active()
                    || state == surge_core::work_item::WorkItemAttemptState::Attention
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        if let Err(error) = settled {
            let history = storage.inspect_run(attempt.run).await.map(|inspection| match inspection.database {
                surge_persistence::runs::inspection::RunDatabaseInspection::Present { events } => events
                    .into_iter().rev().take(32).map(|event| (event.seq,event.kind)).collect::<Vec<_>>(),
                surge_persistence::runs::inspection::RunDatabaseInspection::Absent => Vec::new(),
            });
            let active = tokio::time::timeout(Duration::from_secs(1), engine.snapshot_active_runs()).await;
            panic!(
                "recovered split did not settle: {error}; elapsed={:?}; attempt={:?}; journal={history:?}; active={active:?}; wire={}; trace={}",
                recovery_started.elapsed(), storage.work_items().for_run(attempt.run),
                bounded_child_log(&home.path().join("commit-provider-wire.jsonl")),
                bounded_child_log(&home.path().join("cold-1.trace.log"))
            );
        }
    })
    .catch_unwind()
    .await;
    cancel.cancel();
    let server_result = server.await;
    drop(unwrap_settled_fixture_owner(engine).await);
    settle_fixture_wire(bridge).await;
    server_result.unwrap().unwrap();
    if let Err(panic) = observed {
        std::panic::resume_unwind(panic);
    }
    let events = journal(&storage, attempt.run).await;
    let recovered_envelopes = events
        .iter()
        .map(|event| event.to_run_event(attempt.run).unwrap())
        .collect::<Vec<_>>();
    let execution =
        surge_core::task_execution::TaskExecutionView::compile(attempt.run, &recovered_envelopes)
            .unwrap();
    assert_eq!(execution.tasks[1].id, child_anchor);
    assert_eq!(
        execution.tasks[1]
            .discovered_from
            .as_ref()
            .map(|id| id.as_str()),
        Some("login")
    );
    for stage in execution
        .stages
        .iter()
        .filter(|stage| matches!(stage.node.as_str(), "implement" | "verify" | "verify_split"))
    {
        assert_eq!(
            stage.sessions.len(),
            1,
            "actual agent occurrence has one recorded provider connection"
        );
        assert!(stage.sessions.iter().all(|session| {
            session
                .authority
                .as_ref()
                .is_some_and(|authority| authority.node == stage.node)
                && session.provider.as_ref().is_some_and(|opened| {
                    Some(opened.descriptor.invocation()) == session.invocation
                })
        }));
    }
    let fail_closed =
        matches!(cut, Cut::Accepted) && matches!(damage, Damage::MissingBlob | Damage::CorruptBlob);
    assert_eq!(
        execution
            .tasks
            .iter()
            .map(|task| task.task_id.as_str())
            .collect::<Vec<_>>(),
        if fail_closed {
            vec!["login", "login-a", "login-b"]
        } else {
            vec!["login", "login-a", "login-b", "logout"]
        }
    );
    assert_eq!(execution.tasks[1].started.is_none(), fail_closed);
    assert_eq!(execution.tasks[1].split.is_none(), fail_closed);
    if !fail_closed {
        assert_eq!(
            execution.tasks[1].acceptance_criteria,
            ["Valid users sign in"]
        );
        let first_child_stages = execution
            .stages
            .iter()
            .filter(|stage| stage.task == Some(child_anchor) && stage.node.as_str() == "implement")
            .collect::<Vec<_>>();
        assert_eq!(first_child_stages.len(), 2);
        assert_ne!(first_child_stages[0].entered, first_child_stages[1].entered);
    }
    let planner = execution
        .stages
        .iter()
        .find(|stage| stage.node.as_str() == "verify_split")
        .unwrap();
    assert_eq!(planner.accepted.len(), 1);
    assert_eq!(planner.accepted[0].seq, planner_acceptance);
    assert_eq!(planner.accepted[0].commit.invocation(), commit.invocation());
    assert_eq!(planner.accepted[0].routed.is_none(), fail_closed);
    assert_eq!(
        execution.tasks[2]
            .discovered_from
            .as_ref()
            .map(|id| id.as_str()),
        Some("login")
    );
    if !fail_closed {
        assert_eq!(
            execution.tasks[2].acceptance_criteria,
            ["Empty passwords are rejected"]
        );
        assert_eq!(
            execution.tasks[2].description.as_deref(),
            Some("The part the verifier rejected.")
        );
        let second_child_stages = execution
            .stages
            .iter()
            .filter(|stage| {
                stage.task == Some(execution.tasks[2].id) && stage.node.as_str() == "implement"
            })
            .collect::<Vec<_>>();
        assert_eq!(second_child_stages.len(), 1);
        assert_eq!(second_child_stages[0].sessions.len(), 1);
    }
    let splits: Vec<_> = events
        .iter()
        .filter_map(|event| match event.payload.payload() {
            EventPayload::TaskSplit { task, into, .. } => Some((task, into)),
            _ => None,
        })
        .collect();
    let turns: usize = std::fs::read_to_string(home.path().join("split-script.cursor"))
        .unwrap()
        .parse()
        .unwrap();
    if fail_closed {
        assert!(splits.is_empty());
        assert_eq!(turns, 7);
        assert_eq!(
            storage
                .work_items()
                .for_run(attempt.run)
                .unwrap()
                .unwrap()
                .state,
            surge_core::work_item::WorkItemAttemptState::Attention
        );
    } else {
        assert_eq!(splits.len(), 1);
        assert_eq!(splits[0].0.as_deref(), Some("login"));
        let expected: Vec<toml::Value> = vec![toml::from_str("id='login-a'\ntitle='Sign in valid users'\nacceptance_criteria=['Valid users sign in']\ndiscovered_from='login'").unwrap(), toml::from_str("id='login-b'\ntitle='Reject empty passwords'\ndescription='The part the verifier rejected.'\nacceptance_criteria=['Empty passwords are rejected']\ndiscovered_from='login'").unwrap()];
        assert_eq!(*splits[0].1, expected);
        assert_eq!(turns, 15);
        assert!(
            events
                .iter()
                .any(|event| matches!(event.payload.payload(), EventPayload::RunCompleted { .. }))
        );
    }
    assert_eq!(events.iter().filter(|event| matches!(event.payload.payload(), EventPayload::StageOutcomeCommitted { commit } if commit.context().node.as_str()=="verify_split")).count(), 1);
    let wire: Vec<Value> = std::fs::read_to_string(home.path().join("commit-provider-wire.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(
        wire.iter()
            .filter(|row| row["operation"] == "prompt")
            .count(),
        turns
    );
    let planner_routes = events.iter().filter(|event| matches!(event.payload.payload(), EventPayload::StageRouteCommitted { invocation, .. } if *invocation == commit.invocation())).count();
    assert_eq!(planner_routes, usize::from(!fail_closed));
    let retries = events.iter().filter(|event| matches!(event.payload.payload(), EventPayload::EdgeTraversed { edge, .. } if edge.as_str() == "e_retry")).count();
    assert_eq!(
        retries,
        if fail_closed { 1 } else { 2 },
        "child login-a must receive its own bounded retry"
    );
    assert!(!events.iter().any(|event| matches!(
        event.payload.payload(),
        EventPayload::HumanInputRequested { .. }
    )));
    let planner_prompts = wire
        .iter()
        .filter(|row| {
            row["operation"] == "prompt"
                && row["request"]["prompt"]
                    .to_string()
                    .contains("You are the Task Splitter")
        })
        .count();
    assert_eq!(planner_prompts, 1, "accepted planner must never replay");
    let started: Vec<_> = events
        .iter()
        .filter_map(|event| match event.payload.payload() {
            EventPayload::LoopIterationStarted { item, .. } => Some(item["id"].as_str().unwrap()),
            _ => None,
        })
        .collect();
    assert_eq!(
        started,
        if fail_closed {
            vec!["login"]
        } else {
            vec!["login", "login-a", "login-b", "logout"]
        }
    );
    storage
        .inspect_folded_run(attempt.run)
        .await
        .expect("recovered split journal remains trusted");
    // RunState::Terminal intentionally drops memory. Inspect the genuine prefix
    // immediately before its terminal marker to check the durable ledger projection.
    let prefix: Vec<_> = events
        .iter()
        .take_while(|event| {
            !matches!(
                event.payload.payload(),
                EventPayload::RunCompleted { .. }
                    | EventPayload::RunFailed { .. }
                    | EventPayload::RunAborted { .. }
            )
        })
        .map(|event| surge_core::RunEvent {
            run_id: attempt.run,
            seq: event.seq.as_u64(),
            timestamp: chrono::DateTime::from_timestamp_millis(event.timestamp_ms).unwrap(),
            payload: event.payload.payload().clone(),
        })
        .collect();
    let surge_core::RunState::Pipeline { memory, .. } =
        surge_core::run_state::fold(&prefix).unwrap()
    else {
        panic!("pre-terminal pipeline ledger")
    };
    let ids: Vec<_> = memory.ledger.tasks.keys().map(|id| id.as_str()).collect();
    assert_eq!(
        ids,
        if fail_closed {
            vec!["login", "login-a", "login-b"]
        } else {
            vec!["login", "login-a", "login-b", "logout"]
        }
    );
    {
        for id in ["login-a", "login-b"] {
            let child = memory
                .ledger
                .tasks
                .iter()
                .find(|(key, _)| key.as_str() == id)
                .unwrap()
                .1;
            assert_eq!(
                child.discovered_from.as_ref().map(|origin| origin.as_str()),
                Some("login")
            );
        }
    }
    for event in &events {
        if let EventPayload::SessionOpened {
            opened: Some(opened),
            ..
        } = event.payload.payload()
            && let Some(container) = opened
                .execution_writer
                .as_ref()
                .and_then(|writer| writer.container())
        {
            assert_eq!(
                surge_acp::process_evidence::group_state(container),
                surge_acp::process_evidence::GroupState::Empty,
                "actual provider group remains after settled owners"
            );
        }
    }

    drop(storage);
    drop(owner);
}
