//! Task 23 — Flow Generator validation retries backtrack before materializing.

mod fixtures;

use surge_core::run_event::{BootstrapStage, EventPayload};
use surge_orchestrator::engine::bootstrap::VALIDATION_FAILED_OUTCOME;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn flow_generator_invalid_twice_then_valid_materializes_after_two_edits() {
    let harness = fixtures::bootstrap::BootstrapHarness::new(5).await;
    let driver = harness.start("retry invalid generated flows until one validates");

    harness.wait_for_subscribe_count(1).await;
    harness
        .complete_agent_with_artifact(0, "description.md", "## Goal\nRetry invalid flows.\n")
        .await;
    harness.approve_next_gate().await;

    harness.wait_for_subscribe_count(2).await;
    harness
        .complete_agent_with_artifact(1, "roadmap.md", "## Milestones\n1. Retry flow\n")
        .await;
    harness.approve_next_gate().await;

    harness.wait_for_subscribe_count(3).await;
    harness
        .complete_agent_with_artifact(2, "flow.toml", "this is not valid toml = = =")
        .await;

    harness.wait_for_subscribe_count(4).await;
    harness
        .complete_agent_with_artifact(3, "flow.toml", "schema_version = 1\nstart = 123\n")
        .await;

    harness.wait_for_subscribe_count(5).await;
    harness
        .complete_agent_with_artifact(
            4,
            "flow.toml",
            &fixtures::bootstrap::bundled_flow_toml("linear-3"),
        )
        .await;
    harness.approve_next_gate().await;

    let materialized = driver.await.unwrap().expect("bootstrap succeeds");
    assert_eq!(materialized.materialized_graph.metadata.name, "linear-3");

    let events = harness.read_events().await;
    let payloads = fixtures::bootstrap::event_payloads(&events);

    let flow_edit_count = payloads
        .iter()
        .filter(|payload| {
            matches!(
                payload,
                EventPayload::BootstrapEditRequested {
                    stage: BootstrapStage::Flow,
                    ..
                }
            )
        })
        .count();
    assert_eq!(flow_edit_count, 2);

    let validation_failed_outcomes = payloads
        .iter()
        .filter(|payload| {
            matches!(
                payload,
                EventPayload::OutcomeReported { outcome, .. }
                    if outcome.as_str() == VALIDATION_FAILED_OUTCOME
            )
        })
        .count();
    assert_eq!(validation_failed_outcomes, 2);
}
