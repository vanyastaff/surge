# Results planning history

Current status: local Results COMPLETE after authorized repair4, focused independent5a and all three affected5b lenses. Dispatches **4/4**, no automatic fifth. Full goal and native Windows remain open. Earlier statuses below are historical receipts.

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


## Prospective repair 3 pre-code proposal — native planning and run navigation

**Proposal only; no repair dispatch or implementation. Repair count remains 2/3.**
Prepared from current source at `2923e96c` and read-only observation of the permanent
habit-tracker acceptance journal. Local stage completion above remains a historical
claim about its reviewed closure; native N3/N6 demonstrate additional open Results
requirements. Maintainer recommendation: **ACCEPTABLE pending independent critic
and security review**. A builder must record 3/3 before implementation, after those
reviews and the root's decision. No fourth repair is implied. The witness author
currently owns Cargo under its separate 4/4 mandate; this proposal neither takes
that lease nor modifies its budget.

### Proven evidence and scope

N3: planning journal `run-01M538BSKHNKR64K4C8TC3X3Z6` records description author
StageEntered 5 / SessionOpened 9, Description approval 31, roadmap planner
StageEntered 37 / SessionOpened 41. At watermark 43, native Overview said Roadmap
not started and Nothing has run yet. There is no BootstrapStageStarted in that
prefix. The orchestrator has no emitter reference to that event. `mission::fold`
compiles the planning TaskExecutionView but passes only its recovery into
`from_execution`; actual planning stages disappear. `fold_plan` requires legacy
BootstrapStageStarted for Drafting, and the rendered empty-state predicate ignores
planning activity entirely. Current screen fixtures synthesize that legacy event,
so their previous passes do not establish the real graph-driven planning journey.

N6: `RunsScreen::rows` deliberately suppresses a planning row when its child is
listed, except while that planning row is selected. Selecting the child therefore
changes two rail entries to one. This is not journal deletion: child Overview
loads both persisted journals. It is a navigation defect because Log/Sessions
remain selected-run-specific, while the rendered rail no longer offers the parent.

Within Results, `render_header` additionally overlays a Pending operation phase on
a selected run's own status. A completed parent can consequently appear building
while its child runs. The selected run lifecycle and the enclosing operation phase
must be separate identified facts. The independently proven N7 false FAILED Inbox
parent originates in app.rs and is **not fixed by this Results repair**. Home's 5/5
budget is exhausted; no Home/app.rs/Inbox owner change or new repair budget is
inferred from this proposal.

Approved prospective zone: `mission.rs`, `screens/runs.rs`,
`screens/run_mission.rs`, its Results tests and existing adjacent mission tests;
only the necessary mechanical shared-projection adapter in
`project_lifecycle/loading.rs` may join the dependency closure. No scheduler,
engine emission, persistence authority/schema, provider, witness, receipts,
sandbox, original acceptance runtime/project or main checkout edits. No push/PR/
merge. Full product goal, actual two-milestone app, follow-up, existing-project
journey, platform acceptance and Windows CI remain open.

### Options and selected projection

1. **Selected: generic actual recorded planning steps plus typed review cards.**
   Reuse TaskExecutionView lifecycle/session/summary/failure/coverage evidence.
   Humanized recorded node identifiers and actual participant metadata provide
   useful planning activity without inventing Bootstrap role authority. Existing
   typed approval events still drive Description/Roadmap/Flow review cards.
2. Resolve author roles structurally from typed HumanGateMode::Bootstrap and exact
   graph paths. This could show Roadmap drafting but requires unique-predecessor,
   branching/cycle and ambiguity semantics. It is unnecessary for truthful actual
   work visibility; do not implement it in this repair.
3. Emit BootstrapStageStarted in the engine. Outside Results ownership and cannot
   repair existing journals; reject for this scope.

Add an internal planning execution presentation owned by the Mission composition,
separate from implementation steps and terminal state. It carries the actual
planning RunId, ordered stage occurrences, planning recovery/coverage/through
watermark, caveats and terminal fact. Actual stage occurrence identity stays its
StageEntered sequence; attempts are display counters. Retain planning failure and
unfinished/gap distinctions and actual sessions. No profile-name/node-substring
role heuristics, inferred approval, inferred completion, cleanup or verification.
Use node identity as the neutral planning label rather than the existing heuristic
implementation step_label role mapping. Generic unclassifiable graph work remains
visible with explicit attribution limitations.

