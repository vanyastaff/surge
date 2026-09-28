//! Cancellation must not fabricate an operator decision or timeout.
mod fixtures;

use std::sync::Arc;
use std::time::Duration;
use surge_core::id::RunId;
use surge_core::run_event::EventPayload;
use surge_orchestrator::engine::tools::worktree::WorktreeToolDispatcher;
use surge_orchestrator::engine::{Engine, EngineConfig, EngineRunConfig, RunOutcome};
use surge_persistence::runs::{EventSeq, Storage};

const FLOW: &str = r#"# Smallest possible flow.toml — single Terminal node.
#
# When run via `surge engine run examples/flow_terminal_only.toml`,
# the run starts, immediately hits the Terminal node, emits
# RunCompleted, and exits cleanly — no agent / no MCP / no human
# input required. The fastest possible demo of the pipeline +
# persistence + IPC.

schema_version = 1
start = "gate"


[metadata]
name = "flow_terminal_only"
created_at = "2026-05-05T00:00:00Z"

[nodes.end]
id = "end"
declared_outcomes = []

[nodes.end.position]
x = 0.0
y = 0.0

[nodes.end.config]
node_kind = "terminal"

[nodes.end.config.kind]
type = "success"

[nodes.gate]
id = "gate"
[nodes.gate.position]
x = 0.0
y = 0.0
[[nodes.gate.declared_outcomes]]
id = "approve"
description = "Approved"
edge_kind_hint = "forward"
is_terminal = false
[nodes.gate.config]
node_kind = "human_gate"
delivery_channels = []
mode = { bootstrap = { stage = "description" } }
[nodes.gate.config.summary]
title = "Approval"
body = "Decide"
[[nodes.gate.config.options]]
outcome = "approve"
label = "Approve"
[[edges]]
id = "approved"
to = "end"
kind = "forward"
[edges.from]
node = "gate"
outcome = "approve"
[edges.policy]
on_max_exceeded = "escalate"
"#;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stop_pending_gate_is_durably_aborted_without_a_decision() {
    let temp = tempfile::tempdir().unwrap();
    let storage = Storage::open(temp.path()).await.unwrap();
    let engine = Engine::new(
        Arc::new(fixtures::mock_bridge::MockBridge::new()),
        storage.clone(),
        Arc::new(WorktreeToolDispatcher::new(temp.path().to_path_buf())),
        EngineConfig::default(),
    );
    let id = RunId::new();
    let mut tap = engine.subscribe_tap();
    let handle = engine
        .start_run(
            id,
            toml::from_str(FLOW).unwrap(),
            temp.path().to_path_buf(),
            EngineRunConfig::default(),
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let event = tap.recv().await.unwrap();
            if event.run_id == id
                && matches!(
                    event.event.payload.payload,
                    EventPayload::HumanInputRequested { .. }
                )
            {
                break;
            }
        }
    })
    .await
    .unwrap();
    engine
        .stop_run(id, "test cancellation".into())
        .await
        .unwrap();
    let outcome = tokio::time::timeout(Duration::from_secs(2), handle.await_completion())
        .await
        .expect("gate did not cancel")
        .unwrap();
    assert!(matches!(outcome, RunOutcome::Aborted { .. }), "{outcome:?}");
    let stale = engine
        .resolve_human_input(id, None, serde_json::json!({"outcome": "approve"}))
        .await;
    assert!(matches!(
        stale,
        Err(surge_orchestrator::engine::EngineError::RunNotFound(_))
    ));
    let events = storage
        .open_run_reader(id)
        .await
        .unwrap()
        .read_events(EventSeq(0)..EventSeq(u64::MAX))
        .await
        .unwrap();
    let kinds: Vec<_> = events
        .iter()
        .map(|event| event.payload.payload.discriminant_str())
        .collect();
    assert_eq!(
        kinds.iter().filter(|kind| **kind == "RunAborted").count(),
        1,
        "{kinds:?}"
    );
    for forbidden in [
        "HumanInputResolved",
        "HumanInputTimedOut",
        "BootstrapApprovalDecided",
        "StageFailed",
        "RunFailed",
        "RunCompleted",
    ] {
        assert!(
            !kinds.contains(&forbidden),
            "invented {forbidden}: {kinds:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stop_racing_a_real_decision_preserves_a_complete_event_sequence() {
    let temp = tempfile::tempdir().unwrap();
    let storage = Storage::open(temp.path()).await.unwrap();
    let engine = Engine::new(
        Arc::new(fixtures::mock_bridge::MockBridge::new()),
        storage.clone(),
        Arc::new(WorktreeToolDispatcher::new(temp.path().to_path_buf())),
        EngineConfig::default(),
    );
    let id = RunId::new();
    let mut tap = engine.subscribe_tap();
    let handle = engine
        .start_run(
            id,
            toml::from_str(FLOW).unwrap(),
            temp.path().to_path_buf(),
            EngineRunConfig::default(),
        )
        .await
        .unwrap();
    let (node, call_id) = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let event = tap.recv().await.unwrap();
            if event.run_id == id
                && let EventPayload::HumanInputRequested { node, call_id, .. } =
                    event.event.payload.payload
            {
                break (node, call_id);
            }
        }
    })
    .await
    .unwrap();
    let (stopped, resolved) = tokio::join!(
        engine.stop_run(id, "concurrent cancellation".into()),
        engine.resolve_requested_input(
            id,
            node,
            call_id,
            serde_json::json!({"outcome": "approve"})
        ),
    );
    assert!(
        stopped.is_ok()
            || matches!(
                stopped,
                Err(surge_orchestrator::engine::EngineError::RunNotFound(_))
            )
    );
    if let Err(error) = resolved {
        use surge_orchestrator::engine::EngineError;
        assert!(
            matches!(
                error,
                EngineError::RunNotFound(_) | EngineError::StaleGateRequest
            ) || matches!(&error, EngineError::Internal(message) if message == "gate resolution receiver dropped" || message == "no pending HumanGate to resolve"),
            "{error:?}"
        );
    }
    let outcome = tokio::time::timeout(Duration::from_secs(2), handle.await_completion())
        .await
        .expect("gate did not cancel")
        .unwrap();
    assert!(
        matches!(
            outcome,
            RunOutcome::Aborted { .. } | RunOutcome::Completed { .. }
        ),
        "{outcome:?}"
    );
    let events = storage
        .open_run_reader(id)
        .await
        .unwrap()
        .read_events(EventSeq(0)..EventSeq(u64::MAX))
        .await
        .unwrap();
    let kinds: Vec<_> = events
        .iter()
        .map(|event| event.payload.payload.discriminant_str())
        .collect();
    let terminals = kinds
        .iter()
        .filter(|kind| matches!(**kind, "RunAborted" | "RunCompleted" | "RunFailed"))
        .count();
    assert_eq!(terminals, 1, "{kinds:?}");
    let resolved_count = kinds
        .iter()
        .filter(|kind| **kind == "HumanInputResolved")
        .count();
    let decided_count = kinds
        .iter()
        .filter(|kind| **kind == "BootstrapApprovalDecided")
        .count();
    let reported_count = kinds
        .iter()
        .filter(|kind| **kind == "OutcomeReported")
        .count();
    assert_eq!(resolved_count, decided_count, "{kinds:?}");
    assert_eq!(resolved_count, reported_count, "{kinds:?}");
    for forbidden in ["HumanInputTimedOut", "StageFailed", "RunFailed"] {
        assert!(
            !kinds.contains(&forbidden),
            "invented {forbidden}: {kinds:?}"
        );
    }
}
