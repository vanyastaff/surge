# 12 — Планировщик: парковка до сброса, пробуждение, ротация

**Требования:** R37, R37.1, R38, R38.1, R41
**Blocked by:** 11
**Зона:** `crates/surge-orchestrator/src/engine/engine.rs` · `crates/surge-daemon/src/`
**Волна:** 4
**Status:** in repair — archetype estimator implemented; rotation and end-to-end lifecycle gate remain open

### Checkpoint 2026-10-03

`cargo test -p surge-daemon --test quota_recovery_route_test` passes both
real daemon/ACP cases: typed exhaustion routes A→B in the same workspace, and
exhausted candidates park before the scheduler wakes the same task. The fixtures
require `cargo build -p surge-acp --bin mock_acp_agent` and
`cargo build -p surge-cli --bin surge` for the stage MCP helper. This verifies
reactive task-owned recovery; pre-dispatch capacity rotation remains open.

### Task-owned pre-dispatch slice: verification in progress (2026-10-03)

The current implementation freezes configured route declarations on the real
`WorkItemCommand::Start` path, including `quota_recovery: None`. It records an
actual `StageEntered`-anchored plan, stores separate immutable skip provenance,
and uses the existing one-shot opening permit for the selected provider. A
configured source snapshot is not an observed provider account. Opaque sources
and unspecified models have no reusable skip pin.

Author-controlled ACP evidence: test session `99357` passed all 21 cases in
`quota_recovery_route_test`, including ordinary zero-A/one-B dispatch, all-skipped
no-open suspension and same-attempt wake, mixed skip/actual-429 recovery, explicit
family rejection, exact project scope, changed selected/skipped file sources,
post-429 pin invalidation, exact child environment, and registry/journal secret
sentinels. The test-only host usage oracle verifies retained prior 600 tokens plus
600 after wake: cap 1000 aborts and cap 1500 completes. Those are injected host
usage assertions, not production ACP token accounting.

Cold host park/wake passed in `75708`, `31850`, and `99357`, using kill-and-wait of
the first server process and a new Storage/ACP/Engine/server plus periodic wake.
An earlier second-child failure `3061` had no retained diagnostics and remains
unexplained; future failures retain the isolated fixture. Cold-host readiness
FIFO protection subsequently changed the helper, so its affected outer gates
must be rerun.

The planned binding negative matrix had genuine RED `2010`: an establishment
request appended *after* the plan could still bind. Extending the current prefix
inspection made matrix `71035` green. Later actual phase-crash test `82241` found
a separate production failure: cold reconciliation manufactured a
new same-node `StageEntered`, new logical plan and duplicate provider opening
after an already opened, prompt-authorized session whose wire prompt had not run.
The author added read-only current-occurrence reconciliation before a new entry.
Session `14409` then passed both actual phase-crash tests (unknown opening RPC
and opened/prompt-authorized before the physical prompt) plus the selected-source
guard. Session `73711` passed the authenticated same-node storage classification
law. The original two crash tests and the missing-binding negative still require
final affected reruns. New actual cold Engine same-node reentry passed in `87173`
and, with strengthened identity/snapshot assertions, `8111`: the first owned host
exits 99 after an authenticated self-route commit; a fresh host executes only the
next bounded iteration. Exact two physical/logical identities, actual entry
anchors, distinct provider sessions, retained original descriptor and persisted
self-route snapshot are checked.
These scoped greens do not constitute acceptance or permission to close the ticket.

The original complete nextest gate failed with ENOSPC and remains unverified.
Final integrated strict lint/tests and independent parent reviews are pending.
Scoped strict clippy session `31096` passed for persistence, orchestrator and
daemon with all targets/features and `-D warnings`, after the fixture nesting and
helper conditionals were repaired. Release orchestrator strict clippy `5694`
passed with the new fault hook and its call fully excluded from release builds.
Independent parent library nextest `49073` passed 1677 tests before the latest
planned-park suffix regression was added; this is not the full integration gate.
The planned-park suffix law had genuine RED: an establishment request after
binding could still seal a planned receipt. Shared current-prefix validation
made `51781` green for request, newer entry, completed stage and aborted run,
with unchanged proof/control/wake/cycle authority; the untouched occurrence
still parks.
Ordinary unowned-flow ownership normalization, real ACP usage and trusted
available-route estimates remain explicit requirements/dependencies below; the
runtime-wide capacity row cannot authorize a configured route-specific skip.

Acceptance evidence must distinguish the positive storage inspection law
`authenticated_same_node_reentry_retains_a_newer_unadmitted_plan` from an actual
cold daemon/Engine continuation. The law constructs authenticated outcome and
route records and checks the read-only classifier; it does not execute the next
loop occurrence. The outer acceptance must additionally show that a completed
same-node occurrence permits the next legitimate iteration after restart, with
no repeated old provider effect. Containment of an unfinished occurrence does
not establish restoration of its actual ACP session.

### Pre-dispatch implementation boundary

Read-only architecture review found that `runtime_capacity` has no profile/account
scope, observation timestamp or trusted source reference. It cannot authorize an
account-specific skip. Existing reactive fallback requires an actual primary
`SessionOpened` and typed 429; those checks must remain intact.

The next implementation needs these linked pieces:

1. A host-owned planned-stage journal anchor with stable logical invocation,
   frozen policy hash and control generation, before any provider effect.
2. Typed binding origin (original opening or pre-dispatch plan), with migration
   backfill preserving existing original-opening evidence.
3. Recipe/account-scoped durable capacity observations with trusted source,
   timestamp, expiry/reset and revision. Separate skip evidence from typed 429.
4. Transactional pre-dispatch skip and reservation in frozen candidate order,
   checking opt-in, same provider family, current claim/control and fresh evidence.
5. Reuse the existing one-shot quota opening permit and prompt authorization;
   pass logical invocation separately from selected provider invocation.

Acceptance must show zero primary opens/prompts when a trusted fresh observation
permits skipping it, one fallback open in the retained workspace, rejection of
forged/stale observations and different-family targets, and no duplicate effect
after crashes at plan/reservation/admission/opening boundaries. Unknown availability
permits an attempt without asserting availability; exhausted alternatives park.

