# Reconciliation evidence progress

- Baseline fixture compile/runtime setup failure: `stale-page-baseline-red.log`.
  Storage requires a multi-thread runtime; this is not defect RED.
- Actual baseline RED: `stale-page-baseline-red-2.log`. One test failed its
  preservation assertion. Attempt changed Suspended -> Attention and carried
  `task conflict: storage error: suspended task requires its durable Continue reservation`.
- First GREEN build attempt: `stale-page-green.log`. Compiler caught an incorrect
  helper receiver (`&Storage` instead of the supported `&Arc<Storage>`); no behavior result.
- Corrected GREEN: `stale-page-green-2.log`, 9 passed, 0 failed: four reconciliation
  behavior cases plus five shared mock-bridge fixture unit tests. Rust 1.98.1,
  `CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0 cargo test -p surge-daemon --lib work_items::reconciliation_tests -- --nocapture`.
- The test source was subsequently extended with a policy-aware fixture helper for
  full PlannedCapacity evidence. That extension is not covered by the above run.

The deterministic manual RED demonstrates a real stale-page defect; the exact
historical macOS Capacity/budget failure remains unconfirmed. Full PlannedCapacity,
original budget/wake tests, transaction cases, strict CI-toolchain checks and
independent final review are still pending in this progress record.


## Subsequent executed gates and positive extension

The README records the later PlannedCapacity seam, six transaction cases, both
original budget caps, unbudgeted exhaustion, and scheduler wake passes. Those
results apply to the earlier error-CAS slice; `source-freeze.json` retains its
historical source identity rather than claiming coverage of later edits.

The positive projection extension produced actual assertion RED in three tests:
Reserved -> Launched despite newer SuspendRequested, Launched -> Completed and
newer SuspendRequested -> Terminal, and confirmed Suspended -> Launched. The
bounded receipt is `positive-projection-red-receipt.json`; the full raw snapshot
output remains ignored locally. Full snapshot comparisons are retained in tests,
with bounded failure messages for future runs. Positive CAS and accepted-dispatch
supervision source tests are being completed; their GREEN is not yet claimed.

A subsequent macOS CI job at revision 5f42653 (job 113131022822) failed
`normal_task_mixed_skip_and_actual_429_never_reopens_primary`: 3759 passed, 1 failed,
37 skipped. Its assertion again observed Suspended -> Attention (generation 1,
snapshot 28, Interrupted/Capacity wake), without the attempt diagnostic. Raw source:
`target/ci-repair-evidence/macos-5f42653-complete.log`. The exact mixed429 integration
is added to the post-fix gates. This is another observed failure, not conclusive
attribution to the deterministic stale-page reproduction.


## Post-extension focused GREEN

Actual Rust 1.98.1 gate `reconciliation-positive-green.log`: 15/15 passed in 0.90s,
including both actual accepted-dispatch ownership tests. Exact current macOS failing
mixed429 integration passed 1/1 in 0.86s (`quota-mixed429-positive-green.log`).
Both commands exited zero. All nine source hashes match the positive freeze.
Cargo token was returned to root before any subsequent gate. Strict checks and
independent final review remain open; no claim of historical cause proof is added.
