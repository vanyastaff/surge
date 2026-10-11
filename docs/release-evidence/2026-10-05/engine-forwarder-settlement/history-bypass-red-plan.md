# Actual failed settlement followed by historical success: RED plan

Status: root approved the direction; auxiliary test-hook review and explicit source-window release are required before Rust edits. No schema changes, builds, or native privileged execution are authorized by this document. This is an expected-failure regression plan, not evidence of execution or a durable-settlement fix.

Related accepted durable design: `./durable-settlement-plan.md` (accepted SHA256 `9b509500aa927a3368c0ebfe2d931c71fca035ea6c08f34ce5a365d0e789ac54`, recorded in `durable-settlement-plan-review.json`).

## Causal regression

A real Engine terminal-only run must produce its own valid terminal journal and successfully seal its actual writer. Its actual event forwarder must then fail to acquire a connection from its exhausted real reader pool. The public joined completion must return `SettlementFailed { original: Completed, failures: [ForwarderRead] }`. Reopening that journal must not convert this unconfirmed completion into authorization for intake completion, automatic merge, or CLI success.

No test may create the causal failure by constructing a `SettlementFailed` wrapper, replacing a SQL result, appending a hand-made terminal event, corrupting the database, or manufacturing a settlement receipt. Existing synthetic wrapper tests remain separate coverage.

## Source evidence and deterministic ordering

`runs/reader.rs::RunEventReadLease::read_batch` currently obtains a real pooled connection before either existing read gate. Neither existing gate can force a checkout failure. `engine/run_execution.rs::execute` cancels execution if the forwarder fails first, so exhausting the pool before the terminal run seals would test a different condition.

The pinned local `r2d2-0.8.10/src/lib.rs::get_timeout` attempts real checkout before checking the deadline. With every slot retained, `get_timeout(Duration::ZERO)` returns its actual pool error without relying on a short scheduling window. With an available connection, the same call succeeds. This replaces no error value and requires no user-facing timeout configuration.

The fixture sequence is:

1. Prepare a retained `FixtureHome`, a separate worktree, a real Storage, and an Engine with the terminal-only example graph. Generate the run ID, then install one typed next-feed probe bound to that exact run ID on that Engine before starting the run.
2. Await the selected forwarder's first real blocking worker at a new gate **before checkout**. No connection or SQL statement has yet been acquired by that worker.
3. Await the actual writer lifecycle `Sealed(prefix)` through its watch receiver; require a nonzero prefix. Reject `Failed` or channel loss while still `Running`. This permits the actor to use its ordinary reader until it has sealed; no slots are exhausted before sealing.
4. On the same pool, prove available `get_timeout(ZERO)` succeeds and drop that checked-out connection. Acquire and retain exactly `pool.max_size()` actual pooled connections in an opaque guard with a bounded acquisition deadline. Require zero idle connections and the expected held count. Prove a further actual zero-timeout checkout fails.
5. Release the pre-checkout gate while retaining all slots. Await the actual Engine completion. Require original `Completed`, the sole failure `ForwarderRead`, a public `StreamError`, and no public `Terminal`. These observations follow the actual writer/forwarder joins.
6. Drop the held-slot guard. Prove zero-timeout checkout succeeds again. Drop the probe, stream, Engine, bridge and Storage owners in their proper order. Actual bridge shutdown and fixture cleanup remain checked; retained test pool owners cannot be counted as production leaks.
7. Open a fresh Storage on the same home. Read the genuine events and require a valid terminal fold, exactly one `RunCompleted`, and a journal prefix consistent with the observed seal. Do not reopen an exclusive writer or run repair/recovery before testing historical consumers.
8. Execute the selected historical consumer once and collect its effects. Close all readers, pools, tasks and Storage before fallible fixture cleanup, then assert the safe oracle. The current implementation is expected to fail that final assertion; producer/precondition failures are not accepted as the desired RED.