A prerequisite policy defect was reproduced by
`rotation_does_not_override_retry_after_observed_reset_elapsed`: rotation used to
override retry even at/after an observed reset. The fix allows the original profile
retry at that boundary; 65 capacity tests, strict core all-target/all-feature clippy
and workspace format checks pass. Independent code review returned ACCEPTABLE.

### Ordering observations across attempts

Reservation-scoped inspection is insufficient for pre-dispatch selection: a later
successful opening in another run may supersede its old exhausted marker. The next
storage step must assign a monotonic registry admission epoch before each comparable
provider-opening effect. Admission sets the exact configured recipe to Unknown;
a typed error updates that projection only if its epoch is still current. A late
error from an older attempt remains audit evidence and cannot resurrect exhaustion.

Scope is project + candidate/runtime/account evidence + model + launch fingerprint,
not proven account-global capacity. Current launch fingerprints omit inherited
authentication environment; Unknown account identity must remain Unknown. Historical
markers must not be backfilled into an actionable projection without a complete
ordering of subsequent opens. Primary, fallback, continuation and comparable
openings without quota recovery must all produce supersession barriers. Failed RPC
or host death after admission leaves Unknown; replay inspection grants no new RPC.

Projection inspection then loads the current sealed reservation evidence in one
read snapshot. Only after this ordering exists may pre-dispatch skip/reservation
consume it with the current claim/control and frozen policy fences.

### Reservation evidence implementation checkpoint

Added `inspect_fresh_typed_exhaustion` and opaque `FreshTypedExhaustion`. Inspection
uses one SQLite read snapshot, exact frozen candidate/recipe matching and the
intersection of original/latest observation validity and known resets. Available
evidence cancels exhaustion; unknown/unsupported probes cannot extend the original
typed response's validity. Stored marker reads verify original TTL against the
immutable frozen stage and reset against the retained provider delay, rejecting
inconsistent or older observed evidence.

The storage-boundary tests demonstrated behavioral RED before implementation and
again for reset tampering before repair. After repair, all 34 recovery-cycle tests,
strict scoped clippy and scoped format checks passed; independent re-review returned
ACCEPTABLE. The full persistence library suite also passed: 438 tests. This read-only
API grants no opening authority and does not yet establish
the latest account/recipe observation across different runs. T12 remains in repair.

### Actual-provider capacity recovery

Successful fallback execution cannot establish recovery of the configured primary.
The capacity gate now captures the journal prefix before dispatch and clears only
the runtime identified by the last actual opening for the same node after that
prefix, and only on a successful stage result. Authentication, configuration,
pre-opening and storage failures preserve recorded exhaustion. Missing current
opening identity or unreadable history also preserves it.

The authentication-failure regression failed on the old implementation. The existing
13 capacity tests, added zero-opening missing-binding regression and real daemon
A→B case passed after the fix; the latter directly verifies A remains exhausted
while B's stale observation clears. Independent spec/quality review returned
ACCEPTABLE. Strict orchestrator/daemon all-target/all-feature clippy, workspace fmt
and diff checks passed. This corrects attribution but does not implement pre-dispatch
rotation.

### Registry opening admission ordering

Added registry migration 0026 and `work_items::recipe_capacity`: every comparable
provider opening commits a unique execution-writer admission before the RPC.
AUTOINCREMENT epochs order a broad runtime + existing Debug-AgentKind launch-hash
partition across projects, models and accounts. The latest admission is Unknown
until its exact opening attaches a sealed typed exhaustion receipt. Failed RPCs
and host death retain the Unknown barrier; repeated writer IDs cannot replay an
RPC, even when the original result was missing. No historical admissions are
invented or backfilled.

`record_selected_rate_limit` publishes within its existing transaction only for
matching actual writer, invocation, runtime and launch hash at the latest epoch.
Older typed causes remain audit evidence. A conflicting receipt for the same
opening rejects and rolls back; the original association is immutable. The new
read-only `inspect_current_recipe_exhaustion` checks latest admission, exact
project/frozen candidate/model, original/latest TTL/reset and sealed opening in
one registry read snapshot. This evidence grants no provider-opening authority.

All engine construction paths delegate to one owner that wraps the raw facade.
Standalone production project-description, doctor and daemon-triage callers wrap
with the same canonical registry dependency; library functions receive their
facade rather than opening an unrelated default registry. The wrapper delegates
legacy adapter and all bridge operations, validates returned provider identity,
and closes mismatched openings. Production fixture runtime sentinels were replaced
with the actual runtime identity.

Behavioral RED reproduced reservation evidence surviving a newer no-quota opening
and duplicate-writer facade replay before enforcement. Targeted tests cover
supersession, late original error, missing historical admission, unrelated
runtime/hash, exact project/model/TTL, conflicting origin rollback, storage failure
with zero RPC, receipt mismatch and replay after restart. The daemon A→B case
uses production `new_full` and asserts an actionable exact primary marker plus
one admission per primary/fallback; the wake case checks the alternate constructor
and a barrier for every recorded continuation opening. Pre-dispatch planning,
skip/reservation and account/profile rotation remain open; T12 is still in repair.

### Continue acknowledgment race repair

The real wake suite exposed an intermittent race after `RunContinued`: the daemon
journal subscriber could call `confirm_continued` before the engine authorized the
quota wake prompt. Both belonged to the same operation/generation, but the prompt
fence accepted only ContinueReserved and rejected its legitimate Executing ack.
A deterministic interposed acknowledgment reproduced the same conflict (RED).
The repaired fence accepts either state while preserving exact operation,
generation, current established handoff and durable journal prefix checks. Separate
tests retain both event orderings and reject changed operation/generation; all
three targeted automatic-wake storage tests pass. Broad execution verification is
recorded separately after the complete suites finish.

### Verification resource limit

The requested broad `cargo nextest run -p surge-orchestrator -p surge-daemon`
failed during test-binary linking with `No space left on device`, before test
execution. It is **not** a passing nextest gate. Concurrently queued verification
commands also ended with ENOSPC fingerprint-write failures. All those handles
are terminal. Package-scoped build-cache cleanup restored space without changing
source; verification now runs bounded affected suites sequentially. The complete
T12 nextest gate remains unverified until sufficient build capacity is available.

