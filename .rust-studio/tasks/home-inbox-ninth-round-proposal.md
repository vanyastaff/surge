# Home/Inbox ninth repair: renewed live observation after fallback

Current status: **LOCALLY CLOSED after authorized repair 10/10**. Independent
Phase 5a ACCEPTABLE and generic/domain/security Phase 5b COMPLETE; no open findings.
Final source-bound gates: 388 UI + 5 integration tests, zero ignored; strict
Clippy default/all-feature all-targets, workspace all-target/all-feature check,
fmt and diff passed. Persistence 605 + one existing skip, rustdoc nine warnings
and manual API-GATE are reused against 144 unchanged inputs, not rerun.

Reviewed manifest: `.rust-studio/evidence/home-inbox-tenth/freeze-manifest.json`,
SHA-256 `9952788ac83ba227062bb160a85260433258f737561b58cd33cab4bf282f733d`;
17-file Rust digest `17a3a586270eaeffe272d8c3befc425c86fa049a04fa2a533dd59ea959fca8b9`.
Final review record: `home-inbox-tenth/phase5b-review.json`. Exact prior proposal
bytes are retained in `home-inbox-tenth/reviewed-task-snapshots/` before this update.
Budget remains 10/10; no eleventh dispatch, push or remote merge. Local commit
follows this closed review. Native same-habit/all-eight-task/gpt-6.1-sol,
Windows, MSRV and release acceptance remain open.

## Historical proposal and repair record

The statuses and authorization limits below describe their original checkpoints;
the current closure above supersedes them. Their design and evidence are retained.

Status: PLAN ONLY; maintainer pre-code verdict ACCEPTABLE for focused independent
critic/security review. Actual dispatch count remains 8/10. The user already
authorized up to ten dispatches for the same four blockers and their required
regressions; this introduced H7-GEN-02 regression is inside that scope. No new user
permission or budget reset is requested. Root records 9/10 only immediately before
an author handoff after this plan is accepted. No source/build/test/runtime/provider/
index/memory/commit action is authorized by this design task; only this proposal.

Frozen input: HEAD `6d23ebf069fdaa2f3762c8b540c5f669b965dec8`, 17-file Rust digest
`ec652148523accc097ece400011ce8b108f7755e21256ff3edae7789b7720a14`;
`.rust-studio/evidence/home-inbox-eighth/freeze-manifest.json` SHA-256
`01cd9f58782002ffe8f335d28ce249bee3b375e4df769747696bf01d93729fad`.
Original sixth/seventh/eighth plans and evidence remain immutable review inputs.
Eighth gates: 378 UI unit plus five integration, existing seventh regressions
preserved. H6-SPEC-01/02 and H7-GEN-01/H7-SEC-01/H7-DOM-01 are resolved at reviewed
source. Phase 5a has one introduced blocking P2, H8-SPEC-01; H7-GEN-02 cannot close
until it is repaired. No new behavioral RED for this finding has run yet.

## Producer, failure and protected guard

recover_rejected_subscription (app_observation.rs:247) retains fallback_owners
under the current run token. A successful NONTERMINAL history read returns at
:261 without releasing or changing that marker. sync_run_subscriptions retains
every still-current marker (app.rs:881) and excludes every marked run (:893).
Therefore even a genuinely renewed admission cannot attach its event stream.

The actual producer is GlobalDaemonEvent::RunAccepted, documented in engine/ipc.rs
as a newly admitted run and published by daemon admission paths. AppState's
apply_global_event (:213-218) marks its registry observation Running/coarse row
Active. The real App global subscriber (:1464-1466) then calls sync_run_subscriptions,
but fallback exclusion prevents a new live claim/Subscribe request. The existing
test directly calls Observations.claim for replacement, bypassing this broken App
producer/caller route; retain it as a low-level guard test, not the outer oracle.

The fallback exclusion originally protects a CURRENT failed/in-flight/exhausted
episode from an unchanged Active summary, repeated operation snapshot or arbitrary
sync invocation. Removing all markers after reads, bypassing them for any Active
row or resetting attempts on every sync would turn rejection into unbounded
subscription/history polling. Preserve that invariant and initial plus three
automatic fallback reads. SubscribeOk remains the only attachment signal; a
RunAccepted event requests an observation, not proof of Working or a new session.

