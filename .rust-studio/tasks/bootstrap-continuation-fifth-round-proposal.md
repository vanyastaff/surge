# Proposed fifth repair — established-open connection lifetime

PROPOSAL ONLY. Owner maintainer verdict **ACCEPTABLE for independent critic/security
pre-code review**, not implementation authority. **Explicit human approval for a
fifth repair is required before test/source edits or implementation dispatch.**
Canonical shared ledger remains **4/4 EXHAUSTED**; no reset or fifth dispatch here.

Baseline HEAD `951da616f8de86cd3e1898ecbbfbbb95e5d4dee1`, current frozen21-source
digest `62572fdb2864e17f7f2b4a2b5399263b4297c1f215e11a7b3e3884fde8ed6cd4`.
Preserve all existing source freezes/plans/amendments/receipts. Only this proposal
is written; no tests, Cargo/build, source/fixture edits, runtime/provider or Git
mutation were performed for it.

## Confirmed defect and evidence boundary

CONT-S2-QUALITY-01 (P2): `owned_flow/private_establishment.rs:481–482` matches
`read(&*store.pool.get()?)?` directly in if-let, then calls existing. The temporary
PooledConnection remains alive throughout the successful branch. `existing:341`
requests another connection before that temporary drops. With max2 and one
externally retained connection, a healthy established-open call cannot obtain
the second connection and reaches the pool timeout while one slot would otherwise
suffice. Return-expression evaluation happens before branch temporaries drop.

Rust2024 shortens the failed-match/else temporary scope; it does not drop a
successful if-let scrutinee before evaluating its then-block. Official basis:
[Rust Edition Guide](https://doc.rust-lang.org/edition-guide/rust-2024/temporary-if-let-scope.html).
Critic and independent owner accepted this static finding; owner supplemented its
earlier COMPLETE with NEEDS WORK. **No before-fix execution RED exists yet.**
Existing bounded tests construct/hydrate before retaining the competing connection,
so their success does not cover this fresh established-open path. The separate
claim against lazy `&&` at461 was withdrawn and is expressly OUT OF SCOPE.

## Options and chosen minimum

1. **Choose: read owned Option<Record> inside an explicit connection block.**
   Borrow one conn, perform the existing read, leave the block/drop conn, then
   match the owned Record and call existing. No borrow escapes into native I/O.
   Existing re-read/identity checks remain in their owning helper.
2. Pass a held connection into existing across native open/key/marker work:
   reject; needlessly extends SQL resource ownership through filesystem work and
   changes the established short-read/native-owner boundary for a lifetime fix.
3. Increase pool capacity, remove check_record or substitute guard-only checks:
   reject; masks connection amplification or weakens native/registry authentication.

One production edit site; no new helper/framework, lock, public type/signature,
schema, receipt, wire, private payload or capability change. Keep operation and
establishment locks, retained original key/marker/namespace handles, all native→
stored→native verification, legacy proof, conservative ambiguous-publication
refusal, existing-only reopen and failure-only Attention semantics unchanged.

## Actual caller and test closure

Refresh exact callers of private_establishment::open and existing before editing.
Current paths are private_inputs::{request_identity,verify_request,freeze,hydrate},
reached by owned Flow replay/begin/startup freeze, claim/reconciliation and runtime
input hydration. No caller signature migration is expected. Record that conclusion
in API review rather than claim an automated API-tool pass.

After user approval, add real behavioral tests BEFORE the production lifetime edit
using existing bounded_private_store (max2, hold1), actual public methods and valid
original established private data. Retain the competing connection BEFORE entering
the call that opens establishment; do not prehydrate the object under test first.
Use separate healthy fixtures for replay, cold claim and input hydration as needed
to avoid a live launch lock turning the test into unrelated Busy. Cold here means
new native owner acquisition with stored establishment intact, not secret repair.

Invoke on a real owned worker, preserve actual typed Pool failure/output as the
before-fix RED, and assert expected healthy result/immutable facts/key+marker bytes.
Join the worker and settle owners/pools before cleanup/assertion reporting. No mock,
sleep-only oracle, premature external-connection release or timeout-as-success.
Keep unchanged-original positive, superseded-observation and replaced-native-owner
refusal controls; do not relabel native Busy/refusal as the connection regression.
If setup fails, repair the fixture and capture the genuine behavior separately.

## Revalidation, completion and permission boundary

Require focused before/after bounded-path results, original establishment/marker/
legacy MCP/Attention tests and healthy populated cold twin. Re-run final-source
eight-package nextest and strict full-workspace all-target clippy in both feature
modes; same-process concurrency3, controlled ignored recovery3/restart2,
doctests/docs, fmt/diff and exact current mock/source/worker settlement receipts.
Preserve all eight private-fault/effect assertions and prior race/rollback controls.

Keep EXACT two pending outer/deletion filters, execute each separately as expected
RED, and disclose25 existing ignored tests. The previous4210 pass count is prior
source evidence, not this repair's result or a fixed future denominator. Preserve
prior TDD classifications and author-order limits; no blanket test-first claim.
Normal docs'58 unchanged baseline warnings are not strict-doc cleanliness; API
tools remain unavailable unless actually verified otherwise. Execution evidence
is macOS-only; Windows/Linux/MSRV/native-provider proof remains pending.

After implementation, require fresh5a and focused5b domain/security/API re-review
against exact frozen source. Root may locally commit only after that closure; no
push/merge. Full stage2 seed storage/pins/deletion/empty-MCP HMAC and stages3–7
remain open. Original habit project/history/all eight tasks and exact Sol/model
declarations remain unchanged; real retained-project continuation awaits later
feature acceptance. Independent plan acceptance precedes the human fifth-round
request. **No implementation release is granted by this ACCEPTABLE proposal.**
