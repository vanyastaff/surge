# Results planning history

Status: local Results stage COMPLETE after repair 2, independent affected 5a and all three 5b lenses. Full product journey and native Windows remain unverified. Repair dispatches: **2/3**.

Base: `a2ba09c5d1abe2d5370cca9710d19e88c604dc07`.
Worktree: `/Users/vanyastafford/.codex/worktrees/results-planning-history/surge`.

Approved scope: Results readability from planning through approved implementation startup and recorded implementation. Four typed states: planning only, awaiting implementation startup, recorded implementation, implementation unavailable. Read persisted complete operation records even on cold lookup; distinguish truly absent storage from present-empty, corrupt, or attempted-start evidence. Preserve operation status separately from actual implementation RunEnd. Maintain project/run/generation fencing, post-read revision consistency, both-run refresh watermarks, and bounded stale reload.

Pre-code plan: `/tmp/surge-results-planning-plan.md`. Root authorization and independent harsh (`split_plan_critic`) plus domain/async (`split_plan_async`) verdicts: **ACCEPTABLE**. Domain notes: absence is observational and must not become recovery authority or proof that no transient attempt happened. No-child terminal status and child-present terminal missing history remain distinct.

Owners: UI composes Results; persistence supplies additive read-only existing RunInspection. Reuse existing record validation, journal projection, metadata checks, and WeakEntity/generation ownership. No scheduler, receipts, core/schema, provider, harness, Home, runtime-fixture, or main checkout edits. No push/PR/merge; commit only after independent 5a/5b review.

Original panel integration REDs before behavioral production edits (the actual screen acceptance gap is recorded in Review repair 1):

1. Native async Overview panel reads real planning events while the reserved child DB is absent.
2. Real store-committed queued/preparing child without DB displays awaiting startup.
3. Same panel transitions to real implementation events while the planning watermark is unchanged.

Additional coverage: cold lookup, cancellations, formerly started/terminal missing, contradictory summary/directory, empty/corrupt/unsupported/identity errors, ordinary missing history, stale requests/revisions, and no storage creation/migration.

Gates: focused native Results/mission and persistence inspection/bootstrap tests; affected regressions; strict affected-crate default/all-feature all-target Clippy; owned-source rustfmt; diff check. Shared target directory and debug0 environment apply. Independent 5a then 5b required before stage completion.

Evidence receipts and remaining limitations will be recorded below as checks run.

## Review repair 1

Finding **RPH-S1 — blocking acceptance gap**: the three native async cases construct
`MissionPanel` directly and manually drive reload/refresh/sync. They do not exercise
actual `RunsScreen` selection, Overview construction, and automatic child-watermark
refresh. This is not a demonstrated production defect; the approved outer seam is
missing. Stage 5a reviewer `task_view_plan_critic` returned NEEDS WORK.

Reviewed base is `a2ba09c5d1abe2d5370cca9710d19e88c604dc07`; the five-file source
closure before repair has SHA256
`4247de133462a9a789ff08497e8bd643a1800b94c96f3428911f93bc703060b4`
(sorted relative path/content-hash pairs). Source and destination are the same
`codex/results-planning-history` worktree above. Main/PR150 remain unchanged.

Repair scope: add a real native `RunsScreen` selection/Overview journey with an
isolated-home injection at the existing request/loader seam. Exercise planning-only,
queued/preparing, then recorded child by updating the actual AppState child
watermark, without reselection or manually calling panel sync. Preserve production
selection/refresh ownership, all negative classifications, and existing generation
fences. Show failing-test-first evidence for any discovered behavior change; do not
manufacture a production bug if the stronger acceptance test is already green.
Re-run affected acceptance/regressions/lints, then independent affected 5a review
before 5b. No other source fixes, provider calls, or changes to prior Home/harness
repair budgets are authorized by this dispatch.