Whole-workspace strict clippy passed on the admission + acknowledgment repair
and fixture changes: `cargo clippy --workspace --all-targets --all-features --
-D warnings` (exit 0, 2m27s). Workspace fmt and diff checks passed. The unrelated
future-dependency notice for `block 0.1.6` was informational.

Final bounded verification after the acknowledgment repair and complete fixture
corrections passed with `CARGO_INCREMENTAL=0` and sequential linking:

- Persistence library: 443 passed.
- Orchestrator affected suites: archetypes 1, actual ACP/MCP permission 18, budget 3,
  capacity parking 14, recipe-admission facade 7 passed.
- Daemon affected suites: parity 1, resume stream 6 and real quota routing/wake 2
  passed. Both constructors and primary/fallback/continuation barriers were checked.
- Strict all-feature clippy for the final modified ACP permission fixture passed;
  workspace format and diff checks passed.

The root independently reran final whole-workspace strict clippy with
`CARGO_INCREMENTAL=0` (exit 0, 1m29s), and
`cargo nextest run -p surge-daemon --test quota_recovery_route_test` passed both
actual provider routing and automatic wake cases (2 passed, 0 skipped).

The broader nextest linker ENOSPC failure is retained above; these scoped results
do not replace the full T12 gate. No requirement was retired.

### Root full-workspace candidate check (2026-10-03, still RED)

After package-aware cleanup and rebuilding the ACP/CLI fixture binaries, root ran
`cargo nextest run --workspace --exclude surge-ui --no-fail-fast` with
`CARGO_INCREMENTAL=0`, `CARGO_BUILD_JOBS=1`, and dev/test debug info disabled.
Session `71139` terminated with exit 100: 3374 tests ran, 3345 passed, 27 failed,
2 timed out at the configured 120-second ceiling, and 36 were skipped.
The preserved log is `/tmp/surge-predispatch-root-workspace-lowdebug-20261003.log`.

Failures include bootstrap planning, task/provider and gate recovery, skill
binding, an unowned-rotation expectation and two snapshots. Several actual
provider paths report `provider opening differs from recipe admission`; their
cause and correct repair were still undiagnosed at that checkpoint. This is
executable failure evidence, not a final spec or code-quality verdict. The
candidate was frozen for read-only triage; the formal repair count at that
checkpoint was 2/3. The subsequent third dispatch is recorded below.

Read-only triage identifies test-adapter contract failures: WireBridge rewrites
the launch recipe after admission; AuthorBridge returns a fabricated launch hash
and omits the original writer observation. Keep the production equality fence;
configure the exact real mock command and profile before admission, and make the
in-process double report its received config faithfully. A mismatched-descriptor
negative must still prove cleanup and zero prompt.

Retained timed-out skill journals show a 5.37-second catalog-discovery interval,
followed by SkillBound and SessionOpened, after the test pump's three-second
readiness deadline had already failed. Root's targeted session `25190` passed
the pinned-skill test in 0.736 seconds; this does not clear the failed full gate.
The reviewed fixture-only repair monitors actual readiness, completion and pump
failure under bounded deadlines, preserves trust/revisit assertions and leaves
production HOME discovery and the shared MockBridge timeout unchanged.

### Consolidated formal repair 3/3 (2026-10-03)

Root dispatches the last shared repair after the frozen candidate's full gate
failed. The accepted scope combines the actual-cwd preparation lease below with
the diagnosed fixture corrections, reviewed additive snapshots and an explicit
zero-effect refusal oracle for still-unowned configured rotation. Lead and
independent adversarial pre-code review accepted this shape. The skills readiness
reshape installs the real broadcast receiver before publishing the subscription
counter; bounded local helpers then monitor pump and run completion together.

One builder owns production/storage and relative-cwd acceptance; a helper owns
the isolated test-fixture corrections. This is one consolidated repair dispatch,
not separate new budgets. Root hands the sole Cargo/build ownership to the main
builder; the helper performs no Cargo calls. Final outer verification, Phase 5a
and then Phase 5b remain pending. The count is now 3/3 and must not be reset.

## Что должно заработать

### Actual launch cwd for configured sources (reviewed repair dependency)

Current 5a preparation found that daemon Start freezes relative auth-file pins
against `workspace.checkout`, while provider launch and subsequent guards use
`workspace.path`. Absolute-file fixtures do not prove the required launch-cwd
contract. Fail-closed rejection or a permanently unavailable reusable pin does
not establish support for a valid relative declaration.

Prepare the original owned workspace before freezing configured source pins,
keeping project/config discovery rooted at checkout separately. Provisioning
acknowledgment still requires the real accepted attempt's launch claim; do not
mint that acknowledgment before Start acceptance. Do not copy credentials or
rewrite relative declarations to absolute checkout paths.

The reviewed repair uses an opaque non-Clone/non-Serde per-item Start preparation
guard and a durable preparation fence. Replay precedes filesystem work; begin
and finalize use short registry transactions. Original-owner Git reconciliation
and actual-cwd fingerprinting happen outside the registry transaction under a
secure stable per-item OS lock. A global write transaction through Git/auth I/O
would delay unrelated task controls and is not the selected design.

Finalize must recheck token, command identity and captured version/generation,
accepted revision, workspace/prepared state and control before atomically storing
the ordinary immutable attempt, operation result and consumed preparation.
Generic Start/reserve cannot bypass a live preparation. Lifecycle mutations must
respect that fence; a dead preparation is reclaimable only while holding the same
OS lock, without timeout-based ownership or guessed PID checks. It grants neither
provider effects nor automatic first launch. Existing prepared workspaces must
not be recreated after loss, and failed preparation must preserve retained files.

The secure preparation path is scoped by a pure host-config/graph predicate:
configured `RotationPolicy::Candidate` and an Agent node, including subgraphs.
It performs no auth-file or key I/O before obtaining the guard. Ordinary Starts
without configured rotation (including a supplied legacy policy) and terminal-only
graphs retain their existing cross-platform path; every reserve still enforces
the live preparation fence. Platforms without the required secure lock support
explicitly refuse the new configured-rotation path before provider effects.
They must not silently disable rotation or use an insecure lock fallback.
With configured host rotation enabled, host freeze replaces a supplied legacy
policy, so that combination also requires preparation; it is not an exemption.