## Selected minimal private owner change

Give the existing fallback episode marker a private completion/demand disposition:
recovering, recovered nonterminal, terminal, or exhausted/unconfirmed, together
with its originating Token and optional pending renewed-attachment demand. This
is bookkeeping in the current owner, not a new framework, executor or public API.

A CURRENT fallback completion records its disposition before returning. It does
not automatically sync or clear the exclusion: with no renewed observation, an
unchanged poll still cannot create another attachment or reset the read budget.
Terminal history stays terminal under canonical display, independently of coarse
Active hints. Existing obsolete completion/error/finalizer checks remain first.

Route an actual RunAccepted through one private App handler that applies the
existing state update, records a renewed attachment demand for that EXACT run's
CURRENT fallback token/scope and invokes normal synchronization. Wire the real
global subscription callback to that handler; production tests use this path.
Only the current daemon connection/event owner and current Scope can deliver the
demand; an obsolete connection callback cannot release a replacement marker.
Do not infer a demand from ListRuns Active, connection availability, operation
revision, history success, timer expiry or a manually claimed replacement owner.

The demand is one-shot and fenced to the fallback owner/read episode. If no read
is in flight, transfer to ordinary live claim/Subscribe through the existing
owner checks, consuming only the matching marker/demand. If a current read is
in flight, retain/coalesce that demand until actual read settlement, then service
it once. Do not lose it because claim is temporarily blocked, and do not create
parallel offline/live publishers. A CURRENT newly observed RunAccepted may trigger
one new attachment after a failed/exhausted episode too; this is a new admission
observation, not another automatic retry or a reset of the old episode's budget.
Without that new signal, failed/exhausted episodes stay bounded and excluded.

For the confirmed nonterminal case, the renewed demand must actually reach
subscribe_to_run and SubscribeOk; dropping the marker without servicing demand
is insufficient. A successful attachment creates the normal new live owner and
retires only superseded read-owned restoring diagnostics through the existing
attach_live_owner/completion rules. Preserve actual StreamError provenance and
canonical registry/terminal truth. New live events publish through the current
token as usual; the fallback token and its callbacks/timers become obsolete.

If this new attachment rejects, its NEW current owner enters the existing bounded
fallback path. Do not reuse/reset the old failed episode or schedule additional
subscriptions from unchanged polls. Repeated synchronization while that attempt
is pending/attached is coalesced by the actual subscription marker. Completed,
absent, cancelled and scope-changing paths must settle/clear only their own
matching marker/demand; no old finalizer can remove a replacement subscription.

No broad change to global lifecycle timestamps, registry truth, operation pending
batch settlement, conditional Inbox merge, persistence API or retry framework.
Existing current-owner backoff and source checks remain the owners of those facts.

## Actual outer RED first

After root dispatches nine, add a genuine App/controlled-IPC regression against
the frozen eighth source BEFORE behavior edits:

1. Start from real fixture nonterminal journal and ordinary App subscription.
2. Actual Subscribe request rejects; actual fallback reads and confirms that
   NONTERMINAL journal. Prove no operation association and no live attachment.
3. Deliver renewed Global RunAccepted through the actual App global subscription
   producer/handler, not a direct Observations.claim or token mutation. The
   controlled daemon peer must observe a second Subscribe request and return
   SubscribeOk, then send actual per-run events.
4. Assert the App receives/publishes current observation/events, retains recorded
   history/decisions and exact run identity, and has one actual live publisher.
   Frozen source must fail at the missing second Subscribe/current observation.

The fixture's server replies, global notification and per-run frames use the
existing real IPC facade; a test-only alternate owner mapper is not the oracle.
Private fixture registry/journal writers and IPC tasks/children settle before
explicit cleanup. Retain setup/compile failures separately from behavioral RED.

Required controls through production callers:

- No new RunAccepted: repeated unchanged Active ListRuns, equal operation updates
  and sync invocations cannot create another subscription, reset reads or exceed
  initial plus three. Permanent fallback exhaustion remains visible.
- Renewed demand arrives while fallback read is paused: coalesces once, is serviced
  after actual settlement, and never publishes the old read after new ownership.
- Same run/scope new live owner replaces the fallback: old success/error/timer/
  finalizer cannot clear its current StreamError, flags, subscription or marker.
