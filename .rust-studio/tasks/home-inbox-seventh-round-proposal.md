# Home/Inbox: proposed seventh repair for two Phase 5a blockers

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

Status: PLAN ONLY; maintainer pre-code verdict ACCEPTABLE for independent review.
Home dispatch budget is exhausted at 6/6. There is NO seventh authorization,
author handoff, source/test/build/runtime/provider action, commit or publication.
The sole permitted write during this design is this proposal. Frozen source and
index remain untouched. This is the same Home/Inbox task, not a budget reset.

Review input: HEAD `6d23ebf069fdaa2f3762c8b540c5f669b965dec8`, frozen 19-file/
17-Rust-file closure digest
`1911c4e1f7c5b41d6890a8c57a0bc969441030145e0ad1d4aab41d28f90e32e2`.
Freeze manifest `.rust-studio/evidence/home-inbox-sixth/freeze-manifest.json`,
SHA-256 `b12d412e6a3c79802b254dcbcf63a312643117d45c7119bd329a8345d8cc6a97`.
The original approved contract and sixth-round chronology are retained in
`.rust-studio/tasks/bootstrap-run-status-sixth-round-proposal.md`; they are not
rewritten by this proposal. Case coverage is in the frozen `case-map.md`.
Independent findings are recorded in `phase5a-review.md`; the byte-exact reviewed
task snapshot is `phase5a-reviewed-task-snapshot.md` (task SHA-256
`bfacb2d18842e4da1f52ad8101fc4b1a59caae36aba691c7c558dc3fbd335daa`).
Both are under `.rust-studio/evidence/home-inbox-sixth/` and remain unchanged.

## Confirmed failures and actual producer/caller chain

H6-SPEC-01, blocking: app_observation.rs:269-275 clears operation_inspections and
copies nested related-history errors BEFORE the per-run token is checked by
complete_history_read at :283-285. inspect_operation (:105-183) produces
`Ok(Some((checked_record, Vec<(Token, Result<history, error>)>)))`. Its registry
recheck can remain valid while an offline run owner is replaced by an actual live
publisher. The operation scope/identity/revision are unchanged, so the outer
operation guard accepts the batch. complete_history_read correctly rejects the
obsolete run token, but the stale error has already modified operation triage.
app_state.rs:394 gives operation_inspections priority over corroborated failure
dedup; a duplicate operation card can obscure the correct actual run failure.

The frozen delayed tests cover stale direct run callbacks and changed operation
status. They do not exercise a nested history Err after owner replacement with
the SAME checked operation snapshot. Passing 357 unit/5 integration tests does
not refute this production path or close Phase 5a.

H6-SPEC-02, blocking: app_observation.rs:245 computes retry only from outer
result.is_err(). A valid operation with a CURRENT failed related journal is
outer Ok(Some(...)), so the failure receives no retry. The result also removes
offline_retries as though successful. With no daemon connection, the related
journal can remain absent/unconfirmed forever after a transient error. The
unchanged-snapshot branch in app.rs:585-600 intentionally returns without another
hydration request unless restoring a missing relation; it cannot repair this.

Actual callers are restore_indexed_operations -> read_operation_observed ->
inspect_operation -> read_history/bound_related -> delayed App publication ->
update_bootstrap_view/complete_history_read -> AppState operation_problems and
Inbox items/count. hydrate_related also enters this same owner. Live pumps use
begin_history_read/complete_history_read through publish_read; their existing
diagnostic protection must remain intact.

## Protected inputs and invariants before changing any guard

| Existing guard | Protected input/invariant; preserve it |
|---|---|
| Current Scope and expected operation snapshot | Home/project generation, immutable operation relation and revision. Obsolete lookup/refusal cannot mutate a later project/operation. |
| accepts_observation(token, true) | Exact run, current publisher owner, current in-flight read generation and recorded error revision. Old read cannot publish, clear or finalize another owner. |
| history_unconfirmed_if_unchanged / clear_observation_error_if_unchanged | Actual live errors before/during a read cannot be replaced/cleared by history-owned diagnostics. |
| Detached candidate/prefix/decision watermark checks | Older/conflicting replay cannot regress live terminal, sessions, pending decisions or recorded timestamps. |
| offline_operations coalescing | At most one current operation batch; do not remove it to force retries. |
| Observations.claim live/in_flight/complete checks | Live publisher is exclusive; no parallel offline reader or rereading unchanged completed owners. |
| offline retry limit and history_retry_delay | Initial read plus at most three automatic retries, with existing backoff; no loop, budget reset or unbounded polling. |
| update_bootstrap_view equal snapshot early return | Idempotent operation updates do not reread history on every poll. Do not weaken it as a retry workaround. |
| Checked registry recheck and bound_related | Only authentic operation/project/graph/run relations supply navigation; this repair changes no persistence trust rule. |

