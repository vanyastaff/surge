//! Integration test: loop protection (v1 task 1.3). A stage attempt with no
//! progress, repeated identical tool calls, or too many tool calls ends as a
//! failed attempt routed through the stage's capped retry loop — the next
//! attempt sees why — instead of failing the run.

mod fixtures;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use surge_acp::bridge::event::{BridgeEvent, ToolCallMeta};
use surge_acp::bridge::sandbox::SandboxDecision;
use surge_core::agent_config::AgentConfig;
use surge_core::edge::{Edge, EdgeKind, EdgePolicy, ExceededAction, PortRef};
use surge_core::graph::{Graph, GraphMetadata, SCHEMA_VERSION};
use surge_core::id::{RunId, SessionId};
use surge_core::keys::{EdgeKey, NodeKey, OutcomeKey, ProfileKey};
use surge_core::loop_config::ToolCallLoopGuardConfig;
use surge_core::node::{Node, NodeConfig, OutcomeDecl, Position};
use surge_core::run_event::{EscalationCause, EventPayload};
use surge_core::terminal_config::{TerminalConfig, TerminalKind};
use surge_orchestrator::engine::tools::worktree::WorktreeToolDispatcher;
use surge_orchestrator::engine::{Engine, EngineConfig, EngineRunConfig, RunOutcome};
use surge_persistence::runs::{EventSeq, Storage};

use fixtures::mock_bridge::{MockBridge, RecordedCall};

fn decl(id: &str, kind: EdgeKind) -> OutcomeDecl {
    OutcomeDecl {
        id: OutcomeKey::try_from(id).unwrap(),
        description: id.into(),
        edge_kind_hint: kind,
        is_terminal: false,
        ledger_effect: Default::default(),
    }
}

