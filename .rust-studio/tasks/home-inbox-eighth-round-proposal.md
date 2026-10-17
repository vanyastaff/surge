# Home/Inbox: proposed eighth repair after full Phase 5b

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

Status: PLAN ONLY, reshaped for H8-PLAN-01/H8-SEC-PLAN-01 and H8-ADV-01;
maintainer pre-code verdict ACCEPTABLE for focused independent critic/security
re-review. Previous reviewed bytes are retained in ignored evidence
`home-inbox-eighth/plan-v1.md`, SHA-256
`c0681b17eb8fe962d48edf7248e7044d0cca5d173e32acb84c09980f109de6e2`.
Home is exhausted at 7/7. No eighth authorization or
dispatch exists. Only this new proposal may be written during this task: no Rust,
tests, Cargo, runtime/provider/index/memory action, commit or publication.
This is the same Home/Inbox task and its existing budget, not a new task/reset.

Frozen review baseline: HEAD `6d23ebf069fdaa2f3762c8b540c5f669b965dec8`, Rust digest
`0e508e67bca7a6d7e26f928acb3db53afdf57f92654dae5f79cffbe60fef60ff`;
`.rust-studio/evidence/home-inbox-seventh/freeze-manifest.json` SHA-256
`e0d67dc0f754c46c1e04cf4a77d99d3c5d36d1a864f83b86556086e40a1cb056`.
Original contracts remain in bootstrap-run-status-sixth-round-proposal.md and
home-inbox-seventh-round-proposal.md. Their frozen reviewed bytes and evidence
are retained; do not rewrite chronology or reuse green receipts as proof of these
new cases.

Full review receipt is `.rust-studio/evidence/home-inbox-seventh/phase5b-review.json`;
reviewed task snapshot SHA is
`f9d86f72565e16e2d97092ea103fb08f9aaa5dc12743abe3d9805a0843059606`.
Seventh Phase 5a is ACCEPTABLE: H6-SPEC-01/02 closed, ten focused cases and 367 unit
plus five integration tests green. Full generic/domain/security Phase 5b is NEEDS
WORK with the four blocking P2 findings below. They are source-traced findings;
NO new behavioral RED has yet been executed for them.

## Actual causes and protected invariants

| Finding / current producer and consumer | Protected input/invariant before changing it |
|---|---|
| H7-GEN-01: app.rs:1332 seeds independent registry Crashed; run_stream.rs:367 replaces the stream with a detached candidate lacking registry_status. Canonical display/inspection loses RecoveryRequired for nonterminal/parked history. | Durable replay cannot erase independently observed current registry evidence. Registry Crashed is recovery evidence, never terminal proof. Trusted terminal still wins; candidate data cannot forge a fresh registry observation. |
| H7-GEN-02: app.rs:880 claims a live owner before subscribe_to_run succeeds; rejection at :905 calls finish_run_pump, retiring it. The earlier offline owner/read is obsolete, with no replacement history recovery. | One publisher per exact run; obsolete owner stays obsolete. Subscribe rejection is not RunFailed or proof of process loss. No attach_live_owner before SubscribeOk and no revival of a revoked token. |
| H7-SEC-01: app_observation.rs:253 drops a coalesced newer request; stale publication/error branches :289-320 release the old marker without servicing current R+1. Equal snapshot at app.rs:588 and terminal poll stop at :523 leave no later producer. | Coalescing prevents parallel operation batches but must preserve current pending work. Stale data cannot publish, invalidate or spend a newer retry budget; stale settlement must not discard a newer accepted request. |
| H7-DOM-01: app_state.rs:388 suppresses run inspection for NeedsAttention; Inbox :523-548 substitutes only operation/read-owned reasons and requires currently confirmed history for its related link. Actual StreamError/host-loss reason and the existing standalone inspection route disappear. | Dedup may suppress standalone inspection only when BOTH reasons and available navigation are preserved by the operation item. Otherwise retain canonical standalone inspection; no new historical navigation authority. |

Preserve all existing exact Scope/run/owner/read/error-revision checks, detached
prefix/timestamp/decision-watermark validation, history-vs-live diagnostic guards,
current related-read completion dispositions, operation-record integrity rechecks,
completed-owner exclusion and initial-plus-three bounded retry episodes. Never
remove an equal-snapshot guard or reset retry counters to force progress.

