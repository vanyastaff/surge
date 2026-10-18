# Stage 2 — original establishment-marker identity amendment

PROPOSED; owner verdict **ACCEPTABLE** for focused independent critic/security
review. DOC ONLY; no implementation authorization. Baseline951da616 plus current
initial stage 2 substep 1 (root-reported 15-file source f441d69f…). Accepted stage
2 plan SHA f1e267143979fce52d2a8a5e3bcdd3dc63660979c59141ff272b8b73802ec25d
remains immutable. This amendment only closes its marker-identity underspecification.
Canonical shared repair counter remains **2/3**; this is initial stage 2 shaping,
not a counter reset. The separate empty-MCP future-core gap remains on HOLD.

## Accepted invariant and exact gap

Parent accepted proposal lines353–354 says “missing/replaced key or marker yields
PrivateInputsRecoveryRequired/Corrupt”. Root confirmed this includes identical-byte
marker replacement after establishment. The original stage 2 plan's namespace/key
identity list did not waive that clause. This amendment adds no threat category.

Current `private_establishment.rs::Record` persists namespace/key identity and key
digest; `PrivateOwner::verify_native` validates original/named key handles and calls
`private_inputs::host_key(false)`. That function reads the marker's fixed bytes but
does not compare its original native identity. A different valid file containing
`established-v1` therefore cannot be distinguished by the marker check alone.
Marker loss is already covered by root-reported genuine RED/GREEN; that is not
evidence for replacement. No new tests were executed by this planning task.

## Minimal existing-owner change

Use the SAME `PrivateFiles::retain` and `retained_identity` APIs already used for
the key. Add private marker constant/access and marker file/identity fields to the
existing PrivateOwner, without a new owner/protocol/public API. Both key and marker
File fields must close before their retained PrivateFiles parent; Arc sharing
retains the same originals through actual callers/workers, not independent reopen
authority. Keep existing key bytes/checksum/digest/HMAC behavior unchanged.

Capture validates namespace, retains original marker handle, validates its native
file/security/size/link identity and expected bytes, then rechecks original/named
identity around the existing read/durability barriers. `verify_native` compares
the retained marker and freshly named marker against the captured identity, and
still calls existing-only host_key for key/marker content validation. Cold open
captures current handles and compares marker identity to the external record
before returning a successful owner, request identity, claim or hydrated inputs.
No path string, mtime or fixed marker bytes substitute for native identity.

Extend uncommitted `0035_owned_flow_private_establishment.sql` with
`marker_identity BLOB` (same platform native width16/24 policy as key identity).
Established requires namespace, key identity/digest AND marker identity. A present
marker identity is immutable; Established rows remain wholly immutable/no-delete.
Typed Record decode/check_record/verify_stored require platform-consistent widths,
reject incomplete Established records, and compare the exact marker identity.
Before 0035 is committed, change that migration in place; afterward schema changes
must use the next append-only migration. Do not synthesize an identity for an
already-established incomplete row or quietly treat it as legacy unpinned state.

## Publication, legacy pinning and failure

Preserve existing lock order and short transactions. First establishment still:
commits Establishing intent, records original namespace, then host_key(true)
publishes/reads the immutable key+marker with its existing durability barriers.
PrivateOwner captures both original file handles/identities only after those
publications succeed. Its native verification precedes `publish_established`.

The existing short `publish_established` transaction must write key identity,
key digest and marker identity TOGETHER with state Established, only for the exact
original Establishing record/nonce. No marker identity is recorded after exposing
Established. Recheck original/named native facts around that SQL publication and
before success; update the in-memory Record to precisely match the committed row.
A crash before this commit leaves Establishing and existing conservative refusal;
a crash after it must compare all persisted original identities on reopen. Lost,
incomplete or ambiguous first publication never permits marker recreation/rekey.
Retain current PrivateEstablishmentCommitted and native publication fault phases.

Legacy first pin retains original operation/establishment locks and authenticates
the actual independently stored legacy HMAC/envelope association FIRST. Only then
capture key+marker original handles/identities and atomically INSERT the complete
Established external record. Preserve pre/post native verification and stored
association checks. A missing/replaced marker after this pin refuses. Before the
first historical identity exists, no retroactive substitution-detection claim is
made; this does not permit recreating missing legacy material. No new legacy
fallback, automatic repinning, private content in errors or broader capability.

## Authentic before-fix oracles and platform contract

After root's current strict process settles and before marker production edits,
add tests using current signatures and real original owner/accepted claim/input
fixtures. Setup/compile failures are not behavioral RED. For replacement, retain
the displaced ORIGINAL marker at a different name/location so its inode/file ID
cannot be immediately reused. Publish a different valid protected file with the
SAME `established-v1` bytes at the marker name, keeping namespace+key unchanged.

- Exercise current owner verification/claim validation and retained authenticated
  inputs at their actual use boundaries; require typed recovery/corruption refusal.
  Pair with unchanged-original healthy positives. Assert registry/operation facts,
  key bytes, marker bytes and private object bytes unchanged by refusal; no rekey,
  repair publication, recapture or new operation/receipt is allowed.
- Settle/drop actual owners and pools, perform cold marker replacement, reopen and
  replay the exact old operation (including private MCP hydration where present).
  Require refusal despite identical marker bytes. Fresh valid original cold replay
  remains positive. Independently compare original/replacement native identities.
- Windows may deny live unlink/rename because retained handles protect the original.
  Assert that real denial and intact original owner as a POSITIVE ownership control;
  never pretend unlink succeeded or weaken a refusal assertion to accommodate it.
  Still perform separate cold loss/replacement after all actual handles settle,
  then require reopen/refusal. Keep native flush/close/error/crash phase coverage.
- Adapt existing `retained_private_owners_refuse_marker_loss_at_actual_use_boundaries`
  explicitly: Unix successful loss must refuse; Windows native-denied live loss
  must preserve healthy original ownership, with cold loss refusal tested separately.
  Preserve existing missing/corrupt key/marker/object matrix and legacy proof tests.

Unix evidence remains cooperative original-object ownership, not containment of
an arbitrary malicious same-user actor. Native IDs/retained handles detect tested
replacement; no global filesystem tamper-proof claim or OS sandbox change follows.

## Closure and gate

Affected closure: private_establishment.rs, existing private_inputs marker helper,
0035 schema/decode/integrity tests, private_inputs_establishment_tests.rs and native
fixtures only where required by actual platform observations. Reuse native APIs;
no secret/envelope/manifest/capability format change, new lock or lifecycle.
Root retains builder lease/dispatch decisions. After focused independent acceptance,
record authentic RED, exact GREEN/positive controls and fresh affected strict gates
on final source, with native Windows evidence or explicit pending limitation.
No full integration build while the shape hold remains. Owner **ACCEPTABLE**;
implementation acceptance, full stage 2 gates and the separate core HOLD remain open.