Present review-card absence as **No review recorded** rather than **not started**.
The internal PlanState::NotStarted may remain for compatibility; its new text must
refer to review evidence only. Existing legacy BootstrapStageStarted continues to
produce the existing drafting state and has an explicit control test. A stage's
StageCompleted cannot itself approve its artifact; only typed approval events do.
Edit/re-entry preserves review history and the new actual stage occurrence.

Resolve planning node configuration against the planning source graph at that
occurrence. The authenticated first parent materialization establishes its graph;
a generated/revised child Flow artifact materialization is not a parent graph
revision. Only genuine recorded GraphRevisionAccepted transitions may replace the
parent graph for later parent stages. Journal gaps discard unwarranted graph
attribution. Reuse existing immutable graph binding and projection helpers where
they preserve this distinction, rather than importing role inference or mutating
history. Keep wrong-first/hash/corrupt-later materialization refusal tests.

Avoid a half-migrated from_execution contract: the preferred interface removes the
recovery-only optional input and compiles the planning view once inside the shared
composition adapter, returning Result<Mission, TaskExecutionError>. `fold` continues
to compile the implementation once then delegates; the sole other production
caller, `project_lifecycle/loading.rs` around line 321, reuses its already compiled
implementation and maps the shared adapter error through its existing load result.
There must be no duplicate divergent planning fold and no duplicate implementation
compile. This caller change is a mechanical dependency-closure migration, not a new
Home behavior repair, owner, status overwrite or budget reset. A builder may choose
an equally coherent compile-once internal signature after review, but must migrate
all callers and retain fallible attribution rather than discard errors.

### Native display and refresh boundaries

Render a separate Recorded planning steps group in Overview. Planning's run ID,
coverage, recovery, terminal state and stream observation govern its labels. A
child's live stream cannot make a completed/offline parent agent working now.
Generalize the existing observation predicate by exact run identity and that run's
loaded prefix; retain Ready/error checks, complete coverage, registry-vs-stream
watermark checks and loaded refresh binding. Planning and implementation current
activity/terminal presentation remain separate, even while both are visible.
Render available actual session/profile/runtime and summary through existing
presentation seams; absent metadata remains absent/unknown.

Replace the false Nothing has run yet state. Zero implementation with actual
planning activity must state that implementation has no recorded work yet while
showing the real planning activity/reviews. Truly empty ordinary history and
planned roadmap tasks retain truthful distinct empty/planned wording. No apparent
running state may survive a terminal fact or a journal gap.

Retain both planning and implementation rail rows independent of selection and
label the count **Runs**. Add neutral typed Planning / Implementation labels when
an existing exact BootstrapOperationStatus binds the row; do not infer relation
from names, prompts or path shape. Unknown/cold relation remains a normal exact run
row until existing owner lookup establishes it. Preserve project filtering,
selection identity, ordering/dismissed-failure behavior and action targets.

Results header shows the selected run's own lifecycle/status, with enclosing
operation phase separately labeled as operation status. A terminal Completed
parent plus active implementation child must show completed planning separately
from the implementation/operation activity. Do not synthesize a corrected run
terminal state from operation status or patch app.rs's cache. If upstream cached
status is already wrong, retain explicit journal-vs-cache limitations; the N7 Inbox
finding remains independently open.

### RED-first and complete affected verification map

Before production edits, extend the actual native RunsScreen journey using real
persisted planning envelopes and actual rendered rail/Overview controls. No manual
MissionPanel construction or direct selected assignment may substitute for the
outer acceptance path. At least the following behavioral REDs must reach their
assertions on current production source:

- Graph-driven planning prefix through description approval then roadmap
  StageEntered/SessionOpened, with no BootstrapStageStarted: actual recorded
  planning work visible; no false Nothing has run yet/no claim Roadmap never began.
- Parent and child both listed, select child then parent via rendered controls:
  both rows and stable Runs count remain, parent's exact Log/Sessions identity and
  recorded session data are reachable without an out-of-band selection setter.
