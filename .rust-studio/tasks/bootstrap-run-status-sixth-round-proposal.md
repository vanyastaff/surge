# N7: separate Bootstrap operation state from recorded run lifecycle

Current status: sixth-round implementation AUTHORIZED and dispatched below.
Owner and independent pre-code re-review ACCEPTABLE; N7-SEC-1/N7-ARCH-01 and
N7-ARCH-02 are addressed in the plan. The original proposal itself did not grant
execution authority; the explicit revised user mandate below now does. Home/Inbox
dispatch budget is **6/6**, with no automatic seventh round.
Results/history repair 3 has its own restricted mission/runs/header/loading scope
and does not authorize app.rs changes. The separate witness 4/4 authorization
covers its observer only. No budget is reset by this document.

Historical design phase allowed only this proposal. Main integration is now
committed locally and its author lease released; sixth-round source/Cargo ownership
is transferred only by the dispatch below.

## Actual failure and source-backed cause

Native acceptance operation `run-01M538BSJ1G4MQXWAXAJP81FV2` has a planning run
`run-01M538BSKHNKR64K4C8TC3X3Z6` completed at seq113 and implementation
`run-01M538BSKHWQTW2EHQ2QSHH74A` failed at seq117. Inbox displayed a second
Failed card for the completed planning parent. Its own evidence said
`run completed · bootstrap_complete`. The operation failure had overwritten the
parent's run lifecycle.

`crates/surge-ui/src/app.rs:update_bootstrap_view` (:465-524) selects implementation
only for three implementation phases; terminal phase None falls back to planning.
It maps operation Failed/NeedsAttention to RunStatus::Failed, Completed to
RunStatus::Completed and Cancelled to RunStatus::Aborted, then inserts/overwrites a
UiRun and invents now-based start/end timestamps. These are operation facts being
misattributed to a run. It can also label completed planning as Active while the
operation is implementing.

`app_state.rs:unacknowledged_failures` (:348-353) trusts that coarse UiRun status.
`screens/inbox.rs:370-430` trusts it again to select the Failed/Aborted card and
headline, while reading reason/evidence independently from that run's stream.
That produces the contradictory N7 card, not merely an incorrect header color.

Do not substitute result.run_id as the selected run and retain the mapping:

- bootstrap_supervisor.rs:590-605 records operation BudgetExhausted against a
  planning identity after planning completed; it does not create RunFailed there.
- NeedsAttention names a blocked operation phase and optional related run; it
  is not a run terminal event or a running provider observation.
- Cancelled can settle before a reserved run starts. It does not prove RunAborted
  for every reserved identity.
- Reserved planning/implementation identities are navigation/association facts,
  not evidence that either journal or session exists.

## Reuse and ownership

Reuse the existing trusted display seam, not a second lifecycle classifier:

| Existing module | Role |
|---|---|
| surge-core/src/run_display.rs:21-115 | RunDisplayState represents Working, Done, Waiting and Unknown; terminal facts take priority and registry-only terminal labels do not establish Done. |
| surge-ui/src/run_stream.rs:252-313, 371-406 | Trusted origin/continuity, actual run identity and attributed current observation govern display. Stream disconnection is not terminal failure. |
| surge-persistence/src/runs/query.rs:124+ | aggregate_status derives trusted display/verdict from actual envelopes; reuse it for cold inspection. |
| surge-core/src/run_verdict.rs:75-111 | TerminalClaim.seq identifies the actual terminal event; use its timestamp rather than latest event/poll time. |
| surge-ui/src/app.rs:1149-1207, 1937-1998 | Existing cold journal read/hydration and explicit history-unconfirmed path. |
| surge-ui/src/app_state.rs:312-343 | Shared needs-you count and inspection queue; keep Inbox/badge agreement. |
| surge-core/src/bootstrap_operation.rs:304-355 | Typed operation phase, result and monotonic revision remain operation facts. |
| surge-ui/src/app_tests.rs:105-145 | Real SurgeApp::new_shell fixture permits an outer App update regression without a daemon or provider. |

Trusted history requires RunStarted at seq1, continuity and the appropriate
startup/ownership/graph checks already owned by these modules. Do not tighten
general core replay merely to repair this UI consumer. A terminal registry label,
operation result, contiguous suffix or matching declared hash alone is insufficient.

