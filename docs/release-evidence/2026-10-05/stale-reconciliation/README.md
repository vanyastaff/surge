# Stale task reconciliation: bounded repair evidence

A deterministic production-seam test reproduced confirmed Suspended becoming
Attention with the reason `suspended task requires its durable Continue reservation`.
`stale-page-baseline-red-2.log` is actual assertion RED, not a timeout or compile failure.
The baseline was a manual suspension; the historical macOS Capacity failure remains
unconfirmed because its assertion omitted the attempt reason.

The repaired daemon acquires and retains one claim from current eligibility through
launch. Generic prelaunch inspection, startup validation and dispatch refusals use a
single transaction-fenced persistence primitive. It compares the complete observed
attempt and optional control, including absence, state, generation, operation and
fence. Superseded authority causes no write. The generic outer blind settlement is
removed. Explicit Start/Continue behavior retains its existing path.

## Executed focused gates

All runtime commands used Rust 1.98.1, `CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0`.

| Evidence | Actual result |
| --- | --- |
| `stale-page-green-2.log` | 4 behavior regressions + 5 shared mock fixture tests passed |
| `reconciliation-cas-tests.log` | 6 persistence transaction tests passed |
| `planned-capacity-green.log` | PlannedCapacity registry/journal seam test passed (1) |
| `quota-budget-1000-3.log` | Original 1000-token cap test passed (1) |
| `quota-budget-1500.log` | Original positive 1500-token cap test passed (1) |
| `quota-unbudgeted.log` | Original unbudgeted all-exhausted fixture passed (1) |
| `quota-scheduler-wake.log` | Exact scheduler-wakes-same-task test passed (1, 1.33s) |

The PlannedCapacity fixture uses public opening admission and typed exhaustion,
actual planned-stage binding, `suspend_planned_capacity`, writer sealing and
`confirm_suspension`. It compares the full attempt/control/journal, exact capacity
association, cycle and due wake, and verifies no provider call. It is a deterministic
registry/journal seam test, distinct from the actual ACP/engine integration tests.
It has not yet been run against the old reconciliation body in an isolated source
snapshot; the historical failure is not declared conclusively attributed.

The quota integration fixtures require freshly built `mock_acp_agent` and `surge`
(stage MCP helper). Initial attempts after cache cleanup failed before quota behavior:
first missing mock binary, then missing CLI helper. Their builds and final passes are
recorded. The latter failed run's existing assertion printed unrelated local memory
context; its raw output remains ignored at
`target/ci-repair-evidence/quota-budget-1000-2.log`, not copied into this evidence.
The touched assertions now use bounded diagnostic summaries and preserve all original
assertions and deadlines.

`source-freeze.json` records the bounded source files and actual helper hashes.
The strict affected-crate gate is queued with CI's all-target/all-feature shape:
`CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0 cargo +1.99.0 clippy --locked -p surge-daemon -p surge-persistence --all-targets --all-features -- -D warnings`,
followed by the same default-features command without `--all-features`.
This is an affected-crate gate, not a claim of workspace/native Windows acceptance.
Independent review, strict checks and isolated baseline PlannedCapacity evidence
remain pending. Root additionally identified positive settlement paths for scrutiny;
this source freeze proves the bounded error-fence repair, not the entire broader
positive-projection concurrency contract.


## Positive projection extension

The independently accepted extension produced three actual runtime assertion REDs,
recorded without full configuration/journal dumps in
`positive-projection-red-receipt.json`. All four generic positive projection sites
now use the same complete authority comparison under the retained claim. Explicit
Start/Continue retain their prior path. A stale positive projection causes no write;
a current terminal projection atomically updates control and releases assignment.
The accepted run always transfers its completion and claim to the real supervisor,
even when its projection is Superseded or returns a storage error.

Actual post-extension gates on Rust 1.98.1:

| Evidence | Actual result |
| --- | --- |
| `reconciliation-positive-green.log` | 15 passed: 4 error/stale-page, 1 PlannedCapacity, 3 positive projection, 2 actual accepted-dispatch supervision, 5 shared mock fixture tests |
| `quota-mixed429-positive-green.log` | Exact mixed skipped/exhausted provider and actual 429 wake integration passed (1, 0.86s) |

The supervision tests use a real terminal-only Engine run and TrackedRun, a real
manual Suspend or actual SQLite trigger failure, and the production handoff. They
await completion and assert terminal registry/journal, released assignment/admission,
no active engine run and no provider call. The snapshot projection regressions retain
full equality comparisons and bounded diagnostics.

`positive-projection-source-freeze.json` identifies the source that passed these
focused gates. The earlier `source-freeze.json` remains the historical error-CAS slice.
The latest macOS mixed429 failure is documented by `macos-5f42653-receipt.json`;
passing the focused regression does not establish its historical race schedule.
Strict CI-toolchain checks, native acceptance and independent final review remain open.