- Selected terminal Completed parent plus active child/operation: selected-run
  status is Completed and operation activity is separately identified.

Expand accepted tests without claiming every pre-existing behavior as a new RED:
zero-implementation planning; genuinely empty ordinary history; legacy
BootstrapStageStarted drafting control; typed approval request/approve/edit/reject;
re-entry occurrence/attempts; planning stage failure and terminal unfinished work;
journal gaps and missing attribution; cold persisted owner lookup; generated and
regenerated child Flow materializations; real GraphRevisionAccepted where valid;
child start/completion/failure with parent history retained; parent terminal state
while child runs; independent offline/stale/live watermarks and recovery for each
run; child live observation never upgrades parent activity; parent progress while
child watermark stays fixed and vice versa. Retain original delayed generation,
revision race, bounded stale reload, project alias/foreign refusal and read-only
storage-negative tests.

Tests that assert labels must use fixed expected recorded node/session/run identity
and actual native control labels, not a second implementation of the fold. Keep
pending-read ownership guards and actual fixture writer/pool settlement before
cleanup. Do not read raw live SQLite/WAL bytes or create external production state
for a test. This work introduces no polling timer, detached worker, new dependency,
unsafe code, external public API or authority grant. Graph traversal/view compile
runs once per existing bounded reload, not per render; presentation does not create
a second execution engine.

After RED/GREEN run full affected UI unit/native/integration tests, affected shared
projection tests, strict default/all-feature all-target Clippy, owned-source fmt
and diff checks. Exercise unchanged persistence inspection regressions when the
closure touches their seam; retain unexpected failures and honest denominators.
Use the existing Cargo lease/target strategy only after the root transfers it.
Independent affected Stage 5a must precede full Stage 5b generic/domain/security
reviews; freeze/re-hash the complete closure before local commit. A successful
macOS test gate is not native Windows evidence, CI publication, working-app
acceptance or full-goal completion.

### Independent pre-code verdicts for prospective repair 3

Root maintainer and independent results_repair3_plan_critic plus results_repair3_plan_security: ACCEPTABLE. No author dispatch yet: repair count remains2/3. Implementation waits for the active witness4/4 author's terminal freeze and Cargo lease release. Binding conditions:

- RPC-01 / RPH-R3-HIST-1: preserve every planning StageEntered occurrence/sequence; do not use adjacent-node push_step replacement or role-inference step_label. Typed review events alone approve. A gap leaves historical review with visible coverage limits, never confirmed current state.
- RPC-02 / RPH-R3-ADAPTER-1: compile each planning/implementation history once across history::bootstrap fallback and recorded-child paths, not just inside fold. Fallible shared adapter propagates foreign-run/sequence errors through the sole project_lifecycle caller; no Home cache/status ownership change.
- RPC-03 / RPH-R3-AUTH-1: authenticated first parent graph remains the anchor even after gaps. Generated/regenerated child PipelineMaterialized never replaces or restores parent attribution. GraphRevisionAccepted must validate content/hash and predecessor against parent lineage; otherwise neutral attribution. Retain existing wrong-first/corrupt-later refusals and add same-node/different-config child collision coverage.
- RPH-R3-OBS-1: parent and child retain independent run IDs, coverage/recovery/terminal/watermarks. Exact parent observation and loaded prefix alone can produce planning working-now; live child/op phase cannot. Cover live-child/offline-terminal-parent, live-parent/stale-child, gaps and terminal unfinished stages.
- RPC-04 / RPH-R3-NAV-1: actual rendered child→parent rail clicks, then rendered Log/Sessions tabs, prove exact run/session identities, stable Runs count and status/operation facts. No manual panel/direct tab/selection assignment acceptance shortcut. Retain project filtering, actions and delayed-response fencing.
- RPC-05 / RPH-R3-BOUNDARY-1: selected run status and operation phase separately attributed. N7 app.rs/Inbox remains open; no Home5/5 reset or Windows claim. No execution, scheduler or verification authority changes.

The full goal remains open; this is a reviewed, queued correction of visible native defects rather than a substitute for the actual app journeys.

