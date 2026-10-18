# Proposed fourth repair: private-authentication Attention and connection reuse

PROPOSAL ONLY; owner verdict **ACCEPTABLE for independent critic/security review**.
**Explicit user approval for a fourth repair is required before implementation.**
Canonical `bootstrap-continuation-stage1-plan.md` is **3/3 EXHAUSTED**, shared across
all stages/reviews. This proposal neither authorizes dispatch nor resets that count.
Baseline951da616 plus paused stage2/substep1 source
`8cfb8896c695c6e84b750b337e04d6fce53e6b5e8232ddbe28af4cfe823ba1c4` (16 files).
Existing plans/amendment/source/evidence stay intact. Only this document is written;
no Rust, tests, Cargo, runtime/provider action, commit or fresh acceptance claim.

## Evidence and owning problem

`stage2/substep1/cold-mcp-failure-readonly-diagnosis.md` and
`failed-full-gate-settlement.json` record4201 tests:4199PASS,2FAIL,27skips.
MissingKey directly produced PrivateInputsRecoveryRequired before readiness;
OtherServerObject's expected PrivateInputsCorrupt is source-derived, not retained
full child stderr. Each aggregate aborted on its FIRST case; later faults and
post-readiness unchanged-effect assertions were not executed. Healthy populated
cold twin separately passed. Marker19/19 and strict both modes do not clear this.
Owned processes are root-reported settled; source hashes were rechecked unchanged.

`owned_flow/refusal.rs:225–258` saves early private authentication then propagates
its error before Ready/claim-token mutation. Daemon `work_items.rs:630–663` never
reaches its former startup-failure Attention path. No dispatch is the correct
refusal; repeatedly leaving the durable attempt parked is the regression.
`claim_owned_flow_matching` in launch.rs similarly authenticates before claimant
mutation; automatic reconciliation must not create a claim merely to settle failure.

Core wake_refusal's SOLE permanent reason is public-manifest InputsAssociationMismatch.
Missing key/object/native ownership or failed MAC is not that category. Keep the
immutable receipt/outbox/event contracts unchanged; no schema/wire/core reason,
verification, quota selection, sandbox or containment change is needed.

## Options and chosen contract

1. **Choose: typed private unavailability plus fenced registry Attention under the
   actual original OS guard.** Extend existing preclaim decision/transition owner;
   no Ready/execution/private capability is minted. No permanent mismatch receipt.
2. Reuse Refused/InputsAssociationMismatch for every authentication error: reject;
   mislabels evidence and changes the bounded permanent receipt contract.
3. Hand an unauthenticated Ready claim to the old daemon error handler, or leave
   all errors retrying: reject; respectively weakens ownership or preserves failure.

Use a private classification at the exact private-authentication boundary, retaining
the original error. Admit only known PrivateInputsRecoveryRequired/Corrupt/Unsafe/
Unsupported as private-unavailable outcomes, after independent accepted identity
validation. Busy, pool/SQL/transient I/O, unknown native errors and independent
association corruption remain typed errors/retry/refusal; no catch-all permanent
classification, string matching, rekey, ignored native failure or repair-on-read.
Public manifest parse/association errors keep their existing classification path.

Add `OwnedFlowWakeAdmission::PrivateInputsUnavailable`: its meaning is durable
fenced Attention committed, no claim and no receipt/outbox. Do not broaden Refused's
documented permanent-receipt meaning. This public Rust enum addition requires API
review and exhaustive callers: daemon work_items.rs match, persistence refusal
tests and fresh full symbol inventory; the enum has no serde/wire representation.
Daemon handles it as completed non-dispatch work, before wake reservation/launch.

## Exact ownership, transaction and race fences

Retain existing original FlowLaunch PreparationLock and validate its stored native
identity. Keep the actual guard Arc through decision, commit outcome, worker exit
and existing Ready-gate lifecycle; do not manufacture WorkItemLaunchClaim, inject
None private_owner, call broad settle(run,generation), or expose a new public
refusal-only authority. Reuse PreparedRefusal reservation/abort/join handling where
already present; private-unavailable never calls its receipt publish/delivery path.

