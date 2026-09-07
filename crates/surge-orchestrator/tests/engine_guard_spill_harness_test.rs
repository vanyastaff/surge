//! Integration tests: loop-guard and output-spill wiring proven through the
//! real engine harness (`Engine::start_run` + `MockBridge`), not by calling
//! `RoutingToolDispatcher` directly.
//!
//! First review of task 13 (`.autopilot/competitive-waves/tickets/13-guards-spill.md`)
//! found the primitives (`surge_orchestrator::guard`, `surge_orchestrator::spill`)
//! correct in isolation but disconnected from production: a plain agent node
//! (no `tool_overrides.mcp_add`) got the bare `tool_dispatcher` with no guard
//! and no spill at all; the three builder overrides had no caller outside
//! `mod tests`; the wall-clock deadline was only ever polled from inside a
//! tool dispatch; and spill wrote to `ArtifactStore::from_default_path()`
//! instead of the run's own store. Every test below exercises the seam that
//! finding named — a **non-default** configured value threaded all the way
//! from `EngineRunConfig` down to the dispatcher a plain node actually uses.

mod fixtures;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use surge_acp::bridge::event::{BridgeEvent, ToolCallMeta, ToolResultPayload as BridgeToolResult};
use surge_acp::bridge::facade::BridgeFacade;
use surge_acp::bridge::sandbox::SandboxDecision;
use surge_core::agent_config::AgentConfig;
use surge_core::content_hash::ContentHash;
use surge_core::edge::{Edge, EdgeKind, EdgePolicy, PortRef};
use surge_core::graph::{Graph, GraphMetadata, SCHEMA_VERSION};
use surge_core::id::{RunId, SessionId};
use surge_core::keys::{EdgeKey, NodeKey, OutcomeKey, ProfileKey};
use surge_core::loop_config::ToolCallLoopGuardConfig;
use surge_core::node::{Node, NodeConfig, OutcomeDecl, Position};
use surge_core::spill_config::OutputSpillConfig;
use surge_core::terminal_config::{TerminalConfig, TerminalKind};
use surge_orchestrator::engine::tools::ToolDispatcher;
use surge_orchestrator::engine::tools::worktree::WorktreeToolDispatcher;
use surge_orchestrator::engine::{Engine, EngineConfig, EngineRunConfig, RunOutcome};
use surge_persistence::artifacts::ArtifactStore;
use surge_persistence::runs::{EventSeq, Storage};

use fixtures::mock_bridge::{MockBridge, RecordedCall};