## Selected coherent repair: current owner, pending demand, truthful inspection

Use the current private UI observation owners and completion paths. No framework,
executor, storage owner, public API, IPC protocol or alternate replay is proposed.
The following are cooperating lifecycle fixes, not four independent UI masks.

### H7-GEN-01: preserve independent evidence during atomic candidate publication

publish_candidate already preserves current live/error provenance and decision
watermarks. After validating the candidate and while publishing under the current
run token, preserve the current independently supplied registry_status too. Do not
derive it from candidate events or operation phase/result. A detached candidate
cannot clear Crashed simply because it was constructed with no registry hint.

The current hint is read at atomic publication, not captured from an earlier async
request. A genuine later registry/start observation updates it through its existing
owner (for example daemon-confirmed mark_started); subsequent replay preserves that
new value instead. Do not pin Crashed forever or clear it on connection alone.
RunDisplayState remains the sole classifier: trusted terminal dominates stale
registry hints; nonterminal/parked history with Crashed remains RecoveryRequired;
unknown/corrupt history is never promoted to Working.

### H7-GEN-02: rejected attachment must leave a current history recovery owner

Keep the existing token reservation before SubscribeOk if needed for exclusivity;
do not resurrect the old offline owner. For a CURRENT rejected subscription,
transition that reserved owner to offline/history recovery under the SAME exact
scope/run/owner generation, through a private token-checked Observations method.
The transport never attached: do not call attach_live_owner, claim live activity,
clear a current actual StreamError or invent a terminal/registry event.
The present SubscribeErr branch logs and retires; it does not emit StreamError.
Use the existing read-owned restoring/error provenance for this fallback instead
of introducing a global connection-error policy or manufacturing a stream event.

Perform/request a fresh durable history read under this current owner, using the
existing read/candidate/completion path. This request must survive any coalesced
older operation batch and cannot rely on another ListRuns/global event or changed
operation revision. RunNotActive after the run finishes between ListRuns and
Subscribe is specifically recoverable from its durable terminal journal.

Rejecting an obsolete subscription callback does nothing to current state. A
current failure releases only its own subscription marker, while retaining a
current offline owner long enough to complete/retry its read. Do not use the
existing retire-and-return finalizer for this transition. Current finalizers
settle their real tasks and cannot remove a replacement subscription/owner.

Reuse bounded history backoff and the existing current-owner retry rules. For an
operation-related run, the operation demand/retry owner may service the request;
for an ordinary run, reuse the same per-run read/candidate/completion primitives
with an explicitly bounded fallback episode, never an invented operation
association. The attached live loop at app.rs:929-960 has UNBOUNDED hydration
retries: do not reuse that loop unchanged as rejected-subscription fallback.
Add an initial read plus at most THREE automatic retries with existing
history_retry_delay, current token/read-generation checks and retained exhaustion
diagnostic. A minimal private backoff/episode helper may be shared with operation
recovery; no new poller/global connection policy. Current automatic fallback
demands coalesce into that episode and never reset it. At most one current read
is active. A successful new live
attachment can replace that recovery owner; its old completion/timer is then inert.
Exhaustion retains an explicit current inspection diagnostic, not an Active,
Failed or Aborted guess. It does not reset the seventh-round retry policy.

### H7-SEC-01: preserve pending current demand through actual batch settlement

Replace drop-on-coalesce behavior with a small private pending-demand slot keyed
by current Scope and operation, retaining the latest accepted operation snapshot/
revision and a monotonic request identity. Repeated equivalent requests coalesce
to one pending demand; newer R+2 supersedes R+1. This slot holds requests, not
unchecked records, run authority or a second worker. The active batch retains its
own identity so an old finalizer cannot remove a newer batch marker.

When an active batch exists, hydrate_related records the current demand rather
than silently returning. All settlement paths, including stale Ok, stale Err,
current failure, cancellation and normal completion, go through one private
settlement operation. It releases ONLY the matching actual batch after its work
has settled, then atomically selects current pending demand for service. Scope or
request-generation mismatch discards obsolete pending work. A→B→A never revives
the older demand or marker even with equal paths/run IDs/revision.

