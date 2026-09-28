//! Retry exhaustion must stop the loop after the configured attempt budget.
mod fixtures;
#[path = "fixtures/loop_failure.rs"]
mod scenario;

use surge_core::loop_config::FailurePolicy;
use surge_core::run_event::EventPayload;
use surge_orchestrator::engine::RunOutcome;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn loop_failure_retries_exactly_to_cap_then_fails() {
    let (outcome, events) = scenario::failing_body(FailurePolicy::Retry { max: 2 }).await;
    let starts: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            EventPayload::LoopIterationStarted { index, .. } => Some(*index),
            _ => None,
        })
        .collect();
    assert_eq!(starts, vec![0, 0, 0], "initial attempt plus two retries");
    assert!(matches!(outcome, RunOutcome::Failed { .. }), "{outcome:?}");
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, EventPayload::RunCompleted { .. }))
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn abort_policy_stops_at_first_failed_item() {
    let (outcome, events) = scenario::failing_body(FailurePolicy::Abort).await;
    assert!(matches!(outcome, RunOutcome::Failed { .. }));
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, EventPayload::LoopIterationStarted { .. }))
            .count(),
        1
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn zero_retry_budget_still_attempts_the_item_once() {
    let (outcome, events) = scenario::failing_body(FailurePolicy::Retry { max: 0 }).await;
    assert!(matches!(outcome, RunOutcome::Failed { .. }));
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, EventPayload::LoopIterationStarted { .. }))
            .count(),
        1
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn loop_body_retries_up_to_max_then_succeeds() {
    let (outcome, events) = scenario::scripted_body(
        FailurePolicy::Retry { max: 2 },
        &["failed", "failed", "done", "done", "done"],
    )
    .await;
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
        vec![
            (0, "failed"),
            (0, "failed"),
            (0, "completed"),
            (1, "completed"),
            (2, "completed")
        ]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_item_receives_its_own_retry_budget() {
    let (outcome, events) = scenario::scripted_body(
        FailurePolicy::Retry { max: 2 },
        &[
            "failed", "failed", "done", "failed", "failed", "done", "done",
        ],
    )
    .await;
    assert!(
        matches!(outcome, RunOutcome::Completed { .. }),
        "{outcome:?}"
    );
    let starts: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            EventPayload::LoopIterationStarted { index, .. } => Some(*index),
            _ => None,
        })
        .collect();
    assert_eq!(starts, vec![0, 0, 0, 1, 1, 1, 2]);
}