Additional baseline receipt: `/tmp/surge-results-base-cargotest.log`, unchanged main
`a2ba09c`, same debug0 environment and `cargo test --locked -p surge-persistence --lib`:
**544/544 passed**. This one pass does not establish that the earlier lock failure
was pre-existing or caused by this change; the concern remains unconfirmed.

## Implemented closure

- `crates/surge-persistence/src/runs/inspection.rs`: additive static `inspect_existing_run`, sharing `inspect_run_files` with the existing instance inspection. No new store/schema/dependency.
- `crates/surge-ui/src/screens/runs.rs`: selected run/project request identity; cached status is a refresh hint, not the history ownership authority.
- `crates/surge-ui/src/screens/run_mission.rs`: separate operation/history presentation, both reserved stream watermarks plus operation revision, existing WeakEntity/generation fence, one bounded stale retry.
- `crates/surge-ui/src/screens/run_mission/history.rs`: complete persisted operation lookup, exact project/run/child/graph binding, four bootstrap history states plus ordinary history, post-read record/revision recheck. Terminal registry/operation claims require matching actual journal evidence.
- `crates/surge-ui/src/screens/run_mission/results_history_tests.rs`: native async outward tests, real on-disk positive/negative fixtures and deterministic revision/async response barriers.

Queued/preparing absence requires no database, registry row or attempted-start directory. Cancelling retains its distinct wording. Child attention with PartialStartup/StorageUnconfirmed remains unavailable; no-child blocked planning retains readable planning plus its operation attention. Child-present terminal cancellation with absent history remains unavailable. No-child cancellation/failure never creates an implementation RunEnd. The factual planning-only label is “No implementation launch is recorded”; absent child intent does not prove that a human has not approved flow.

## Executed evidence

All Cargo commands ran sequentially in this worktree with `CARGO_TARGET_DIR=/Users/vanyastafford/Develop/surge/target CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0`.

| Receipt | Actual command/result |
|---|---|
| `/tmp/surge-results-history-red.log` | `cargo test -p surge-ui results_history_ -- --nocapture`: **0/3**, all three behavioral assertions reached and failed on absent child DB / stale unchanged planning count. Only explicit-home injection had been mechanically extracted before RED. Initial test macro/context compiler setup errors were corrected before this actual RED receipt. |
| `/tmp/surge-results-history-final.log` | Same focused command: **22/22**, including the three native panel integration cases, banner text, cold child lookup, blocked/failed planning, cancellation distinctions, matching terminal success, contradiction/corruption/identity refusal, deterministic revision barrier and delayed generation response, bounded stale reload, and request scope identity. |
| `/tmp/surge-results-ui-final.log` | `cargo test -p surge-ui -- --nocapture`: **294/294** unit/native + **5/5** integration, no ignored tests. This ran after the final production changes; the last subsequent edit only extended the existing terminal test table with a positive matching case, verified by the final focused 22/22 receipt. |
| `/tmp/surge-results-inspection-green.log` | `cargo test -p surge-persistence --lib inspection -- --nocapture`: **25/25**, including static absence, empty/attempt/corrupt evidence, symlink refusal and existing WAL/read-only/projection regression tests. |
| `/tmp/surge-results-persistence-nextest.log` | `cargo nextest run --locked -p surge-persistence`: **586/586 passed**, one existing ignored `cross_process_writer_lock_blocks_second_process` stub. Default Nextest profile. |
| `/tmp/surge-results-clippy-all.log` | `cargo clippy -p surge-ui -p surge-persistence --all-targets --all-features -- -D warnings`: **exit 0** on frozen source. |
| `/tmp/surge-results-clippy-default.log` | `cargo clippy -p surge-ui -p surge-persistence --all-targets -- -D warnings`: **exit 0** on frozen source. |
| `/tmp/surge-results-fmt.log` | Explicit five Rust files, `rustfmt --check --edition 2024 --config skip_children=true`: **exit 0**. `git diff --check`: **exit 0**. |

