//! A decision captured for an earlier visit must never consume a later resolver.
mod fixtures;
use fixtures::runtime_home as runtime_home_fixture;
use runtime_home_fixture::FixtureHome;
use std::{sync::Arc, time::Duration};
use surge_core::{id::RunId, run_event::EventPayload};
use surge_orchestrator::engine::{
    Engine, EngineConfig, EngineRunConfig, tools::worktree::WorktreeToolDispatcher,
};
use surge_persistence::runs::Storage;
/// A genuine hang fails here. Sized like the crate's other engine round-trip
/// tests: a loaded Windows runner needs several seconds for the durable
/// open/append/reopen steps of the scenario, which the former 5 s budget also
/// had to absorb together with fixture setup and teardown.
const WATCHDOG: Duration = Duration::from_secs(30);
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
[[nodes.gate.declared_outcomes]]
id = "edit"
description = "Revise"
edge_kind_hint = "backtrack"
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
[[nodes.gate.config.options]]
outcome = "edit"
label = "Edit"
[[edges]]
id = "approved"
to = "end"
kind = "forward"
[edges.from]
node = "gate"
outcome = "approve"
[edges.policy]
on_max_exceeded = "escalate"
[[edges]]
id = "again"
to = "gate"
kind = "backtrack"
[edges.from]
node = "gate"
outcome = "edit"
[edges.policy]
max_traversals = 3
on_max_exceeded = "fail"
"#;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn old_card_cannot_resolve_a_new_visit_to_the_same_gate() {
    // Home creation and removal are filesystem setup/teardown, not the
    // behaviour under test; on Windows they run native ACL work and bounded
    // retries on handle release, so they stay outside the hang watchdog.
    let temp = FixtureHome::new().unwrap();
    tokio::time::timeout(WATCHDOG, async {
        {
            let storage = Storage::open(temp.path()).await.unwrap();
            let engine = Engine::new(
                Arc::new(fixtures::mock_bridge::MockBridge::new()),
                storage,
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
            let first = next_request(&mut tap, id).await;
            engine
                .resolve_gate_input(
                    id,
                    first.0.clone(),
                    first.1,
                    serde_json::json!({"outcome":"edit"}),
                )
                .await
                .unwrap();
            let second = next_request(&mut tap, id).await;
            assert_eq!(first.0, second.0);
            assert_ne!(first.1, second.1);
            let wrong_node = engine
                .resolve_gate_input(
                    id,
                    "other".try_into().unwrap(),
                    second.1,
                    serde_json::json!({"outcome":"approve"}),
                )
                .await;
            assert!(
                matches!(
                    &wrong_node,
                    Err(surge_orchestrator::engine::EngineError::StaleGateRequest)
                ),
                "wrong-node result: {wrong_node:?}"
            );
            let stale = engine
                .resolve_gate_input(
                    id,
                    first.0.clone(),
                    first.1,
                    serde_json::json!({"outcome":"approve"}),
                )
                .await;
            assert!(matches!(
                stale,
                Err(surge_orchestrator::engine::EngineError::StaleGateRequest)
            ));
            assert!(matches!(
                engine
                    .resolve_human_input(id, None, serde_json::json!({"outcome":"approve"}))
                    .await,
                Err(surge_orchestrator::engine::EngineError::MissingGateRequestIdentity)
            ));
            // Rejected stale/unbound decisions must leave the current resolver usable.
            engine
                .resolve_gate_input(
                    id,
                    second.0,
                    second.1,
                    serde_json::json!({"outcome":"approve"}),
                )
                .await
                .unwrap();
            assert!(matches!(
                handle.await_completion().await.unwrap(),
                surge_orchestrator::engine::RunOutcome::Completed { .. }
            ));
        }
    })
    .await
    .expect("gate identity fixture exceeded watchdog");
    temp.close().unwrap();
}

async fn next_request(
    tap: &mut tokio::sync::broadcast::Receiver<surge_orchestrator::engine::RunEventTap>,
    id: RunId,
) -> (
    surge_core::keys::NodeKey,
    surge_core::id::GateRequestId,
    surge_persistence::runs::EventSeq,
) {
    next_request_after(tap, id, surge_persistence::runs::EventSeq::ZERO).await
}

async fn next_request_after(
    tap: &mut tokio::sync::broadcast::Receiver<surge_orchestrator::engine::RunEventTap>,
    id: RunId,
    after: surge_persistence::runs::EventSeq,
) -> (
    surge_core::keys::NodeKey,
    surge_core::id::GateRequestId,
    surge_persistence::runs::EventSeq,
) {
    loop {
        let event = tap.recv().await.unwrap();
        if event.run_id != id || event.event.seq <= after {
            continue;
        }
        if let EventPayload::HumanInputRequested {
            node,
            session: None,
            call_id: Some(call_id),
            ..
        } = event.event.payload.payload
        {
            return (
                node,
                surge_core::id::GateRequestId::from_event_call_id(&call_id).unwrap(),
                event.event.seq,
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reopening_after_owner_loss_reissues_an_unanswered_gate_with_fresh_identity() {
    // Home creation and removal are filesystem setup/teardown, not the
    // behaviour under test; on Windows they run native ACL work and bounded
    // retries on handle release, so they stay outside the hang watchdog.
    let temp = FixtureHome::new().unwrap();
    tokio::time::timeout(WATCHDOG, async {
        {
            let storage = Storage::open(temp.path()).await.unwrap();
            let build = |storage| {
                Engine::new(
                    Arc::new(fixtures::mock_bridge::MockBridge::new()),
                    storage,
                    Arc::new(WorktreeToolDispatcher::new(temp.path().to_path_buf())),
                    EngineConfig::default(),
                )
            };
            let engine = build(storage.clone());
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
            let old = next_request(&mut tap, id).await;
            handle.completion.abort();
            assert!(handle.completion.await.unwrap_err().is_cancelled());
            drop(engine);
            drop(storage);
            let storage = Storage::open(temp.path()).await.unwrap();
            let engine = build(storage);
            let mut tap = engine.subscribe_tap();
            let resumed = engine
                .resume_run(id, temp.path().to_path_buf())
                .await
                .unwrap();
            // Reopening forwards the durable history before new appends. Select by
            // the persisted sequence boundary, independently of the identity
            // assertion below, so accidental identity reuse still fails.
            let fresh = next_request_after(&mut tap, id, old.2).await;
            assert!(fresh.2 > old.2);
            assert_eq!(old.0, fresh.0);
            assert_ne!(old.1, fresh.1);
            assert!(matches!(
                engine
                    .resolve_gate_input(id, old.0, old.1, serde_json::json!({"outcome":"approve"}),)
                    .await,
                Err(surge_orchestrator::engine::EngineError::StaleGateRequest)
            ));
            engine
                .resolve_gate_input(
                    id,
                    fresh.0,
                    fresh.1,
                    serde_json::json!({"outcome":"approve"}),
                )
                .await
                .unwrap();
            assert!(matches!(
                resumed.await_completion().await.unwrap(),
                surge_orchestrator::engine::RunOutcome::Completed { .. }
            ));
        }
    })
    .await
    .expect("restart identity fixture exceeded watchdog");
    temp.close().unwrap();
}