## Selected minimal repair

### One token-checked disposition owns all related-read effects

Move related-read diagnostic publication/clearing behind the SAME AppState owner
that validates and completes the run read. No operation_inspections mutation from
an unfiltered `for (_, history)` loop remains. Validate the token before any run
fold, run-bound operation diagnostic, retry intent or finalization is touched.

Use a small private completion disposition to distinguish at least obsolete,
current confirmed, current accepted-absent and current incomplete/error outcomes.
The current bool loses the distinction needed by its operation caller. Keep one
canonical completion implementation; live publish_read may project its confirmed
bit for existing pump behavior. All direct callers/tests retain their semantic
assertions when adapting to the disposition. Do not add a second token checker or
replay implementation in SurgeApp.

Completion captures the originating token and diagnostic provenance BEFORE it
finishes that read. A current nested failure can contribute a run-bound operation
diagnostic and a retry request. An obsolete completion contributes neither and
cannot clear a newer diagnostic. Existing newer-live-error/provenance guards also
apply to run-bound operation diagnostics; they cannot bypass a refused stream
diagnostic merely by writing another collection.

Separate checked OPERATION-record diagnostics from RELATED-RUN-read diagnostics
inside one private operation-inspection owner: optional record refusal plus
run-keyed history observations carrying their tokens. A successful child read
clears only its matching history observation, not a current record-integrity
refusal or a different run's error. A checked current operation observation may
retire the matching record-level refusal; a stale related read cannot. Centralize
selection of current displayable reasons; Inbox/count consumers receive that
selection rather than flattening unchecked stored strings.

Current record-integrity refusal remains visible even if historical child failure
is corroborated. Stale/replaced-run errors do not resurrect duplicate operation
triage. Reuse existing canonical terminal/operation dedup and pending-decision
rules; do not suppress all operation problems or promote a read error into a
RunFailed/Aborted event. Positive record status may still be stored independently
when its own checked operation guard is current; that does not grant authority to
publish its obsolete related histories.

### Retry accepted CURRENT failures under the shared owner

Derive retry need from the checked operation observation AND the per-run
completion dispositions, not only outer Result. A current related-read error,
current older-prefix refusal requiring another read or incomplete expected-started
history requests the existing bounded backoff path. A legitimately absent
never-started reserved run remains absent and does not create a retry storm.
Obsolete run tokens are neutral: no retry, no error publication, no counter reset
and no owner claim on their behalf.

Preserve initial plus three retries and existing delays. Track a queued retry's
operation scope/snapshot and the exact failed run owner/last-read identity after
finish_read. The timer must verify BOTH before scheduling another read; scope or
operation equality alone is insufficient. If another read, actual live publisher,
project/home generation or operation snapshot supersedes it, the queued work is
discarded without mutating the replacement owner's state or claiming a new owner.
Reuse/extend Observations' private last-read identity query rather than reconstruct
private token fields in App. Retry still claims/begins through the existing shared
owner, respects in-flight/live/complete exclusion, and never starts a parallel pump.
The last-read query deliberately does not require in_flight: completion has
already finished that read. It still compares exact scope/run/owner/read/error
revision. Use the same query for retained run-bound diagnostic provenance; otherwise
every just-published diagnostic would become stale immediately after finish_read.

The operation batch can reread only eligible failed related runs; successful
terminal owners retain their complete guard. Current lookup/recheck errors retain
their existing operation-owned retry rule; they are not falsely made run-owned.
Aggregate current run dispositions deterministically, preserving a current error
for either related run. One successful sibling cannot reset a failing sibling's
budget. Reset attempts only after the current retry episode is actually resolved,
not merely because the checked record was outer Ok. Stale batches/timers cannot
clear counters for a newer episode. At exhaustion retain the diagnostic and an
explicit retry-exhausted observation; no new run lifecycle or automatic restart.
An explicit user refresh may start a new observation episode only through its
ordinary current-owner path; unchanged background operation polls do not reset it.

No change to the unchanged-snapshot early return is required for automatic
recovery: the accepted current failure owns its own bounded retry. This removes
the dependency on a daemon event or operation revision change.

## Outer RED tests required before production edits