The actual native disk tests use GPUI's supported `BackgroundExecutor::allow_parking` mode for mixed real SQLite/Tokio I/O. Behavioral assertions remain active. Ordering-sensitive tests use oneshot barriers/controlled delivery of real decoded snapshots rather than elapsed-time guesses.

## Open evidence and limits

- `/tmp/surge-results-persistence-regressions.log`: plain parallel `cargo test -p surge-persistence --lib` returned **543/544**, failing unchanged `work_items::start_preparation::secure_lock::tests::arc_retains_real_exclusion_until_last_effect_owner_exits` at its final reacquisition assertion. `/tmp/surge-results-lock-isolated.log`: the exact test subsequently passed **1/1**. Keep both receipts open; the successful Nextest run does not erase the CargoTest concern. No baseline attribution or secure_lock repair is claimed.
- Checks ran on macOS. New Unix permission/symlink cases are correctly gated; no native Windows run or Windows CI success is claimed. Existing PR150 Windows issues and repair budgets remain outside this task.
- Fixture writers close explicitly, reads/controlled tasks settle, and store/pool clones drop before `FixtureHome::close`. Windows retained home cleanup uses the existing bounded sharing-handle cleanup. No raw locked lease/SHM byte snapshots were added.
- Operation and journal reads remain separate observations. Revision recheck is not an atomic filesystem/cross-database transaction and does not authorize launch or recovery.
- Main checkout remained unchanged (`git status --short` only showed the pre-existing `.l3home/`). No provider calls, commits, push, PR or merge occurred. No memory writes.
- Independent 5a then 5b and parent-owned final platform/full-goal acceptance remain required. This record does not self-certify overall completion.


## Repair 1 build evidence

Repair dispatch count remains **1/3**. Finding RPH-S1 now has executable full-screen coverage; independent re-review must determine resolution.

The new `results_history_actual_screen_overview_tracks_planning_to_implementation` test uses the actual `RunsScreen`, clicks the planning rail row and Overview tab, and lets normal screen rendering construct and refresh `MissionPanel`. It starts without cached bootstrap status, then observes real store-committed queued/preparing child states and real persisted child envelopes applied through `AppState` / `RunStreamState::apply_recorded`.

After implementation is already recorded, further `StageEntered` and `StageCompleted` journal appends update only the child stream. Planning watermark, both cached and persisted operation revisions, selected run/request and original panel entity stay unchanged. Native step labels must change from “Spec: working now” to “Spec: finished · drafted”. No manual `MissionPanel::sync`, `refresh_key`, `reload`, or panel struct literal is used by this journey.

The stronger test was already green after the mechanical explicit-home injection/observation seam; no new production classification bug was manufactured or fixed. Production home resolution moved from the panel constructor to its existing screen caller with the same resolver. Test-only home override is set before the first screen render. Existing row/tab/step observations expose native control labels for the acceptance oracle.

A test-only RAII pending-load observer reserves its count before enqueue/first poll and drops it after the actual request ends. The counter is shared by the fixture and screen across superseded and replaced panels, so a new panel's zero cannot hide an old weak panel's outstanding read. Fixtures require the shared count to be zero before closing actual stores/retained homes. Production owner/scheduling topology is unchanged.

| Receipt | Actual result |
|---|---|
| `/tmp/surge-results-repair1-screen.log` | Actual screen journey **1/1 GREEN**, including the fixed-revision child-progress assertions. Compiler/setup errors were corrected before this behavioral receipt. |
| `/tmp/surge-results-repair1-screen-mutation.log` | Authorized bounded mutation omitted only the actual screen `panel.sync` call. New screen journey reached behavioral **0/1**, exit 101, failing to observe queued revision 3 / AwaitingStartup. Original source restored byte-for-byte afterward; `runs.rs` SHA256 before and after restoration: `54a4583c38ef7088056166ae73785cfb3a78186c4f9b39533de92cf583449193`. No mutation remains. |
| `/tmp/surge-results-repair1-ui.log` | Restored final source: full UI **295/295** unit/native plus **5/5** integration, zero ignored; includes all 23 Results history cases and existing Runs regressions. |
| `/tmp/surge-results-repair1-clippy-all.log` | Strict affected crates, all targets / all features / `-D warnings`: **exit 0**. |
| `/tmp/surge-results-repair1-clippy-default.log` | Strict affected crates, all targets / default features / `-D warnings`: **exit 0**. |
| `/tmp/surge-results-repair1-fmt.log` | Explicit five Rust files, rustfmt check: **exit 0**; git diff check: **exit 0**. |