Core/persistence/daemon execution contracts remain unchanged. One bounded public
Rust read-only inspection companion is proposed below; this is not purely UI
source closure and requires its negative tests and API review. App owns
operation polling/index/navigation; AppState owns run evidence and queue selection;
Inbox renders the returned typed evidence. No new dependencies, crate, unsafe
code, public IPC protocol or scheduler policy are needed.

## Selected narrow design

### Operation refresh never mutates run lifecycle

update_bootstrap_view stores the operation snapshot/project relation/index only.
It must not create a UiRun, overwrite status/timestamps, invent a run stream, mark
a run live/dead or choose a run lifecycle from operation phase/result.

Validate returned operation_id against the requested identity. Preserve the
monotonic revision and immutable planning/implementation relationship: stale or
conflicting responses cannot regress the current operation or redirect navigation.
Exact repeated snapshots are idempotent.

An operation phase may select a progress label or suggested navigation anchor;
it never grants Working/Done to that anchor. Terminal operation details use their
typed result/reason while each related run keeps its own trusted display.

Separate operation-poll subscription bookkeeping from real run-subscription
bookkeeping. The operation timer must not set a per-run live flag and real-run
pruning must not silently cancel/restart operation polling. Reuse existing owner
fields/loops; do not introduce a new background subsystem.

### Hydrate related real runs without fabricating reserved runs

Operation recovery schedules bounded read-only hydration of both associated run
identities, including the completed planning parent while implementation runs.
Do not rely only on phase-selected ID or daemon ListRuns, which describes hosted
runs rather than all durable history (app_state.rs:260-280).

Coalesce hydration through ONE current owner per actual run, shared by operation
reads and the actual live pump. The generation/watermark protocol below is
mandatory; project identity and an in-flight boolean alone are insufficient.
Unchanged terminal snapshots do not reread all history every two seconds. Retry
current read failures through this owner and the existing backoff path.

Refactor the existing history application path only enough to allow authenticated
operation-related journal reads before a UiRun exists; hydrate_run_history currently
returns early when no UiRun exists (:1960). A valid actual journal can add its own
row and trusted stream. A reserved identity with no journal remains an operation
reference, with no fabricated UiRun/session/terminal/time or failure card. If a run
was independently observed started but its journal cannot be read/validated, retain
the explicit Unknown/inspection path.

Keep real parent and child streams/history reachable when live ListRuns is empty
or omits the parent. sync_run_subscriptions must retain required operation-related
history instead of pruning it merely because it is absent from hosted rows.
Actual subscription live/error flags still belong only to actual run pumps.

For cold restoration, registry summaries provide lookup metadata. Publish terminal
lifecycle only from the trusted journal display; registry-only Failed/Completed/
Aborted with missing/corrupt history stays Unknown. Preserve authoritative
terminal-over-stale-registry behavior. Where cold restoration sets an end stamp,
select the matching TerminalClaim.seq envelope, not events.last() (:1176): a later
ForkCreated/audit append cannot change when the run ended. Missing/invalid actual
timestamps remain unavailable; do not use polling time as a replacement.

### N7-SEC-1 / N7-ARCH-01: hydration generation and nonregression

begin_display_history (:252-277) clears trusted fold, sessions and pending
decisions. Never call it on published state merely because an independent async
operation read returned: a prefix N may arrive after live N+1..M or after a
same-run reconnect. A stale error/finalizer can be equally destructive.

Each actual run has one current hydration/pump owner in AppState/App bookkeeping.
Its private token binds runtime home, project root/Git scope, monotonically
increasing project generation, exact run ID, current pump/owner generation and
read generation. Operation reads join/request work from that owner instead of
starting another fold publisher. A live pump replacing an offline owner, reconnect,
home/project switch or explicit retry owner replacement revokes the old token.
Project A to B to A receives a new generation; equal paths/run IDs cannot revive
an old callback. Use existing HistoryScope generation discipline as the model,
without making ProjectHistory and live-stream owners competing publishers.

Every success, error and finalizer checks the complete token against the current
owner before ANY mutation, including invalidation, clearing pending decisions,
live=false, subscription removal, retry scheduling and in-flight cleanup. An old
finalizer may finish its own task but cannot remove a replacement owner's marker
or subscription. Old errors are ignored for current display, not applied as a new
Unknown state. Operation status callbacks additionally require current operation
identity/revision and captured project/home generation.

Selected publication policy is reject/retry older prefixes, not local-tail merge.
Build and validate replay in a detached candidate first. At publish, recheck token
and compare candidate through-seq with current trusted display seq and pending
decision watermark. If candidate is below either, discard it without touching
current fold/sessions/decisions and schedule a current-owner retry. Do not accept
it and let finish_display_history invalidate the newer state afterward. Live
events can continue updating the published fold while a read is in flight.

