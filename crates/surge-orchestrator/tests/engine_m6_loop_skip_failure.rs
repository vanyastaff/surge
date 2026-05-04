//! M6 placeholder: loop body failure with `on_iteration_failure = Skip`.
//! Requires a mock agent that emits a failure outcome on iteration N, so the
//! loop skips that iteration and continues. Full e2e needs mock_acp_agent
//! scripted to fail on a specific iteration.

#[test]
#[ignore = "M6 loop skip failure: requires mock agent scripted to fail specific iterations (M7)"]
fn loop_body_failure_with_skip_policy_continues_loop() {
    // M7: build a loop with on_iteration_failure = Skip and a mock agent that
    // emits a failure outcome on iteration 2. Verify iteration 3 still runs.
}
