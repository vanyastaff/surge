//! Integration test: agent switching on limits (v1 task 1.4). A stage whose
//! agent hits its usage limit moves to the first `[capacity].fallback_agents`
//! entry with capacity left, recorded as `StageRuntimeRotated`; when none
//! fits, the run parks and wakes as before.

mod fixtures;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use surge_acp::bridge::error::SendMessageError;
use surge_acp::bridge::event::BridgeEvent;
use surge_core::agent_config::{AgentConfig, NodeLimits};
use surge_core::edge::{Edge, EdgeKind, EdgePolicy, PortRef};
use surge_core::graph::{Graph, GraphMetadata, SCHEMA_VERSION};
use surge_core::id::{RunId, SessionId};
use surge_core::keys::{EdgeKey, NodeKey, OutcomeKey, ProfileKey};
use surge_core::node::{Node, NodeConfig, OutcomeDecl, Position};
use surge_core::run_event::EventPayload;
use surge_core::terminal_config::{TerminalConfig, TerminalKind};
use surge_orchestrator::engine::tools::ToolDispatcher;
use surge_orchestrator::engine::{Engine, EngineConfig, EngineRunConfig, RunOutcome};
use surge_orchestrator::profile_loader::{DiskProfileSet, ProfileRegistry};
use surge_persistence::runs::Storage;
use surge_persistence::runs::seq::EventSeq;

use fixtures::mock_bridge::{MockBridge, RecordedCall};

struct UnusedDispatcher;

#[async_trait::async_trait]
impl ToolDispatcher for UnusedDispatcher {
    async fn dispatch(
        &self,
        _ctx: &surge_orchestrator::engine::tools::ToolDispatchContext<'_>,
        call: &surge_orchestrator::engine::tools::ToolCall,
    ) -> surge_orchestrator::engine::tools::ToolResultPayload {
        surge_orchestrator::engine::tools::ToolResultPayload::Unsupported {
            message: format!("unused: {}", call.tool),
        }
    }
}

/// A disk profile `implementer-claude@1.0` running on Claude Code.
fn profiles() -> (tempfile::TempDir, Arc<ProfileRegistry>) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("implementer-claude-1.0.toml"),
        r#"
schema_version = 1

[role]
id = "implementer-claude"
version = "1.0.0"
display_name = "implementer-claude"
category = "agents"
description = "agent rotation fixture"
when_to_use = "Tests"

[runtime]
recommended_model = "test-model"
agent_id = "claude-code"

[[outcomes]]
id = "done"
description = "done"
edge_kind_hint = "forward"

[prompt]
system = "test"
"#,
    )
    .unwrap();
    let registry = Arc::new(ProfileRegistry::new(
        DiskProfileSet::scan(dir.path()).unwrap(),
    ));
    (dir, registry)
}