Repair changed only `screens/runs.rs`, `screens/run_mission.rs`, `screens/run_mission/results_history_tests.rs` and this task record. The full stage dependency closure still includes the unchanged-in-repair `run_mission/history.rs` and persistence `runs/inspection.rs`. Initial RED evidence is preserved. Earlier CargoTest 543/544, isolated 1/1, Nextest 586 with one existing ignored stub, and root's unchanged-main CargoTest 544/544 remain distinct receipts; no cause/baseline-flakiness attribution is claimed. No Windows execution claim, provider call, main/.l3home change, commit, push or memory write. Independent 5a then 5b remain pending.

## Review repair 2

Independent Stage 5b generic, domain and security reviews returned NEEDS WORK.

- **RPH-Q1 — blocking** (security alias RPH-AUTH-1): `bound_events` rejects any second PipelineMaterialized although bootstrap legitimately records generated and regenerated child Flows. Existing guard protected initial graph/capture identity. Preserve that protection by binding the FIRST materialization to capture and checking every materialization's own content/hash consistency; later generated artifacts do not replace parent identity. Preserve WorkItem binding refusal and terminal checks. Add persisted actual-screen parent/generated/revised graph coverage through planning, queued/preparing, recorded implementation; wrong first graph/hash and corrupt later hash remain refused.
- **RPH-Q2 — blocking**: raw request project path versus canonical capture falsely rejects an alias of the same project. Existing guard protected foreign project refusal. Reuse canonical project identity while preserving original selection/project request and async generation fencing. Do not replace exact repository identity with broad starts_with ownership. Add actual-screen/loader same-canonical-alias positive and foreign-project negative.

Repair dispatch count recorded BEFORE implementation: **2/3**. Previous reviewed five-file closure SHA256 `6f50f8873f38fe8206876321f935586b0fcb4fafcbddaf5fc3cdec4201fb9200`; constituent hashes `/tmp/surge-results-repair1-frozen-sha.txt`. Same source/destination worktree/base. Scope limited to these two blockers and required tests; no Stage A graph authority, scheduler, verification, Home, harness or main/PR150 repairs. Builder must capture actual RED for both before production changes, retain original acceptance/mutation receipts, rerun affected full UI and strict lints/fmt, then independent affected 5a BEFORE 5b. Cause of prior CargoTest lock failure remains unconfirmed. No memory or provider writes.

### Repair 2 implementation and evidence

Both actual native RunsScreen cases failed before production edits. `/tmp/surge-results-repair2-red.log` records 0/2; `/tmp/surge-results-repair2-red-authentic.log` repeats 0/2 using the actual bundled bootstrap parent graph, followed by generated and revised implementation graphs. The failures are the intended history/project identity refusals, not compilation or fixture setup failures.

The private loader now authenticates the first graph against the immutable capture and checks every graph's own content hash. Later materializations remain journal observations and do not replace parent or approved child pins. Project comparison uses exact roots from the existing ProjectScope resolver on a blocking task, preserving the original MissionRequest and generation/revision fences. It does not use descendant ownership. No public API or other production owner changes in this repair.