No sleeps, retry-to-green, deadline increases, ignored errors, global environment mutations, or external tracker/merge calls are introduced. Bounded deadlines detect a failed fixture rather than establish successful absence of an effect.

## One Engine hook, two typed modes

The Engine owner must reuse one per-Engine, one-shot, opt-in `test-support` handoff before `RunExecution::new` spawns its feed forwarder. It is separate from the existing private completion-poll gate. The same typed feed-preparation helper consumes the selected hook for all three actual forwarder routes: fresh-run `RunExecution::new`, active-resume `RunExecution::new`, and already-terminal resume through `run_execution::settle_terminal`. The last route currently spawns its own feed directly and must receive the same prepared feed or invoke that same helper before spawning. No route may silently bypass an armed next-feed hook and leave it for a later run. The historical regression consumes the hook on a fresh run; terminal-resume hook consumption receives a focused routing control. The helper creates the feed from that exact writer, then synchronously consumes the test request. `RunExecution::new` and `settle_terminal` receive the resulting configured feed. The request is bound to an explicit `RunId`; encountering another feed consumes the request and fails explicitly instead of carrying it forward. This is one hook and one consumption helper, not a separate terminal-resume injector.

The typed request has two modes:

- `BeforeCheckout`: installs the new persistence pool probe and returns `(FeedReadGate, FeedPoolProbe)`. The gate owns only barrier channels; the probe owns a complete actual read lease and cloned lifecycle receiver. Only this mode selects the fixture-only zero-timeout checkout.
- `AfterCheckout`: installs the existing persistence `install_read_gate_for_test` and returns its existing `FeedReadGate`. This is for the separately owned public-completion-abort-during-SQL regression; it does not change checkout timeouts.

Engine owns the typed request `FeedProbeKind::{BeforeCheckout, AfterCheckout}` and result `FeedProbe::{BeforeCheckout { gate: FeedReadGate, pool: FeedPoolProbe }, AfterCheckout(FeedReadGate)}`. Use these typed controllers, not an arbitrary callback or error injector. A oneshot handoff transfers the selected controller to the test. Duplicate installation, poisoned state, installation failure, or a lost controller receiver must fail explicitly; lost receivers/drop must release any installed barrier. If preparation fails after acquiring an actual writer, the owner drops the configured feed, then uses `close_after_error` to await actual writer close and preserve any close failure before registration or returning the preparation error; it must not drop the writer and claim settlement. This applies equally to the already-terminal resume route. No second independent Engine feed hook is needed.

Agreed persistence controller surface; owning agents finalize ordinary error signatures before dependent edits:

- `RunEventFeed::install_pool_probe_for_test() -> Result<(FeedReadGate, FeedPoolProbe), StorageError>`.
- `FeedReadGate::entered()` and `FeedPoolProbe::wait_sealed()` observe the actual worker and lifecycle.
- `FeedPoolProbe::check_available_zero_timeout()` obtains a real connection, executes `SELECT 1`, then drops it.
- `FeedPoolProbe::exhaust_all_slots()` returns opaque `FeedPoolSlots` owning actual `PooledConnection` values followed by an independent complete read-lease owner in drop order; the guard exposes `count()` and `capacity()`, never raw connections or SQL mutation access. Bound acquisition to fixture pools with capacity at most 16; larger pools fail setup rather than loop without a fixture bound. `FeedPoolProbe::verify_exhausted_zero_timeout()` requires actual zero idle slots and an actual zero-timeout checkout failure.
- `FeedReadGate::release()` opens the barrier; gate `Drop` also opens it for unwind cleanup.

Blocking checkout/control work must run on a blocking worker, not an async worker. The separate gate holds only channels. The probe owns an actual `RunEventReadLease` and the cloned lifecycle receiver. Its slot guard independently retains that complete owner, with connections dropped before the owner; no raw writer/SQL authority is exposed. Both probe and slot guard intentionally retain the original writer exclusion and must be dropped before any exclusion-release or cleanup claim. The `AfterCheckout` mode returns only the existing gate, with no probe/read lease, so the separately owned public-abort exclusion test cannot be satisfied by a test controller retaining the writer lease. Test-support fields are absent in ordinary builds; with the probe absent, the existing production `.get()` path and timing remain unchanged.

