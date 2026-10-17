# Home/Inbox tenth repair: shared current-read settlement services renewal

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
critic/security review. Actual count remains 9/10. The existing explicit max-ten
mandate covers this necessary H7-GEN-02/H8-SPEC-01 cross-reader regression; no
repeat user question or budget reset. Root records 10/10 only before author code
handoff after plan acceptance. No eleventh dispatch is automatic. Only this new
proposal is writable during design: no Rust/tests/Cargo/runtime/provider/index/
memory/commit changes or execution.

Frozen ninth input: HEAD `6d23ebf069fdaa2f3762c8b540c5f669b965dec8`, 17-file Rust digest
`053b40c8b8520fb9fb58a94601f7463e0518905c74dd3b36cfe2d72f7e27ccc2`.
Manifest `.rust-studio/evidence/home-inbox-ninth/freeze-manifest.json` SHA-256
`bdee0251c47f265830fa00bc38e08c83eae466e560cba22eda70f134c2429764`.
Baseline UI 384 unit plus five integration; all eighth/seventh regressions retained.
Ninth focused Phase 5a ACCEPTABLE and generic/domain Phase 5b COMPLETE. Security
Phase 5b NEEDS WORK for one P2 H9-SEC-01. This finding is source-traced; its new
authentic behavior RED has NOT yet run. Prior plan/evidence bytes remain frozen.

## Exact missed producer/caller path

Initial actual Subscribe rejects. Fallback attempt zero fails and enters backoff.
A newer accepted operation snapshot triggers related-history inspection. Since
Observations.claim reuses a recovering owner (run_observation.rs:76-79), that
producer starts the next read generation under the SAME owner (:179 in
app_observation.rs). Actual current Global RunAccepted sets renewed=true, but
sync_run_subscriptions correctly waits while is_reading is true (app.rs:942).

The operation read then confirms nonterminal history through complete_related_read
(app_observation.rs:493-506). Its caller/batch settlement drains pending operations
only, not fallback renewal. The older fallback timer sees its last-read generation
superseded (:296-297) and correctly exits. settle_fallback_read is called solely
by the fallback loop (:285). No current producer services the already pending
renewal, so a second Subscribe requires an unrelated future sync. This is a
completion-notification omission, not a reason to weaken any stale-token guard.

## Current production completion map: mandatory full closure

| Producer | Current canonical completion / missed App settlement |
|---|---|
| Rejected-subscription fallback loop, app_observation.rs:271-297 | publish_read -> AppState::complete_history_read; only this path explicitly calls settle_fallback_read and drains operations. |
| Related operation histories, :493-506 | complete_related_read -> complete_history_read -> finish_read; updates owned diagnostics/retry classification, but no renewal settlement. |
| Attached live hydration, app.rs:2170-2203 | hydrate_run_history[_observed] -> publish_read; its pump consumes a bool, losing the shared completion token/disposition. |
| Initial cold summary hydration, app.rs:1404-1413 | begin_history_read -> read_history -> publish_read; caller drains operations only. |
| Checked operation recheck refusal, app_observation.rs:210-217 | Current reads are finish_read(false) after I/O without publication; must release current read-blocked demands without treating rejected bytes as confirmed. |
| Stale operation snapshot rejection, :477-483 | Token-checked finish_read(false), no candidate publication; same explicit settled-without-publication case. |

Workspace search found no other production finish_read/complete_history_read/
complete_related_read callers beyond those and their shared definitions. Direct
AppState test calls are low-level tests, not production replacement oracles.
Recheck this map before author edits; do not fix one operation branch with an
ad hoc sync while leaving another current completion unable to wake demand.

## Protected inputs and selected minimal repair

Keep canonical completion in AppState: exact current Scope/run/owner/read and
diagnostic revision checks occur BEFORE fold, diagnostic or finalizer effects.
Candidate/prefix/timestamp/decision-watermark validation and current error-source
guards remain unchanged. Obsolete completion is neutral. finish_read must occur
only for its matching read after actual I/O settles. Newer sibling errors and
operation refusal provenance cannot be lost during notification.

Retain the existing private ReadCompletion (token plus disposition/provenance)
instead of reducing it to bool before the App can settle the shared owner. Add
ONE private App-owned current-read-settlement operation used by every producer
in the table. It consumes canonical completion after data/diagnostic publication
and read finalization, then rechecks current owner AND last-read identity at the
App mutation point. The original completion being current earlier is insufficient
if another owner/read has since replaced it.

