# Bootstrap telemetry writer settlement

Native Windows source 77fff58 failed `restart_after_child_commit_uses_committed_artifact_not_newer_parent_version` with WriterAlreadyHeld. Telemetry previously returned after append/flush and relied on asynchronous Drop to release its writer. It now awaits close on both successful and failed writes, retaining both typed errors when writing and settlement fail. Public BootstrapError adds WriterClose and TelemetryWriteAndClose; external exhaustive matches require updating. Cancellation settlement and concurrent deduplication are not claimed.

Independent specification and API/security reviews are COMPLETE. The two new valid single-worker multi-thread Tokio regressions initially failed with WriterAlreadyHeld: immediate reacquisition after successful telemetry and after a real SQLite trigger rejects the write. After the repair, the bootstrap driver unit zone passes 6/6; freshly reread events preserve exactly one telemetry record. No new sleep or retry masks release. An initial current-thread fixture was invalid because Storage requires a multi-thread runtime; its setup failure is not bug RED evidence.

Executed gates:

- `env CARGO_INCREMENTAL=0 cargo test --locked -p surge-orchestrator --lib bootstrap_driver::tests:: -- --nocapture` — 6/6 PASS.
- `env CARGO_INCREMENTAL=0 cargo +1.96.0 check --locked -p surge-orchestrator --all-targets` — PASS. An earlier attempt failed solely due to ENOSPC; both receipts remain.
- `cargo clippy --locked -p surge-orchestrator --lib --tests -- -D warnings` — PASS after boxing combined errors, before the final display-only wording correction.
- `rustfmt --edition 2024 --check crates/surge-orchestrator/src/bootstrap_driver.rs` and `git diff --check` — PASS.

Final tests and MSRV bind the manifest source SHA. Native Windows supervisor GREEN remains pending the next CI revision. Raw command output is retained compressed without invented headers.