A candidate at/above current watermarks must also match the already trusted
prefix identities/content through the retained watermark. Use bounded retained
event identity/digest evidence in the existing stream owner if needed; equal
sequence numbers alone are not compatible-history proof. On a current-token
conflict, publish explicit inspection/unconfirmed evidence and retire pending
actions rather than silently choosing either history. A stale token never performs
that invalidation. Commit candidate display/session/decision state atomically in
one UI update, preserving the current owner's live/error flags and bookkeeping;
do not overwrite those flags from a detached default stream. Decision watermark
does not regress. Existing origin/continuity/ownership checks remain required.

This is the causal shared hydration closure needed by the new operation-related
reads. It does not authorize unrelated global timestamp or Fleet redesign.

### N7-ARCH-02: cold offline operation facts have a real source

recover_bootstrap_operations (:574-597) currently needs a daemon facade. The
desktop index contains only operation_id/project_path, not reserved run IDs,
result, reason or authoritative project ownership. It is a lookup hint only.
Do not treat it as an accepted record, supply operation_id to a for-run lookup,
or invent reserved run IDs to make cold recovery work.

Reuse project_lifecycle/loading.rs:67-112 and run_mission/history.rs:76-100 for
checked registry ownership and pre/post operation recheck. Existing
Storage::inspect_existing_bootstrap_operation_for_run (inspection.rs:161-177)
and bootstrap_operations::read_record_for_run (:623-643) use the full existing
checked decoder, including stored payload/capture/intent/continuation integrity.
When a genuine run summary is available, this existing lookup is sufficient.

For an indexed operation with no run yet (pre-launch NeedsAttention or cancellation),
add a bounded read-only by-operation-ID companion in the SAME inspection module,
reusing bootstrap_operations::read/decode (:646+) rather than duplicating SQL
decoding in UI. This is an explicitly proposed public Rust inspection surface;
no execution protocol endpoint or new storage owner is introduced. Open with the
same RetainedConnection::read_only and checked existing namespace/object-identity
policy as the current counterpart, with no create/migrate/rekey/discovery writes.
Read one exact operation and bounded payload; no unbounded operation-table scan.

The cold resolver captures home/project generation and original registry
namespace/object observation identity. Missing registry/record is explicitly
absent; invalid/inconsistent record is corrupt/unavailable; unsupported payload
remains typed uninspectable. These outcomes produce an operation-inspection item
without fabricated run relation, outcome, phase, timestamps or navigation target.
Do not convert corrupt into absent or trust the desktop index's project path.

An available supported record must match requested operation ID and recorded
repository/Git common-directory project ownership, using existing resolver rules.
Only that checked record supplies planning/implementation IDs and operation
phase/result/reason. Validate each real journal through the existing scoped
history inspection. Retain missing reserved journals as absent references, not
real started runs. Lookup alone grants neither execution authority nor permission
to redirect navigation to unvalidated foreign records.

Registry and journals are separate observations: after bounded journal reads,
recheck the exact operation record/revision/immutable identities against the same
original registry namespace/object observation. A replacement of that namespace/
object, changed record or home/project generation discards the assembled snapshot
and schedules a bounded current-owner retry; current corruption becomes explicit
inspection. Reuse existing checked-opening identity facilities; the new companion
must preserve their policy, not merely reopen an arbitrary same-path SQLite file.
Live run publication still obeys the per-run token/watermark protocol above.

Invoke this cold read path before/independently of daemon connection attempts.
It must publish supported operation reasons and both validated histories when
the daemon never connects, including after relaunch. Connecting later can update
the same operation monotonically, not replace valid offline facts with index
guesses. Project registry paging/discovery stays bounded with existing cursors;
exact indexed operation lookup does not claim full-project coverage. No daemon
or storage writer is needed to read these facts.

### Inbox consumes trusted run facts and separate operation problems

unacknowledged_failures uses each run's existing trusted RunDisplayState::Done
with Failed/Aborted, not UiRun.status. Inbox derives its failure badge/headline from
that same selected fact; it must not reclassify using the coarse field after the
filter. Completed planning is absent from failure triage even if an old cached
UiRun.status is Failed. Unknown/corrupt history appears as inspection, never a
failed run. Keep the shared badge/needs-you count on the same selection result.

