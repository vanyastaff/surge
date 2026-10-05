# R26 concurrency fixes

## Config save

Confirmed RED: `cargo test -p surge-core --lib config::io::save_tests -- --nocapture` exit 101, one pass and three meaningful failures: predictable symlink victim overwritten, concurrent writers lose shared temp pathname (ENOENT), mode 0644 instead of 0600. Log `/tmp/surge-release-r26-config-red.log`.

Independent plans approved R23 and R15. Changed `crates/surge-core/src/config/io.rs`: secure same-directory NamedTempFile, retained descriptor write_all, sync_all and atomic persist. RAII cleans own temp on all errors. Root approved workspace tempfile prod dependency in core Cargo.toml, disjoint R06 manifest changes. No parent-directory power-loss durability promise, no malicious-parent protection claim.

GREEN: `cargo test --locked -p surge-core --lib config::io::save_tests -- --nocapture` exit0, 4 passed `/tmp/surge-release-r26-config-green.log`. Tests cover symlink, 8x20 concurrent saves with valid config readers, failed persist preserves destination directory marker and cleans owned temp, Unix0600. Final-current-source GREEN rerun `/tmp/surge-release-r26-config-green-final.log` exit0 4passed after test initializer cleanup.

## Admission wake

Single production consumer confirmed via rg: server.rs spawn_drain_task waits then drains all available queued runs. R23 pre-code plan ACCEPTABLE notify_one on both completion and BootstrapAdmissionGuard rollback, single retained/coalesced permit.

Two regression tests added before implementation expect pre-subscribe wake and exact queued RunId dispatch. Initial attempt compilation-blocked by git2 API upgrade, R06 corrected. Meaningful RED `/tmp/surge-release-r26-admission-red-retry.log` exit101 8pass/2fail both expected wake timeouts. Both producers now notify_one and wait_changed docs make single consumer/coalesced retained permit contract explicit. GREEN `/tmp/surge-release-r26-admission-green.log` exit0 10passed.


## Shared verification queue performed

R12 RED `/tmp/surge-release-r12-red.log` exit101 1pass/2fail historical run missing known_cost_usd and unknown future schema not rejected. Owner implemented; GREEN `/tmp/surge-release-r12-green.log` exit0 3passed and `/tmp/surge-release-r12-registry-green.log` exit0 1passed.
R11 memory::store GREEN `/tmp/surge-release-r11-green.log` exit0 40passed.
R15 Telegram RED blocked same git2 compile errors `/tmp/surge-release-r15-red.log`; retry pending.

No commits. rustfmt and git diff --check current owned files passed. R23 final independent source spec/quality ACCEPTABLE for both fixes; reviewer independently read GREEN logs but did not rerun them. No additional defects found. COMPLETE scoped fixes with meaningful RED→GREEN; full workspace lint/gates owned by coordinator remain pending.

## Remaining shared queue evidence

- git2 surge-git all-targets check PASS `/tmp/surge-release-r06-git-check.log`.
- Telegram outgoing GREEN3 `/tmp/surge-release-r15-green.log`; replies meaningful RED1fail1pass `/tmp/surge-release-r15-reply-red.log`, final GREEN2 `/tmp/surge-release-r15-reply-green-final.log` including storage lookup failure failclosed.
- Daemon startup GREEN2 nextest `/tmp/surge-release-r29-green.log`; security lib GREEN5 `/tmp/surge-release-r29-socket-green.log`. Initial red filename renamed accurately: code already fixed by R29 from separate real RED evidence.
- Notify token diagnostic GREEN1 `/tmp/surge-release-r16-green.log`.
- MCP stderr secure capture GREEN4 `/tmp/surge-release-r15-mcp-green.log`.
- SURGE_HOME isolated defaults GREEN2 `/tmp/surge-release-r11-defaultpath-green.log`.
- Merge gate meaningful RED11pass3fail `/tmp/surge-release-r25-red.log`. First GREEN15pass1fixture-fail SingleThreadedRuntime `/tmp/surge-release-r25-green.log`; owner repairing fixture. Merge libGREEN10 `/tmp/surge-release-r25-unit-green.log`, emit serdeGREEN1 `/tmp/surge-release-r25-emit-green.log`.
- Git lib first76pass1fixture-fail2ignored `/tmp/surge-release-r06-git-tests.log` invalid disk name macOS rejects; owner replaced fixture with raw index entry. Exact GREEN1 `/tmp/surge-release-r06-git-nonutf8-green.log`. R06 now owns cargo slot for suite then releases coordinator.

## Full-gate repair 1

Coordinator strict all-targets Clippy pass4 found excessive_nesting in the concurrent test's inner 20-write loop (`/tmp/surge-release-final-clippy-all-pass4.log`). Extracted plain `repeatedly_save_and_read` helper; barrier remains once per each of 8writers, still20saves each/160total. No lint allowance. rustfmt+git diff --check PASS. No cargo (coordinator owns final rerun). Production unaffected.

## Full-nextest repair: wake scheduler writer fixture ownership

RED actual workspace nextest log `/tmp/surge-release-final-nextest.log`: seven `wake_scheduler::tests` failures at park helper line545 with WriterAlreadyHeld. Source confirmed fixture ownership bug: create_run returns an active RunWriter; callers discarded it, whose Drop requests actor shutdown asynchronously. Actual actor correctly retains WriterLease until exit; park immediately attempts a second writer. close().await joins actor and then releases ownership (run_writer.rs373); exclusive refusal must remain intact.

Coordinator accepted plan. Changed ONLY test region wake_scheduler.rs: all10 create_run sites now explicitly close().await before subsequent fixture/scheduler access; helper comment states required ownership handoff. No sleeps/retries or production ownership relaxation. rustfmt and git diff --check PASS. No Cargo while coordinator full nextest running. GREEN awaiting coordinator serialized `cargo test --locked -p surge-daemon --lib wake_scheduler::tests -- --nocapture` and final full nextest. Source fixture repair implemented, runtime outcome UNVERIFIED until rerun.

## Full-gate wake fixture length repair

Coordinator verified earlier wake tests9/9 GREEN. Integrated Clippy then found Attention regression test102/100 lines due explicit close chain. Extracted shared async create_fixture_run helper containing exact create_run→close.await; replaced all10 testsites with helper call, all assertions retained, no new lint allow. rustfmt+diffcheck PASS, no Cargo; final root rerun pending.