No tests are added/run in this plan-only task. After explicit seventh-round
authorization, add two authentic failures through the existing actual App offline
operation producer and production Inbox selection/rendering. Use the retained
fixture registry/journal readers and existing controlled async observation seam;
do not inject a replacement acceptance algorithm or weaken existing assertions.

1. **H6-SPEC-01 RED:** checked Failed operation and real completed parent/failed
   child; cause an actual related-journal read Err and pause before publication.
   Restore readable fixture history and attach the replacement live publisher
   through the existing production SubscribeOk/controlled IPC owner path. Publish
   current trusted child Failed while keeping operation identity/revision/scope
   EXACTLY unchanged. Release the old offline batch. Assert no stale operation
   inspection, exactly the real child failure card/count, no planning failure,
   current sessions/terminal/flags retained and no obsolete retry scheduled.
   This must fail via the pre-token operation mutation on frozen sixth source.

2. **H6-SPEC-02 RED:** new App never connects to a daemon; genuine checked operation
   and both real fixture journals. Temporarily make one journal unreadable using
   a private fixture filesystem obstruction after its real writers are closed.
   Let its CURRENT read finish with nested Err while record inspection succeeds;
   restore the same original fixture object and advance controlled backoff. Without
   updating operation revision, calling manual hydrate or creating a daemon/live
   event, the automatic retry must restore the related trusted history and correct
   Inbox. On frozen source it remains unhydrated because retry sees only outer Ok.

Retain fixture setup/teardown failures separately from behavior RED. Use private
test runtime homes and existing process/SQLite settlement; no actual habit-tracker
runtime mutation or provider RPC is part of these regressions. The first case's
controlled IPC publisher is a test fixture, not the user's daemon/provider.

Additional mandatory regression branches:

- Current checked operation corruption/refusal is not erased by a successful
  sibling history or obsolete run error. Current related errors remain visible;
  current success clears only its owned diagnostic. Keep operation-vs-run dedup.
- Permanent related-read failure executes exactly initial plus three bounded
  retries, then retains explicit exhaustion/inspection without spinning. Count
  actual read invocations; do not merely assert a counter implementation.
- Stale related failure after replacement schedules zero extra reads/claims; a
  queued current retry becomes inert if live owner/read generation changes before
  its timer. Old completion/error/finalizer cannot clear newer in-flight markers.
- Scope A to B to A, same run and unchanged operation snapshot: late batch and
  retry timer remain obsolete; old owner cannot become current from equal paths.
- Both related reads fail, or one succeeds and the other fails: one bounded
  episode, no duplicate owner, no premature reset or loss of the sibling reason.
- Current older-prefix retry preserves newer terminal/decision watermark; actual
  live diagnostics before/during reads retain the sixth-round provenance guards.
- Never-started absent reserved run does not retry indefinitely or become a UiRun.
- Existing no-create/no-migrate/read-only integrity, current operation lookup
  refusal, exact navigation, live error preservation and reconnect tests retain
  their current assertions. Genuine IPC children/tasks, readers and pools settle
  before explicit fixture cleanup; no serial mask, ignore or timeout inflation.

## Source closure and gates after authorization

Expected private UI closure: app_observation.rs publication/retry aggregation,
app_state.rs completion disposition/inspection provenance, run_observation.rs
minimal guarded last-read identity support, existing app.rs private retry/owner
bookkeeping only if needed, and app_tests/home_inbox_sixth.rs plus the existing
controlled subscription test support. Inbox changes are limited to mechanical
consumption of the private diagnostic selection if its current direct field reads
require it. No new module/trait or alternate observation subsystem is proposed.

Persistence inspection/API, schema, source validation, generation/prefix guards,
Results projection, scheduler, verification receipts and witness source remain
unchanged. If a concretely necessary persistence/public API write is discovered,
stop and reshape/review that dependency before dispatch; this plan grants none.
No habit continuation implementation or remote publication is included.

Verification must bind new source hashes to command outputs and refreshed case
map. Run authentic focused RED/GREEN, then full affected UI baseline 357 unit plus
5 integration AND all added cases, with honest executed/ignored/filtered counts.
Refresh strict affected default and all-feature/all-target Clippy with -D warnings,
the required workspace all-target/all-feature check, workspace formatting and
diff checks. Preserve all existing assertions and gating thresholds.

