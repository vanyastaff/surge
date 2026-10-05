# R25 Reliability implementation

## Result

Both assigned source-proven P1 defects implemented and verified with deterministic local regressions. No commits, pushes, publishing, live tracker/Telegram requests, or user-data changes. Production and test source frozen for coordinator's final workspace gate and R32 independent final review. Role verdict: implementation verified; final release acceptance remains pending those gates (not a GO certification).

## L3 dropped completion / uncertain external merge

Changed:
- `crates/surge-daemon/src/automation_merge_gate.rs`
- `crates/surge-daemon/tests/daemon_merge_gate_e2e.rs`
- `crates/surge-persistence/src/intake_emit_log.rs`

Pre-code plan independently ACCEPTABLE by R23; coordinator accepted subsequent boundedness and dedicated uncertainty marker refinements.

- Reconcile durable journal completions at immediate startup tick, every 30 seconds, and broadcast lag.
- Include all ticket states, including Completed, using existing validated `intake_completion::durable_outcome`; only Completed runs enter the gate.
- At most 64 keyset candidates per pass; retained exact task_id cursor avoids fixed-LIMIT starvation. Terminal Merged/Blocked/legacy Proposed/Uncertain receipts excluded before journal reads. Errors retain candidates for later sweep.
- Bound each tracker completion handler to 30 seconds. Other handlers cannot be permanently wedged behind a provider call.
- Fail closed on emit-log inspection errors.
- Insert unique `MergeAttempted` durable reservation before publishing merge RPC; only inserted winner proceeds. Failed insert and duplicate reservation never issue merge.
- Interrupted attempt without terminal receipt becomes durable `MergeUncertain`; manual-inspection warning/comment/blocked label are best effort. No blind retry of external merge. Classification precedes current label/provider fetch, so removing surge:auto after publication cannot erase the uncertain prior effect.
- MergeUncertain prevents repeated notifications/RPC across restart. Successful known merges retain Merged receipt and existing optional publication policy.
- Added additive emit-kind strings on unconstrained existing TEXT field; no schema migration needed.

Actual checks:
1. RED full e2e: 11 PASS / 3 FAIL, exit 101, `/tmp/surge-release-r25-red.log`. Failures demonstrate absent startup recovery, fail-open missing receipt DB, and missing manual uncertainty classification.
2. Initial GREEN: 15 PASS / 1 FAIL (lag fixture tried opening Storage on unsupported single-thread runtime), `/tmp/surge-release-r25-green.log`. This was a test setup failure; retained honestly.
3. Repaired full e2e `cargo test -p surge-daemon --test daemon_merge_gate_e2e`: 16 PASS / 0 FAIL in 1.00s, `/tmp/surge-release-r25-green-retry.log`.
   Includes startup terminal-ticket journal without broadcast; restart success dedup; deterministic actual lag (dedicated receiver runtime, burst cannot interleave); removed-auto-label uncertainty and restarted warning suppression; missing emit DB; INSERT trigger rejecting merge reservation; normal pinned-head merge/already-merged/blocked/conflict paths.
4. `cargo test -p surge-daemon --lib automation_merge_gate`: 10 PASS, `/tmp/surge-release-r25-unit-green.log`. Independent fixed expected keys verify 65 terminal tickets split into exact 64+1 keyset pages and success receipt filtering.
5. Emit-kind parse roundtrip: 1 PASS, `/tmp/surge-release-r25-emit-green.log` (R26 executed).
6. Scoped rustfmt passed.

## ACP option phase hang

Changed:
- `crates/surge-acp/src/bridge/worker.rs` option phase only
- `crates/surge-acp/src/bin/mock_acp_agent/sdk_v1.rs` controlled test stall
- `crates/surge-acp/tests/bridge_control_lifecycle.rs`

Coordinator independently read worker/call sites and returned ACCEPTABLE pre-code plan.

Use existing `handshake_step` for session/set_config_option with the SAME handshake deadline, shutdown token, caller reply closure and effect fence. Applied-error cleanup redacts HandshakeFailed text identically to previous handshake cleanup. Initialize-only retry policy unchanged; published option requests never retried.

Mock records actual requested option before a finite 4-second stall, permitting RED regressions to clean up child processes before asserting failure.