Service current demand by starting one fresh checked operation inspection through
the existing owner and original registry recheck. Do not publish old R as R+1 or
accept the old histories. Current live/completed/in-flight run exclusions still
apply. A pending request blocked by an in-flight current run read is retained
until that actual read settles; it is not lost because claim returned None.
This also services the explicit fresh-history demand after rejected attachment.

Old completion remains neutral for new diagnostics/retry counters. Servicing a
pending same-episode read consumes its existing bounded retry allowance where
applicable; it cannot reset exhaustion. A genuinely newer accepted operation
snapshot uses the already-defined new snapshot episode. The active batch's own
failure and a pending demand must not each schedule a parallel retry: retain one
current episode/request and one timer, preserving both current failed siblings.
Equal snapshot polling remains idempotent, terminal operation polling may still
stop, and progress no longer depends on either poll restarting.

### H7-DOM-01: conditional merge; preserve canonical standalone inspection

Centralize selection of current operation and related-run inspection facts in
AppState. Merge typed operation phase/reason, current run display/next action,
actual observation_error/StreamError, recorded host-loss detail and current
read-owned diagnostic without replacing one provenance with another. Attribute
reasons to their operation/run; deduplicate identical text, not distinct facts.

Suppress standalone run inspection ONLY when the operation item includes BOTH
its current inspection facts AND the already available current eligible exact
run link. Use the same current related-link selection already consumed by Inbox
operation items, with current project/run filtering. Extract that PRIVATE
selection if necessary so AppState suppression and Inbox construction agree.
If history invalidation makes session_history_confirmed false, do not suppress
the otherwise eligible canonical standalone inspection. Two items are then
correct: operation problem and unconfirmed run inspection, each with its own
facts. The original contract allows supersession only when no reason/navigation
is lost; it does not demand one unconditional card for an unmergeable unknown.

Standalone inspection already has an exact-run source: inspection_runs iterates
existing run_streams, filters run_in_project, selects Unknown/RecoveryRequired
and preserves the pending-decision duplicate rule. Inbox :505-519 constructs
Source::Inspection { run_id }; its existing OpenRun handler (:742-743/:1401+)
selects THAT ID without execution authority. App::open_run_cockpit requests that
selection. Run mission history independently checks current operation/project/run
ownership (history.rs:124-135), origin, graph and source; unavailable/foreign
history is displayed as such. Preserve this established inspection route instead
of creating a new historically authenticated navigation link.

Include the CURRENT actual run error and recorded host-loss detail in standalone
inspection evidence, not just generic Next action. A valid merged item contains
that same evidence. Shared needs-you counts and rendered selection use the same
conditional merge. Pending human decisions remain separate; merging grants no
answer/retry/continue authority. A canonical Unknown inspection link is an attempt
to inspect an unconfirmed run, not proof of its current namespace, ownership or
outcome; do not label it Open recorded if only inspection is available.

A never-started reserved identity does not gain a stream or UiRun from read_history
None/accepted Absent; therefore operation IDs alone create no standalone link.
Known foreign paths are excluded by run_in_project. Its unknown-path fallback
intentionally retains just-accepted/unconfirmed streams; it is NOT authenticated
ownership. Such inspection remains read-only and the loader rejects mismatched
records. No new operation-related link is created for an unvalidated reserved or
foreign stream. Preserve and test these existing distinctions, not a stronger
identity guarantee the current interface does not provide.

No retained once-confirmed bit, registry/run object identity certificate or new
navigation capability is introduced. The previous plan incorrectly assumed that
UI seam existed: read_history returns paged Vec<ReadEvent>, Bootstrap observation
covers only registry recheck and RetainedConnection is private. Selecting the
existing standalone route removes that known persistence/API dependency instead
of deferring it to an author. No new native UI identity owner is proposed.

## Actual RED first; no new RED has yet run

After explicit eighth authorization, add failures against the frozen seventh
source through actual App/cold/IPC/Inbox producers. Positive preconditions must
prove real registry/journal identities, current owner and scope before the failing
assertion. Test setup/compile/teardown failures are not behavioral RED.