Keep the frozen persistence source/input hashes and its 605 executed/605 passed/
1 existing deferred cross-process skip receipt. Do not claim a rerun when unchanged
inputs permit explicitly source-bound reuse; do not hide a new skip. If inputs
change, refresh the affected gate rather than reusing the old claim. Windows and
MSRV/native product claims remain unverified until their actual gates run.

After author freeze, focused independent Phase 5a must recheck BOTH blockers and
actual callers/tests on that exact source. Only then complete all still-pending
full Phase 5b generic/domain/security/public API reviews for the combined Home
closure; private UI repair does not eliminate the existing persistence API review.
Commit authority and any author handoff follow root/user workflow, not this plan.

## Alternatives, verdict and permission boundary

Selected: one token-checked related-read completion disposition, provenance-aware
operation diagnostics and current-owner bounded retry. It repairs the shared
producer/caller semantics without weakening their protected inputs.

Rejected: filter stale errors only when rendering Inbox (stale state/retry effects
would remain); retry every outer Ok/unchanged poll (busy loop and terminal-owner
guard bypass); retry all nested errors before token validation (obsolete callback
could take over a live run); clear all diagnostics on any success (loses genuine
operation refusal/sibling errors); loosen token or stream-error guards (reopens
the sixth-round failures).

Maintainer-grade verdict: ACCEPTABLE for independent pre-code review. Crate owner
is private surge-ui observation; current Token/Scope, checked storage readers,
candidate publisher, terminal selection and backoff are reused. The required new
information is a completion disposition and diagnostic provenance already present
in the producer, not a new execution authority. No strategic product choice is
open for these two confirmed blockers.

Focused independent plan critic and security review must accept both proposed
dispositions before root asks the user. Root should request a concrete explicit
seventh-round mandate only after those reviews, explaining Home 6/6 exhaustion
and the review-repair requirement. Until
the user grants it, source remains frozen and no repair is dispatched. The same
habit tracker with all eight tasks/two milestones, exact codex-acp/gpt-6.1-sol,
follow-up/history, broader continuation, existing-project and platform acceptance
remain the next unfinished goal; this repair does not complete the app.


## Explicit revised mandate and actual seventh handoff

User answered: "Да, разрешаю седьмой раунд". This explicitly authorizes one
additional repair of H6-SPEC-01/02, actual RED/GREEN tests, required gates,
independent focused 5a then full pending 5b/API and a local stage commit.
No push/remote merge is authorized. Accepted design body SHA256 is
857bb7e02d90b1d73114e285aeb1d55a0e7c333c395b3ca426d914abb321d278;
its byte-exact snapshot is evidence/home-inbox-seventh/approved-plan-snapshot.md.
Owner and independent critic/security all returned ACCEPTABLE before this mandate.
H7-ADV-01/02 remain implementation cautions within that accepted plan.

Actual repair dispatch is now **7/7**, recorded immediately before the same usable
Home implementer handoff. No eighth round is automatic. Source HEAD is
6d23ebf069fdaa2f3762c8b540c5f669b965dec8 plus the frozen sixth Rust closure
1911c4e1f7c5b41d6890a8c57a0bc969441030145e0ad1d4aab41d28f90e32e2.
Destination remains codex/results-planning-history in this worktree.
One author holds Rust/Cargo ownership; root/reviewers remain read-only for source.
No new persistence API/source or Results semantic changes belong to this repair.
Source/result review must precede local commit and actual habit continuation.

## Seventh author execution and freeze chronology

Same dispatch 7/7 resumed the original implementer. Verified accepted body SHA
857bb7e02d90b1d73114e285aeb1d55a0e7c333c395b3ca426d914abb321d278 and saved all
17 frozen Rust files under evidence/home-inbox-seventh/before; sixth evidence was
not edited. outer-red.log was a fixture compile/setup failure (Arc<Storage> and
Fn receiver), not behavioral RED. outer-red-2.log authentically failed BOTH actual
App/Inbox scenarios: two executed, 357 filtered, exit 101. Stale nested error was
published before run-token validation after real SubscribeOk replacement; current
nested error did not retry without daemon/status change. Positive real registry,
parent/child journal and current live child Failed conditions were established.

The shared completion now returns obsolete/confirmed/absent/incomplete disposition,
with captured last-read identity distinct from post-publication diagnostic revision.
Operation record refusals and per-run diagnostics have separate provenance and
central current selection. Retry episodes retain current scope/operation snapshot,
serial, bounded attempt and exact per-failed-sibling last-read tokens; current
errors retry initial plus three, obsolete errors remain counter-neutral. Reads
stay under the existing per-run owner and completed sibling histories are retained.