That shared operation updates the matching fallback completion disposition from
the current canonical result/state, preserves the existing one-shot renewed
demand, and services it once through normal sync/claim/Subscribe when no read is
in flight and current truth is not terminal. It also drains eligible current
operation demand through its existing owner. It creates no RunAccepted event,
renewal signal, Working status, provider session or retry budget. With no actual
current renewed signal it cannot attach a fallback-excluded run.

For current read retirement WITHOUT publication, pass an explicit private
settled-without-publication disposition and original token through the same seam.
It grants no confirmed-history fact and leaves true record/refusal diagnostics
untouched. It may release an existing CURRENT renewal after the real read ends,
subject to canonical terminal truth and current owner checks. Obsolete retirement
does not wake anything or remove a replacement marker. Do not fake Confirmed for
rejected operation bytes or fabricate read failure evidence.

The async inspect_operation recheck-error path currently returns only Err(String)
after retiring reads, losing their tokens. Carry its bounded (at most two related
reads) retirement notifications alongside the existing private InspectionResult
to the App batch publisher, or an equivalent explicit private result envelope.
Use a token-checked AppState retirement helper returning current-retired versus
obsolete, not raw finish_read followed by a fabricated completion. The stale
record branch returns the same notifications to batch settlement. These are
local return values, not a global event bus or deferred scan of all owners.

Batch ordering matters: complete each related read canonically and retain its
completion/disposition while aggregating owned sibling failures and retry state.
Release only the matching operation batch after its actual work settles. Service
the collected current read settlements/renewal before starting another pending
operation read that could consume the same owner. Queue only still-current retry
tokens; one renewed live claim must not create a parallel read, erase a different
failed sibling or reset an exhausted episode. Existing operation coverage/request
identities and coalescing remain the owners of batch demand.

Live/cold callers notify this SAME settlement seam through the existing weak App
owner after canonical publication; adapt the live pump's confirmed bool only at
its final consumer. Do not create a second fold or a callback framework. Fallback
loop exhaustion accounting stays origin-owned: generic operation/cold/live reads
cannot reset its attempt count, invent exhaustion or reuse an old timer's authority.
The superseded fallback timer remains inert, as it should.

Scope is checked per current delivery/completion, preserving the ninth fix for
one global receiver across project selection/change. Do not restore a connection-
wide project Scope pin. Old connection/global/read success/error/finalizer remains
inert; actual newer StreamError and read-owned restoration keep their provenance.
Terminal current history cannot be changed into Working by notification or a
transport hint. Unrelated sync/poll is not a recovery dependency.

## Authentic outer RED before production edits

Use the actual App/IPC and checked fixture registry/journals, with the existing
controlled_operation_reads barrier AFTER I/O and BEFORE publication:

1. Actual first Subscribe rejects; real fallback attempt zero reads an obstructed
   fixture journal, fails and waits in its bounded backoff. Prove actual read count.
2. Restore the SAME original journal object. A real newer operation revision starts
   related inspection using the recovering child owner and advances read generation.
   Pause that actual batch before publication; prove the current read is in flight.
3. Deliver actual current Global RunAccepted through real IPC/App receiver. Prove
   renewal is pending and no second Subscribe occurs while the read is active.
4. Release operation publication. Without manual sync, another poll, direct owner
   claim or another global event, the peer must receive the second Subscribe and
   respond SubscribeOk. Verify current events/history/decisions and one live owner.
   Release/advance the old fallback timer and prove it stays obsolete.

Frozen ninth must fail at the missing second Subscribe. Mechanical setup/compile/
teardown failures are not RED. Preserve real positive source/identity/owner
preconditions; no test-only acceptance algorithm or manually claimed replacement.

Required current-path controls:

- Related success, error, absent expected/unexpected history and explicit current
  retirement; one existing renewed demand is serviced only after actual settlement.
  Expected-absent does not fabricate a run; true refusal/error remains visible.
- Obsolete related completion/error/retirement cannot wake, clear, finalize or
  invalidate a new owner's state. Current read failure preserves actual StreamError.
- No RunAccepted: unchanged polls/equal status and shared completion notifications
  cannot exceed initial plus three fallback/retry reads or reset exhaustion.