Secure lock implementation belongs in a private persistence module. It does not
depend upward on orchestrator or introduce a generic public filesystem API.
Descriptor ownership transfer and Darwin ACL FFI remain unsafe audit sites even
when most operations use safe nix wrappers.

Acceptance requires real normal daemon Start with relative files whose checkout
and actual-cwd contents differ; selected/skipped/post-429 actual-file changes;
concurrent exact replay; direct mutation bypass refusal; stale/control/archive
fences; host-death reclamation; and responsive unrelated-item controls during
blocked fingerprinting. Compare retained files, raw index and HEAD and assert no
premature Prepared acknowledgment or provider RPC. This is a reviewed dependency,
not an implemented repair or final 5a acceptance.

The reviewed responsiveness oracle blocks the actual daemon's worker immediately
before host fingerprinting, after original-owner Git preparation. A different
item's Archive must complete through that same server before release, and the
original normal Start must then perform its actual freeze and provider launch.
The hook and call are debug-only, use an exact item selector and verified bounded
regular markers, and grant no authority. This proves the pre-fingerprint worker
boundary; it does not claim cancellation of an arbitrary kernel credential read.
Combine it with short-transaction laws and the real relative-file outer test.
The worker must own the preparation guard throughout Git/fingerprint work;
cancellation of the awaiting async request cannot unlock it while that work
continues, and a detached worker must never finalize Start in the background.

The new normal-Start relative-cwd acceptance test produced behavioral RED in
builder session `69110` (exit 101 after successful compilation): checkout-based
freeze disagrees with the actual launch file, the task enters Attention with
`selected configured source changed before provider effect`, and no A prompt is
sent. This was behavioral failure, not a setup failure. After the actual-cwd
preparation change, builder session `39038` passed the same outer scenario
(exit 0, 0.68 seconds), including actual source 429 and next-task zero-A/one-B
effects assertions. The remaining lease, crash, source-drift and full-workspace
gates remain pending; one outer GREEN is not final acceptance.

The generic-Start bypass law also produced genuine RED in builder session
`85434`: a preparation existed, but direct Start still reserved an attempt.
Migration `0028`, opaque guard ownership, snapshot/token finalization and
universal live-preparation fences are under implementation in repair 3. Session
`78346` failed compilation while the private secure-fd helper was being adapted
to nix 0.29; it is not behavioral GREEN or an accepted candidate. The builder
retains sole Cargo ownership until final handoff.

Additional author-loop evidence in the same unfrozen repair 3: preparation laws
passed 13/13 in session `68935`, after behavioral RED `42983` showed that replacing
the lock pathname could bypass a still-live descriptor. The preparation row now
binds held directory/lock identity; replacement cannot prove the old owner dead.
Actual daemon pre-fingerprint responsiveness passed in `73963`: another item's
Create and Archive completed through the same server before release, without an
early provider RPC. Awaiter cancellation passed in `69388`: the blocking worker
kept the guard until completion and discarded output created no attempt or effect.

The current quota suite passed 35/35 in `14975` before the final lock-identity
hardening; it needs revalidation on the final source. Strict checks `47653` and
`94330` stopped on fixture compilation/lint errors. Their mechanical corrections
remain author WIP until a new terminal gate confirms them. These targeted results
do not retire the earlier full-workspace failure or constitute final 5a/5b review.

The current author whole-workspace strict gate passed in `61236` (exit 0,
all targets/all features, `-D warnings`), and current ACP/CLI fixture binaries
rebuilt in `80731`. Canonical nextest `79688` then terminated exit 100:
3451 tests ran, 3395 passed, 56 failed, 36 skipped, no timed-out tests, in
147.481 seconds. Its log is
`/tmp/surge-predispatch-author-repair3-workspace-20261003.log`.
All 35 quota-route tests passed on this source, including the latest lock and
actual pre-fingerprint boundaries. The global result remains RED.

Of the 56 failures, 53 are copies of the new shared fixture's negative admission
test: its storage setup rejects `SingleThreadedRuntime` before reaching the
admission assertion. Two killed-Continue scenarios elapsed while waiting for
`RunContinued` with a registry transaction held; cause and faithful repair remain
under diagnosis. One skill-approval test failed to create a scheduled pool thread
with OS error 35 (`WouldBlock`), rather than failing a skill assertion. Preserve
these errors and verify resource-bounded full execution without skipping tests or
weakening assertions. Source is still unfrozen author WIP within repair 3/3;
neither final spec compliance nor independent code/security review has begun.

The shared fixture's runtime annotation was corrected without weakening the
descriptor mismatch law. Targeted `24224` passed 4/4 with two test threads,
including both unchanged eight-second post-ack assertions (5.259/5.317 seconds).
The full skill-binding suite passed 10/10 in `96579`, also with two threads.
These results do not establish a global pass or prove every previous failure was
resource contention. Test-only owned-child cleanup and bounded journal diagnostics
were then added; deadlines, SQL trigger and behavioral assertions remain intact.

Workspace doctests excluding UI passed in `74437`: 5 passed, 7 existing ignored,
0 failed. Metadata-only `cargo check -p surge-daemon --release` passed in `52788`,
including compilation without the debug-only preparation hook/call. Formatting
passed in `29181`. Final strict check `18822` and a complete, unfiltered canonical
nextest run with two test threads still need terminal confirmation on the latest
fixture source; source freeze and independent 5a/5b remain pending.

The unfiltered resource-bounded full run `66117` terminated exit 100 in
273.664 seconds: 3451 ran, 3449 passed, 2 failed, 36 skipped. Log:
`/tmp/surge-predispatch-author-repair3-workspace-bounded-20261003.log`.
Both post-ack Continue scenarios passed with their unchanged eight-second
deadlines. The remaining failures were the cold planned-park proof and concurrent
preparation first creation; no final candidate freeze or review acceptance occurred.

Retained cold diagnostics show only `usage_seq` 0→24 and `usage_unknown` true→false
differed between full attempt snapshots; binding, config, state and identity were
identical. The first proof now uses the real `sync_usage` projection before capture,
preserving exact second-child equality. The actual cold scenario passed 10/10
repeats in `56553`. This is projection timing evidence, not proof of real ACP spend
or a resolution of the earlier lost-diagnostic child failure `3061`.