Private payload authentication and original journal/startup reads remain outside
SQL transactions. Hold the original guard while collecting immutable accepted
facts and exact observation. Under BEGIN IMMEDIATE reload accepted operation,
launch identity, source/config/manifest/receipt and independent revision/graph/
workspace association. Preserve classify's operator-race/Obsolete precedence.
For a public-coherent/private-unavailable wake, require exact item version,
active run, archive state, accepted revision, attempt binding/generation/state,
full current control payload/generation, cycle invocation/generation/revision/
closed state, validated capacity transfer, wake identity/due time, registry wake
and unchanged observed manifest. No error authorizes a stale observation to write.

Reuse a small PRIVATE checked Attention SQL transition extracted from existing
commit_refusal state writes/control::write_control, with explicit observed facts
and checked affected-row counts. Proof remains at its owning call sites, not an
optional-owner parameter or generic bypass. Atomically set attempt/control Attention
and a bounded redacted diagnostic, increment exact item version, disarm only that
cycle/registry wake. Keep source/accepted snapshots, claimant token, generation,
quota receipts/candidates and permanent mismatch receipt/outbox unchanged. No new
opening, writer, reservation, ACP/MCP/provider effect or launch intent is allowed.
Revalidate original native ownership around commit using existing seams; heavy
filesystem/authentication I/O never moves under SQL. Commit failure is not success;
retain actual guard until the write worker exits, and let cold observation resolve
the committed state before any later action. No new receipt-delivery worker.

For launch.rs automatic `claim_reconciliation` (observation supplied), use the same
preclaim private-failure decision under original guard plus exact observed attempt/
control, item version and independent association fences. Superseded returns the
existing None result without mutation. Settled private unavailability commits
Attention then returns its original typed error, never Some(claim). Direct claim
without reconciliation observation continues to refuse without broad new implicit
state mutation. Normal healthy claim and existing mismatch recovery are preserved.

## Complete held-connection closure

Reuse WorkItemLaunchClaim::verify_lock_in(conn): native→stored-private→native,
with the SAME checked connection. Replace only held-transaction standalone calls:
control.rs::settle_reconciliation_update148/157 and
recovery_cycles.rs::owner_fence286. Its fence/check_cycle consumers include
begin/finish/rebind cycle, candidates/observations/wakes, rate_limit.rs,
handoffs.rs and handoffs/opening.rs. Refresh every call site before editing.
Retain standalone pre-connection checks (control132, work_items and launch sites).
Do not remove native checks or replace private verification with guard-only tests.

Before those production edits, extend the existing bounded-private-store fixture:
max2 connections, hold one externally, and invoke ACTUAL public
settle_reconciliation_error plus begin_recovery_cycle under a valid private-owned
claim/control. Preflight can borrow/release the sole available connection; the
held transaction must reuse it. Assert actual result/SQL facts, with healthy and
superseded controls. Run on an owned worker, collect the real pool failure as RED,
join/settle all owners before releasing fixture; no mock or sleep-only oracle.

## Acceptance and approval boundary

Existing two integration failures are genuine before-fix RED; retain every original
assertion. After repair execute ALL eight MissingKey/CorruptKey/MissingObject/
ChangedObject/OtherServerObject/OtherPurposeEnvelope/ManifestSandbox/ManifestBinding
cases, preserve child logs through diagnosis, and verify final Attention plus
unchanged real effect logs. Keep healthy populated twin, original healthy marker/
MCP/native controls, mismatch immutable receipt/outbox and delivery, existing
Ready hook, superseded/cancel/item-version/control/cycle/wake races and lock
replacement tests. Race after observation must never overwrite operator intent.

After user approval only: smallest complete fix, independent implementation/API
reviews, full current-source eight-package nextest with EXACT two known pending
outer/deletion filters and25 existing ignored reported separately; execute both
expected REDs separately. Strict both modes, docs/doctests with baseline warnings,
controlled ignored recovery, fmt/diff and source-bound native/platform/MSRV limits
remain required. Local stage commit only after closure; no push/merge. Original
habit project, all eight tasks and exact model declarations remain unchanged;
real Sol continuation awaits the later feature stages. Owner ACCEPTABLE as a
concrete proposal for review; **implementation remains HOLD pending explicit user
fourth-round approval after independent plan review**. Counter stays3/3 exhausted.
