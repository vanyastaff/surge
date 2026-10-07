# Windows CI prerequisite repair — 2026-10-07

This checkpoint repairs prerequisites and adds a native liveness regression.
Production process liveness remains unchanged to capture Windows RED. Complete
Windows CI repair remains open. [Plan](../../../plans/2026-10-07-002-fix-windows-ci-plan.md).

Baseline source `87ad59a`, [Windows job](https://github.com/vanyastaff/surge/actions/runs/37669506963/job/112957544208):
3635 tests run, 3520 passed, 115 failed, 33 skipped. macOS/Ubuntu tests and all
platform Clippy checks passed on that source. [Native baseline receipt](windows-baseline-87ad59a.log.gz).

Local macOS commands and receipts:

| Command | Result | Receipt |
|---|---|---|
| `cargo test -p surge-core --lib` | 864 passed | [core](core-green.log.gz) |
| `cargo test --locked -p surge-acp --test facade_contract` | 2 passed | [facade](facade.log.gz) |
| `cargo test --locked -p surge-orchestrator --test engine_task_ledger_test` | 18 passed | [shared fixture/ledger](shared-mock.log.gz) |
| `cargo test --locked -p surge-orchestrator --lib engine::hooks::tests` | 15 passed, 422 filtered | [hooks](hooks.log.gz) |
| `cargo build --locked -p surge-acp --bin mock_acp_agent` | passed | [mock build](mock-build.log.gz) |
| Scoped final rustfmt check | passed | [format](fmt-final.log.gz) |
| `CARGO_INCREMENTAL=0 cargo +1.99.0 clippy --locked -p surge-core -p surge-acp -p surge-orchestrator -p surge-daemon --all-targets --all-features -- -D warnings` | passed, 40.66 seconds | [Clippy](clippy-green.log.gz) |

The [initial lint](clippy.log.gz) failed from disk exhaustion, not a successful
lint check. Only generated incremental build cache was removed; sources,
runtime data and release artifacts were retained. The retry passed.

[Manifest](manifest.json) binds exact edited source and raw receipts. No native
Windows GREEN is claimed here; Windows-only regression execution is pending.

Receipts use gzip to preserve original output bytes without Git whitespace normalization.