Actual checks:
1. RED `cargo test -p surge-acp --test bridge_control_lifecycle option -- --nocapture`: 0 PASS / 2 FAIL; dropped caller shutdown took 4.006s and option phase lacked shared deadline classification. `/tmp/surge-release-r25-acp-red.log`.
2. GREEN same command: 2 PASS in 0.53s; deadline phase set_config_option, child reap, subsequent healthy open, exactly one published option, caller drop + prompt shutdown. `/tmp/surge-release-r25-acp-green.log`.
3. `cargo test -p surge-acp --test bridge_control_lifecycle --test bridge_config_options`: lifecycle 15 PASS, configuration 3 PASS. `/tmp/surge-release-r25-acp-regression.log`.
4. Scoped rustfmt passed.

## Remaining boundaries

- Coordinator final clippy/workspace nextest/smoke and independent R32 review pending. No claim these have passed in this role.
- R31 owns tracker automation documentation update and has exact contract plus runtime results.
- Manual uncertainty resolution has no new CLI/UI workflow. Durable `merge_uncertain` receipt and warning logs expose classification; notifications/comments/labels are best effort and failure is traced. A failed notification must not be called completed delivery.
- No live GitHub/Linear provider integration tested. Tests use controlled mock sources/subprocess.
- The initial three daemon regressions and two ACP regressions were executed failing before production fixes. Additional lag/page/insert-failure checks strengthened the already-reproduced fix afterward; they are supplementary, not falsely claimed RED evidence.

MEMORY: An irreversible external request needs a unique durable pre-dispatch receipt. Recovery must classify its uncertain outcome before current mutable policy (labels) or provider reads; otherwise revocation/outage can hide an already-published operation. No retry may infer non-execution from a lost response.

## Independent review repair 1 — returned merge error is also uncertain

R32 found P1: `merge_pr` Err previously wrote MergeBlocked without explicit uncertainty, despite possible provider effect with lost response. Coordinator accepted minimal plan; repair budget dispatch 1.

- Test FIRST `published_merge_error_is_durable_uncertainty_and_never_replayed`: unarmed mock counts published effect then returns error. Requires persisted MergeUncertain, unknown-outcome/manual-inspection message, and no second RPC or warning after restart.
- Actual RED exit 101, failed because MergeUncertain was absent: `/tmp/surge-release-r25-repair1-red.log`.
- Shared `classify_uncertain_merge` now handles every merge_pr Err and interrupted attempts. Persist Uncertain before best-effort delivery. Message is generic and does not publish arbitrary provider error text. Known Conflict remains definitive blocked; known successful merge paths unchanged.
- Removed inaccurate unconditional 'will retry' logs from blocked comment/label failures; recovery eligibility depends on existing durable receipts. Incomplete effects do not record successful delivery.
- Full GREEN `cargo test -p surge-daemon --test daemon_merge_gate_e2e`: **17 PASS / 0 FAIL in 1.01s**, `/tmp/surge-release-r25-repair1-green.log`. Includes all existing normal/conflict/known-success paths.
- Scoped rustfmt passed. Cargo slot released and production/tests frozen. Coordinator strict workspace gate and R32 targeted re-review pending; do not treat this report as release certification.

## Repair 2 — strict clippy documentation formatting

Coordinator strict workspace clippy identified doc-list continuation/backtick lints only in merge gate additions. Added blank module-doc paragraph separator and backticks around Completed/MergeAttempted/MergeUncertain identifier mentions. Scoped rustfmt passed; no behavior change or suppression. No Cargo invocation; coordinator reruns strict gate. Source frozen again.

## Nextest route fixture failure — stale socket readiness

Observed coordinator full-suite failure `continued_accepted_route_survives_rejected_control_ack_without_second_route` at shared request helper line26: LocalSocket connect OsNotFound. Retained project `.tmp6Y0RaW`, home `.tmpGb0PtU`; read-only event journal ends RunSuspended, no continuation sent.

Source ownership analysis: fixture kills and waits original child. Some(crash_ack) branch removes old cold.sock before restarted host; None branch did not. `cold_host_with_config` readiness accepts path existence, so old owned socket lets helper return before new server startup. Server's normal `SocketDirectory::prepare` removes stale owned socket, creating race with request connect. No production defect established by this failure.

Coordinator accepted narrow fixture plan. Added 6 lines only in None branch removing known dead original child's stale cold.sock before cold_wire_host, matching existing Some branch. Scoped rustfmt passed, no production changes, no connect retries or weakened assertions. No Cargo invoked in this role; coordinator owns actual regression rerun and full nextest. Source frozen again.

## Owned Flow IPC fixture ordering repair

Coordinator latest full nextest failed locator replay test at owned_flow_ipc.rs151 with SQLite no such table events. Source-proven ordering: owned Flow receipt acknowledges immutable acceptance and schedules launch asynchronously (owned_flows.rs108–119). create_run opens events.sqlite before migration commit (storage.rs228–237); strict nonmutating inspect_folded_run propagates schema failures instead of inventing an empty history. Fixture attempted inspection before launch readiness.

