//! Task 23 — bootstrap materializes the remaining bundled archetypes.

mod fixtures;

use surge_orchestrator::engine::validate::validate_for_m6;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bootstrap_materializes_bug_fix_refactor_and_spike_archetypes() {
    for archetype in ["bug-fix", "refactor", "spike"] {
        let harness = fixtures::bootstrap::BootstrapHarness::new(3).await;
        let driver = harness.start(&format!("materialize the {archetype} archetype"));

        harness.wait_for_subscribe_count(1).await;
        harness
            .complete_agent_with_artifact(
                0,
                "description.md",
                &format!("## Goal\nExercise the {archetype} bootstrap path.\n"),
            )
            .await;
        harness.approve_next_gate().await;

        harness.wait_for_subscribe_count(2).await;
        harness
            .complete_agent_with_artifact(1, "roadmap.md", "## Milestones\n1. Plan\n2. Execute\n")
            .await;
        harness.approve_next_gate().await;

        harness.wait_for_subscribe_count(3).await;
        harness
            .complete_agent_with_artifact(
                2,
                "flow.toml",
                &fixtures::bootstrap::bundled_flow_toml(archetype),
            )
            .await;
        harness.approve_next_gate().await;

        let materialized = driver.await.unwrap().expect("bootstrap succeeds");
        assert_eq!(materialized.materialized_graph.metadata.name, archetype);
        validate_for_m6(&materialized.materialized_graph).unwrap();
    }
}