/// implement --done--> end; implement --partial--> implement (backtrack,
/// at most twice, escalate when exhausted).
fn retrying_implementer_graph() -> Graph {
    let implement = NodeKey::try_from("implement").unwrap();
    let end = NodeKey::try_from("end").unwrap();
    let mut nodes = BTreeMap::new();
    nodes.insert(
        implement.clone(),
        Node {
            id: implement.clone(),
            position: Position::default(),
            declared_outcomes: vec![
                decl("done", EdgeKind::Forward),
                decl("partial", EdgeKind::Backtrack),
            ],
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
    let edge = |id: &str, outcome: &str, to: &NodeKey, kind| Edge {
        id: EdgeKey::try_from(id).unwrap(),
        from: PortRef {
            node: implement.clone(),
            outcome: OutcomeKey::try_from(outcome).unwrap(),
        },
        to: to.clone(),
        kind,
        policy: EdgePolicy::default(),
    };
    let mut partial = edge("e_partial", "partial", &implement, EdgeKind::Backtrack);
    partial.policy.max_traversals = Some(2);
    partial.policy.on_max_exceeded = ExceededAction::Escalate;
    Graph {
        schema_version: SCHEMA_VERSION,
        metadata: GraphMetadata::new("loop-protection-harness", chrono::Utc::now()),
        start: implement.clone(),
        nodes,
        edges: vec![edge("e_done", "done", &end, EdgeKind::Forward), partial],
        subgraphs: BTreeMap::new(),
    }
}

fn tool_call(session: SessionId, call_id: &str, path: &str) -> BridgeEvent {
    BridgeEvent::ToolCall {
        session,
        call_id: call_id.into(),
        tool: "read_file".into(),
        args_redacted_json: format!(r#"{{"path":"{path}"}}"#),
        sandbox_decision: SandboxDecision::Allow,
        meta: ToolCallMeta {
            mcp_id: None,
            injected: false,
        },
    }
}

async fn wait_for_prompts(mock: &MockBridge, count: usize) {
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let sent = mock
                .recorded_calls
                .lock()
                .await
                .iter()
                .filter(|call| matches!(call, RecordedCall::SendMessage { .. }))
                .count();
            if sent >= count && mock.subscribe_count() >= count {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("attempt reached");
}

/// Run the harness: the first attempt plays `first` (events for its session),
/// the second reports `done`. Returns the outcome, the second attempt's
/// prompt and the journal.
async fn run_two_attempts(
    guard: ToolCallLoopGuardConfig,
    first: impl FnOnce(SessionId) -> Vec<BridgeEvent>,
) -> (RunOutcome, String, Vec<EventPayload>) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), b"a").unwrap();
    std::fs::write(dir.path().join("b.txt"), b"b").unwrap();
    let storage = Storage::open(dir.path()).await.unwrap();
    let mock = Arc::new(MockBridge::new());
    let engine = Engine::new(
        mock.clone(),
        storage.clone(),
        Arc::new(WorktreeToolDispatcher::new(dir.path().to_path_buf())),
        EngineConfig::default(),
    );
    let (one, two) = (SessionId::new(), SessionId::new());
    mock.pin_session_ids(vec![one, two]).await;
    for event in first(one) {
        mock.enqueue_event(event).await;
    }
    let driver_mock = mock.clone();
    let driver = tokio::spawn(async move {
        wait_for_prompts(&driver_mock, 1).await;
        driver_mock.pump_scripted_events().await;
        wait_for_prompts(&driver_mock, 2).await;
        let prompt = driver_mock.last_prompt().await.unwrap_or_default();
        driver_mock
            .enqueue_event(BridgeEvent::OutcomeReported {
                session: two,
                outcome: OutcomeKey::try_from("done").unwrap(),
                summary: "done on the second attempt".into(),
                artifacts_produced: vec![],
                verification_report: None,
            })
            .await;
        driver_mock.pump_scripted_events().await;
        prompt
    });
    let run = RunId::new();
    let handle = engine
        .start_run(
            run,
            retrying_implementer_graph(),
            dir.path().into(),
            EngineRunConfig {
                tool_call_loop_guard: Some(guard),
                ..EngineRunConfig::default()
            },
        )
        .await
        .unwrap();
    let outcome = tokio::time::timeout(Duration::from_secs(30), handle.await_completion())
        .await
        .expect("run finishes")
        .unwrap();
    let prompt = driver.await.unwrap();
    storage
        .inspect_folded_run(run)
        .await
        .expect("a journal with a loop-protection retry stays trusted");
    let events = storage
        .open_run_reader(run)
        .await
        .unwrap()
        .read_events(EventSeq::ZERO..EventSeq(u64::MAX))
        .await
        .unwrap()
        .into_iter()
        .map(|event| event.payload.payload)
        .collect();
    (outcome, prompt, events)
}

fn assert_retried_after(
    outcome: &RunOutcome,
    prompt: &str,
    events: &[EventPayload],
    cause: EscalationCause,
    reason: &str,
) {
    assert!(
        matches!(outcome, RunOutcome::Completed { terminal } if terminal.as_str() == "end"),
        "{outcome:?}"
    );
    assert!(
        events.iter().any(|event| matches!(event,
            EventPayload::EscalationRequested { cause: c, .. } if *c == cause)),
        "expected a {cause:?} escalation"
    );
    assert!(events.iter().any(|event| matches!(event,
        EventPayload::OutcomeReported { outcome, summary, .. }
            if outcome.as_str() == "partial" && summary.starts_with("Loop protection ended this attempt"))));
    assert!(events.iter().any(|event| matches!(event,
        EventPayload::EdgeTraversed { edge, kind: EdgeKind::Backtrack, .. } if edge.as_str() == "e_partial")));
    assert!(
        prompt.starts_with("## Feedback from the previous attempt"),
        "{prompt}"
    );
    assert!(prompt.contains(reason), "{prompt}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_idle_attempt_counts_as_failed_and_retries() {
    let (outcome, prompt, events) = run_two_attempts(
        ToolCallLoopGuardConfig {
            idle_limit_secs: 1,
            ..ToolCallLoopGuardConfig::default()
        },
        |_| Vec::new(),
    )
    .await;
    assert_retried_after(
        &outcome,
        &prompt,
        &events,
        EscalationCause::LoopGuardNoProgress,
        "no progress",
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn repeated_identical_calls_end_the_attempt_and_retry() {
    let (outcome, prompt, events) = run_two_attempts(
        ToolCallLoopGuardConfig {
            max_repeat_tool_calls: 1,
            ..ToolCallLoopGuardConfig::default()
        },
        |session| {
            vec![
                tool_call(session, "c1", "a.txt"),
                tool_call(session, "c2", "a.txt"),
            ]
        },
    )
    .await;
    assert_retried_after(
        &outcome,
        &prompt,
        &events,
        EscalationCause::LoopGuardRepeatedToolCall,
        "called 2 times in a row",
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_tool_call_cap_ends_the_attempt_and_retries() {
    let (outcome, prompt, events) = run_two_attempts(
        ToolCallLoopGuardConfig {
            max_tool_calls: 1,
            ..ToolCallLoopGuardConfig::default()
        },
        |session| {
            vec![
                tool_call(session, "c1", "a.txt"),
                tool_call(session, "c2", "b.txt"),
            ]
        },
    )
    .await;
    assert_retried_after(
        &outcome,
        &prompt,
        &events,
        EscalationCause::LoopGuardToolCallCap,
        "2 tool calls in one attempt",
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wall_clock_trips_climb_the_ladder_to_the_human_gate() {
    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::open(dir.path()).await.unwrap();
    let mock = Arc::new(MockBridge::new());
    let engine = Engine::new(
        mock.clone(),
        storage.clone(),
        Arc::new(WorktreeToolDispatcher::new(dir.path().to_path_buf())),
        EngineConfig::default(),
    );
    let run = RunId::new();
    let mut tap = engine.subscribe_tap();
    let handle = engine
        .start_run(
            run,
            retrying_implementer_graph(),
            dir.path().into(),
            EngineRunConfig {
                // Every attempt is past its budget the moment it starts.
                tool_call_loop_guard: Some(ToolCallLoopGuardConfig {
                    node_wall_clock_limit_secs: 0,
                    ..ToolCallLoopGuardConfig::default()
                }),
                ..EngineRunConfig::default()
            },
        )
        .await
        .unwrap();
    let gate = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let event = tap.recv().await.unwrap();
            if event.run_id != run {
                continue;
            }
            if let EventPayload::HumanInputRequested { node, .. } = event.event.payload.payload {
                break node;
            }
        }
    })
    .await
    .expect("the exhausted ladder asks a human instead of failing the run");
    assert_eq!(gate.as_str(), "implement_escalation");
    engine.stop_run(run, "test done".into()).await.unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(10), handle.await_completion()).await;
    let events: Vec<EventPayload> = storage
        .open_run_reader(run)
        .await
        .unwrap()
        .read_events(EventSeq::ZERO..EventSeq(u64::MAX))
        .await
        .unwrap()
        .into_iter()
        .map(|event| event.payload.payload)
        .collect();
    // Two loop rounds plus the extra attempt, each ended by the deadline.
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(
                event,
                EventPayload::EscalationRequested {
                    cause: EscalationCause::LoopGuardNodeDeadline,
                    ..
                }
            ))
            .count(),
        4
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, EventPayload::RunFailed { .. }))
    );
}
