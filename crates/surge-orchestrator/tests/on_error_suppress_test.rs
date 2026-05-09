//! Integration tests for `on_error` hook suppression at the run-task boundary.
//!
//! These tests drive the public `Engine::start_run` path with a scripted
//! `MockBridge` session crash. The assertions prove the central stage-error
//! catch site runs hooks before deciding whether to persist `StageFailed`.

#![allow(clippy::too_many_lines)]

mod fixtures;

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use surge_acp::bridge::event::{BridgeEvent, SessionEndReason};
use surge_acp::bridge::facade::BridgeFacade;
use surge_core::agent_config::{AgentConfig, NodeLimits};
use surge_core::edge::{Edge, EdgeKind, EdgePolicy, PortRef};
use surge_core::graph::{Graph, GraphMetadata, SCHEMA_VERSION};
use surge_core::hooks::{Hook, HookFailureMode, HookInheritance, HookTrigger, MatcherSpec};
use surge_core::id::{RunId, SessionId};
use surge_core::keys::{EdgeKey, NodeKey, OutcomeKey, ProfileKey};
use surge_core::node::{Node, NodeConfig, OutcomeDecl, Position};
use surge_core::run_event::EventPayload;
use surge_core::terminal_config::{TerminalConfig, TerminalKind};
use surge_orchestrator::engine::tools::ToolDispatcher;
use surge_orchestrator::engine::tools::worktree::WorktreeToolDispatcher;
use surge_orchestrator::engine::{Engine, EngineConfig, EngineRunConfig, RunOutcome};
use surge_persistence::runs::Storage;
use surge_persistence::runs::seq::EventSeq;

use fixtures::mock_bridge::MockBridge;

