# Forwarder settlement baseline test — review and evidence

The reconstructed nonfatal plan `plan.md` SHA256
`94153dc9e459a0d2ae1e62da138589334903430dbc0f187fc2a9c8f591252ab5`
received root owning ACCEPTABLE and independent ACCEPTABLE before source authoring.
The historical independent review counter remains 1/3. The earlier proposed status
in the frozen plan is superseded by this review receipt, not a new review cycle.
Production implementation is not authorized by this receipt. Root authorized only
the actual baseline test and minimal cross-crate test witness.

## Independent ownership oracle

The `test-support` feature is absent from default persistence builds. Orchestrator's
dev-dependency enables it. Storage records only a Weak reference and a one-shot
manager-destruction notification for each actual reader pool. The sole strong marker
moves into the actual SqliteConnectionManager's initializer closure, including the
Windows owning manager. Every r2d2 Pool clone retains that same manager; the test
never receives an owning marker. Pinned r2d2 0.8.10 SharedPool owns its manager,
and r2d2_sqlite 0.25.0 stores the initializer in the manager itself. This proves
manager retention, not physical SQLite connection close order.

The control opens a real reader, obtains its public unpolled stream, drops the
reader and observes the retained manager. Dropping the stream must trigger actual
manager destruction and make Weak upgrade fail. The engine oracle then runs the
literal terminal-only example graph through real Engine::start_run, confirms
RunOutcome::Completed, drains every recorded pool witness for that RunId, drops
returned events, tap, Engine and Storage, and waits at most three seconds for
actual manager destruction. It samples all Weak references before runtime shutdown.
The bounded wait accommodates real r2d2 background jobs without retrying cleanup.

The test executes in an isolated child with a sixty-second parent watchdog. After
saving the ownership observation the child drops its runtime, waiting for actual
blocking SQL and cancelling the baseline infinite async task, then closes its
fixture and asserts the saved observation. A leak cannot become a pass through
runtime cleanup, and failed RED cannot leave a forever task in other test runtimes.

## Native correlation

Current Windows job 113124792605 live evidence is retained at
`target/ci-repair-evidence/windows-dd8-job-live.log`: the capacity test
`estimate_none_and_never_observed_does_not_block_dispatch` reports Windows OS 32
at engine_capacity_park_test.rs:285 at 03:07:59. That failure establishes current
native cleanup failure but does not independently isolate the forwarder owner.
The actual manager oracle below must supply that separate baseline evidence.

## Execution

Actual baseline command on rustc 1.98.1 (48a229cea 2026-09-01):

```text
CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0 cargo test -p surge-orchestrator --test engine_forwarder_ownership_test ownership_tests::completed_run_releases_actual_reader_pool_owners -- --exact --nocapture
```

The first compilation failed because the reused mock fixture's own unit test
requires a crate-root runtime_home_fixture alias. Adding that alias in the tiny
integration harness fixed compilation without changing the shared fixture or
ownership assertions. Separate log: engine-forwarder-baseline-compile-1.log.

The actual run then exited 101 with a behavioral assertion: **1 of 2 actual reader
pool managers remained retained** after confirmed RunOutcome::Completed and all
legitimate owners dropped. The independent public-stream control passed first.
Child result: 0 passed / 1 failed in 3.09 seconds; parent observed its actual exit
101 and failed in 3.13 seconds. The child dropped its runtime before making the
saved assertion; no child process or endless forwarder survives this RED.
See engine-forwarder-baseline-red.log and baseline-source-freeze.json.
Direct rustfmt --check of all four touched Rust files passed. Cargo token released
to root. No production forwarder fix has been made.

## Builder dependency order after actual RED and root authorization

1. Persistence: runs/writer.rs command/actor lifecycle, run_writer.rs serialized
   seal and priority Drop stop, writer_slot.rs lease integration if needed;
   bounded read helper in subscribe.rs/reader.rs and opaque feed in runs module.
   Preserve public infinite subscription semantics and Windows owning connection.
2. Orchestrator: engine/event_tap finite owned forwarder, run_task.rs seal outcome,
   engine.rs delayed handoff and joined completion plus preparation/terminal resume
   cleanup, handle.rs/error.rs typed settlement failure. Re-run the unchanged oracle.
3. Consumer closure: enumerate all RunOutcome matches; daemon tracking/inbox/task
   controls, CLI forced-abort UNKNOWN, facade/bootstrap/intake/UI handling. Coordinate
   with the independently active reconciliation CAS repair. No partial completion
   claim before all consumers and accepted plan's deterministic ownership gates pass.
