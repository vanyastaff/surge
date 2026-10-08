# R05 Production build and packaging

Scope: native CLI/daemon release; no publishing, no user data changes, no source edits.

- Host: aarch64-apple-darwin; rustc 1.98.1.
- Baseline command: `cargo build --locked --release -p surge-cli -p surge-daemon`.
- Build log: `/tmp/surge-release-build.log`; still running at initial report.
- `python3.12 -m unittest discover -s scripts -p 'test_release.py' -v`: PASS, 5 tests in 12.556 seconds. Includes four archive formats/targets using fixtures, archive membership/content and independent checksums, missing/empty/duplicate/traversal/symlink/corrupt/truncated rejection, tag/version checks.
- Test log: `/tmp/surge-release-packaging-tests-py312.log`.
- Initial `python3` invocation failed because host default is Python 3.9.6 without tomllib. Script documents Python 3.11+ and release CI pins 3.12; this is environment mismatch rather than packaging implementation failure. Initial failure log retained at `/tmp/surge-release-packaging-tests.log`.
- `python3.12 scripts/release.py metadata --ref refs/tags/v0.1.0`: PASS (`publish=true`, `prerelease=false`); no publish action taken.

Current release assets contain only surge, surge-daemon, README.md, LICENSE-MIT, LICENSE-APACHE. Desktop UI not in release workflow. Exact supported target set is Windows x86_64 MSVC, macOS x86_64 and ARM64, Linux x86_64 GNU. Only native ARM64 build can be verified here; other native builds remain CI evidence requirements.

Potential documentation improvement: explicitly mention Python 3.11+ / python3.12 for local packaging checks in development/release instructions.

Baseline build completed exit 0 (`Finished release profile`) on 2026-10-05. Concurrent authorized source/lock updates occurred during build; this result is baseline diagnostic only and MUST NOT count as final integrated release evidence. Final native packaging/smoke intentionally deferred until rebuilt integrated state. No source edits by R05; cargo compile slot released to root's regression RED queue.

## CLI owner regression follow-up

The initial integrated nextest run exposed obsolete pre-owned-Flow fixtures and a real foreground exit-status bug. Updated cli_run_lifecycle/examples_smoke/fault_injection to committed clean Git sources, relative project-scoped flow paths, isolated homes, real retained daemon owner and cleanup. The gate test now requires a persisted HumanInputRequested event, pending nonterminal gate, explicit cancellation and durable aborted result. The checkpoint injection now kills/asserts the actual daemon owner (99), while the CLI receipt succeeds and replay confirms committed nonterminal stage history. Scope security normalization remains unchanged.

Root executed RED evidence: `/tmp/surge-release-final-cli-owner-red.log` had 3 passes, 3 failures: two genuine foreground/watched failure exits were incorrectly 0 despite persisted RunFailed; one gate fixture referenced the wrong replay projection field (fixed to view.active_node). Root watcher-unit RED `/tmp/surge-release-final-cli-watch-red.log` had 4 passes, 2 failures confirming Failed/Aborted live outcomes incorrectly returned Ok.

Production engine.rs now reuses require_completed for live outcomes and validates exact one durable terminal record on disk fallback. Missing/conflicting/duplicate records fail closed; failure/aborted records preserve reasons. Pending root GREEN verification; no cargo execution by R05 during this follow-up.

Independent R26 review found the default disk `engine watch` branch still returned success after printing any history. Root captured actual old-binary RED (RunFailed, exit 0) in `/tmp/surge-release-final-cli-disk-watch-red-confirmed.log`. That branch now uses the same authoritative durable completion gate. CLI lifecycle integration asserts disk-watch exit codes for completed/failed runs, missing terminal while awaiting human input, and aborted after cancellation. Conflicting/duplicate terminal evidence is covered by the fixed event fixture unit test. GREEN verification remains root-owned and pending.

## Gate oracle correction and aborted aggregation

The first GREEN attempt passed watcher units (7), lifecycle success/failure cases (5), examples (10), and fault (1), but gate fixture failed on a stale replay. Actual binary reproduction captured pending projection in `/tmp/surge-release-r05-gate-projection.json`: both active_node and view.active_node are gate. Fixture race read replay before a later log observed HumanInputRequested; fixed to observe durable event first then fetch fresh projection.

Actual owned StopRun contract routes through `owned_flows::stop` to WorkItemCommand::Suspend. Settled evidence `/tmp/surge-release-r05-gate-aborted-events.log` has RunSuspended, not RunAborted; despite historical filename it is suspension evidence. Gate oracle now asserts RunSuspended, nonterminal replay and disk watch refusal. Prior report's expected aborted gate contract is superseded by this verified owner contract; aborted watcher behavior remains covered by direct synthetic terminal/history assertions.

Separate genuine projection defect: aggregate_status ignored RunAborted even though display classified Done(Aborted). Root RED `/tmp/surge-release-final-abort-projection-red2.log` executed 1 test and failed terminal=true assertion. Initial abort RED filename captured 0 tests and is NOT proof. Minimal production fix handles RunAborted alongside RunCompleted; failed=false and historical active-node retention remain consistent. Final GREEN root-owned pending.

## Ignored real-daemon restart regression

Root's ignored test `/tmp/surge-release-integrated-ignored-restart.log` failed only obsolete idle oracle `elapsed >= 11s`. Source contract `lifecycle::drain_until` and actual `idle_shutdown` require prompt idle shutdown; grace is a ceiling, not mandatory wait. Replaced daemon_restart fixture with two real-process scenarios: idle configured30s grace restart <10s, and test-owned SIGSTOP/11s-delayed SIGCONT forcing actual CLI wait beyond its former10s limit. Both retain actual old Child, require successful original exit, changed PID and healthy ping; delayed signal thread is cancellable and joined before original child reaping or cleanup. No production shim or timing-only workaround. Root GREEN pending.

Root GREEN restart output `/tmp/surge-release-integrated-restart-green.log`: 2 tests passed, 0 failed, 0 ignored, 13.67 seconds. Actual idle prompt restart and actual delayed-owner >10-second wait both verified. No further restart source changes needed.

Independent R26 follow-up rejected inherited successor pidfile-driven SIGKILL: a text PID is insufficient process identity even in an isolated home. Removed that operation entirely. Restart fixture retains its successor Unix IPC connection and sends correlated Shutdown, requires ShutdownOk, bounds read/write and socket removal, and asserts normal cleanup succeeds. Only original retained Child receives kill/wait; delayed SIGCONT owner joins before reaping. New cleanup version awaits root rerun and R26 final review; prior2/2 GREEN applies before this cleanup refinement.