fn graph() -> Graph {
    let implement = NodeKey::try_from("implement").unwrap();
    let end = NodeKey::try_from("end").unwrap();
    let mut nodes = BTreeMap::new();
    nodes.insert(
        implement.clone(),
        Node {
            id: implement.clone(),
            position: Position::default(),
            declared_outcomes: vec![OutcomeDecl {
                id: OutcomeKey::try_from("done").unwrap(),
                description: "done".into(),
                edge_kind_hint: EdgeKind::Forward,
                is_terminal: false,
                ledger_effect: Default::default(),
            }],
            config: NodeConfig::Agent(AgentConfig {
                profile: ProfileKey::try_from("implementer-claude@1.0").unwrap(),
                prompt_overrides: None,
                tool_overrides: None,
                sandbox_override: None,
                approvals_override: None,
                bindings: vec![],
                rules_overrides: None,
                limits: NodeLimits::default(),
                hooks: vec![],
                custom_fields: Default::default(),
            }),
        },
    );
    nodes.insert(
        end.clone(),
        Node {
            id: end.clone(),
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
        metadata: GraphMetadata::new("agent-rotation-harness", chrono::Utc::now()),
        start: implement.clone(),
        nodes,
        edges: vec![Edge {
            id: EdgeKey::try_from("e_done").unwrap(),
            from: PortRef {
                node: implement,
                outcome: OutcomeKey::try_from("done").unwrap(),
            },
            to: end,
            kind: EdgeKind::Forward,
            policy: EdgePolicy::default(),
        }],
        subgraphs: BTreeMap::new(),
    }
}

fn engine(
    mock: &Arc<MockBridge>,
    storage: &Arc<Storage>,
    registry: Arc<ProfileRegistry>,
    fallback_agents: Vec<String>,
) -> Engine {
    Engine::new_full(
        mock.clone(),
        storage.clone(),
        Arc::new(UnusedDispatcher),
        Arc::new(surge_notify::MultiplexingNotifier::new()),
        None,
        Some(registry),
        EngineConfig {
            fallback_agents,
            ..EngineConfig::default()
        },
    )
}

async fn journal(storage: &Arc<Storage>, run: RunId) -> Vec<EventPayload> {
    storage
        .open_run_reader(run)
        .await
        .unwrap()
        .read_events(EventSeq::ZERO..EventSeq(u64::MAX))
        .await
        .unwrap()
        .into_iter()
        .map(|event| event.payload.payload)
        .collect()
}

fn opened_agents(events: &[EventPayload]) -> Vec<Option<String>> {
    events
        .iter()
        .filter_map(|event| match event {
            EventPayload::SessionOpened { agent_id, .. } => Some(agent_id.clone()),
            _ => None,
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_rate_limited_stage_moves_to_the_fallback_agent_and_finishes() {
    let (_profiles, registry) = profiles();
    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::open(dir.path()).await.unwrap();
    let mock = Arc::new(MockBridge::new());
    mock.fail_next_send_message(SendMessageError::RateLimited {
        retry_after: Some(Duration::from_secs(3600)),
        details: "usage limit reached".into(),
    })
    .await;
    let (first, second) = (SessionId::new(), SessionId::new());
    mock.pin_session_ids(vec![first, second]).await;
    mock.enqueue_event(BridgeEvent::OutcomeReported {
        session: second,
        outcome: OutcomeKey::try_from("done").unwrap(),
        summary: "done on codex".into(),
        artifacts_produced: vec![],
        verification_report: None,
    })
    .await;
    let pump_mock = mock.clone();
    let pump = tokio::spawn(async move {
        pump_mock.wait_for_subscribe_count(2).await;
        pump_mock.pump_scripted_events().await;
    });
    let engine = engine(&mock, &storage, registry, vec!["codex-acp".into()]);
    let run = RunId::new();
    let handle = engine
        .start_run(run, graph(), dir.path().into(), EngineRunConfig::default())
        .await
        .unwrap();
    let outcome = tokio::time::timeout(Duration::from_secs(10), handle.await_completion())
        .await
        .expect("run finishes")
        .unwrap();
    pump.await.unwrap();
    assert!(
        matches!(&outcome, RunOutcome::Completed { terminal } if terminal.as_str() == "end"),
        "{outcome:?}"
    );

    let events = journal(&storage, run).await;
    let rotations: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            EventPayload::StageRuntimeRotated { node, from, to, .. } => {
                Some((node.as_str().to_owned(), from.clone(), to.clone()))
            },
            _ => None,
        })
        .collect();
    assert_eq!(
        rotations,
        [(
            "implement".to_owned(),
            "claude-acp".to_owned(),
            "codex-acp".to_owned()
        )]
    );
    assert_eq!(
        opened_agents(&events),
        [Some("claude-acp".into()), Some("codex-acp".into())]
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, EventPayload::RunParked { .. }))
    );
    storage
        .inspect_folded_run(run)
        .await
        .expect("a journal with an agent rotation stays trusted");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn with_every_fallback_exhausted_the_run_parks_and_wakes() {
    let (_profiles, registry) = profiles();
    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::open(dir.path()).await.unwrap();
    storage
        .observe_capacity(&surge_core::capacity::CapacityWindow::observed_429(
            "codex-acp",
            Some(Duration::from_secs(3600)),
            chrono::Utc::now(),
        ))
        .await
        .unwrap();
    let mock = Arc::new(MockBridge::new());
    mock.fail_next_send_message(SendMessageError::RateLimited {
        retry_after: None,
        details: "usage limit reached".into(),
    })
    .await;
    let engine = engine(&mock, &storage, registry, vec!["codex-acp".into()]);
    let run = RunId::new();
    let handle = engine
        .start_run(run, graph(), dir.path().into(), EngineRunConfig::default())
        .await
        .unwrap();
    let outcome = tokio::time::timeout(Duration::from_secs(10), handle.await_completion())
        .await
        .expect("run parks")
        .unwrap();
    assert!(matches!(outcome, RunOutcome::Parked { .. }), "{outcome:?}");
    assert!(
        !journal(&storage, run)
            .await
            .iter()
            .any(|event| matches!(event, EventPayload::StageRuntimeRotated { .. })),
        "an exhausted fallback is never chosen"
    );

    // Wake: the resumed attempt runs and finishes.
    let session = SessionId::new();
    mock.pin_next_session_id(session).await;
    mock.enqueue_event(BridgeEvent::OutcomeReported {
        session,
        outcome: OutcomeKey::try_from("done").unwrap(),
        summary: "done after waking".into(),
        artifacts_produced: vec![],
        verification_report: None,
    })
    .await;
    let resumed = engine.resume_run(run, dir.path().into()).await.unwrap();
    let pump_mock = mock.clone();
    let pump = tokio::spawn(async move {
        pump_mock.wait_for_subscribe_count(2).await;
        pump_mock.pump_scripted_events().await;
    });
    let outcome = tokio::time::timeout(Duration::from_secs(10), resumed.await_completion())
        .await
        .expect("resumed run finishes")
        .unwrap();
    pump.await.unwrap();
    assert!(
        matches!(outcome, RunOutcome::Completed { .. }),
        "{outcome:?}"
    );
    let sends = mock
        .recorded_calls
        .lock()
        .await
        .iter()
        .filter(|call| matches!(call, RecordedCall::SendMessage { .. }))
        .count();
    assert_eq!(sends, 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_rotation_onto_the_partner_agent_is_flagged() {
    let (profiles_dir, _) = profiles();
    std::fs::write(
        profiles_dir.path().join("implementer-codex-1.0.toml"),
        std::fs::read_to_string(profiles_dir.path().join("implementer-claude-1.0.toml"))
            .unwrap()
            .replace("implementer-claude", "implementer-codex")
            .replace("agent_id = \"claude-code\"", "agent_id = \"codex\""),
    )
    .unwrap();
    let registry = Arc::new(ProfileRegistry::new(
        DiskProfileSet::scan(profiles_dir.path()).unwrap(),
    ));
    // implement (Codex) --done--> verify (Claude) --done--> end;
    // verify --failed--> implement, a capped retry edge that makes them
    // partners.
    let mut g = graph();
    let implement = NodeKey::try_from("implement").unwrap();
    let verify = NodeKey::try_from("verify").unwrap();
    let mut verify_node = g.nodes[&implement].clone();
    verify_node.id = verify.clone();
    verify_node.declared_outcomes.push(OutcomeDecl {
        id: OutcomeKey::try_from("failed").unwrap(),
        description: "failed".into(),
        edge_kind_hint: EdgeKind::Backtrack,
        is_terminal: false,
        ledger_effect: Default::default(),
    });
    if let NodeConfig::Agent(cfg) = &mut g.nodes.get_mut(&implement).unwrap().config {
        cfg.profile = ProfileKey::try_from("implementer-codex@1.0").unwrap();
    }
    g.nodes.insert(verify.clone(), verify_node);
    let mut verified = g.edges[0].clone();
    verified.id = EdgeKey::try_from("e_verified").unwrap();
    verified.from.node = verify.clone();
    g.edges[0].to = verify.clone();
    g.edges.push(verified);
    let mut retry = g.edges[0].clone();
    retry.id = EdgeKey::try_from("e_retry").unwrap();
    retry.from.node = verify;
    retry.from.outcome = OutcomeKey::try_from("failed").unwrap();
    retry.to = implement;
    retry.kind = EdgeKind::Backtrack;
    retry.policy.max_traversals = Some(2);
    g.edges.push(retry);

    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::open(dir.path()).await.unwrap();
    let mock = Arc::new(MockBridge::new());
    // implement succeeds; verify's first attempt hits the limit.
    mock.pass_next_send_message().await;
    mock.fail_next_send_message(SendMessageError::RateLimited {
        retry_after: Some(Duration::from_secs(3600)),
        details: "usage limit reached".into(),
    })
    .await;
    let (implemented, limited, checked) = (SessionId::new(), SessionId::new(), SessionId::new());
    mock.pin_session_ids(vec![implemented, limited, checked])
        .await;
    let pump_mock = mock.clone();
    let pump = tokio::spawn(async move {
        pump_mock.wait_for_subscribe_count(1).await;
        pump_mock
            .enqueue_event(BridgeEvent::OutcomeReported {
                session: implemented,
                outcome: OutcomeKey::try_from("done").unwrap(),
                summary: "implemented on codex".into(),
                artifacts_produced: vec![],
                verification_report: None,
            })
            .await;
        pump_mock.pump_scripted_events().await;
        pump_mock.wait_for_subscribe_count(3).await;
        pump_mock
            .enqueue_event(BridgeEvent::OutcomeReported {
                session: checked,
                outcome: OutcomeKey::try_from("done").unwrap(),
                summary: "checked on codex".into(),
                artifacts_produced: vec![],
                verification_report: None,
            })
            .await;
        pump_mock.pump_scripted_events().await;
    });
    let engine = engine(&mock, &storage, registry, vec!["codex-acp".into()]);
    let run = RunId::new();
    let handle = engine
        .start_run(run, g, dir.path().into(), EngineRunConfig::default())
        .await
        .unwrap();
    let outcome = tokio::time::timeout(Duration::from_secs(10), handle.await_completion())
        .await
        .expect("run finishes")
        .unwrap();
    pump.await.unwrap();
    assert!(
        matches!(outcome, RunOutcome::Completed { .. }),
        "{outcome:?}"
    );
    assert!(
        journal(&storage, run)
            .await
            .iter()
            .any(|event| matches!(event,
        EventPayload::StageRuntimeRotated { node, to, same_as_partner: true, .. }
            if node.as_str() == "verify" && to == "codex-acp"))
    );
}
