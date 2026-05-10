//! Task 19 — bootstrap driver smoke test with scripted agents and approvals.

mod fixtures;

use std::sync::Arc;
use std::time::Duration;

use surge_acp::bridge::event::BridgeEvent;
use surge_acp::bridge::facade::BridgeFacade;
use surge_core::id::{RunId, SessionId};
use surge_core::keys::OutcomeKey;
use surge_orchestrator::bootstrap_driver::run_bootstrap_in_worktree;
use surge_orchestrator::engine::tools::ToolDispatcher;
use surge_orchestrator::engine::tools::worktree::WorktreeToolDispatcher;
use surge_orchestrator::engine::{Engine, EngineConfig};
use surge_persistence::runs::Storage;

async fn wait_for_subscribe_count(mock: &fixtures::mock_bridge::MockBridge, expected: usize) {
    for _ in 0..100 {
        let count = mock
            .recorded_calls
            .lock()
            .await
            .iter()
            .filter(|call| matches!(call, fixtures::mock_bridge::RecordedCall::Subscribe))
            .count();
        if count >= expected {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("timed out waiting for {expected} bridge subscribers");
}

async fn approve_next_gate(engine: &Engine, run_id: RunId) {
    for _ in 0..100 {
        let result = engine
            .resolve_human_input(
                run_id,
                None,
                serde_json::json!({"outcome": "approve", "comment": "ok"}),
            )
            .await;
        if result.is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("timed out waiting for pending bootstrap HumanGate");
}

async fn report_agent_outcome(
    mock: &fixtures::mock_bridge::MockBridge,
    session: SessionId,
    outcome: &str,
    artifact: &str,
) {
    mock.enqueue_event(BridgeEvent::OutcomeReported {
        session,
        outcome: OutcomeKey::try_from(outcome).unwrap(),
        summary: "scripted".into(),
        artifacts_produced: vec![artifact.into()],
    })
    .await;
    mock.pump_scripted_events().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn run_bootstrap_materializes_followup_graph() {
    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::open(dir.path()).await.unwrap();
    let mock = Arc::new(fixtures::mock_bridge::MockBridge::new());
    let bridge: Arc<dyn BridgeFacade> = mock.clone();
    let dispatcher =
        Arc::new(WorktreeToolDispatcher::new(dir.path().to_path_buf())) as Arc<dyn ToolDispatcher>;
    let engine = Arc::new(Engine::new(
        bridge,
        storage,
        dispatcher,
        EngineConfig::default(),
    ));

    let run_id = RunId::new();
    let description_session = SessionId::new();
    let roadmap_session = SessionId::new();
    let flow_session = SessionId::new();
    mock.pin_session_ids(vec![description_session, roadmap_session, flow_session])
        .await;

    let driver_engine = engine.clone();
    let worktree = dir.path().to_path_buf();
    let driver = tokio::spawn(async move {
        run_bootstrap_in_worktree(
            driver_engine.as_ref(),
            "build an adaptive bootstrap flow".into(),
            run_id,
            worktree,
            None,
        )
        .await
    });

    wait_for_subscribe_count(&mock, 1).await;
    tokio::fs::write(
        dir.path().join("description.md"),
        "## Goal\nBuild adaptive bootstrap.\n",
    )
    .await
    .unwrap();
    report_agent_outcome(&mock, description_session, "drafted", "description.md").await;
    approve_next_gate(engine.as_ref(), run_id).await;

    wait_for_subscribe_count(&mock, 2).await;
    tokio::fs::write(
        dir.path().join("roadmap.md"),
        "## Milestones\n1. Bootstrap graph\n2. Driver\n3. CLI\n",
    )
    .await
    .unwrap();
    report_agent_outcome(&mock, roadmap_session, "drafted", "roadmap.md").await;
    approve_next_gate(engine.as_ref(), run_id).await;

    wait_for_subscribe_count(&mock, 3).await;
    tokio::fs::write(
        dir.path().join("flow.toml"),
        include_str!("fixtures/golden_multi_milestone_flow.toml"),
    )
    .await
    .unwrap();
    report_agent_outcome(&mock, flow_session, "drafted", "flow.toml").await;
    approve_next_gate(engine.as_ref(), run_id).await;

    let materialized = driver.await.unwrap().expect("bootstrap driver succeeds");
    assert_eq!(materialized.bootstrap_run_id, run_id);
    assert_eq!(
        materialized.materialized_graph.metadata.name,
        "golden_multi_milestone"
    );
    assert_eq!(
        materialized
            .artifacts
            .iter()
            .map(|artifact| artifact.name.as_str())
            .collect::<Vec<_>>(),
        vec!["description", "roadmap", "flow"]
    );
}