## Consumer tests and honest boundaries

Use a shared test fixture implementation where practical so each consumer runs the same real Engine producer. Do not make consumers depend on a pre-generated mutable database or an outcome-only fake.

- **Intake:** tests beside `intake_completion.rs` reopen Storage and call real `durable_outcome`; demand that it does not return success. A companion invokes `reconcile_pending` once with a seeded correlated active ticket and checks no terminal transition or terminal-comment outbox authorization. The engine journal is real; ticket setup may use existing registry test fixtures.
- **Merge:** a unit test beside `automation_merge_gate.rs` invokes actual `reconcile_page` once, awaiting its return. Reuse the existing `MockTaskSource` L3 label, ready-head and merge-outcome arrangement from `daemon_merge_gate_e2e.rs::startup_recovers_completed_terminal_ticket_without_broadcast`. Demand no `merge_pr` call and no `MergeAttempted` authorization. This exercises the actual historical decision and local mock provider dispatch; it makes no claim of a real remote merge.
- **CLI:** a child test in `commands/engine.rs::watch_tests` receives `SURGE_HOME` through `Command::env`, creates the actual failed-settlement run, drops its producing owners, then calls actual `require_completed_history`. Demand an error. `require_completed_events` alone is insufficient because it omits reopening. The parent must settle the child and preserve its failure output. The child oracle must also preserve checked fixture cleanup.

Direct awaited reconciliation passes avoid interpreting a quiet timer interval as proof that no side effect occurred. The existing merge/intake integration tests that fabricate completed journals are useful setup references only; their synthetic terminal events are not reused as the causal producer.

## File ownership and dependency order

1. Independent architecture reviewer reviews this auxiliary seam and consumer scope. Root explicitly releases the source window only after the currently running caller builds exit.
2. Persistence owner owns `crates/surge-persistence/src/runs/reader.rs`, test-support exports and focused pool-control tests. No overlapping persistence edits are permitted.
3. Caller owner owns `crates/surge-orchestrator/src/engine/engine.rs`, `run_execution.rs`, typed hook module/types and orchestrator Cargo feature. The hook is shared with that owner's public-abort test.
4. CLI/daemon consumer owner owns historical tests, shared consumer fixture support if needed, and daemon/CLI dev-dependency feature enablement. No daemon `work_items.rs` edits are included.
5. Root schedules each actual build/test. No agent runs Cargo concurrently or interprets source review as runtime evidence. Durable schema implementation remains gated on the intended actual RED.

The opt-in orchestrator `test-support` feature forwards `surge-persistence/test-support`; daemon/CLI enable it only for development/test dependencies. No default public production API expansion or new dependency is intended. Final exact signatures and paths are frozen jointly by the two owning source agents before editing dependents.

## Proposed focused checks

Final names may be adjusted once by owners before the source freeze; retain one exact filter per receipt:

```text
cargo test -p surge-persistence --features test-support feed_pool_probe_controls -- --exact --nocapture
cargo test -p surge-orchestrator --features test-support actual_completed_run_reports_exhausted_forwarder_pool -- --exact --nocapture
cargo test -p surge-daemon history_after_actual_settlement_failure -- --nocapture
cargo test -p surge-cli history_after_actual_settlement_failure -- --nocapture
```

Use the compiled fully qualified test names for `--exact`; the short placeholders above are not evidence that a test exists or ran. Record revision/source hashes, command, process exit, producer controls and failing assertion. The first two are producer/control checks expected to pass; the consumer checks must be RED specifically because current historical recovery grants success despite the actual failed settlement. After the durable implementation, the same safe assertions must pass without weakening the producer or history validity checks.