- A→B→A and home/current connection replacement reject delayed old demands and
  callbacks even when run IDs/path strings repeat. Current legitimate demand
  still works in the new scope; no manually claimed owner as replacement oracle.
- New operation revision alone does not authorize live renewal/reset exhaustion;
  existing pending operation hydration still progresses under its original owner.
- Terminal fallback stays Done; a transport hint cannot make it Working. Absent
  reserved identity creates no row/session/action. Previously accepted canonical
  registry hint and conditional inspection merge cases remain unchanged.
- Duplicate sync while an accepted renewed Subscribe is pending causes one request.
  If a genuinely new request rejects, only its new current bounded episode owns
  recovery; no automatic resurrection of the completed old episode.

## Closure, review and gates

Minimal private UI closure: app.rs real global-event handler/sync demand handoff;
app_observation.rs fallback completion/settlement; run_observation.rs only if the
existing current-owner transition needs a small private disposition method;
AppState integration only as required to preserve the actual producer route;
existing App/controlled subscription/home-inbox tests. No public surface or new
framework. Retain all eighth fixes and existing assertion strength.

Refresh authentic focused RED/GREEN, full UI baseline 378 unit + five integration
plus new cases, strict affected default and all-feature/all-target Clippy with
-D warnings, workspace all-target/all-feature check, workspace fmt and diff.
Bind exact source/log hashes, narrow eighth-to-ninth diff and case map in a new
freeze receipt. No timeout inflation, serial mask, ignored tests or fake counts.

Reuse persistence 605 passed/one existing deferred skip, rustdoc exit0/nine retained
warnings and completed manual API review ONLY if all 144 persistence/source inputs
and API hashes remain unchanged; explicitly say reuse, not rerun. No persistence,
API compatibility/version mitigation, Results, scheduler, verification receipts,
witness or runtime/native provider change belongs here.

Independent focused Phase 5a must verify H8-SPEC-01 through the actual producer and
the complete four-blocker caller closure, then affected generic/domain/security
Phase 5b must accept this narrow repair. This is not a new broad audit. Root makes
the local stage commit only after required reviews; no push or remote merge.

Maintainer-grade verdict: ACCEPTABLE for focused critic/security review. The
private marker needs to distinguish a bounded recovery episode from a renewed
host observation; current Token/Scope/owner and IPC producer already provide the
seam. The old guard is narrowed by an explicit current event, not removed.
No strategic choice or extra human permission is needed inside the authorized
max-ten scope. Actual count stays 8/10 until root records the ninth author handoff.

Same habit tracker, all eight tasks/two milestones, exact codex-acp/gpt-6.1-sol,
follow-up/history and current platform/native acceptance remain the next unfinished
journey. This repair and synthetic tests do not complete that project.


## Actual ninth handoff within existing explicit max-ten mandate

Owner and independent critic/security returned ACCEPTABLE on body SHA256
65f4df29b495498fc9c4643d96fcd2f000963c7e71a553f6768e91d1dfa3718f.
The exact body and review are preserved in evidence/home-inbox-ninth/.
User already explicitly authorized maxTEN for the same four blockers and required
repair regressions. H8-SPEC-01 is an introduced GEN02 regression within it.
Actual dispatch **9/10** is now recorded immediately before the same usable author
handoff. One further focused dispatch remains if required; no eleventh automatic,
no budget reset and no new human permission request. No push/remote merge.

Source/destination HEAD6d23ebf069fdaa2f3762c8b540c5f669b965dec8 plus frozen eighth
Rustec652148523accc097ece400011ce8b108f7755e21256ff3edae7789b7720a14,
codex/results-planning-history in this worktree. Sole Rust/source/Cargo lease
passes to the author until explicit freeze/release. Connection/scope rejection
must precede AppState mutation; duplicate ACTUAL RunAccepted while pending must
coalesce. Actual App/globalIPC/Subscribe RED first; no manual owner claim oracle.
Local commit only after independent focused5a then affected5b; native samehabit
all8/codex-acp/gpt-6.1-sol continuation remains next, not complete.


## Ninth author verification and review handoff