Concurrent creation produced genuine RED in `78381` and `11547`. An independent
two-thread OS diagnostic, using a held directory FD and fresh basename each round,
recorded 903 `ENOENT` errors in 1000 old `O_CREAT` rounds. Exclusive creation plus
noncreating reopen only on raw `EEXIST` completed all 2000 opens in 1000 rounds.
Artifacts: `/tmp/surge-openat-race-independent-20261003.py` and `.json`. Owning
maintainer and independent security pre-code lenses accepted this bounded change.
All secure flags, held inode/path/UID/mode/ACL/link checks remain; other errors
fail closed, without retrying unsafe inputs or unlinking locks. Temporary caller
and errno diagnostics were removed. The actual task concurrency law passed 30/30
repeats in `11472`; 12 other preparation/security/lifecycle laws passed in `59950`.

Corrected-source strict clippy `97825` passed all workspace targets/features with
`-D warnings` (exit 0, 1m48s). Corrected release/docs, refreshed fixture binaries
and the final complete nextest run still require terminal evidence. This remains
author WIP in the same consolidated repair 3/3, not a fourth review repair or a
completed T12 slice. Independent 5a must precede 5b after final executable gates.

Corrected release `75403`, doctests `86694` (5 passed, 7 existing ignored) and
fixture binary refresh `95312` terminated successfully. Full run `29932` then
terminated exit 100 in 274.271 seconds: 3451 ran, 3450 passed, 1 failed,
36 skipped. Log: `/tmp/surge-predispatch-author-repair3-workspace-corrected-20261003.log`.
The remaining cold Continue test observes `ContinueReserved` instead of
`Executing` immediately after Engine-active readiness. Independent read-only
inspection confirms that this predicate sees the historic question and active
run before the daemon supervisor asynchronously acknowledges `RunContinued` in
the registry. The bounded fixture correction waits within the same eight-second
deadline for active readiness and the exact durable state, generation, operation
and attempt binding; existing assertions and journal counts remain. It is still
unfrozen author work in repair 3/3; final full gates and both review stages remain
pending.

### Ordinary flow ownership normalization (reviewed next dependency)

The actual-cwd preparation dependency required a bounded amendment of this next
unit. The complete revised pre-code plan is retained in
`.autopilot/competitive-waves/owned-flow-normalization-plan.txt`. Owning maintainer,
independent behavior and security reviewers accepted it after resolving stable
CLI replay routing, typed accepted flow origin, immutable provisional ownership,
owner-aware Stop/Resume and authenticated private MCP hydration on cold startup
and wake, including frozen empty lists. This is design acceptance only. It creates
no implementation evidence and does not close the current repair, T12 or the goal.

Configured rotation on ordinary CLI/daemon flow launches must enter the same
durable task owner, workspace, opening fences and automatic wake lifecycle as
task launches. The current task-owned pre-dispatch implementation does not close
this criterion. Until normalization exists, unowned rotation must fail explicitly
before any provider effect; freezing a policy alone is not support.

The reviewed next unit creates a real graph-backed task and accepted revision,
reserves an attempt with the host's run ID, and records a durable owned-flow
launch intent and operation result in one registry transaction. The exact graph
hash is a typed accepted-contract origin, without invented feature criteria.
Initial prompt and every launch input survive acknowledgment. The coordinator
does not launch detached work; existing admission, claims and startup recovery
remain the effect owners.

Operation replay is inspected before current source-cleanliness checks, discovery
or config freezing. Exact replay returns the original attempt, frozen config and
workspace intent; any changed explicit request field, including complete
wire-safe run configuration, conflicts. First acceptance captures a clean immutable
Git base and freezes host config before the transaction. Workspace preparation uses that
retained base. Dirty source or an arbitrary explicit `--worktree` is rejected on
first acceptance without changing source files, index or HEAD.

The reviewed request contract is a separate `OwnedFlowRunConfig` DTO with
`deny_unknown_fields`, not serialized `EngineRunConfig`: the latter skips its
in-process `agent_registry`. Every allowed explicit field participates in bounded
canonical request identity, with sorted map keys and preserved list ordering.
The DTO excludes agent registries, memory-store overrides, caller quota policy
and host-derived memory claims; fallible legacy conversion rejects supplied
internal data instead of dropping it. Clients construct the DTO before host
context seeding. Existing task-requirements serialization and hashes stay intact.

Keep incoming request identity, immutable host-prepared startup inputs and live
provider configuration separate. The host accepts graph/base/workspace, seeds,
budget, guards and quota policy once. Provider registry and effective auth sources
are rebuilt and checked against the accepted model/recipe/pins before effects,
without saving credential-bearing registry snapshots or silently freezing anew.
Exact replay returns the original receipt despite catalog drift; an incompatible
catalog at first launch, cold reconciliation or wake requires attention on the
original attempt with zero RPC. Returning an accepted receipt grants no effect.

The accepted flow contract holds the graph hash and exact original user prompt,
including an empty prompt. Flow-origin context returns that prompt without task
criteria or title decoration. Host context seeding adds separate seeds/defaults;
frozen configuration and the startup journal must match the accepted prompt.

A reserved attempt alone is not a recoverable queued request: existing startup
reconciliation intentionally skips Reserved attempts without startup history.
Only an explicit durable owned-flow launch intent authorizes reconciliation of
that case. It permits startup attempts, not provider effects, and does not weaken
the generic Reserved safeguard. CLI launches must retain daemon ownership and
automatic wake rather than create a separate local recovery lifecycle.

Pending intent eligibility also requires exact run/binding/generation, an active
Reserved attempt, permitting current control and no archive; a generally valid
Attention or Suspended launch claim is insufficient. Cancel/suspend must fence
pending dispatch atomically. Existing startup history takes the authenticated
resume/hydration path or attention, never another first launch. Operation/run-ID
collisions and the startup-commit-before-intent-ack crash window require checks.

Acceptance must cover CLI and daemon ordinary flow starts with zero exhausted-A
opens and one selected-B open; matching stream/registry run identity; concurrent
exact replay; changed `run_config` rejection; replay after source/config changes;
crashes after acceptance and queued acknowledgment before startup; startup commit
before intent acknowledgment; unknown/internal DTO fields and every allowed
input's conflict behavior; exact prompt after restart; catalog drift replay and
unstarted drift refusal; invalid-control pending-intent refusal; retained
workspace and frozen budget through park/wake; and unchanged dirty files, index
and HEAD on first-acceptance rejection. Inventory other unowned launch surfaces
before declaring T12 complete. This is a reviewed plan, not implemented evidence.