## Repair 3 dispatch

Repair dispatch count: **3/3**, recorded before author dispatch. Source/destination:
`codex/results-planning-history` at merge commit `5bf1104e`; same worktree above.
Current main `6722b495` has been integrated and full local integration gates passed.
Approved proposal and independent RPC/RPH conditions above remain binding.
Findings N3/N6 plus Results header misattribution are blocking. N7 remains the
separate Home/Inbox sixth-round mandate, not part of this repair. Witness4/4 stays
closed. No fourth Results repair is automatic.

Outer RED must exercise real RunsScreen rendered controls before behavior edits.
Required complete closure includes compile-once/fallible composition, authenticated
parent graph lineage, independent observations and actual child-to-parent Log and
Sessions navigation. Shared projection adapter in project_lifecycle/loading.rs is
mechanical only. No scheduler, receipts, provider, runtime or Home owner edits.
Full affected gates and independent 5a then full 5b precede local commit.

## Repair 3 independent reviews

Frozen source closure c7193d6ac803123f235b7b11ec87d94a221edb2c79ee8828e0a05b46b1375a1f:
5a ACCEPTABLE; generic5b COMPLETE. Domain5b COMPLETE with advisory RPH-R3-DOM-A1;
security5b NEEDS WORK with blocking RPH-R3-SEC-1 (P2), same fallback-style issue.
Root confirmed theme.rs's explicit semantic-role documentation and accepts the
security disposition as a required RPC-03 correction; domain severity disagreement
is retained. No execution authority or terminal/approval fabrication was found.
Missing graph attribution currently keeps Agent styling through Lane::Build.

Overall stage remains NEEDS WORK, repair count3/3. No source repair dispatched.
Concrete narrow prospective fourth-round plan is
results-neutral-attribution-fourth-round-proposal.md. User's Home/Inbox sixth
mandate does not extend this task; independent proposal review then explicit
Results extension are required. All successful gates and failures remain retained.

User subsequently answered "Да, разрешаю четвёртый раунд Results" after the
focused critic/security proposal re-review ACCEPTABLE. Current Results dispatch
count **4/4**, recorded before reusing its author. Narrow scope/RED/gates and
review conditions are in results-neutral-attribution-fourth-round-proposal.md.
No automatic fifth. Home's sixth slot remains authorized/reserved, not dispatched.


## Repair 3 author freeze and executed gates

Author verdict: **BUILD COMPLETE**, awaiting independent affected 5a then full 5b generic/domain/security review. Shared count remains **3/3**; no fourth implementation dispatch is automatic. HEAD `c2001306cce946390b5105644526f99d2b8cf37b`.

The six-source closure SHA256 is `c7193d6ac803123f235b7b11ec87d94a221edb2c79ee8828e0a05b46b1375a1f`. Individual source hashes are in `.rust-studio/evidence/results-repair3/frozen-source-sha.txt`; commands, environment, result denominators and receipt hashes are in `manifest.json` in that directory. No source changed after the final full UI receipt and frozen lint gates.

Implemented: independent actual planning occurrences keyed by StageEntered sequence, every recorded session, neutral node labels, summaries/failures/recovery/terminal/coverage and watermark facts. Typed reviews alone approve; absent review says No review recorded, with historical-current uncertainty after gaps. The first authenticated parent graph remains the anchor, generated/regenerated child artifacts cannot replace or restore it, and genuine revisions require matching predecessor plus content/hash validation. No profile-substring role inference is used for planning. The fallible shared adapter compiles planning once; the history loader chooses the child/fallback before composition, compiles supplied child history once, and the sole lifecycle caller propagates errors mechanically.

Both rail rows remain selectable with a Runs count and exact typed operation membership. Cold loader ownership may inform these read-only labels only for the unchanged selected project request, without writing AppState. Header lifecycle belongs to the selected run and the operation has its own label. Exact full RunId, actual parent Log entries and Sessions opening remain reachable through rendered child-to-parent clicks and rendered tabs. Independent parent/child observation and watermark tests prevent a live child, stale parent or operation phase from upgrading planning activity.

