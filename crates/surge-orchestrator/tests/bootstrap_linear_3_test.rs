//! Task 21 — bootstrap materializes the linear-3 archetype.

mod fixtures;

use surge_core::run_event::{BootstrapStage, EventPayload};
use surge_orchestrator::engine::validate::validate_for_m6;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bootstrap_linear_3_materializes_valid_followup_graph() {
    let harness = fixtures::bootstrap::BootstrapHarness::new(3).await;
    let driver = harness.start("ship a small focused CLI improvement");

    harness.wait_for_subscribe_count(1).await;
    harness
        .complete_agent_with_artifact(
            0,
            "description.md",
            "## Goal\nShip a small focused CLI improvement.\n",
        )
        .await;
    harness.approve_next_gate().await;

    harness.wait_for_subscribe_count(2).await;
    harness
        .complete_agent_with_artifact(
            1,
            "roadmap.md",
            "## Milestones\n1. Specify\n2. Implement\n3. Verify\n",
        )
        .await;
    harness.approve_next_gate().await;

    harness.wait_for_subscribe_count(3).await;
    harness
        .complete_agent_with_artifact(
            2,
            "flow.toml",
            &fixtures::bootstrap::bundled_flow_toml("linear-3"),
        )
        .await;
    harness.approve_next_gate().await;

    let materialized = driver.await.unwrap().expect("bootstrap succeeds");
    assert_eq!(materialized.materialized_graph.metadata.name, "linear-3");
    validate_for_m6(&materialized.materialized_graph).unwrap();

    let events = harness.read_events().await;
    assert!(events.iter().any(|event| {
        matches!(
            &event.payload.payload,
            EventPayload::PipelineMaterialized { graph, .. }
                if graph.metadata.name == "linear-3"
        )
    }));
    let telemetry = events
        .iter()
        .find_map(|event| match &event.payload.payload {
            EventPayload::BootstrapTelemetry {
                stage_durations,
                edit_counts,
                archetype,
            } => Some((stage_durations, edit_counts, archetype)),
            _ => None,
        })
        .expect("BootstrapTelemetry event emitted");
    assert!(telemetry.0.contains_key(&BootstrapStage::Description));
    assert!(telemetry.0.contains_key(&BootstrapStage::Roadmap));
    assert!(telemetry.0.contains_key(&BootstrapStage::Flow));
    assert!(telemetry.1.is_empty());
    assert!(telemetry.2.is_none());
}
