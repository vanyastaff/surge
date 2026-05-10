//! Task 22 — bootstrap materializes a roadmap-driven multi-milestone flow.

mod fixtures;

use surge_core::loop_config::IterableSource;
use surge_core::node::NodeConfig;
use surge_orchestrator::engine::validate::validate_for_m6;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bootstrap_multi_milestone_materializes_outer_and_inner_loops() {
    let harness = fixtures::bootstrap::BootstrapHarness::new(3).await;
    let driver = harness.start("build three roadmap milestones with per-task verification");

    harness.wait_for_subscribe_count(1).await;
    harness
        .complete_agent_with_artifact(
            0,
            "description.md",
            "## Goal\nBuild a three-milestone roadmap.\n",
        )
        .await;
    harness.approve_next_gate().await;

    harness.wait_for_subscribe_count(2).await;
    harness
        .complete_agent_with_artifact(
            1,
            "roadmap.md",
            "## Milestones\n1. Intake\n2. Execution\n3. Review\n",
        )
        .await;
    harness.approve_next_gate().await;

    harness.wait_for_subscribe_count(3).await;
    harness
        .complete_agent_with_artifact(
            2,
            "flow.toml",
            include_str!("fixtures/golden_multi_milestone_flow.toml"),
        )
        .await;
    harness.approve_next_gate().await;

    let materialized = driver.await.unwrap().expect("bootstrap succeeds");
    validate_for_m6(&materialized.materialized_graph).unwrap();
    assert!(has_roadmap_milestone_outer_loop(
        &materialized.materialized_graph
    ));
    assert!(has_current_milestone_task_loop(
        &materialized.materialized_graph
    ));
}

fn has_roadmap_milestone_outer_loop(graph: &surge_core::graph::Graph) -> bool {
    graph.nodes.values().any(|node| {
        let NodeConfig::Loop(config) = &node.config else {
            return false;
        };
        let IterableSource::Artifact { name, .. } = &config.iterates_over else {
            return false;
        };
        name == "roadmap.milestones"
            && graph.subgraphs.get(&config.body).is_some_and(|body| {
                body.nodes
                    .values()
                    .any(|inner| matches!(inner.config, NodeConfig::Loop(_)))
            })
    })
}

fn has_current_milestone_task_loop(graph: &surge_core::graph::Graph) -> bool {
    graph.subgraphs.values().any(|subgraph| {
        subgraph.nodes.values().any(|node| {
            let NodeConfig::Loop(config) = &node.config else {
                return false;
            };
            matches!(
                &config.iterates_over,
                IterableSource::LoopItem { var, jsonpath }
                    if var == "milestone" && jsonpath == "tasks"
            )
        })
    })
}