Actual pre-behavior RED: `outer-red-authentic.log` is **0/3**, all intended assertions reached: missing actual description planning occurrence; hidden completed parent after child selection; header waiting to build versus Completed. The first two receipts preserve hidden-parent and incomplete-display-replay setup failures separately. `outer-green-attempt.log` was a visibility compiler failure, not a behavioral RED. `outer-green.log` was **2/3**, exposing shortened header identity. `ui-green.log` was **328/330**: the new recovery fixture lacked RunStarted and correctly projected Unknown; that negative remains and a startup-complete positive was added. Navigation assertions passed but pending reads remained at cleanup; the fixture now settles actual superseded reads before releasing storage/home. Initial all-feature lint rejected one redundant closure; no suppression was added. These receipts remain retained.

Final commands all used `--locked`, the authorized `target/auth-required-diagnostic`, incremental=0, dev/test debug=0, test strip=symbols, jobs=2, login=false:

| Receipt | Final command/result |
| --- | --- |
| `ui-final-closure.log` | `cargo test --locked -p surge-ui -- --nocapture`: **330/330** unit/native plus **5/5** integration, **0 ignored**. Includes **31/31 Results**, **22/22 mission**, **15/15 project_lifecycle**. |
| `persistence-inspection.log` | `cargo test --locked -p surge-persistence --lib inspection -- --nocapture`: **25/25**, **0 ignored**, **535 filtered**. |
| `clippy-all-frozen.log` | `cargo clippy --locked -p surge-ui -p surge-persistence --all-targets --all-features -- -D warnings`: **exit 0**. |
| `clippy-default-frozen.log` | Same strict affected-crate/all-target command with default features: **exit 0**. |
| `fmt.log` / `diff-check.log` | All six owned Rust files rustfmt check, edition2024/skip_children=true; `git diff --check`: **exit 0**. |

The final UI gate is default features; all-feature and default all-target builds are independently linted. No required integration was skipped. UI integration tests only write their isolated decision-file fixtures, so no hardcoded helper publication was needed; the existing target/debug symlink and main checkout were not modified by this author. Existing linker compact-unwind and dependency future-incompatibility warnings are retained. No fresh native Windows, MSRV or full-workspace CI claim. Prior unexpected Fleet/secure_lock receipts remain unchanged with their causes unconfirmed.

All changes are in the approved six Rust files plus this task/evidence record. No app.rs/app_state/Inbox/Home owner, scheduler, authority/schema, provider, acceptance fixture/runtime/capture/bundle, dependency/unsafe/protocol or memory changes. No commit/push/PR/merge. N7 and the queued Home/Inbox mandate are separate; all eight tasks/two milestones and full product/platform goal remain open. Rust/Cargo lease released after terminal gates and source hash verification; reviews must use this frozen closure before any root-owned commit.


## Fourth-round independent closure

Focused5a ACCEPTABLE; affected generic, domain/async/history and security/integrity
5b COMPLETE, no remaining blocker/advisory. RPH-R3-SEC-1, RPH-R3-DOM-A1,
RPH-R4-PLAN-1 and RPH-R4-PLAN-SEC-1 resolved in code. Explicit attribution from the
same graph lookup reaches the final production style owner; live/unavailable
planning stays neutral without changing activity, identity, sessions or caveats.
Authenticated Agent/HumanGate/external and ordinary implementation stay unchanged.

Root independently verified six final sources, seven fourth-round receipt hashes
and the narrow repair patch. All reviewers verified exact patch reconstruction
and unchanged third-round evidence. Final cumulative source closure is
dd4d2056cab2ba676a53be458d48eed4d641fc19c679910f201eccdc799cd4c6.
335 UI plus5 integrations passed, zero ignored; strict UI default/all-feature
all-target Clippy, six-source fmt and diff passed. Persistence inspection25/25 is
unchanged historical third-round evidence, not a claimed rerun. All failures and
initial review severity disagreement remain retained.

Local Results closure only: no Windows/MSRV/current full-workspace CI, live
provider, full app or product completion claim. No publication. Home/Inbox sixth
round and authentic same-project continuation/all eight tasks remain open.

## Fourth-round author freeze (historical handoff)