1. H7-GEN-01: actual cold App read of independently registered Crashed run plus
   valid nonterminal journal. Publish its candidate and assert RecoveryRequired
   inspection survives. Include a valid capacity-parked journal that would otherwise
   become WaitingForCapacity. Control: a genuine later registry/start observation
   supersedes Crashed without inventing Working; trusted terminal still wins.
2. H7-GEN-02: pause actual offline read, list an Active run, then make the real
   controlled IPC SubscribeRun reply reject/RunNotActive after durable terminal
   publication. No later daemon/global/operation change may rescue the test.
   Current fallback must recover that journal and retain truthful Inbox history.
   Release the old read and prove it stays obsolete; no owner resurrection,
   duplicate pump, false live flag or damage to current StreamError/finalizer.
3. H7-SEC-01: pause actual operation R inspection, accept R+1 requiring real child
   history and call its normal hydration producer while R is active. Cover BOTH
   old Ok rejection and old Err rejection. After old settlement and without another
   poll/update/manual hydration, R+1 history must load. Cover terminal R+1 with no
   live run, R+2 coalescing, bounded current failure, same-episode exhaustion and
   A→B→A late success/error/finalizer. Count actual reads/owners/timers.
4. H7-DOM-01: real App -> rendered Inbox with NeedsAttention(StorageUnconfirmed)
   and related current actual StreamError/live state. Assert both operation and
   run reasons and one coherent count. Invalidate previously corroborated started
   history with a real prefix conflict: actions remain retired/Unknown and the
   canonical standalone inspection/OpenRun attempt remains instead of an invalid
   merge. Include pending-decision preservation,
   recorded host loss, absent reserved and foreign-prefix controls; no fabricated
   run navigation or lifecycle.

Navigation controls distinguish a current eligible merged operation link,
invalidated history requiring canonical standalone inspection, absent reserved
identity and known foreign project. The loader must still report current source
unavailable/foreign as appropriate. No exact-original namespace identity guarantee
or historical authentication claim is made by the standalone inspection route.

Ordinary rejected-subscription fallback permanently failing must execute exactly
initial plus three automatic reads, retain exhaustion/inspection, and stop. Count
actual reads with no operation association or daemon event rescuing the test.
Owner replacement, stale timer and A→B→A cannot restart the exhausted episode.

Also interleave pending operation demand, subscription rejection and owner
replacement in one controlled scenario: one current publisher services the request;
older batches/errors/finalizers cannot clear its marker or retry episode. Preserve
all ten seventh cases and original sixth assertions, including same-owner live
error provenance, bounded sibling retries, timestamp/prefix compatibility and
no-create/no-migrate integrity.

Use existing private test runtime homes, real fixture registry/journals and
controlled IPC/subprocess seams. Explicitly settle actual children, workers,
readers and pools before fixture cleanup. No serial mask, skip, weakened assertion,
timeout inflation or real provider/habit-runtime mutation. This plan runs none of
these tests or actions now.

## Complete narrow source closure and gates after authorization

Private surge-ui only: app.rs subscription failure/handoff and current request
production; app_observation.rs batch identity/pending-demand settlement and bounded
retry integration; app_state.rs merged inspection selection; run_observation.rs
minimal token-checked offline fallback/current demand support; run_stream.rs
registry-hint preservation and current inspection-reason access;
screens/inbox.rs consumption/rendering; existing App/observation/cold/recovery
subscription/private fixture test files. Extract a private helper only to keep
these producers on one owner path, not a new framework or public interface.

No persistence API/source/schema, Results projection, scheduler, verification
receipt, sandbox, witness or continuation implementation change belongs here.
Any concretely necessary extra dependency requires a reviewed plan amendment
before author dispatch. Do not patch public API versions or compatibility policy
as a side effect of this UI repair.

Refresh authentic focused RED/GREEN and full UI baseline 367 unit + five
integration plus all added cases, with honest executed/filtered/ignored counts.
Run strict affected default and all-feature/all-target Clippy -D warnings, required
workspace all-target/all-feature check, workspace fmt and diff checks. Bind source,
logs, actual commands and case map to a new freeze manifest with a narrow seventh-
to-eighth diff; earlier green receipts do not cover the new behavior.