Approved fixture-only change in owned_flow_ipc.rs: wait at most 10s for the accepted registry attempt's Completed projection. Daemon publishes it only after validating startup and strict terminal journal/binding (work_items.rs789–804). Unexpected state fails immediately with state+diagnostic; deadline includes state+diagnostic+journal directory. After barrier perform one strict folded inspection and require Terminal; all existing frozen-input/replay/conflict/history assertions remain. No SQL errors swallowed, schema retries or production mutation. Scoped rustfmt passed; source frozen. Actual target rerun pending serialized Cargo authorization:
`cargo test --locked -p surge-daemon --test owned_flow_ipc locator_replay_survives_missing_source_and_changed_configuration -- --exact`

### IPC diagnostic compile correction

Coordinator targeted compile rejected private `Storage::run_dir` in deadline diagnostic (E0624). Removed that diagnostic argument only; run/state/diagnostic remain. Scoped rustfmt passed, no Cargo invoked, fixture frozen. Prior reference to journal directory diagnostic is superseded; no passing target evidence claimed here.

### Read-only scheduled pool resource failure assessment

Latest nextest also failed approved_skill_completion_does_not_leave_a_human_gate_recovery_record before its assertions: dependency scheduled-thread-pool Worker::start calls thread.spawn(...).unwrap(), OS35 WouldBlock. r2d2 default starts three scheduled worker threads per Pool; Surge open_run_reader creates a new pool each call in addition to registry pool. ScheduledThreadPool Drop signals shutdown but does not join workers, so pool churn can transiently overlap threads. This is a pressure amplifier, not proof of a leak or a resolved transient. Single failure and neighboring successful tests support host pressure; no resource snapshot at failure exists in this role. Recommend isolated serialized target and full bounded-concurrency repeat; if recurring measure process/thread counts and retained reader/child lifetimes before production changes. No pool source modified.

## Recurrent maintenance thread exhaustion closure

Two full nextest runs at four test threads failed OS35 creating scheduled-thread-pool workers. Source tracing established new 3-worker r2d2 executors per registry/reader DB pool and default CompletePendingScheduled retaining workers until dead fixed-rate reaper tick (30sec). Final fix creates an independent three-worker executor per DB pool with DiscardPendingScheduled on final executor-owner drop. Connection capacities, pragmas, idle timeout/max lifetime and live reader ownership stay unchanged. All scheduled r2d2 jobs use Weak SharedPool; upgraded in-flight jobs and live owners retain the executor, so only future callbacks without surviving owners are discarded. R26 independently accepted lifetime proof.

A static shared executor experiment is explicitly superseded: discarded r2d2 fixed-rate reaper handles would retain perpetual dead jobs in a static scheduler. No shared executor/cache/vendor patch is present.

Meaningful final oracle observes actual r2d2 on_acquire worker ThreadIds and their thread-local destructor exits, independently of factory constants. It scopes actual canonical SQLite connection paths to the fixture's canonical home, excluding parallel unrelated tests. It retains64 distinct readers, checks live reads and live worker retention, then churns64 more readers and requires all observed worker identities to exit within3seconds after ownership release. Original behavior RED0/390 exited (/tmp/surge-release-r25-pool-exit-red.log); final behavior GREEN390/390 exits and524 actual connection establishments in0.24sec (/tmp/surge-release-r25-pool-exit-scoped-green-retry.log). First scoped attempt failed with0observedconnections because macOS /var alias differed; canonicalization corrected only the observer. Retained log /tmp/surge-release-r25-pool-exit-scoped-green.log documents that fixture repair honestly.

Final command with CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 OPENSSL_STATIC=1 MACOSX_DEPLOYMENT_TARGET=15.0:
- cargo test --locked --offline -p surge-persistence reader_pool_churn_releases_actual_maintenance_workers_promptly -- --nocapture: PASS1.
- cargo clippy --locked --offline -p surge-persistence --all-targets --all-features -- -D warnings: PASS (/tmp/surge-release-r25-pool-scoped-clippy.log).

Dependency closure: root Cargo.toml scheduled-thread-pool0.2.7 workspace dep; persistence manifest reference; Cargo.lock adds exactly existing scheduled-thread-pool dependency to persistence with no version changes; private runs/pool.rs plus mod declaration and both existing builder sites. No commits by R25. Source frozen and Cargo released to root for full workspace four-thread regression/native gates. This is timely ownership cleanup, not a process-global thread cap; full gate recurrence verification remains root-owned until executed.