Native journeys cover planning, committed queued/preparing child absence, recorded implementation, and further child-only progress with unchanged planning watermark and operation revision. The alias journey also refuses a real nested foreign root. Persisted fixtures cover immutable parent/child graph pins, wrong first graph/hash/content, and corrupt later materializations in both journals. All writers close explicitly; shared pending-read guards reach zero before storage/home cleanup; canonical-resolution blocking tasks are awaited. No new detached task or timer.

| Receipt | Observed result |
| --- | --- |
| `/tmp/surge-results-repair2-focused.log` | 27/27 Results tests passed. |
| `/tmp/surge-results-repair2-ui.log` | 298/299 passed; unchanged Fleet lost-ack test panicked with Tokio context shutting down. Cause unconfirmed. |
| `/tmp/surge-results-repair2-fleet-isolated.log` | Exact Fleet test 1/1 passed; does not establish cause. |
| `/tmp/surge-results-repair2-ui-refresh.log` | Fresh full UI 299/299 plus 5/5 integration passed. |
| `/tmp/surge-results-repair2-clippy-all.log` | Initial lint rejected two needless test Box replacements; changed inner values without new allocations. |
| `/tmp/surge-results-repair2-clippy-all-final.log` | Strict affected crates, all targets, all features, `-D warnings`: exit 0. |
| `/tmp/surge-results-repair2-clippy-default.log` | Strict affected crates, all targets, default features, `-D warnings`: exit 0. |
| `/tmp/surge-results-repair2-focused-final.log` | Final test-source revision after allocation cleanup: 27/27 Results tests passed. |
| `/tmp/surge-results-repair2-fmt.log` | Explicit five Rust files, rustfmt check: exit 0; git diff check: exit 0. |
| `/tmp/surge-results-repair2-persistence-cargotest.log` | Requested final plain default `cargo test --locked -p surge-persistence --lib`: 544/544 passed. This does not explain the earlier lock failure. |

Earlier plain persistence 543/544, isolated lock 1/1, Nextest 586 plus one existing ignored stub, and unchanged-main 544/544 receipts remain separate. No causal claim about either intermittent failure. Repair 2 changes only this task record, private history loader and its fixtures. Native Windows has not been executed. Independent affected 5a then 5b remain required; no commit or completion declaration.

## Independent final review and local completion boundary

After repair 2 freeze, `task_view_plan_critic` returned affected Stage 5a ACCEPTABLE: RPH-S1 retained, RPH-Q1 and RPH-Q2 resolved, no affected spec regression. Only then Stage 5b ran. `split_plan_critic` (generic Rust), `split_plan_async` (domain/API/async) and `receipts_plan_security` (identity/read-only storage) each returned COMPLETE, with no introduced blocking findings. RPH-AUTH-1 is the retained security alias of RPH-Q1. No review was signed off by the implementer. Shared repair count remains **2/3**.

Root independently rehashed all five Rust files and the frozen task record against `/tmp/surge-results-repair2-frozen-sha.txt`: all six matched the reviewed closure `012969328911a52d64be71971728940a8828deba8059f139a832599105489f3a`. This final completion-note change affects only this record. Rust source is unchanged since executed checks and independent review.

Final current macOS gate: focused 27/27; full UI 299/299 + integration 5/5; plain persistence CargoTest 544/544, exit 0; strict default/all-feature all-target affected Clippy, owned-source rustfmt and diff checks passed. Initial RED, repair-specific REDs, bounded mutation, and earlier unexpected failures retain their separate receipts and unconfirmed causes. No suppression, Fleet/secure_lock edit, provider replay or native Windows success is claimed.

Local commit closes only this Results dependency closure. PR150 stays at a2ba09c with Windows CI red and its fourth repair mandate unanswered. This stage does not fix the separately diagnosed gate graph-authority/recovery-observability defects, complete the actual new-app/existing-project journeys, authorize publication of this independent branch, or achieve the full active goal. Next authorized work is the separately reviewed Stage A graph-authority and historical-projection closure, followed by Stage B truthful recovery presentation. No memory update.