Preserve the 144 unchanged persistence/source-input hashes, 605 passed plus one
existing deferred skip and rustdoc exit0/nine retained warnings as explicitly
source-bound REUSE only. If those inputs change, refresh the affected check and
review rather than claiming reuse. Current-source Windows/MSRV/native/product
acceptance remains unverified until actually executed.

After freeze, focused independent Phase 5a must confirm all four stable IDs and
seventh-case preservation. Then affected full generic/domain/security Phase 5b
must accept the combined Home closure. API-GATE is already COMPLETE by manual
review: new exports MINOR, the existing 4MiB inspection refusal MAJOR on paper,
within accepted scope. Reuse that verdict only with unchanged API/source-input
hashes; no API mitigation/source/version write is required or authorized here.
Local stage commit follows root/user workflow only after required reviews; no
push, PR or remote merge is included.

## Alternatives, verdict and user extension boundary

Selected: preserve current independent evidence during publication, retain one
current recovery owner on failed attachment, and service pending current demand
through shared settlement; render one evidence-preserving inspection selection.
This keeps lifecycle/ownership local to the existing private observation module.

Rejected: replay old offline authority after Subscribe failure; blindly mark Crashed
as Running; remove equality/token/coalescing guards; force rereads on every poll;
reset retries when pending work appears; suppress unmergeable standalone inspection;
use reserved IDs as navigation proof; hide run errors behind operation text. These
either reopen previous failures or violate the original accepted contract.

Maintainer-grade verdict: ACCEPTABLE for focused independent pre-code review.
No strategic product choice is open for these four confirmed blockers. The added
information is pending request identity and current inspection evidence, not new
execution/navigation authority. H8-PLAN-01/H8-SEC-PLAN-01 are addressed by
conditional merge with the existing standalone route, eliminating the unsupported
retained-identity feature. H8-ADV-01 is addressed by explicit initial-plus-three
ordinary fallback, not reuse of the unbounded attached loop. Independent
critic/security must accept this revised concrete plan
before root requests explicit eighth-round authorization. Home remains 7/7 until
the human grants that extension and root records the actual dispatch.

Permission option for human review, not authority: instead of approving only
dispatch eight, the revised mandate may raise this SAME Home/Inbox limit from
seven to ten, allowing at most three focused additional dispatches 8-10 for ONLY
these four blockers and their required regressions/review repairs. Dispatch eight
would still start only after explicit human acceptance and root's dispatch record.
This option avoids a new permission question for a mechanical re-review correction;
it does not reset the count, allow new strategic scope, or authorize publication.
Any new scope/strategic dependency still requires reshape/review and human approval.
Until a response, the actual limit remains seven and no extra dispatch is allowed.

The same habit tracker's eight tasks/two milestones, codex-acp with exact
gpt-6.1-sol, follow-up/history, broader continuation, existing-project improvement
and Windows/native acceptance remain unfinished. A repaired UI and green synthetic
tests do not complete that project. No requirement is retired by this proposal.


## Explicit mandate and actual eighth handoff

User answered: "Да, продлить до 10 в этом объёме". The same Home/Inbox limit is
now TEN, allowing at most three additional focused dispatches8-10 ONLY for
H7-GEN-01/02, H7-SEC-01, H7-DOM-01 and their necessary repair regressions/gates.
This is not a budget reset or authority for new strategic scope. No push/merge.

Actual author handoff **8/10** is recorded immediately before dispatch to the
same usable Home implementer. Two further focused dispatches remain if required;
no eleventh is automatic. Accepted design body SHA256
8d8e2322146ed5e2d1b9ecaa54b365a6c0c750b53fa035e5e353ad9069481d6e
is preserved byte-exact in evidence/home-inbox-eighth/approved-plan-snapshot.md.
Owner, independent critic and security each returned ACCEPTABLE on that body.

Source/destination: HEAD6d23ebf069fdaa2f3762c8b540c5f669b965dec8 plus frozen
seventh Rust closure0e508e67bca7a6d7e26f928acb3db53afdf57f92654dae5f79cffbe60fef60ff,
branchcodex/results-planning-history in this worktree. One author holds the sole
Rust/source/Cargo lease until explicit freeze/release. Root/reviewers read-only
for Rust. Local completed-stage commit only after required independent reviews.
Samehabit/all8/codex-acp/gpt-6.1-sol continuation follows CLOSED Home review.


