# Proposed sixth repair — noncreating Windows read admission

PLAN ONLY; explicit human sixth-round authorization REQUIRED before implementation.
Canonical shared5/5 is EXHAUSTED across stages/reviews. This is not authorization,
a counter reset, source edit or permission to push/merge. Owner verdict: ACCEPTABLE
as a bounded proposal for independent pre-code review, pending final5a consolidation.
Only this document is written; no Rust/tests/Cargo/build/runtime/provider action.

## Actual defect and bounded decision

Against HEAD77c5f128 plus actual49-source gate digest
2c44436fc185ebe824aacfe45136630bff23605dc4836a2c6676a10394252b16,
current COMPLETE50-source digest is
5a53f4c609e45c35f41b4386bb13405d68020e6d8506c6c7c38d124e29424e03.
Original49 freeze was incomplete; reconstructed daemon fixture is accounted with
its provenance limitation, not represented as contemporaneous original freeze.
Final5a is NEEDS WORK for ONLY CONT-S2-SPEC-01, no other code blocker; critic/security
accepted the bounded correction design. Windows refusal mutation is source-proven.
RunUseOwner::database (runs/host_settlement/control.rs:215–238) invokes Windows
RunRoot::run(run,false). state_home/windows/mod.rs:145–148 falls back from missing
artifacts to namespace.child(...,true), creating that directory BEFORE latest
opening/route/database association is authenticated. Existing authentic controlled
origin plus missing artifacts therefore mutates even when NULL/malformed latest
opening later refuses. Existing fixtures leave artifacts present and miss this.
The registry read-only amendment permits only exact SQLite registry WAL/SHM companions;
it grants no artifacts creation, even on a failed read. This is a real source-proven
contract defect; native Windows RED has NOT been executed here.

Choose a crate-private noncreating getter, not a global semantic change:
`RunRoot::existing_run(&Arc<Self>, RunId) -> NativeResult<RunRoute>` on Windows,
matching the actual Unix existing_run purpose. It verifies retained root, opens run
namespace via existing_child, opens artifacts via existing_child ONLY, builds the
same RunRoute, then verifies it. Missing paths/errors propagate; no create fallback,
lock file creation, permissive owner, optional artifacts or naked path authority.
Reuse Namespace::existing_child native NOFOLLOW/reparse/security/identity contracts,
RunRoute verify/reader_route_identity/database(false), and existing retention/drop order.
Switch ONLY read-admission control::database Windows call to this getter; creator/
writable RunRoot::run semantics remain intact. A tiny private factoring helper is
acceptable if it preserves existing native checks; no public bool/mode escape hatch.

Alternatives: changing run(false) globally risks altering writable compatibility and
expands caller closure; rejecting all Windows readers loses supported behavior;
moving SQL checks earlier alone leaves valid-row missing-artifacts reads mutating.
None is selected. No new schema, public API, VFS, FFI, coordination framework or UI.

## Complete caller/error/lifetime contract

Exact production edits should be Windows RunRoot getter plus control::database use.
Inventory all RunRoot::run(false)/existing_run callers before finalizing: host_settlement
existing_parts is writable admission and retains prior behavior; reserve/bind/create
still creates original artifacts/lock/database as designed. ReadOnly admission must
not call a producer helper indirectly. Inspect static/native direct-read routes and
SqliteNamespaceOwner::existing for equivalent accidental fallback; report any proven
same-contract path before scope consolidation, not speculative unrelated cleanup.

Control/native identities are checked before run SQLite opening. Healthy complete
route plus authentic latest Opening+NULL remains exact OpeningNotReady, not success.
Missing artifacts is native/typed RunOpening refusal; it is NOT OpeningNotReady,
Absent/None/empty history, private corruption, Busy or a fabricated SQLite error.
Malformed latest row, wrong route/control/database and unsupported version stay
permanent refusal; never fall back to an older opening. Preserve exact existing
StorageError::RunOpening(OpenError) seam and ordinary Sqlite error conversion.
No new error variant/signature is required. New getter/native types stay crate-private;
no public capability/export/semver change beyond already accepted Stage2 API changes.
Original shared control/retained native owner remains held by manager/connection/
worker through actual final close; nested writer-reader reuses its SAME owner.
No change to shared readers, exclusive deletion, native ACL checks or Unix scalar
DB identity fix; no raw extra database descriptor may be introduced.

## RED first, native platform proof and positive controls