Author verdict **BUILD COMPLETE**, frozen for independent focused 5a then affected generic/domain/security 5b. Current Results dispatch count **4/4**, following the explicit user fourth-round authorization; no automatic fifth. The domain advisory/security blocker severity disagreement remains historical evidence; the author does not replace independent review.

Cumulative six-source closure SHA256 `dd4d2056cab2ba676a53be458d48eed4d641fc19c679910f201eccdc799cd4c6` against HEAD `c22696f338578d18fb21d491d753e33c08ec9cac`. Fourth-round changes touch only `mission.rs`, `screens/run_mission.rs` and its adjacent Results tests. The other three source files remain byte-identical to reviewed repair3. `.rust-studio/evidence/results-repair4/frozen-source-sha.txt` contains all six hashes. `narrow-repair.patch` is relative to separately reconstructed baseline source snapshots verified byte-for-byte against all six third-round hashes. `baseline-verification.json` additionally verifies the original third manifest and every original receipt remain unchanged. The existing task/proposal records receive this new freeze; the historical third manifest/task hash is not rewritten.

The same authenticated graph/node lookup now records internal Available/Unavailable planning provenance. Planning-only unavailable configuration falls back to neutral Result; shared implementation `stage_step` is unchanged. One actual final style owner supplies step_chip's border, background and optional live cue. Unavailable planning uses External even for live Working/Waiting, preserving factual text, exact occurrence/session/node IDs, caveats, state, indicator presence and outcome icons. Authenticated Agent/HumanGate and authenticated external-node positive controls preserve their existing styles; ordinary implementation uses its unchanged None policy. Unknown is never inferred from Result lane alone. No new theme, lane, classifier, provider or authority.

Genuine RED before behavior changes: `red-style.log` **0/4**, with missing-config Build-vs-Result, offline Working/Waiting actual final border/tint, live Working/Waiting final border/tint/cue, and real RunsScreen complete-prefix/missing-node/no-session/exact-live-parent actual style assertions all reached. The native step retained Working and its recorded label before the failing color assertion. `red-lineage.log` **3/6**, failing gap, invalid predecessor/content and missing-config neutral projection expectations while three other controls passed. Only the two assertions encoding the defective Build fallback were replaced; original history/lineage/session/review assertions remain. Before RED, provenance was wired from the existing lookup and the existing final style computation was mechanically extracted without changing its returned colors. No compiler/setup failure is described as RED.

Final gates, sequential Cargo lease, login=false, CARGO_TARGET_DIR=target/auth-required-diagnostic (absolute worktree target), incremental=0, dev/test debug=0, test strip=symbols, jobs=2:

| Receipt | Command/result |
| --- | --- |
| `ui-green.log` | `cargo test --locked -p surge-ui -- --nocapture`: **335/335 unit/native +5/5 integration**, **0 ignored**. Includes **23/23 mission** and **35/35 Results**. |
| `clippy-all.log` | `cargo clippy --locked -p surge-ui --all-targets --all-features -- -D warnings`: **exit0**. |
| `clippy-default.log` | `cargo clippy --locked -p surge-ui --all-targets -- -D warnings`: **exit0**. |
| `fmt.log` | All six cumulative Rust files, `rustfmt --check --edition 2024 --config skip_children=true`: **exit0**. |
| `diff-check.log` | `git diff --check`: **exit0**. |

The full UI gate is default features; both requested UI lint feature configurations are separately recorded. Persistence has no fourth-round seam change; repair3's **25/25 inspection** is retained as historical evidence and was not rerun. Existing linker compact-unwind and dependency future-incompatibility warnings remain recorded; no suppression. No helper publication needed for UI-only tests; target/debug symlink is unchanged. No fresh native Windows/MSRV/full-workspace CI claim.

No Home/Inbox/app.rs/app_state, authority/schema/scheduler, receipts/provider/witness/native-fixture/runtime/main or memory changes. No commit/push/PR/merge. N7, Home's authorized/reserved sixth round, all eight tasks/two milestones, actual provider/native/product and Windows acceptance remain open. Source is frozen after terminal gates and rehash. Exclusive Rust/Cargo lease released for independent focused 5a THEN affected generic/domain/security 5b; no source edits after freeze without further explicit authorization.