Same actual **9/10**, no budget reset or self-dispatch ten. Author reproduced
H8-SPEC-01 on frozen eighth production after mechanical callback extraction:
`outer-red.log` exit101, one executed/378filtered, second Subscribe expected2
versus actual1 after real RunNotActive, confirmed ordinary nonterminal history and
actual global IPC RunAccepted. Setup field/method compile failure is separate.
The minimal repair adds current fallback completion/one-shot demand bookkeeping,
services only actual current admission after actual read settlement, and retains
unchanged-poll exclusion and bounded initial-plus-three recovery.

Pre-freeze caller validation found an introduced connection-wide project Scope
pin. `receiver-scope-red.log` genuinely failed through the actual shared receiver
connected before project selection. The receiver now guards its connection
lifetime by the current facade Arc, captures Scope per delivery, and checks
explicit delayed-delivery Scope before AppState mutation/renewal. Current EOF is
connection-owned across project changes; obsolete connection events and EOF are
inert. The production bounded reconnect dispatch remains intact. Test fixtures
share receiver/event/EOF ownership but disable actual user-daemon reconnection.

Final six focused tests pass through real IPC and App callers: confirmed
nonterminal renewal; held fallback/current live reads plus duplicate ACTUAL
RunAccepted frames during read and pending Subscribe; exactly one live owner
AFTER SubscribeOk/BEFORE intentional transport error; retained diagnostic/history
after actual read/current finalizer settlement; terminal truth; four unchanged
exhausted reads/new admission/new rejected episode; old connection and A→B→A
callback refusal before state mutation; same receiver no-project→project and
project changes; actual old/current receiver Task completion/EOF ownership.
DaemonEngineFacade closes per-run transport on StreamError. Earlier fixture
expectations of live feed/subscription after that intentional current closure
were corrected with stronger explicit pre-error publisher/post-settlement
checks. Failed targeted receipts and separate compile/setup receipts remain.
No production suppression, timeout inflation, ignore or serial mask was added.

Final exact-source full UI **384/384 +5/5**, zero ignored/filtered; affected
UI+persistence all-targets strict clippy default and all-features both exit0 with
-D warnings; workspace all-targets/all-features check0; fmt/diff0. Six new ninth
behavior tests are included. All preceding sixth/seventh/eighth fixture bytes
are an exact unchanged prefix, retaining original assertions and all four caller
closures. Only app.rs, app_observation.rs and appended home_inbox_sixth.rs differ
from frozen eighth's17 Rust inputs. No public/persistence/Results/scheduler/
provider/runtime/index/main source or memory changes.

All144 persistence/doc input hashes and receipt hashes are unchanged: prior
605passed/one existing skip and doc0/nine warnings plus manual API-GATE are reused,
not rerun. Automated public API tools remain unavailable. Sixth61/seventh45/
eighth56 frozen evidence hashes are verified unchanged. New ninth before snapshots,
narrow patch, case map, command/failure/gate history and manifest bind current
source. Test linker unwind-size and dependency future-incompatibility warnings
remain reported; strict clippy has no warnings.

Author BUILD verified; source frozen and sole Rust/source/Cargo lease released.
Independent focused5a for H8-SPEC-01 and required four-blocker regressions, then
affected generic/domain/security5b remain PENDING. No self-closed review or author
commit/push/merge; root may dispatch tenth only if independently required inside
the existing mandate. Native same habit all8/codex-acp/gpt-6.1-sol history journey,
Windows/MSRV/release acceptance remain open and are not completed by these tests.


## Ninth independent reviews and final authorized repair preparation

Focused5a returned ACCEPTABLE. Generic and domain5b returned **COMPLETE**.
Security5b returned **NEEDS WORK** on H9-SEC-01P2: an operation reader reuses the
recovering fallback owner and advances its read generation; actual renewed
RunAccepted waits, but operation completion never services fallback renewal and
its original timer correctly becomes obsolete. Shared current-read settlement
notification is required, not revived old authority or reset budgets.
See evidence/home-inbox-ninth/phase5b-review.json for original trace and verdicts.

All gates remain green at384+5 but do not cover this missing interleave.
Current count9/10; explicit human max10 already covers the required GEN02 repair
regression. Tenth pre-code plan is being prepared/reviewed, with no extra user
permission needed and no tenth code handoff yet. No eleventh is automatic.
This post-review update changes documentation only; frozen reviewed task bytes
are in phase5b-reviewed-task-snapshot.md. Rust and prior evidence unchanged.
