//! M6 placeholder: loop body failure with `on_iteration_failure = Retry { max: 2 }`.
//! Requires a mock agent scripted to fail on first two attempts, succeed on
//! the third. Full e2e needs mock_acp_agent with attempt-counter logic.

#[test]
#[ignore = "M6 loop retry: requires mock agent scripted to fail then succeed (M7)"]
fn loop_body_retries_up_to_max_then_succeeds() {
    // M7: build a loop with on_iteration_failure = Retry { max: 2 } and a mock
    // agent that fails on attempts 1 and 2, succeeds on attempt 3.
    // Verify LoopIterationCompleted fires once (after the retry).
}