Removing synthetic failure rows must not hide legitimate operation-level problems.
Represent operation NeedsAttention and operation failures without a corroborating
failed run as an operation-sourced Inbox item: stable operation identity, typed
phase/result/reason and related run links where actual history is available.
Examples include BudgetExhausted after completed planning and pre-launch attention.
Do not label that card as a failed planning/implementation run.

Reuse existing read-only OpenRun navigation for actual recorded related runs.
When none exists, the operation card still shows its reason and existing operation
context; do not manufacture a run just to make a button work. Operation observation
does not add retry/cancel authority or a new continuation action. Existing reviewed
Bootstrap controls retain their exact operation/revision semantics.

Avoid duplicate triage for one attributed failure: if a typed operation RunFailed/
RunAborted result is corroborated by that exact run's trusted terminal, show the
real run card with operation context, not a second operation-failure card.
NeedsAttention and Unknown inspection referring to the same run may share one
operation-owned inspection item, preserving both reasons and source navigation.
Do not suppress unrelated run inspection, pending human decisions or historical
failures. Cancelled operation alone creates neither failed-run triage nor Aborted
facts; an actual RunAborted journal remains independently eligible for triage.

## Alternatives

1. Selected: operation-only update plus trusted run Inbox and bounded related
   history hydration. Fixes the causal path and keeps operation problems visible.
2. Remove only run.status assignment. Too shallow: synthetic inserts/timestamps,
   lost cold parent history and NeedsAttention visibility would remain.
3. Map by result.run_id. Rejected: BudgetExhausted/reserved/cancelled cases still
   manufacture run lifecycle.
4. Redesign UiRun/global-event time model or migrate every screen. Deferred:
   broader than N7 and not necessary if existing trusted display is the sole
   lifecycle authority for this queue. No new status enum is required here.

## Actual outer RED and verification plan

No tests have been written or run for this proposal. After explicit sixth-round
authorization, first add a regression through SurgeApp::update_bootstrap_view and
the real Inbox selection/rendering seam, not merely a new pure helper.

Use the existing App new_shell/GPUI test fixture with an isolated temporary
desktop-index sink and controlled trusted parent/child journal envelopes. Do not
write the user's SURGE_HOME or start a daemon/provider. The production update path
and production Inbox items must run; a test-only alternate mapper is insufficient.
Use the existing load_bootstrap_index_at/save_bootstrap_index_at file seam. Resolve
the production index location once for the App instance; the test supplies its
temporary index location to that same owner. Do not mutate process-wide SURGE_HOME
or replace update_bootstrap_view with a test-specific implementation. This is a
private constructor/dependency seam, not a new public persistence interface.

RED fixture: parent Completed, child Failed, operation Failed/result child RunFailed.
Call the actual App update, navigate/render Inbox and assert exactly the child
failure identity/badge and no planning failure. Also assert parent UiRun lifecycle
metadata/timestamps are unchanged by the operation update. This must fail against
the present app.rs path before production edits. Set no observation times from the
test clock as a surrogate for journal facts.

Required cases through the same outer seams:

- Completed parent + implementing child: operation progress remains implementation;
  parent remains Done and child only Working with its own attributed observation.
- Planning failure vs implementation failure: exact failed run identity and one
  failure item; stable operation relation/navigation.
- NeedsAttention with absent reserved run, and with an actual nonterminal run:
  operation attention visible; no synthetic run Failed/Aborted/time/session.
- BudgetExhausted after planning Completed: operation failure visible; planning
  remains Completed and never enters failed-run triage.
- Cancelled before launch and after one actual aborted run: no blanket run Aborted;
  actual independent terminal remains visible.
- Completed/Failed registry or operation result with absent, malformed, gapped,
  bad-origin or contradictory journal: Unknown/inspection, not terminal success
  or failed-run card. Terminal evidence overrides stale registry labels.
- Stale operation revision, mismatched operation identity and changed associated
  run IDs: no snapshot regression or navigation redirection.
- Interleave global RunAccepted/RunFinished, ListRuns and repeated operation polls
  in both orders. The operation cannot overwrite run evidence or multiply pumps.
- Cold restart/index recovery with no hosted runs: both real journals load,
  planning completion and child failure persist, operation reason remains visible,
  reserved absent run does not become a real row. History still works offline.
- Terminal followed by harmless lineage event: terminal timestamp remains that
  terminal event's timestamp, never poll/latest-event time.
