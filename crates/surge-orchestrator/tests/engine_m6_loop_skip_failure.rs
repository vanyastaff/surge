//! Skip policy must record failures and advance through every item.
mod fixtures;
#[path = "fixtures/loop_failure.rs"]
mod scenario;

use surge_core::loop_config::FailurePolicy;
use surge_core::run_event::EventPayload;
use surge_orchestrator::engine::RunOutcome;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn loop_failure_with_skip_continues_through_all_items() {
    let (outcome, events) = scenario::failing_body(FailurePolicy::Skip).await;
    let completions: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            EventPayload::LoopIterationCompleted { index, outcome, .. } => {
                Some((*index, outcome.as_ref()))
            },
            _ => None,
        })
        .collect();
    assert_eq!(
        completions,
        vec![(0, "failed"), (1, "failed"), (2, "failed")]
    );
    assert!(
        matches!(outcome, RunOutcome::Completed { .. }),
        "{outcome:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn loop_body_failure_with_skip_policy_continues_loop() {
    let (outcome, events) =
        scenario::scripted_body(FailurePolicy::Skip, &["done", "failed", "done"]).await;
    assert!(
        matches!(outcome, RunOutcome::Completed { .. }),
        "{outcome:?}"
    );
    let results: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            EventPayload::LoopIterationCompleted { index, outcome, .. } => {
                Some((*index, outcome.as_ref()))
            },
            _ => None,
        })
        .collect();
    assert_eq!(
        results,
        vec![(0, "completed"), (1, "failed"), (2, "completed")]
    );
}
