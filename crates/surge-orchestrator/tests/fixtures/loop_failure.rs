use std::sync::Arc;
use surge_core::loop_config::FailurePolicy;
use surge_core::node::NodeConfig;
use surge_core::run_event::EventPayload;
use surge_core::terminal_config::TerminalKind;
use surge_orchestrator::engine::tools::worktree::WorktreeToolDispatcher;
use surge_orchestrator::engine::{Engine, EngineConfig, EngineRunConfig, RunOutcome};
use surge_persistence::runs::{EventSeq, Storage};

#[path = "static_loop_graph.rs"]
mod static_loop_graph;

pub async fn failing_body(policy: FailurePolicy) -> (RunOutcome, Vec<EventPayload>) {
    let dir = tempfile::tempdir().unwrap();
    let home = crate::runtime_home_fixture::FixtureHome::new().unwrap();
    let storage = Storage::open(home.path()).await.unwrap();
    let bridge = Arc::new(super::fixtures::mock_bridge::MockBridge::new());
    let engine = Engine::new(
        bridge,
        storage.clone(),
        Arc::new(WorktreeToolDispatcher::new(dir.path().into())),
        EngineConfig::default(),
    );
    let mut graph = static_loop_graph::build_static_loop_graph();
    let NodeConfig::Loop(config) = &mut graph.nodes.get_mut(&graph.start).unwrap().config else {
        panic!("expected loop");
    };
    config.on_iteration_failure = policy;
    let body = graph.subgraphs.values_mut().next().unwrap();
    let NodeConfig::Terminal(config) = &mut body.nodes.get_mut(&body.start).unwrap().config else {
        panic!("expected terminal body");
    };
    config.kind = TerminalKind::Failure { exit_code: 1 };
    let run = surge_core::RunId::new();
    let handle = engine
        .start_run(run, graph, dir.path().into(), EngineRunConfig::default())
        .await
        .unwrap();
    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        handle.await_completion(),
    )
    .await
    .unwrap()
    .unwrap();
    let reader = storage.open_run_reader(run).await.unwrap();
    let events = reader
        .read_events(EventSeq(1)..EventSeq(reader.current_seq().await.unwrap().as_u64() + 1))
        .await
        .unwrap();
    drop(reader);
    drop(engine);
    drop(storage);
    home.close().unwrap();
    dir.close().unwrap();
    (
        outcome,
        events
            .into_iter()
            .map(|event| event.payload.payload)
            .collect(),
    )
}

fn scripted_graph(policy: FailurePolicy) -> surge_core::graph::Graph {
    use surge_core::edge::{Edge, EdgeKind, EdgePolicy, PortRef};
    use surge_core::keys::{EdgeKey, NodeKey, OutcomeKey};
    let mut graph = static_loop_graph::build_static_loop_graph();
    let NodeConfig::Loop(config) = &mut graph.nodes.get_mut(&graph.start).unwrap().config else {
        panic!("expected loop");
    };
    config.on_iteration_failure = policy;
    let body = graph.subgraphs.values_mut().next().unwrap();
    let success = body.start.clone();
    let failed = NodeKey::try_from("body_failed").unwrap();
    let mut failure_node = body.nodes[&success].clone();
    failure_node.id = failed.clone();
    let NodeConfig::Terminal(config) = &mut failure_node.config else {
        panic!("terminal");
    };
    config.kind = TerminalKind::Failure { exit_code: 1 };
    body.nodes.insert(failed.clone(), failure_node);
    let agent: surge_core::node::Node = toml::from_str(
        r#"
id = "worker"
[[declared_outcomes]]
id = "done"
description = "Success"
edge_kind_hint = "forward"
is_terminal = false
[[declared_outcomes]]
id = "failed"
description = "Failure"
edge_kind_hint = "forward"
is_terminal = false
[config]
node_kind = "agent"
profile = "implementer@1.0"
[config.prompt_overrides]
system = "Process one iteration."
"#,
    )
    .unwrap();
    let worker = agent.id.clone();
    body.nodes.insert(worker.clone(), agent);
    body.start = worker.clone();
    for (outcome, target) in [("done", success), ("failed", failed)] {
        body.edges.push(Edge {
            id: EdgeKey::try_from(format!("edge_{outcome}").as_str()).unwrap(),
            from: PortRef {
                node: worker.clone(),
                outcome: OutcomeKey::try_from(outcome).unwrap(),
            },
            to: target,
            kind: EdgeKind::Forward,
            policy: EdgePolicy::default(),
        });
    }
    graph
}

pub async fn scripted_body(
    policy: FailurePolicy,
    script: &[&str],
) -> (RunOutcome, Vec<EventPayload>) {
    use surge_acp::bridge::event::BridgeEvent;
    let dir = tempfile::tempdir().unwrap();
    let home = crate::runtime_home_fixture::FixtureHome::new().unwrap();
    let storage = Storage::open(home.path()).await.unwrap();
    let bridge = Arc::new(super::fixtures::mock_bridge::MockBridge::new());
    let sessions: Vec<_> = script
        .iter()
        .map(|_| surge_core::SessionId::new())
        .collect();
    bridge.pin_session_ids(sessions.clone()).await;
    let replies: Vec<_> = script.iter().map(|s| (*s).to_owned()).collect();
    let pump_bridge = bridge.clone();
    let pump = tokio::spawn(async move {
        for (index, (session, outcome)) in sessions.into_iter().zip(replies).enumerate() {
            pump_bridge.wait_for_subscribe_count(index + 1).await;
            pump_bridge
                .enqueue_event(BridgeEvent::OutcomeReported {
                    session,
                    outcome: surge_core::keys::OutcomeKey::try_from(outcome.as_str()).unwrap(),
                    summary: "scripted iteration result".into(),
                    artifacts_produced: vec![],

                    verification_report: None,
                })
                .await;
            pump_bridge.pump_scripted_events().await;
        }
    });
    let engine = Engine::new(
        bridge,
        storage.clone(),
        Arc::new(WorktreeToolDispatcher::new(dir.path().into())),
        EngineConfig::default(),
    );
    let run = surge_core::RunId::new();
    let handle = engine
        .start_run(
            run,
            scripted_graph(policy),
            dir.path().into(),
            EngineRunConfig::default(),
        )
        .await
        .unwrap();
    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        handle.await_completion(),
    )
    .await
    .unwrap()
    .unwrap();
    if matches!(outcome, RunOutcome::Completed { .. }) {
        pump.await.unwrap();
    } else {
        pump.abort();
        match pump.await {
            Ok(()) => {},
            Err(error) if error.is_cancelled() => {},
            Err(error) => panic!("script pump failed: {error}"),
        }
    }
    let reader = storage.open_run_reader(run).await.unwrap();
    let events = reader
        .read_events(EventSeq(1)..EventSeq(reader.current_seq().await.unwrap().as_u64() + 1))
        .await
        .unwrap();
    drop(reader);
    drop(engine);
    drop(storage);
    home.close().unwrap();
    dir.close().unwrap();
    (
        outcome,
        events
            .into_iter()
            .map(|event| event.payload.payload)
            .collect(),
    )
}
