//! Task 23 — Flow Generator validation retries backtrack before materializing.

mod fixtures;
use fixtures::runtime_home as runtime_home_fixture;

use surge_core::run_event::{BootstrapStage, EventPayload};
use surge_orchestrator::engine::bootstrap::VALIDATION_FAILED_OUTCOME;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn flow_generator_invalid_twice_then_valid_materializes_after_two_edits() {
    let harness = fixtures::bootstrap::BootstrapHarness::new(5).await;
    let driver = harness.start("retry invalid generated flows until one validates");

    harness.wait_for_subscribe_count(1).await;
    harness
        .complete_agent_with_artifact(0, "description.md", fixtures::bootstrap::VALID_DESCRIPTION)
        .await;
    harness.approve_next_gate().await;

    harness.wait_for_subscribe_count(2).await;
    harness
        .complete_agent_with_artifact(1, "roadmap.toml", fixtures::bootstrap::VALID_ROADMAP)
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
    assert_eq!(materialized.graph.metadata.name, "linear-3");

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
    harness.close();
}

/// Both author turns produce valid future-child graphs. They must not replace
/// the parent graph which owns the first edit and the second review occurrence.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn valid_flow_edit_requires_fresh_second_gate() {
    use fixtures::mock_bridge::RecordedCall;
    use futures::FutureExt as _;
    use std::time::Duration;

    let harness = fixtures::bootstrap::BootstrapHarness::new(4).await;
    let driver = harness.start("review a valid Flow, edit it, then approve its replacement");
    let observed = std::panic::AssertUnwindSafe(async {
        for (index, file, content) in [
            (
                0,
                "description.md",
                fixtures::bootstrap::VALID_DESCRIPTION.to_owned(),
            ),
            (
                1,
                "roadmap.toml",
                fixtures::bootstrap::VALID_ROADMAP.to_owned(),
            ),
        ] {
            harness.wait_for_subscribe_count(index + 1).await;
            harness
                .complete_agent_with_artifact(index, file, &content)
                .await;
            harness.approve_next_gate().await;
        }
        harness.wait_for_subscribe_count(3).await;
        harness
            .complete_agent_with_artifact(
                2,
                "flow.toml",
                &fixtures::bootstrap::bundled_flow_toml("linear-3"),
            )
            .await;
        let first = wait_for_flow_request(&harness, 1).await;
        harness.edit_next_gate("Use the revised Flow name").await;
        harness.wait_for_subscribe_count(4).await;
        let mut revised = surge_core::BundledFlows::by_name_latest("linear-3")
            .unwrap()
            .graph;
        revised.metadata.name = "revised-flow".into();
        harness
            .complete_agent_with_artifact(3, "flow.toml", &toml::to_string(&revised).unwrap())
            .await;
        let second = wait_for_flow_request(&harness, 2).await;
        let EventPayload::HumanInputRequested {
            node,
            call_id: Some(first_id),
            ..
        } = first.payload.payload()
        else {
            panic!("first Flow request identity")
        };
        let EventPayload::HumanInputRequested {
            call_id: Some(second_id),
            ..
        } = second.payload.payload()
        else {
            panic!("second Flow request identity")
        };
        assert_ne!(first_id, second_id);
        assert!(first.seq < second.seq);
        assert!(
            harness
                .engine
                .resolve_requested_input(
                    harness.run_id,
                    node.clone(),
                    Some(first_id.clone()),
                    serde_json::json!({"outcome":"approve"}),
                )
                .await
                .is_err(),
            "old request cannot answer the new review"
        );
        assert!(
            harness
                .engine
                .resolve_requested_input(
                    harness.run_id,
                    node.clone(),
                    Some("wrong-flow-request".into()),
                    serde_json::json!({"outcome":"approve"}),
                )
                .await
                .is_err(),
            "wrong request cannot answer the new review"
        );
        harness.approve_next_gate().await;
    })
    .catch_unwind()
    .await;

    // Stop the actual engine owner before joining its observer on failures.
    // Aborting only the observer would detach the RunHandle's execution task.
    let joined = if observed.is_err() {
        let _ = harness
            .engine
            .stop_run(harness.run_id, "fixture assertion failed".into())
            .await;
        let _ = driver.await;
        None
    } else {
        let mut driver = driver;
        match tokio::time::timeout(Duration::from_secs(10), &mut driver).await {
            Ok(result) => Some(result),
            Err(_) => {
                let _ = harness
                    .engine
                    .stop_run(harness.run_id, "fixture completion timed out".into())
                    .await;
                let _ = driver.await;
                None
            },
        }
    };
    let verified = std::panic::AssertUnwindSafe(async {
        if observed.is_ok() {
            let handoff = joined.expect("bootstrap driver must settle").unwrap().expect("valid revised Flow hands off");
            assert_eq!(handoff.graph.metadata.name, "revised-flow");
            let events = harness.read_events().await;
            let payloads = fixtures::bootstrap::event_payloads(&events);
            let entries: Vec<_> = events.iter().filter(|event| matches!(event.payload.payload(), EventPayload::StageEntered { node, .. } if node.as_str() == "flow_gate")).map(|event| event.seq).collect();
            assert_eq!(entries.len(), 2);
            assert!(entries[0] < entries[1]);
            assert_eq!(payloads.iter().filter(|event| matches!(event, EventPayload::HumanInputRequested { node, .. } if node.as_str() == "flow_gate")).count(), 2);
            assert_eq!(payloads.iter().filter(|event| matches!(event, EventPayload::BootstrapEditRequested { stage: BootstrapStage::Flow, .. })).count(), 1);
            for (node_name, outcome_name, expected) in [("flow_generator", "drafted", 2), ("flow_gate", "edit", 1), ("flow_gate", "approve", 1)] {
                assert_eq!(payloads.iter().filter(|event| matches!(event, EventPayload::StageCompleted { node, outcome, .. } if node.as_str() == node_name && outcome.as_str() == outcome_name)).count(), expected);
            }
            let author_commits: Vec<_> = events.iter().filter_map(|event| match event.payload.payload() {
                EventPayload::StageOutcomeCommitted { commit } if commit.context().node.as_str() == "flow_generator" => Some(commit.invocation()),
                _ => None,
            }).collect();
            assert_eq!(author_commits.len(), 2);
            assert_eq!(payloads.iter().filter(|event| matches!(event, EventPayload::StageRouteCommitted { invocation, .. } if author_commits.contains(invocation))).count(), 2);
            assert_eq!(payloads.iter().filter(|event| matches!(event, EventPayload::GateStageRouteCommitted { stage_entry_seq, .. } if entries.iter().any(|entry| entry.as_u64() == *stage_entry_seq))).count(), 2);
            assert!(!payloads.iter().any(|event| matches!(event, EventPayload::RunRecoveryRequired { .. } | EventPayload::RunFailed { .. } | EventPayload::RunAborted { .. })));
            let calls = harness.mock.recorded_calls.lock().await;
            assert_eq!(calls.iter().filter(|call| matches!(call, RecordedCall::SendMessage { session } if harness.sessions[2..].contains(session))).count(), 2);
        }
    }).catch_unwind().await;
    harness.close();
    if let Err(panic) = observed.and(verified) {
        std::panic::resume_unwind(panic);
    }
}

async fn wait_for_flow_request(
    harness: &fixtures::bootstrap::BootstrapHarness,
    expected: usize,
) -> surge_persistence::runs::ReadEvent {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let events = harness.read_events().await;
            assert!(!events.iter().any(|event| matches!(event.payload.payload(), EventPayload::RunRecoveryRequired { .. })), "actual Engine refused the gate revisit: {events:?}");
            let requests: Vec<_> = events.iter().filter(|event| matches!(event.payload.payload(), EventPayload::HumanInputRequested { node, .. } if node.as_str() == "flow_gate")).collect();
            if requests.len() == expected {
                return requests[expected - 1].clone();
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }).await.expect("fresh Flow review must be durably requested")
}