Перед диспатчем ноды движок смотрит, влезет ли работа в остаток окна. Не влезает — ран паркуется с временем пробуждения и виден в inbox как ожидающий, а не как молча вставший. После сброса он просыпается сам, с замороженным бюджетом, который переармируется точно так же, как при обычном resume. Если ротация включена, вместо парковки берётся следующий настроенный профиль того же рантайма.

## Из брифа, дословно

> «before dispatching a node, refuse to start work that cannot finish inside the remaining window; park the run with a wake time instead of stalling»
> «Optional rotation across configured accounts — Surge never copies or stores provider credentials»
> «a run that exhausts its provider window resumes automatically after reset with its frozen budget intact, and the pause is visible in the inbox»

## Разделы спецификации

Истории 34–37, 49–50. Решения §13, §14, §19, §20. Границы: `surge-core::capacity`. Швы §2 и §3.

### Unplanned owner-loss containment (reviewed repair dependency)

The actual process-crash oracle exposed a duplicate opening: after a durable
`SessionOpened` and prompt authorization, startup replay appended another
`StageEntered`, cleared the old quota plan and admitted a new provider session.
The test held the facade before the wire prompt; production cannot use that
test-only knowledge to infer that the authorized prompt had no effects.

Before entering a restored stage, inspect its original current occurrence and
registry provenance under the authentic launch claim. Match node, attempt,
entry/plan sequences, logical invocation, frozen policy and control generation.
Existing reservations or uncertain/established/executing handoffs without
independently verified continuation/wake authority require recovery attention.
Missing or inconsistent provenance also requires attention. Even a plan with no
admission must reuse its original occurrence and normal transactional gates;
restart never manufactures a fresh logical plan or opening authority.

Keep original provider/session/writer identities. Authenticated committed outcome
and routing evidence may permit legitimate later reentry; confirmed planned
parking and existing verified wake paths remain eligible. A read-only inspection
result grants no effect capability. The crash oracles must prove unchanged
stage/plan/admission/open/prompt counts as well as retained identities, while
ordinary dispatch, all-skipped wake, mixed exhaustion and frozen-budget tests
remain green. This is a reviewed containment repair, not completed evidence.
Recovering the same ACP session under verified continuation authority remains a
required product outcome; attention does not replace that requirement.

## Критерии приёмки

### ACP usage measurement boundary (2026-10-03)

The real warmup-before-planned-park fixture exposed an existing accounting gap:
`surge-acp::bridge::tokens::extract_usage` returns no spent-token snapshot.
The mock emits both context-window `UsageUpdate` and end-turn `PromptResponse`
usage, but current production consumption does not charge the latter. Nonzero
real ACP spending and its conservation therefore remain unproved; a host-owned
budget oracle can verify scheduling/rearm behavior without claiming measured
provider spending.