First outer GREEN was outer-green-2.log, two executed/357 filtered, exit 0;
outer-green-1.log retained an intermediate compile refusal (RunId is not Ord).
The planned matrix reached 6/6, then 9/9. targeted-2.log retained 8/9 with a new
fixture controlled-signal timeout, reproduced in late-scope-diagnose.log: its
extra update_bootstrap_view had already started the current batch, correctly
coalescing the attempted second controlled read. Removed that duplicate fixture
setup call without weakening assertions. The old and current paused batches then
proved separate generation/marker ownership.

The original expected-started boundary was pinned with real checked Failed AND
Completed operation records and missing journal objects. expected-started-red.log
failed completed=true authentically (one executed, 366 filtered, exit 101). The
Completed result now requests bounded journal inspection only; it supplies no
run row/stream/event/lifecycle. A genuinely absent never-started reserved identity
still does not retry or become a UiRun. targeted-final.log passed all ten new
tests (357 filtered), including permanent single/both/sibling-repaired errors,
actual read counts/exhaustion, queued replacements and A→B→A, post-finish error
provenance, older prefix, genuine record refusal and no daemon recovery.

The earlier complete UI gate passed 366 unit + 5 integration, then strict Clippy
reported one collapsible_if (exit 101). That mechanical fix and the expected-started
case are covered by refreshed final gates: 367/367 unit + 5/5 integration, zero
ignored/filtered; strict affected default and all-feature/all-target Clippy with
-D warnings; full workspace all-target/all-feature check; fmt and diff checks.
All final exits are zero. Unchanged persistence input hashes explicitly bind reuse
of the sixth 605/605 + one existing deferred skip and rustdoc exit 0 (nine retained
warnings); no rerun is claimed. Exact commands, source/log hashes, narrow seventh
diff and cases are in evidence/home-inbox-seventh/freeze-manifest.json.

Author source is frozen for independent focused 5a on BOTH blockers, then the
still-pending full generic/domain/security/public API 5b of the combined closure.
Author does not close those reviews. Sole Rust/source/Cargo lease is released
with the terminal handoff; no commit/push/merge or provider/native/runtime/user
project action occurred. Dispatch remains 7/7; no automatic eighth round.
Real same habit tracker history/all eight tasks with codex-acp/gpt-6.1-sol,
Windows/MSRV and native/product acceptance remain open.


## Full seventh Phase 5b outcome (post-freeze documentation)

Focused Phase5a is ACCEPTABLE; H6-SPEC-01/02 are resolved. Full generic, domain
and security reviewers each returned **NEEDS WORK**. Required source-traced
blockers H7-GEN-01/02, H7-SEC-01 and H7-DOM-01 are recorded with caller evidence
in evidence/home-inbox-seventh/phase5b-review.json. No reviewer executed a new
reproduction; a subsequent authorized repair must first establish actual RED.
API reviewer returned **API-GATE COMPLETE — manual review; MAJOR compatibility
impact on paper**. New exports are intentional MINOR additions; shared 4MiB
refusal narrows existing inspection behavior. Release impact is understood but
RELEASE-GATE/version/changelog/MSRV/publish are not claimed or changed.

Author gates remain green at367+5, targeted10, strictboth/workspacefmt/diff; they
do not close the four missing scenarios. No eighth repair has been dispatched,
and Home remains exhausted7/7. A consolidated pre-code eighth plan is being
prepared, with independent critique/security required before a revised human
mandate. No Rust/provider/runtime/index/memory/commit or external publication
action is authorized by this review.

This is a documentation-only status update after the immutable seventh freeze.
The exact reviewed task bytes are preserved in phase5b-reviewed-task-snapshot.md;
all seventeen Rust source files and prior frozen evidence remain unchanged.
Samehabit/all8/codex-acp/gpt-6.1-sol journey still follows CLOSED Home review.


Eighth proposal is now independently ACCEPTABLE at design SHA256
8d8e2322146ed5e2d1b9ecaa54b365a6c0c750b53fa035e5e353ad9069481d6e.
The first version was RESHAPE NEEDED for an unavailable retained-navigation
identity seam. The accepted reshape uses canonical standalone inspection when
conditional merge cannot preserve an eligible link, plus explicitly bounded
ordinary fallback. V1 bytes and both review dispositions are preserved under
evidence/home-inbox-eighth/. Human max10 option is PROPOSED ONLY; actual budget
remains exhausted7/7. No eighth code/test/build/author dispatch has occurred.