- Duplicate current renewal while read/Subscribe pending coalesces once. Cancellation,
  new Scope/home, A→B→A, connection replacement and new owner make old callbacks
  inert; current legitimate demand still works. No connection-wide Scope pin.
- Current terminal history stays Done, no live promotion; registry recovery hint
  and conditional operation/standalone inspection remain correct. Both failed
  siblings retain their separate current diagnostic/retry state.
- Live hydration, cold helper and current rejected-operation read retirement each
  exercise the common settlement path, not just the newly reproduced branch.

All eighth/ninth behavioral fixture bytes/assertions remain preserved; append
new cases. Actual IPC workers/children, readers/pools and fixture storage settle
before explicit cleanup. No timeout inflation, serial masks, ignore or provider/
actual habit-runtime action. No tests or execution are performed by this plan.

## Closure, gates and final authorized dispatch

Private UI closure only: app_observation.rs common read publication/settlement and
operation completion collection; app.rs live/cold helper callers and weak App
notification; app_state.rs only minimal completion/retirement disposition plumbing
if needed; run_observation.rs only existing token-checked retirement support if
required; existing App/controlled IPC tests. No new framework, public/persistence
API, Results, scheduler, verification receipt, witness or continuation changes.

After approval/dispatch, refresh authentic focused RED/GREEN, full UI baseline
384 unit plus five integration plus new cases, strict affected default and all-
feature/all-target Clippy -D warnings, workspace all-target/all-feature check,
workspace fmt and diff. Bind new source/log hashes, caller map, narrow ninth-to-
tenth diff and case receipts in the actual freeze manifest. Prior green receipts
do not prove these new orderings.

Explicitly reuse unchanged 144 persistence/source-input hashes, 605 passed plus
one existing deferred skip, rustdoc exit0/nine warnings and completed manual
API-GATE; never claim rerun. A changed input requires the affected gate/review.
Current Windows/MSRV/native same-habit acceptance remains unexecuted.

Focused independent Phase 5a must close H9-SEC-01 and preserve H8-SPEC-01 in the
full four-blocker context, then affected generic/domain/security Phase 5b must
accept this narrow caller closure. No new broad scan is required. Root local
commit only after closed required reviews; no push/remote merge.

Maintainer verdict: ACCEPTABLE for focused pre-code critic/security review. The
existing read result already has exact identity/disposition; the missing seam is
shared App settlement across readers reusing one owner. This removes a path-
specific notification instead of introducing new execution authority. Existing
max-ten scope already authorizes this regression; actual count stays 9/10 until
root records the accepted tenth handoff. No eleventh is automatically authorized.

Same habit tracker all eight tasks/two milestones, codex-acp/gpt-6.1-sol,
follow-up/history, broader continuation and platform/product acceptance remain
the next unfinished goal. Synthetic UI repair is not completion of that project.


## Actual tenth handoff within existing explicit max-ten mandate

Owner and independent critic/security each returned ACCEPTABLE on accepted body
SHA2567df0a88342769f9b364b1aac8fbfb53bfd38909490ff53511a46039ed35d703d,
preserved byte-exact in evidence/home-inbox-tenth/approved-plan-snapshot.md.
Existing explicit user max10 covers the same four blockers and their required
repair regressions; H9-SEC-01 is within it, so no repeated user question.
Actual dispatch **10/10** is recorded immediately before the same usable author
handoff. No eleventh is automatic and no count reset. No push/remote merge.

Source/destination HEAD6d23ebf069fdaa2f3762c8b540c5f669b965dec8 plus frozen ninth
Rust053b40c8b8520fb9fb58a94601f7463e0518905c74dd3b36cfe2d72f7e27ccc2,
codex/results-planning-history in this worktree. Sole Rust/source/Cargo lease
passes to the author until freeze/release. Shared current-read settlement must
collect canonical completions/explicit retirements, release the matching batch
without early pending drain, service current renewal, recheck and queue only
still-current retry tokens, then drain pending operation demand (H10-ADV-01).
Authentic actual App/IPC RED before production edits, all caller paths required.
Local stage commit only after independent focused5a then affected5b.
Native samehabit/all8/codex-acp/gpt-6.1-sol continuation follows closedHome and
is not accomplished by these synthetic checks.


## Tenth author verification and review handoff