## Eighth author verification and review handoff

Same actual dispatch **8/10**, no budget reset or subsequent author dispatch.
All four accepted blockers reproduced through actual App/cold/IPC/Inbox paths
on frozen seventh production before behavior edits (`outer-red.log`: four
behavior failures plus one registration-only pass). Current source preserves the
latest independent registry hint at atomic publication, performs bounded same-owner
fallback after rejected attachment, settles exact operation batches and drains the
newest current demand, and merges operation/inspection evidence only while retaining
current reasons and existing eligible exact navigation. No persistence/API/Results
execution semantics or new historical-object navigation authority were introduced.
Only seven approved private UI files differ from frozen seventh Rust.

Required combined SEC01 coverage found a record-recovery/pending-settlement lost
wake. Authentic explicit RED is `settlement-episode-red-2.log`; the repair clears
only the recovered record flag and services the settled record-only demand without
resetting attempts, dropping failed siblings, or reclaiming a replaced run owner.
The original full gate 377/378 failure is retained: an older sixth test conflated
an obsolete callback with the newly required fresh current read. Root approved a
test-only barrier migration retaining every original assertion while the current
read is held, then asserting legitimate current refusal and no synthetic run after
release. No production suppression was added. Two cfg(test) type-complexity lint
errors were repaired mechanically with named/reused test aliases; failed strict
receipt is retained.

Final exact-source gates: full UI **378/378 plus 5/5**, zero ignored/filtered;
strict affected UI+persistence all-targets clippy default and all-features both
exit0 with `-D warnings`; workspace all-targets/all-features check exit0; fmt and
diff checks exit0. `ui-full-complete.log` is after the final aliases. Targeted
final filter passed 11 registrations (ten behavioral functions plus a parent
no-op child registration); actual cold child invokes ordinary/capacity/terminal
cases. All ten seventh regression functions remain and pass in the unfiltered
full suite. All failed/setup receipts remain separately classified in evidence.

All 144 persistence/doc input hashes and reused receipt hashes were rechecked.
Prior persistence **605 passed plus one existing platform skip** and cargo doc
exit0/nine warnings are reused, not rerun. Manual API-GATE COMPLETE is unchanged;
automated public-API/semver tools remain unavailable. The sixth 61 and seventh 45
frozen evidence records were verified byte-unchanged. New eighth evidence contains
before snapshots, narrow patch, case map, command/gate history, current source
binding and freeze manifest. Linker unwind-size and dependency future-incompatibility
warnings remain reported in test/check logs; strict clippy has no warnings.

Author BUILD verification is complete; source frozen and sole Rust/source/Cargo
lease released for independent focused5a (all four findings plus introduced repair
regressions) then affected generic/domain/security5b. Review is PENDING, not
self-closed. No author commit/push/merge, provider/native/user habit launch, main
source or memory change. Windows/MSRV/release/native acceptance and the same habit
all8/codex-acp/gpt-6.1-sol history continuation remain open. No ninth dispatch is
authorized by the author.


## Focused eighth Phase5a outcome and authorized follow-on preparation

Independent reviewer returned **NEEDS WORK**, H8-SPEC-01 blockingP2: successful
nonterminal fallback retains a marker that excludes the production RunAccepted
subscription path. The existing manually claimed replacement test does not prove
that path. H7-GEN-01, H7-SEC-01, H7-DOM-01 and H6-SPEC-01/02 are resolved;
GEN02 terminal recovery and bounded errors pass but its introduced nonterminal
transition remains open. See evidence/home-inbox-eighth/phase5a-review.json.

This is within the explicitly approved max10 scope (four blockers and required
repair regressions). No renewed human permission is required. Actual count stays
8/10 while a focused ninth pre-code plan is prepared/reviewed; no ninth author
code handoff has occurred. Phase5b remains gated.
Frozen reviewed task bytes are in phase5a-reviewed-task-snapshot.md; this update
is documentation only. All17Rust and existing frozen receipts remain unchanged.