- Inbox needs-you badge/count, failure/inspection dedup, dismissal and exact run
  navigation agree with the same evidence; unrelated pending decisions remain.
- Pause an actual related-run read at prefix N, then process live terminal or
  decision events through M>N. Releasing old success/error/finalizer preserves
  terminal/pending/session state and cannot clear a replacement subscription.
  Verify retry of an older compatible prefix and explicit handling of a current
  conflicting prefix; no stale begin_display_history mutation is allowed.
- Replace a same-project/same-run pump or reconnect while a read is delayed;
  release its completion, error and finalizer separately. Only the new generation
  can publish or clear ownership. Repeat project A to B to A and a home change.
- Construct a new App with daemon connection permanently unavailable and real
  durable parent/child journals plus checked registry operation. Offline Inbox
  and navigation expose parent Completed and child Failed; operation-only
  BudgetExhausted/NeedsAttention remain correct, including a no-run-yet operation.
- Offline lookup with absent/corrupt/unsupported operation, forged index project,
  foreign capture, changed pre/post operation revision and replaced registry
  namespace/object: explicit absent/unavailable inspection, no invented relation
  or navigation. Delayed old-project/offline callback cannot populate new scope.
- Read-only API negative tests: absent registry never creates it; old schema is
  never migrated; no writer/opening/operation/private-object creation or rekey;
  rejected symlink/object/namespace substitution follows current inspection policy.
  Existing for-run counterpart behavior and bounded paging stay compatible.

Run targeted AppState/App/Inbox/cold-hydration tests, affected existing UI tests,
strict UI Clippy for required feature combinations, formatting and diff checks.
Use the established main integration builder/Cargo ownership protocol, not a
second concurrent build. Independent 5a and focused quality/domain/security 5b
must review actual source and evidence. Record exact commands, failures and scope.
Native replay on the durable failed fixture follows code gates and requires a
current-source bundle; never rewrite its journal to obtain the expected UI.

## Causal file closure and adjacent work

Expected sixth-round source closure:

- app.rs: operation update/poll bookkeeping/index recovery, related-run hydration,
  per-run owner generations, truthful cold/offline restoration and run-history
  retention; app_tests.rs outer RED and controlled asynchronous boundary tests.
- app_state.rs: trusted failure/inspection selection and shared count, operation
  attention/failure selection/dedup, current hydration-token ownership and atomic
  nonregressing publication; existing state tests.
- run_stream.rs: minimal trusted terminal/time/watermark/prefix-compatibility
  access and detached replay publication; reuse origin/continuity/display owner,
  no parallel fold or caller mutation before generation/watermark validation.
- screens/inbox.rs: operation source identity/labels, canonical terminal selection,
  exact related navigation, no synthetic times and outer GPUI regressions.
- surge-persistence runs/inspection.rs and bootstrap_operations.rs: bounded
  read-only by-operation-ID companion using the existing checked opening/decoder;
  any minimal observation-identity plumbing must preserve current source namespace
  policy. Explicit public Rust API review plus no-create/no-migrate/substitution
  negative tests. No persistence execution/launch semantics change.
- project_lifecycle/loading and run_mission/history ownership/recheck rules are
  reused for the App cold resolver. Do not fork their integrity checks into a
  more permissive desktop-index adapter. Any required extraction is part of this
  named read-only closure, not permission for broader screen refactoring.
- The existing Runs/mission/header/loading adapters are dependencies to verify,
  not blanket authority to reopen their separate Results repair. If a new
  required write there appears, name it and include it explicitly in the sixth
  round review/mandate before dispatch rather than borrow Results' budget.

No new core/persistence/daemon execution behavior, task continuation, sandbox,
verification receipt, scheduler, provider observer or global lifecycle protocol.

Adjacent pre-existing issues remain explicit: apply_global_event currently uses
receipt-time starts/ends and Active stubs for unknown outcomes; Fleet reads coarse
UiRun status; broad global timestamp/source-model cleanup is not included in N7.
Do not claim this round fixes them or proves every desktop lifecycle surface.
N7 must introduce no new such timestamps and must remove its operation-generated
ones. If a targeted outer regression demonstrates an adjacent write is essential
to this fix, reshape the proposal before requesting authorization.

## Maintainer verdict, risks and approval boundary

Owner pre-code verdict: ACCEPTABLE for focused independent re-review of the full
UI plus bounded read-only persistence closure. Existing trusted
display/inspection owns run truth; operation status owns progress/problems. The
fix earns its depth by removing the duplicate lifecycle mapper and reusing the
same queue evidence for labels, counts and navigation.