fn suppress_command(dir: &Path, outcome: &str) -> String {
    let json = format!(r#"{{"action":"suppress","outcome":"{outcome}"}}"#);
    let path = dir.join(format!("suppress-{outcome}.json"));
    std::fs::write(&path, json).unwrap();
    if cfg!(target_os = "windows") {
        let literal_path = path.display().to_string().replace('\'', "''");
        format!("powershell -NoProfile -Command Get-Content -Raw -LiteralPath '{literal_path}'")
    } else {
        format!(r#"cat "{}""#, path.display())
    }
}

fn on_error_suppress_hook(id: &str, command: String) -> Hook {
    Hook {
        id: id.into(),
        trigger: HookTrigger::OnError,
        matcher: MatcherSpec::default(),
        command,
        on_failure: HookFailureMode::Warn,
        timeout_seconds: Some(5),
        inherit: HookInheritance::Extend,
    }
}

fn outcome_decl(id: &str) -> OutcomeDecl {
    OutcomeDecl {
        id: OutcomeKey::try_from(id).unwrap(),
        description: format!("{id} outcome"),
        edge_kind_hint: EdgeKind::Forward,
        is_terminal: false,
    }
}

fn agent_node(hooks: Vec<Hook>, declared_outcomes: Vec<&str>) -> Node {
    Node {
        id: NodeKey::try_from("agent_1").unwrap(),
        position: Position::default(),
        declared_outcomes: declared_outcomes.into_iter().map(outcome_decl).collect(),
        config: NodeConfig::Agent(AgentConfig {
            profile: ProfileKey::try_from("implementer@1.0").unwrap(),
            prompt_overrides: None,
            tool_overrides: None,
            sandbox_override: None,
            approvals_override: None,
            bindings: vec![],
            rules_overrides: None,
            limits: NodeLimits::default(),
            hooks,
            custom_fields: BTreeMap::new(),
        }),
    }
}

fn terminal_node(id: &str) -> Node {
    Node {
        id: NodeKey::try_from(id).unwrap(),
        position: Position::default(),
        declared_outcomes: vec![],
        config: NodeConfig::Terminal(TerminalConfig {
            kind: TerminalKind::Success,
            message: None,
        }),
    }
}

fn graph_with_agent(hooks: Vec<Hook>, declared_outcomes: Vec<&str>, edge_outcome: &str) -> Graph {
    let agent = NodeKey::try_from("agent_1").unwrap();
    let end = NodeKey::try_from("end").unwrap();
    let mut nodes = BTreeMap::new();
    nodes.insert(agent.clone(), agent_node(hooks, declared_outcomes));
    nodes.insert(end.clone(), terminal_node("end"));

    Graph {
        schema_version: SCHEMA_VERSION,
        metadata: GraphMetadata {
            name: "on-error-suppress".into(),
            description: None,
            template_origin: None,
            created_at: chrono::Utc::now(),
            author: None,
        },
        start: agent.clone(),
        nodes,
        edges: vec![Edge {
            id: EdgeKey::try_from("agent_to_end").unwrap(),
            from: PortRef {
                node: agent,
                outcome: OutcomeKey::try_from(edge_outcome).unwrap(),
            },
            to: end,
            kind: EdgeKind::Forward,
            policy: EdgePolicy::default(),
        }],
        subgraphs: BTreeMap::new(),
    }
}

async fn run_crashing_agent(
    build_graph: impl FnOnce(&Path) -> Graph,
) -> (RunOutcome, Vec<surge_persistence::runs::reader::ReadEvent>) {
    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::open(dir.path()).await.unwrap();
    let mock = Arc::new(MockBridge::new());
    let bridge: Arc<dyn BridgeFacade> = mock.clone();
    let dispatcher =
        Arc::new(WorktreeToolDispatcher::new(dir.path().to_path_buf())) as Arc<dyn ToolDispatcher>;
    let engine = Engine::new(bridge, storage.clone(), dispatcher, EngineConfig::default());

    let run_id = RunId::new();
    let session_id = SessionId::new();
    mock.pin_next_session_id(session_id).await;
    mock.enqueue_event(BridgeEvent::SessionEnded {
        session: session_id,
        reason: SessionEndReason::AgentCrashed {
            exit_code: Some(137),
            stderr_tail: "simulated crash".into(),
        },
    })
    .await;

    let graph = build_graph(dir.path());
    let handle = engine
        .start_run(
            run_id,
            graph,
            dir.path().to_path_buf(),
            EngineRunConfig::default(),
        )
        .await
        .expect("start_run");

    let mock_for_pump = mock.clone();
    let pump = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        mock_for_pump.pump_scripted_events().await;
    });

    let outcome = tokio::time::timeout(Duration::from_secs(10), handle.await_completion())
        .await
        .expect("run timed out")
        .expect("run handle join");
    pump.await.unwrap();
    drop(engine);

    let reader = storage.open_run_reader(run_id).await.unwrap();
    let last = reader.current_seq().await.unwrap();
    let events = reader
        .read_events(EventSeq(0)..EventSeq(last.0 + 1))
        .await
        .unwrap();
    (outcome, events)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn on_error_suppresses_failure_into_declared_outcome() {
    let (outcome, events) = run_crashing_agent(|dir| {
        graph_with_agent(
            vec![on_error_suppress_hook(
                "recover",
                suppress_command(dir, "retry_later"),
            )],
            vec!["retry_later"],
            "retry_later",
        )
    })
    .await;
    let event_debug: Vec<String> = events
        .iter()
        .map(|ev| format!("{:?}", ev.payload.payload))
        .collect();
    match outcome {
        RunOutcome::Completed { terminal } => assert_eq!(terminal.as_ref(), "end"),
        other => panic!(
            "expected Completed after on_error suppression, got {other:?}; events: {event_debug:#?}"
        ),
    }

    let mut saw_hook_executed = false;
    let mut saw_suppressed_outcome = false;
    for ev in &events {
        match &ev.payload.payload {
            EventPayload::HookExecuted { hook_id, .. } if hook_id == "recover" => {
                saw_hook_executed = true;
            },
            EventPayload::OutcomeReported { outcome, .. } if outcome.as_str() == "retry_later" => {
                saw_suppressed_outcome = true;
            },
            EventPayload::StageFailed { .. } => {
                panic!("suppressed failure must not persist StageFailed: {ev:?}");
            },
            _ => {},
        }
    }
    assert!(saw_hook_executed, "missing HookExecuted audit event");
    assert!(saw_suppressed_outcome, "missing suppressed OutcomeReported");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_on_error_hook_crash_records_stage_failed() {
    let (outcome, events) =
        run_crashing_agent(|_| graph_with_agent(vec![], vec!["done"], "done")).await;
    match outcome {
        RunOutcome::Failed { error } => assert!(error.contains("session ended")),
        other => panic!("expected Failed without on_error hook, got {other:?}"),
    }

    assert!(
        events
            .iter()
            .any(|ev| matches!(ev.payload.payload, EventPayload::StageFailed { .. })),
        "missing StageFailed event"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn undeclared_suppression_falls_through_to_stage_failed() {
    let (outcome, events) = run_crashing_agent(|dir| {
        graph_with_agent(
            vec![on_error_suppress_hook(
                "rogue",
                suppress_command(dir, "retry_later"),
            )],
            vec!["done"],
            "done",
        )
    })
    .await;
    match outcome {
        RunOutcome::Failed { error } => assert!(error.contains("session ended")),
        other => panic!("expected Failed for undeclared suppression, got {other:?}"),
    }

    let mut saw_hook_executed = false;
    let mut saw_stage_failed = false;
    for ev in &events {
        match &ev.payload.payload {
            EventPayload::HookExecuted { hook_id, .. } if hook_id == "rogue" => {
                saw_hook_executed = true;
            },
            EventPayload::StageFailed { .. } => saw_stage_failed = true,
            EventPayload::OutcomeReported { outcome, .. } if outcome.as_str() == "retry_later" => {
                panic!("undeclared suppression must not record retry_later outcome");
            },
            _ => {},
        }
    }
    assert!(saw_hook_executed, "missing HookExecuted audit event");
    assert!(saw_stage_failed, "missing StageFailed event");
}