1. Before production edits, add current-signature Windows regression at the actual
   public static/current inspect_run and inspect_folded_run entry points. Create a
   real NewControlled run, settle writer/reader managers/workers and retained run
   handles, then remove its empty artifacts directory under test-only control.
   Preserve authentic outside control, original run/lock/database and SQL facts.
   Snapshot BEFORE, invoke each inspection, snapshot AFTER BEFORE any verifying SQL.
   Require typed refusal, artifacts STILL ABSENT, unchanged facts/native IDs/durable
   bytes and no new paths except specifically approved registry SQLite companions.
   Current source should genuinely RED because artifacts is recreated. Compile-only
   evidence or a Unix-only reproduction is not native Windows RED.
2. Repeat missing-artifacts refusal with authentic bound latest row, provisional
   Opening+NULL and malformed latest row. Fixture corruption is explicit/private;
   do not manufacture production authority. Healthy-artifacts real producer pause
   remains existing OpeningNotReady test with successful resume/migration inspection.
3. Native identity controls: cold foreign replacement of run namespace/database with
   original retained elsewhere must refuse and preserve BOTH sets; unsafe/reparse
   artifacts must not be followed/created. During live native retention, an actual
   OS rename/removal denial is positive owner protection; do not pretend it succeeded.
   No new claim that persistent artifact inode IDs were previously recorded.
4. Positive ordinary creation: real reserve→bind→writer startup creates original
   artifacts/lock/database; current writer+64 independent readers still work. Healthy
   cold/open/read succeeds; latest native route/database checks and malformed-control
   refusal persist. Do not pass by rejecting every reader or weakening creator tests.

Use the accepted safe observation oracle: cold raw hash only after actual settlement;
live snapshots in exec'd no-SQLite helper, no same-process extra DB-FD close. Preserve
approved registry companion identity/owner/link/security restrictions. Each fixture
joins workers, drops pools/native owners and performs explicit original-owner cleanup.
Native Windows missing/reparse/foreign-identity tests, controlled deletion positive/
resume/read-contention tests and existing ownership suites remain REQUIRED/PENDING.
Host is macOS; NO native Windows execution path is currently evidenced. No actual
Windows RED/PASS is claimed here; local test preparation is not Windows RED. To
preserve test-first, production implementation remains HELD until BOTH explicit
human sixth-round authorization AND native Windows execution availability. Obtaining
an external runner/publication requires separate explicit outward permission; none
is inferred. No new infrastructure search/install/CI work is authorized by this plan.

## Gates, consolidated proof and release boundary

Keep original gate/freeze/failure/review receipts and the incomplete49 provenance
limit. Final5a is consolidated as the single blocker above. After an authorized,
platform-feasible correction, re-freeze COMPLETE source closure (baseline COMPLETE50;
new count must be derived, not forced)
with paths/hashes/digest; include all prior Stage2 source, new tests/helper and exact
worktree delta. No partial two-file manifest or old pre-fix receipt as current proof.
Run focused genuine RED→GREEN, full affected eight-package suite, default/all-features
strict workspace, fmt/diff, doctests/docs with existing baseline accounting, required
recovery/restart/IPC/native-control gates and focused5a→5b domain/security/API review.
Record exact commands/process settlement and platform/tool/MSRV limits. Future fresh
Linux/macOS/native Windows CI remains PENDING until an authorized execution path
exists; no CI publication/push/merge permission is inferred. Local macOS results
cannot claim Windows, MSRV1.96, provider or cross-platform completeness. Local commit only after
root-required reviews/gates; no push/merge. Same habit/history/all8/codex-acp/exact
gpt-6.1-sol continuation remains goal; no new UI/provider execution in this repair.

Owner ACCEPTABLE for concrete independent review; implementation remains HELD until
explicit human authorization, native Windows execution availability and root's bounded
source/test lease. Shared5/5 unchanged.


## Concrete optional native execution path — separate human authorization

Local `.github/workflows/ci.yml` already supports workflow_dispatch and a
windows-latest Test Suite matrix. Root may request explicit permission in the
same sixth-round question to publish one diagnostic `codex/` branch and dispatch
that EXISTING workflow, only for native Windows RED then GREEN evidence. This
permission is NOT currently granted and is not inferred from local commits.
No new CI framework/workflow, PR, merge or user messages are included. Each
published checkpoint must be locally reviewable first; retain exact source SHA,
Windows test results and known global outer RED separately from the focused
regression verdict. An overall expected-red workflow is not a green-CI claim.
If the human does not authorize publication or provide an already available
native Windows runner, native execution and implementation remain held as above.
No successful dispatch/runner availability is claimed before it is observed.
