# Exact active-registration retirement

Status: bounded implementation passed the unchanged two-test regression after an actual baseline failure. The owning reviewer authorized implementation after two independent precode acceptances (architecture and domain). Review counter 1/3 is retained. Full strict and native acceptance remain open.

The actual completion wrapper currently releases its writer and event-forwarder owners before awaiting the active map write. A replacement registration for the same run can appear in that interval. Removing by run ID alone then removes the replacement.

The deterministic baseline adds a private, per-Engine, `cfg(test)` one-shot gate after `RunExecution::execute` returns and before the existing map removal. The replacement case acquires a real replacement writer, calls the production `register_active_run` primitive, releases the old completion, and checks that the public `stop_run` reaches the replacement cancellation token. This deliberately tests the production registration primitive; it does not claim full replacement-dispatch admission for an already-terminal run. A separate positive control joins ordinary completion and checks that public stop returns `RunNotFound`.

Both cases release barrier channels, join the actual completion, close any replacement writer, drop owners, shut down the bridge and runtime, and clean the fixture home before their final behavioral assertions. No cleanup retry or timeout increase is part of the fix.

After an observed runtime baseline failure and owning authorization, the bounded repair captures a `Weak` identity of the existing unique `params.gate_resolutions` allocation before moving execution ownership. Under one active-map write guard, it compares the current entry using `Weak::ptr_eq`, without upgrading or retaining pending gate values, and removes only the matching registration. No new public identity or run-task field is needed.

This scope does not settle hard-aborted completion registration cleanup, status/ledger update fencing, durable historical settlement, or native Windows acceptance. Those remain separate obligations.

## Executed evidence

The exact command was `CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0 cargo test -p surge-orchestrator --lib settled_completion_ -- --nocapture`, using rustc 1.98.1. Baseline session 65150 exited 101: the ordinary retirement control passed and replacement preservation failed (45.01 seconds compilation, 0.14 seconds tests). After the authorized identity fix, session 59648 exited 0 with both tests passing (27.24 seconds compilation, 0.23 seconds tests). The test source hash is identical in both receipts. The adjacent `registration-retirement-{red,green}-source-freeze.json` files record exact source and raw-log hashes; corresponding gzip archives preserve the raw bytes.