Same actual **10/10**, final authorized author dispatch. No budget reset, author
commit/publication or eleventh dispatch. Authentic outer App/IPC RED on frozen
ninth: `outer-red.log` exit101, one executed/384filtered, actual second Subscribe
expected2/actual1 after first real rejection/fallback read1, SAME journal restored,
newer checked operation read reusing recovering owner, and actual current global
RunAccepted. All actual I/O/batch/IPC cleanup preceded the assertion.

The private caller closure now retains complete ReadCompletion identity/disposition
through publication. ONE App settle_current_reads operation rechecks current owner
AND last-read identity and updates only its matching fallback/actual renewed demand.
Explicit token-checked AppState retirement returns Retired without publication,
confirmation or diagnostic fabrication. Checked recheck failure carries at most
TWO retirement notifications; stale checked snapshots use the same helper. Live
normal completion/final pending await, cold summary hydration, fallback and related
operation producers notify the same App seam. Canonical fold/prefix/decision/error/
timestamp and per-delivery connection/scope rules remain their existing owners.

H10-ADV-01 order is explicit: collect canonical completion/diagnostic/retry facts;
release only matching operation batch WITHOUT early drain; service current renewal;
recheck all still-current retry tokens after any new live claim, queue surviving
origin facts; THEN drain pending operation demand. Origin fallback alone owns
initial-plus-three attempts/exhaustion; generic notifications never create an
admission signal, reset budget, confirm rejected bytes or promote terminal history.

Four new actual-producer tests cover twelve required scenarios: success/equal pending
batch; child+parent errors/sibling retry identity retention; unexpected and expected
absence; actual cancellation snapshot with stale-record retirement; retained-record
recheck refusal and current max-two retirement; terminal/no renewal; both failed
siblings without renewal; real ordinary cold/current live helper completion; A→B→A
obsolete cold completion while current live read/StreamError held; actual obsolete
checked-retirement/error with current replacement read/marker protected. SubscribeOk
live-owner proof precedes intentional StreamError. Actual read/current finalizer
settlement precedes retained diagnostic/history/livefalse/no marker assertions.
Old timers stay obsolete and no rescue poll/sync/manual owner claim is the oracle.

All eighth/ninth fixture bytes/assertions are unchanged. One necessary sixth caller
assertion now consumes completion.confirmed() rather than returned bool, preserving
its original refusal assertion. Only app.rs, app_observation.rs, app_state.rs and
app_tests/home_inbox_sixth.rs differ from frozen ninth's17 Rust inputs. Cold ownership
rules were mechanically extracted unchanged; a fixture was corrected to register
an ordinary canonical project path after explicit /var versus /private/var guard
precondition evidence. Required sibling retry assertions follow actual batch/read
settlement. All compile/setup/fixture and strict mechanical failures remain separately
classified; no timeout inflation, serial mask, ignore or assertion weakening.

Final exact-source full UI **388/388 plus5/5**, zero ignored/filtered. Affected
UI+persistence all-targets strict clippy default/all-features both0 with-D warnings;
workspace all-targets/all-features check0; fmt/diff0. Complete-suffixed gate receipts
follow final mechanical needless-borrow/range-pattern fixes. The focused matrix
passed4/4; all cases also pass within final unfiltered full suite. Test linker
unwind-size and dependency future-incompatibility warnings remain reported; strict
clippy has no source warnings. Caller map and source/log/narrow-patch hashes are
sealed in tenth evidence.

All144 persistence/doc inputs and reused receipt hashes verified unchanged: prior
605passed/one existing skip, cargo doc0/nine warnings and completed manual API-GATE
are reused, not rerun. No persistence/public API/Results/scheduler/receipts/witness/
provider/runtime/index/main source or memory change. Sixth61/seventh45/eighth56/
ninth48 frozen evidence hashes remain unchanged. No automated API tool installation.

Author BUILD verified; source frozen and sole Rust/source/Cargo lease released.
Independent focused5a for H9-SEC-01/H8-SPEC-01 with preserved four-blocker caller
closure, then affected generic/domain/security5b remain PENDING. Author does not
self-close review. Root may make local stage commit only after required closure;
no push/merge or automatic eleventh author dispatch. Windows/MSRV/release/native
same habit all8/codex-acp/gpt-6.1-sol history journey remain open; synthetic UI
verification does not complete that project.