/// A single Agent node ("implement") wired to a Terminal success node on its
/// `"done"` outcome. No `tool_overrides` — deliberately a "plain" node, the
/// exact shape the first review found unguarded (`RoutingToolDispatcher` was
/// only built when `tool_overrides.mcp_add` was non-empty).
fn agent_to_terminal_graph() -> Graph {
    let agent_key = NodeKey::try_from("implement").unwrap();
    let end_key = NodeKey::try_from("end").unwrap();

    let mut nodes = BTreeMap::new();
    nodes.insert(
        agent_key.clone(),
        Node {
            id: agent_key.clone(),
            position: Position::default(),
            declared_outcomes: vec![OutcomeDecl {
                id: OutcomeKey::try_from("done").unwrap(),
                description: "stage completed".into(),
                edge_kind_hint: EdgeKind::Forward,
                is_terminal: false,
                ledger_effect: Default::default(),
            }],
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
        end_key.clone(),
        Node {
            id: end_key.clone(),
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
        metadata: GraphMetadata::new("guard-spill-harness", chrono::Utc::now()),
        start: agent_key.clone(),
        nodes,
        edges: vec![Edge {
            id: EdgeKey::try_from("e_done").unwrap(),
            from: PortRef {
                node: agent_key,
                outcome: OutcomeKey::try_from("done").unwrap(),
            },
            to: end_key,
            kind: EdgeKind::Forward,
            policy: EdgePolicy::default(),
        }],
        subgraphs: BTreeMap::new(),
    }
}

fn read_file_call(call_id: &str, path: &str) -> BridgeEvent {
    let session_id = SessionId::new(); // placeholder — caller overwrites `session`
    BridgeEvent::ToolCall {
        session: session_id,
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn configured_repeat_threshold_is_honored_not_the_default() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), b"hello").unwrap();

    let storage = Storage::open(dir.path()).await.unwrap();
    let mock = Arc::new(MockBridge::new());
    let bridge: Arc<dyn BridgeFacade> = mock.clone();
    let dispatcher =
        Arc::new(WorktreeToolDispatcher::new(dir.path().to_path_buf())) as Arc<dyn ToolDispatcher>;

    let session_id = SessionId::new();
    mock.pin_next_session_id(session_id).await;
    // Two IDENTICAL calls (same tool, same args, different call_id). The
    // *default* threshold (3) would let both through — only a configured
    // value of 1 refuses the second. This is what distinguishes "the config
    // was read" from "the default was silently used instead".
    let mut first = read_file_call("c1", "a.txt");
    if let BridgeEvent::ToolCall { session, .. } = &mut first {
        *session = session_id;
    }
    let mut second = read_file_call("c2", "a.txt");
    if let BridgeEvent::ToolCall { session, .. } = &mut second {
        *session = session_id;
    }
    mock.enqueue_event(first).await;
    mock.enqueue_event(second).await;
    mock.enqueue_event(BridgeEvent::OutcomeReported {
        session: session_id,
        outcome: OutcomeKey::try_from("done").unwrap(),
        summary: "ok".into(),
        artifacts_produced: vec![],
    })
    .await;
    let mock_for_pump = mock.clone();
    let pump = tokio::spawn(async move {
        mock_for_pump.pump_after_subscribe(1).await;
    });

    let engine = Engine::new(bridge, storage.clone(), dispatcher, EngineConfig::default());

    let run_id = RunId::new();
    let run_config = EngineRunConfig {
        // Non-default: the built-in default is 3. If this were ignored and
        // the dispatcher fell back to `ToolCallLoopGuardConfig::default()`,
        // both calls below would succeed.
        tool_call_loop_guard: Some(ToolCallLoopGuardConfig {
            max_repeat_tool_calls: 1,
            node_wall_clock_limit_secs: 3600,
        }),
        ..EngineRunConfig::default()
    };
    let handle = engine
        .start_run(
            run_id,
            agent_to_terminal_graph(),
            dir.path().to_path_buf(),
            run_config,
        )
        .await
        .expect("start_run");

    let outcome = handle.await_completion().await.unwrap();
    pump.await.unwrap();
    match outcome {
        RunOutcome::Completed { terminal } => assert_eq!(terminal.as_ref(), "end"),
        other => panic!("expected Completed, got {other:?}"),
    }

    let calls = mock.recorded_calls.lock().await;
    let replies: Vec<&RecordedCall> = calls
        .iter()
        .filter(|c| matches!(c, RecordedCall::ReplyToTool { .. }))
        .collect();
    assert_eq!(
        replies.len(),
        2,
        "expected exactly two tool replies, got {replies:?}"
    );
    match replies[0] {
        RecordedCall::ReplyToTool {
            call_id, payload, ..
        } => {
            assert_eq!(call_id, "c1");
            assert!(
                matches!(payload, BridgeToolResult::Ok { .. }),
                "first call is within the configured threshold and must dispatch, got {payload:?}"
            );
        },
        other => panic!("expected ReplyToTool, got {other:?}"),
    }
    match replies[1] {
        RecordedCall::ReplyToTool {
            call_id, payload, ..
        } => {
            assert_eq!(call_id, "c2");
            assert!(
                matches!(payload, BridgeToolResult::Error { .. }),
                "second identical call must be refused by the guard at threshold=1, got {payload:?}"
            );
        },
        other => panic!("expected ReplyToTool, got {other:?}"),
    }
    drop(calls);

    // The guard's trip must reach the durable event log the same way an MCP
    // escalation does (R39: "raising EscalationRequested"), not just refuse
    // the call and leave no trace.
    let reader = storage.open_run_reader(run_id).await.unwrap();
    let events = reader.read_events(EventSeq(0)..EventSeq(64)).await.unwrap();
    let escalation_reasons: Vec<&str> = events
        .iter()
        .filter_map(|re| match &re.payload.payload {
            surge_core::run_event::EventPayload::EscalationRequested { reason, .. } => {
                Some(reason.as_str())
            },
            _ => None,
        })
        .collect();
    assert_eq!(
        escalation_reasons.len(),
        1,
        "expected exactly one EscalationRequested event, got {escalation_reasons:?}"
    );
    assert!(
        escalation_reasons[0].contains("read_file"),
        "escalation reason should name the repeated tool: {}",
        escalation_reasons[0]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn node_wall_clock_deadline_trips_without_any_tool_call() {
    let dir = tempfile::tempdir().unwrap();
    // This test drives the run to a genuine `RunOutcome::Failed`
    // (`StageError::LoopGuardTripped`), which trips
    // `engine::hooks::memory_writeback::record_node_failure` — route it at
    // a throwaway store instead of the developer's real `~/.surge/memory.db`.
    let memory_dir = tempfile::tempdir().unwrap();
    let store_path = memory_dir.path().join("memory.db");

    let storage = Storage::open(dir.path()).await.unwrap();
    let mock = Arc::new(MockBridge::new());
    let bridge: Arc<dyn BridgeFacade> = mock.clone();
    let dispatcher =
        Arc::new(WorktreeToolDispatcher::new(dir.path().to_path_buf())) as Arc<dyn ToolDispatcher>;

    let session_id = SessionId::new();
    mock.pin_next_session_id(session_id).await;
    // Deliberately no `ToolCall` scripted at all — `check_loop_guard` (which
    // only runs from inside `dispatch`) never gets a chance to see the
    // deadline. Only the timer-driven poll inside the stage's event loop can
    // catch it. No `OutcomeReported` is scripted either: a wall-clock trip
    // now ends the stage itself (ticket 17), so the run reaches a terminal
    // failure without the agent ever reporting anything.
    let mock_for_pump = mock.clone();
    let pump = tokio::spawn(async move {
        mock_for_pump.wait_for_subscribe_count(1).await;
    });

    let engine = Engine::new(bridge, storage.clone(), dispatcher, EngineConfig::default());

    let run_id = RunId::new();
    let run_config = EngineRunConfig {
        // Already exceeded the instant the dispatcher is built (deterministic,
        // no sleeping needed to trip it) — mirrors
        // `guard::tests::deadline_past_budget_escalates`.
        tool_call_loop_guard: Some(ToolCallLoopGuardConfig {
            max_repeat_tool_calls: 100,
            node_wall_clock_limit_secs: 0,
        }),
        memory_store_path: Some(store_path),
        ..EngineRunConfig::default()
    };
    let handle = engine
        .start_run(
            run_id,
            agent_to_terminal_graph(),
            dir.path().to_path_buf(),
            run_config,
        )
        .await
        .expect("start_run");
    pump.await.unwrap();

    // A node already past its wall-clock budget must fail the run outright —
    // not merely leave a mark while the turn keeps burning tokens (the bug
    // ticket 17 fixes: "raising EscalationRequested rather than burning
    // budget" required the budget to actually stop, not just get a log line).
    //
    // Explicit timeout, not a bare `.await`: a regression to the old
    // `continue`-past-the-trip behavior has no scripted `OutcomeReported` to
    // fall back on, so the run never terminates at all. Without this bound
    // that regression reads as a hang nextest's `terminate-after` catches
    // only after 120s (`.config/nextest.toml`) — indistinguishable in CI from
    // a flaky test. This turns it into an immediate, named assertion failure.
    let outcome = tokio::time::timeout(Duration::from_secs(10), handle.await_completion())
        .await
        .expect(
            "run did not terminate within 10s — the wall-clock trip must end the stage \
             (StageError::LoopGuardTripped), not merely log and `continue`",
        )
        .unwrap();
    match outcome {
        RunOutcome::Failed { error } => assert!(
            error.contains("wall-clock"),
            "failure reason should name the wall-clock trip: {error}"
        ),
        other => panic!("a node past its wall-clock deadline must fail the run, got {other:?}"),
    }

    // The guard's verdict must leave a durable, queryable trace — answered by
    // a query against the typed `cause`, never by scanning `reason` prose
    // (`.autopilot/competitive-waves/spec.md` §15 / History 45).
    let reader = storage.open_run_reader(run_id).await.unwrap();
    let escalations = surge_persistence::runs::read_escalations(&reader)
        .await
        .unwrap();
    let guard_escalations: Vec<_> = escalations
        .iter()
        .filter(|e| surge_persistence::runs::is_loop_guard_cause(e.cause))
        .collect();
    assert_eq!(
        guard_escalations.len(),
        1,
        "expected exactly one loop-guard-caused escalation, got {escalations:?}"
    );
    assert_eq!(
        guard_escalations[0].cause,
        surge_core::run_event::EscalationCause::LoopGuardNodeDeadline
    );
    assert_eq!(
        guard_escalations[0].node.as_ref().map(|n| n.as_str()),
        Some("implement"),
        "the query must name the node that looped, not just that some run escalated"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn configured_spill_cap_is_honored_and_lands_in_the_runs_own_artifact_store() {
    let dir = tempfile::tempdir().unwrap();
    let big_content = "x".repeat(5_000);
    std::fs::write(dir.path().join("big.txt"), &big_content).unwrap();

    let storage = Storage::open(dir.path()).await.unwrap();
    let mock = Arc::new(MockBridge::new());
    let bridge: Arc<dyn BridgeFacade> = mock.clone();
    let dispatcher =
        Arc::new(WorktreeToolDispatcher::new(dir.path().to_path_buf())) as Arc<dyn ToolDispatcher>;

    let session_id = SessionId::new();
    mock.pin_next_session_id(session_id).await;
    let mut call = read_file_call("c1", "big.txt");
    if let BridgeEvent::ToolCall { session, .. } = &mut call {
        *session = session_id;
    }
    mock.enqueue_event(call).await;
    mock.enqueue_event(BridgeEvent::OutcomeReported {
        session: session_id,
        outcome: OutcomeKey::try_from("done").unwrap(),
        summary: "ok".into(),
        artifacts_produced: vec![],
    })
    .await;
    let mock_for_pump = mock.clone();
    let pump = tokio::spawn(async move {
        mock_for_pump.pump_after_subscribe(1).await;
    });

    let engine = Engine::new(bridge, storage.clone(), dispatcher, EngineConfig::default());

    let run_id = RunId::new();
    let run_config = EngineRunConfig {
        // Non-default: the built-in default is 64 KiB, comfortably above
        // the 5000-byte file below. Only a configured cap this small forces
        // a spill.
        output_spill: Some(OutputSpillConfig {
            max_output_bytes: 64,
        }),
        ..EngineRunConfig::default()
    };
    let handle = engine
        .start_run(
            run_id,
            agent_to_terminal_graph(),
            dir.path().to_path_buf(),
            run_config,
        )
        .await
        .expect("start_run");

    let outcome = handle.await_completion().await.unwrap();
    pump.await.unwrap();
    match outcome {
        RunOutcome::Completed { terminal } => assert_eq!(terminal.as_ref(), "end"),
        other => panic!("expected Completed, got {other:?}"),
    }

    let calls = mock.recorded_calls.lock().await;
    let result_json = calls
        .iter()
        .find_map(|c| match c {
            RecordedCall::ReplyToTool {
                call_id,
                payload: BridgeToolResult::Ok { result_json },
                ..
            } if call_id == "c1" => Some(result_json.clone()),
            _ => None,
        })
        .expect("expected an Ok reply to call c1");
    drop(calls);

    let parsed: serde_json::Value = serde_json::from_str(&result_json).unwrap();
    assert_eq!(
        parsed["spilled"],
        serde_json::json!(true),
        "output over the configured cap must be spilled, got {parsed}"
    );
    let locator = parsed["locator"]
        .as_str()
        .expect("spilled result must carry a locator");
    let hash_hex = locator
        .strip_prefix(&format!("artifact://{run_id}/"))
        .unwrap_or_else(|| panic!("locator {locator} must be scoped to run {run_id}"));
    let hash: ContentHash = hash_hex.parse().unwrap();

    // The store the *run* owns is rooted at `storage.home().join("runs")`
    // (`Engine::start_run`'s own construction) — reconstruct that exact
    // store and read the artifact back from it. If spill had instead used
    // `ArtifactStore::from_default_path()` (`~/.surge/runs`), this open
    // would fail: nothing was ever written under `storage.home()`.
    let run_store = ArtifactStore::new(storage.home().join("runs"));
    let fetched = run_store.open(run_id, hash).await.unwrap();
    assert_eq!(
        fetched,
        serde_json::json!({ "content_text": big_content })
            .to_string()
            .into_bytes(),
        "the run's own artifact store must hold the exact spilled tool output"
    );
}