The pinned schema 1.9.1 describes response usage as per-turn while its component
fields describe session totals. The contradiction and divergent implementations
are tracked in [upstream issue 1860](https://github.com/agentclientprotocol/agent-client-protocol/issues/1860).
Context-window occupancy must not be substituted for spent tokens. A linked
implementation must make accumulation semantics and measurement coverage explicit,
preserve unknown/missing usage, and prove no double charging through multiple
turns, resume, reconnect and event replay. Token history/estimation requirements
remain open wherever real protocol usage is required; no requirement is retired.

The reviewed accounting dependency freezes a versioned adapter usage contract:
unknown, per-turn, or provider-session cumulative. Keep raw independent `u64`
token buckets, context occupancy and monetary cost separate; do not infer a
provider's contract from monotonic samples or fill in a model/currency. Before a
prompt RPC, persist a host turn identity. Store its raw usage receipt, normalized
delta, measurement coverage and cumulative baseline atomically, with exact
replay idempotence and conflicting replay rejection. A loaded session needs a
trusted baseline; missing reports, interrupted turns and declining cumulative
counters preserve incomplete coverage rather than becoming zero spending.

Acceptance must distinguish per-turn `100/300/50` from cumulative
`100/400/450`, charging `450` in both explicitly contracted cases. It must cover
duplicate delivery, restart, loaded sessions, missing usage, occupancy updates,
wide counters, cache buckets and non-USD cost. Task totals must expose coverage
separately from pricing. This remains a reviewed next dependency, not implemented
accounting evidence for the routing change.

Version-specific source research further limits the default contract. Surge's
builtin `npx` launch commands do not pin adapter packages; registry metadata and
a package found in the npm cache do not prove the version actually launched.
Keep the generic accounting contract unknown until the host establishes the
exact adapter artifact and its documented semantics.

The [Claude ACP 0.23.1 implementation](https://raw.githubusercontent.com/zed-industries/claude-agent-acp/v0.23.1/src/acp-agent.ts)
resets its usage accumulator per prompt and sums independent input, output and
cache buckets. Its [background-task regression](https://raw.githubusercontent.com/zed-industries/claude-agent-acp/v0.23.1/src/tests/acp-agent.test.ts)
also includes earlier background results in a later prompt's report. Treat this
as an explicitly contracted reported processing window, not complete request-only
spend; interruption and monetary cost coverage still need separate proof.
The [Codex ACP 0.16.0 prompt implementation](https://raw.githubusercontent.com/zed-industries/codex-acp/v0.16.0/src/codex_agent.rs)
returns a stop reason without response usage. These findings neither prove all
versions unsupported nor authorize inference of spend from context occupancy.

- [ ] `CapacityPolicy::decide(estimate,&window) -> Dispatch | Park{wake_at}`
- [ ] `estimate` — медиана длительности и расхода по архетипу ноды из **существующих** таблиц аналитики
- [ ] Нет истории по архетипу → оценки нет → **отказа в диспатче нет**
- [ ] Парковка видна в `surge inbox` с временем пробуждения
- [ ] Пробуждение после сброса автоматическое; замороженный бюджет переармируется как в `1ed5caa`
- [ ] Ротация — opt-in; триггер: окно исчерпано И ротация включена → следующий профиль того же рантайма
- [ ] Учётные данные не копируются и не хранятся
- [ ] Тест на движковом харнессе: исчерпание → парковка → пробуждение → бюджет цел

## Исполнение

Агенты Rust Code Studio (R42): `rust-scout` — локация, `rust-builder` — реализация,
`test-engineer` — тесты, `rust-reviewer` — ревью. Гейт: `cargo clippy --workspace
--all-targets --all-features -- -D warnings` + `cargo nextest run` + `cargo fmt` — все зелёные.
Отсутствующая зависимость или инструмент → верни `BLOCKED`, не устанавливай.

## Реализация: сверка M3 (2026-10-03)

- Перед каждым agent dispatch работает `CapacityPolicy::decide`; отсутствие
  наблюдения или оценки не блокирует первый dispatch. Доказательство production
  пути: `engine_capacity_park_test::estimate_none_and_never_observed_does_not_block_dispatch`.
- На повторной сверке прежняя оценка по узлу в текущем ране была удалена: она не
  отвечала критерию §19 и могла выдать нерелевантную историю за archetype estimate.
- `RunHistoryWorkEstimator` читает завершённые раны того же archetype из
  `PipelineMaterialized` и медианы строк `stage_executions`; выборка ограничена
  1000 последними завершёнными ранами. Миграция 0007 привязывает usage-сессии к
  попытке узла; 0008 хранит известную стоимость отдельно от неизвестной. Если
  хотя бы одно usage-событие не содержит цены, стоимость этой попытки исключается
  из spend median. Без archetype metadata/history estimate остаётся `None`.
- Тест проверяет фильтр архетипа, median времени, median известной стоимости,
  исключение незавершённых ран и отсутствие подмены отсутствующей цены нулём.

Проверки текущей сверки: `cargo test -p surge-orchestrator --lib` — 410 passed;
`cargo test -p surge-orchestrator --test engine_capacity_park_test` — 13 passed;
`cargo test -p surge-persistence --lib` — 433 passed; `cargo check --workspace`,
`cargo clippy -p surge-persistence -p surge-orchestrator --all-targets
--all-features -- -D warnings`, `cargo fmt --all -- --check` и `git diff --check`
— passed.

До durable account/profile handoff `Decision::Rotate` теперь возвращается к
настроенной политике парковки: без reset используется `blind_backoff`, а без
него не изобретается время пробуждения. Engine test фиксирует 77 секунд при
включённом, но ещё не реализованном rotation candidate.

Открыто: критерий ротации профиля пока не работает в production dispatch path;
движковый тест полного цикла `exhaustion → rotate/park → wake → resume` ещё не
закрывает все ветки.

Проверки: `cargo test -j2 -p surge-orchestrator --lib run_history_estimator` — 2
passed; `cargo test -j2 -p surge-orchestrator --test engine_capacity_park_test
estimate_none_and_never_observed_does_not_block_dispatch` — 1 passed.

## Перенесено из ревью таска 11 (2026-09-06) — читать до начала работы

Оба ревьюера, независимо друг от друга, уткнулись в одно и то же ограничение таска 11.
Оно там не дефект — таск 11 не обязан был его снимать, — но **этот таск об него споткнётся
на первом же шаге**, если начать с `decide`, а не с источника.

### 1. Ёмкость сегодня живёт в памяти процесса. Тебе она нужна до отправки.

`surge doctor` строит окно из **своего же только что упавшего** smoke-вызова (ветка
`Err(detail)`), а `HealthTracker` — структура в памяти процесса. Межпроцессной памяти
о наблюдённых 429 нет вовсе. То есть окно **не переживает процесс**: `doctor` покажет
ёмкость, только если отказ прилетел ему самому, здесь и сейчас.

`CapacityPolicy::decide(estimate, &window)` вызывается **перед диспатчем**, и в этот
момент у свежего процесса окно будет пустым всегда — не потому что провайдер щедр,
а потому что никто в этом процессе ещё не получал отказа. Политика, построенная на таком
источнике, будет диспатчить в исчерпанное окно и узнавать об этом из собственного 429 —
ровно то поведение, которое требование R37 запрещает («refuse to start work that cannot
finish inside the remaining window»).

→ **Условие: источник ёмкости — событие рана, а не память процесса.** Наблюдённый 429
уже пишется в лог как `StageFailed`; окно должно собираться оттуда. Это первый шаг таска,
а не последний.

### 2. Половина R35.1 сегодня не утверждаема, и это твоя зона

Ревью craft проверило и зафиксировало честно: «оценка не блокирует диспатч» **нельзя
утверждать на таске 11**, потому что `CapacityPolicy::decide` и `Park` не существуют
нигде в воркспейсе — грепом пусто. То есть «не блокирует» сегодня истинно структурно
(блокировать нечего), а не доказано.

→ Критерий «нет истории по архетипу → оценки нет → отказа в диспатче нет» — это **не
формальность и не унаследованная галочка**. Это единственное место, где утверждение
станет проверяемым. Тест должен краснеть, если отсутствие оценки начнёт означать отказ.

### 3. Классификатор рейт-лимита — один, и он уже есть

Таск 11 свёл два разошедшихся классификатора в один, живущий в `surge-core::capacity`;
`surge-acp::pool` теперь зовёт его. **Третьего не заводи** — если тебе нужен образец,
которого нет, добавляй в существующий и прогоняй тесты пула до и после.

Там же удержана граница, которую легко снести: дефолт «60 секунд» — это **политика
бэкоффа пула**, а не наблюдение. `parse_retry_after` в `capacity` возвращает `Option`
и обязан отказываться выдумывать. Парковка обязана вести себя так же: нет времени
сброса — нет выдуманного `wake_at`.

## Перенесено из ревью M1 (2026-09-06) — не забыть на следующих этапах

**В приёмку M3 (решение), обязательно:**

1. **Конфиг уже виден оператору, но его никто не читает.** `[capacity]` с
   `blind_backoff = "5m"` сериализуется в **каждый** `surge.toml`, который пишет
   `surge init` — проверено. То есть человек может отредактировать задокументированную
   ручку, которая ни на что не влияет, пока M3 не свяжет `CapacityPolicy` с
   `SurgeConfig.capacity`. Это не дефект M1 (потребитель по плану на M3), но M3 обязан
   закрыть разрыв, а не оставить ручку-обманку.
2. **`validate()` закрывает только один слой из двух.** Он отвергает непредставимый для
   `TimeDelta` бэкофф, но не календарный: `blind_backoff = "9000000000000s"` **загружается
   чисто** и тихо клампится в `+262142-12-31` внутри `decide` — ровно тот исход, который
   комментарий самой валидации объявляет предотвращаемым («явная ошибка конфига лучше, чем
   тихий кламп тремя слоями ниже»). Паникой это больше не является, поэтому не блокировало
   M1. → Добавить `Utc::now().checked_add_signed(delta).is_some()`, либо ограничить поле
   потолком, который сообщение уже обещает («минуты и дни»).

**В приёмку M5 (поверхность):**

3. **Группа WAITING не закреплена.** Тест проверяет **метку**, а не **группу**: удаление
   всего блока `if !waiting.is_empty()` из `print_inbox` оставляет **все 138 тестов
   `surge-cli` зелёными** (мутация G ревьюера). Исходный дефект — припаркованный ран,
   видимый только через `--json`, — по-прежнему не прибит.
   Корневая причина названа: строковый шов `attention: &'static str` позволяет `classify` и
   `print_inbox` расходиться независимо. Настоящее решение — enum, но это **пред-существующая
   форма**, не введённая этим таском; решать, менять ли её, на M5.

**Различение, которое надо удержать при приёмке M2–M5.** Ревью применило правило «грепни
боевых вызывающих у каждой новой `pub`» ко всему, что добавил M1, и нашло нулевых
вызывающих у `CapacityPolicy`, `WorkEstimate`, `RotationPolicy`, `Degraded`. **Это не тот же
дефект, что был у `parse_reset_hint`:**

- у них потребитель — `run_task.rs` на M3, и план **прямо называет** это место посадки;
- `parse_reset_hint` был недостижим потому, что **уже живой** производитель не обновили,
  пока ADR утверждал, что поведение переехало.

Первое — факт плана, второе было дефектом. Разница в том, **утверждал ли кто-то, что оно
уже работает**.

## Решения, принятые оркестратором на ревью M2 (2026-09-06)

**1. `CanonicalRuntimeId` — обязательный критерий приёмки M3, newtype назван умолчанием.**

Контракт «вызывающий уже нормализовал ключ» держится **прозой на голом `&str`** и
**отказывает открыто**: если M3 передаст `claude`, а строка в `runtime_capacity` лежит под
`claude-acp`, то `status()` вернёт `NeverObserved` → `decide` вернёт `Dispatch{None}` → ран
уйдёт **в исчерпанный рантайм**. Это направление ошибки, которое таблица рисков этого же
тикета исключает: «избыточный отказ терпим, недостаточный — нет».

В M2 не делалось намеренно: `CanonicalRuntimeId` рябит в `CapacityWindow.runtime: String`,
закоммиченный на M1, а единственный вызывающий живёт на M3.

→ **M3 обязан** либо ввести newtype, конструируемый только через
`Registry::normalize_agent_id`, либо доказать тестом, что не-нормализованный алиас на
вызывающей стороне невозможен. Прозы недостаточно — её недостаточность уже доказана
тестом `unnormalized_aliases_are_not_collapsed_by_this_store_alone`.

**2. `observed_at_ms` убран из `runtime_capacity`.**

Колонку писал каждый `observe` и не читал **никто**: ни одного `SELECT` во всём воркспейсе,
проекция `status()` её не берёт, ни один тест её не утверждает. При этом она заставляла бы
M3 выдумывать значение. Устаревание уже представимо слоем выше — через
`seconds_until_reset`, о чём говорит комментарий в той же миграции.

Ничего не отгружено, поэтому удаление бесплатно; возврат — одна аддитивная миграция.

**3. Поправка к моему собственному критерию приёмки.** Требование «тест на `NaN` писать на
колонке SQLite, в отличие от JSON» стояло на **ложной посылке**: SQLite молча переписывает
`NaN` в `NULL` при записи. Достижимая порча — значение вне диапазона (`1.5`), и `+inf`,
который хранится честно. См. память проекта, `surge-sqlite-nan-becomes-null`.

## Сознательное сужение на M5: терминальные раны больше не показывают ёмкость (2026-09-07)

**Это откат того, за что боролось ревью таска 11 — и он намеренный.** Записано, чтобы
следующий не прочитал как регресс и не «починил» назад.

**Что было на таске 11.** `classify` короткозамыкал на `summary.status.is_terminal()` и
возвращал `capacity: None`. Ревью показало, что это баг: сигнал 429 живёт как раз в
**терминальных упавших** ранах (`StageFailed` с текстом рейт-лимита), то есть сканировался
класс, где сигнала нет, и пропускался тот, где он есть. Починили — стали сканировать
`Failed | Aborted | Crashed`.

**Что стало на M5.** Колонка ёмкости переехала из фолда журнала в точечную выборку по
`runtime_capacity`, чтобы у факта остался **один дом**: журнал рана A физически не знает
о 429, который получил ран B на том же рантайме. Побочно снят долг производительности
таска 11 — измерено ~200–380 мс на сотне терминальных ранов против долей миллисекунды.

Но у мёртвого не-припаркованного рана **нет дешёвого честного способа приписать рантайм**:
`RunParked.runtime` у него отсутствует (он не парковался), а `SessionOpened.agent_id` в
старых журналах сырой, и чтение его — это ровно то полное чтение журнала, которое и было
долгом. Гадать по совпавшей строке реестра — тот же грех, что у старого скана.

→ Поэтому `Failed`/`Aborted`/`Crashed` дают `NeverObserved`, а живая колонка остаётся
только у `Parked` (там рантайм известен из собственного `RunParked.runtime`).

**Причины разные, и это существенно.** На таске 11 было **короткое замыкание** — ответ
неверен по построению. Сейчас — **отсутствие атрибуции**: ответа честно нет.

**Что потерял оператор:** для рана, умершего по рейт-лимиту, инбокс больше не показывает
ёмкость рантайма рядом с ним. Причина смерти по-прежнему видна в тексте отказа.

**Чем закрыть, если понадобится:** писать канонический рантайм в событие отказа так же,
как это делает `RunParked`, — тогда атрибуция станет дешёвой и сужение можно снять.
