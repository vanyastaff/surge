//! Task 23 — bootstrap HumanGate edit-loop cap fails after repeated edits.

mod fixtures;

use surge_core::run_event::{BootstrapStage, EventPayload};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn repeated_description_edits_fail_after_configured_cap() {
    let harness = fixtures::bootstrap::BootstrapHarness::new(4).await;
    let driver = harness.start("keep asking for description edits until the cap trips");

    for attempt in 0..4 {
        harness.wait_for_subscribe_count(attempt + 1).await;
        harness
            .complete_agent_with_artifact(
                attempt,
                "description.md",
                &format!("## Goal\nDescription draft attempt {}.\n", attempt + 1),
            )
            .await;
        harness
            .edit_next_gate(&format!("please revise attempt {}", attempt + 1))
            .await;
    }

    let error = driver
        .await
        .unwrap()
        .expect_err("bootstrap should fail once edit-loop cap is exceeded");
    let message = error.to_string();
    assert!(
        message.contains("edit-loop cap exceeded"),
        "unexpected error: {message}"
    );

    let events = harness.read_events().await;
    let payloads = fixtures::bootstrap::event_payloads(&events);

    let description_edit_count = payloads
        .iter()
        .filter(|payload| {
            matches!(
                payload,
                EventPayload::BootstrapEditRequested {
                    stage: BootstrapStage::Description,
                    ..
                }
            )
        })
        .count();
    assert_eq!(description_edit_count, 3);

    assert!(payloads.iter().any(|payload| {
        matches!(
            payload,
            EventPayload::EscalationRequested {
                stage: Some(BootstrapStage::Description),
                ..
            }
        )
    }));
}