Material risks: accidentally hiding operation-only errors after removing phantom
runs; losing completed parent history during subscription pruning; callback races
overwriting newer revisions; duplicate triage; cold registry-only positives;
tests validating a helper while the real App path remains wrong. The outer cases
above are required gates, not optional coverage.

Review dispositions proposed, not verified implementation:
N7-SEC-1/N7-ARCH-01 are addressed by one exact-run hydration owner, full-generation
token checks on success/error/finalizer, detached candidate replay, prefix identity
compatibility and nonregressing display/decision watermarks with delayed-reader
tests. N7-ARCH-02 is addressed by checked offline registry facts independent of
the daemon, the explicitly scoped operation-ID companion, pre/post identity and
revision recheck, and offline/absent/corrupt/read-only negative tests. No review
finding is closed until the independent reviewers accept this revised contract.

Independent pre-code critic/security review and explicit human sixth-round scope
authorization are still required. Home remains 5/5 until that authorization; no
implementation dispatch is permitted now. The full app goal, continuation from
all required stages, two milestones/eight tasks, follow-up/existing-project and
current platform/Windows acceptance remain open.

## Independent pre-code re-review and authorization request

Reviewed proposal body SHA-256:
`51d91d8076e5f02ddf277081f263b83c9a790b682323597ffc47d2634600ac09`.
Root maintainer, continuation_plan_critic and continuation_plan_security:
**ACCEPTABLE for design**, not implemented behavior. N7-ARCH-01,
N7-ARCH-02 and N7-SEC-1 addressed at the plan level. Implementation must verify
actual retained registry identity across reopening, bounded payload before JSON
allocation/decoding, obsolete error/finalizer refusal, no-create/migrate/rekey and
whole-registry substitution, plus authentic App-to-Inbox behavioral RED.

Root requested explicit sixth-round authorization through the user question
surface. **Pending response; Home remains 5/5. No implementation dispatch.**
Independent Results and main integration may continue. No push, PR or remote merge
is included in this request.

## Explicit revised user mandate

User answered: "Да, разрешаю шестой раунд". Sixth-round implementation,
verification and independent review of the approved full UI plus read-only
persistence closure are authorized. Publication and remote merge are excluded.
This supersedes the pending-response status above. Dispatch count remains five
until the sixth author dispatch is recorded; maximum is now six, with no
automatic seventh. Results repair 3 runs first to keep shared projection/Cargo
ownership sequential; this does not reduce the approved N7 scope.

## Sixth-round pre-dispatch reservation

The sixth slot is reserved; five dispatches have occurred. Record **6/6** again
immediately before the actual author handoff. Original identity remains
desktop Home/Inbox; no reset, automatic seventh round or remote publication.
Source/destination is codex/results-planning-history, same worktree. At reservation
HEAD was c2001306 plus frozen uncommitted Results closure c7193d6a. The user then
authorized the narrow fourth Results repair before this author was handed off;
complete that repair first and bind the sixth handoff to its resulting tree.
No sixth author handoff or Rust write has occurred. No budget reset is implied.

User's explicit sixth-round answer authorizes the full approved App/AppState/
Inbox/run_stream and bounded read-only persistence inspection closure. First prove
actual App-update-to-Inbox RED with private index-location injection only; then
implement the complete offline/generation/terminal/operation-problem contract.
Independent 5a then full generic/domain/security/API 5b required before closure.
Author may update this record/evidence, not the frozen Results files, and must
release the sole Cargo lease for final review. No commit or publication by author.

## Actual sixth-round author handoff

Dispatch **6/6** recorded before author handoff. Source/destination is
codex/results-planning-history in this worktree, Results source commit
`c68c6fb9`. Results4/4 independent5a and all affected5b are closed; source is
committed. This supersedes the earlier reservation/queue status, without resetting
either task. User has explicitly authorized this sixth round and reaffirmed the
sequence: close Results, execute Home/Inbox, then continue the same real habit
tracker through codex-acp/gpt-6.1-sol with its history and all eight tasks intact.

Implement the complete reviewed UI plus read-only persistence closure, with actual
App-update/Inbox RED first and controlled offline/generation/publication tests.
No Results source change unless a new concrete required dependency is named and
reviewed first. No raw terminal resume, provider/fixture/runtime edits or external
publication. New continuation implementation remains separate and open; neither
synthetic tests nor this UI fix completes the real app.
