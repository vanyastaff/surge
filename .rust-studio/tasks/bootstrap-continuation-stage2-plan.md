# Bootstrap-owned continuation — stage 2 owner plan

Status: FINAL PROPOSAL FOR PRE-CODE REVIEW; owner verdict **ACCEPTABLE** for the
storage boundary below. Independent critic/security acceptance and root release
are required before Rust edits. This document dispatches no implementation.

Current baseline independently read: `951da616f8de86cd3e1898ecbbfbbb95e5d4dee1`;
working tree was clean before this document. Stage 1 is locally committed and
its review closure is root-reported complete. The canonical ledger remains
`bootstrap-continuation-stage1-plan.md`, Canonical review-repair record: **2/3**
shared across every stage/5a/5b; one automatic implementation repair remains.
Planning/review iterations do not reset it. The preliminary stage 2 inventory's
39673ff5 baseline and 0/3 count are historical, not this plan's authority.

Accepted parent: first 604 lines of `bootstrap-owned-continuation-proposal.md`,
body SHA `f4cb0605fc46606d43c1d296cae75beb69cd618f2ede886c4d7eafb0c0db84c9`,
plus stage 2 allocation at lines 723–748. This task performed read-only source
inspection and writes only this plan. No Cargo/tests/provider/runtime/git mutation
or platform/MSRV execution is claimed. Toolchain last reported 1.98.1; MSRV 1.96
is declared, unverified here. No new ecosystem API/dependency is needed.

## Outcome and allocation

Stage 2 delivers working private seed **storage** operations, external namespace/
key establishment and retained dependency/deletion enforcement within the existing
owned-Flow operation owner. Storage authenticates exact bytes and their stored
association; it does not certify the source's semantic eligibility. No daemon
continuation endpoint, seed marker writer, semantic import, source capture,
provider invocation or generic existing-directory adoption becomes reachable.

| Stage | Required responsibility |
|---|---|
| 2 | Durable private-required reservation before returning ownership, including empty MCP; external write-once native namespace/key identity; immutable authenticated seed objects; source/private dependency pins and atomic deletion exclusion; existing-only reopen/reconciliation. |
| 3 | Original workspace identity/owner authentication, shared continuous workspace exclusion through every execution admission and real settlement. A stage 2 registry association or pin is not that owner. |
| 4 | Authenticate actual source terminal/prefix/failure-tail/checkpoint, capture the allowlisted semantic seed, validate semantic/frame content, issue authenticated read data separately from launch-capable execution seed, extend stage 1 fold/load guard once. |
| 5 | Expose the single continuation operation; coordinate stages 2–4 and atomically accept bridge/item/revision/attempt/workspace/seed/receipt, then publish/start successor under retained owners. |
| 6/7 | Joined native inherited history/navigation and full current-source/provider/platform acceptance. Outer continuation RED remains open in stage 2. |

Stage 2 reserves and freezes seed storage metadata inside the existing operation
row. Its ordinary `finalize` must refuse a seed-required operation that lacks the
later authenticated source/workspace preparation, and all generic claim/hydration
reconciliation paths must refuse to dispatch such an operation. Tests can exercise
accepted associations at the private registry boundary using fixed fixture facts;
they must not create a public acceptance bypass. No fake production eligibility
token, default authenticator, always-true checker or public constructor for a
future AuthenticatedExecutionSeed is introduced to make this stage self-launching.
The delivered storage operations are real (reserve, immutable seal, authenticate,
cold reopen, retain, refuse deletion), not a public unused seed-activation facade.

## Verified owner and caller inventory

| Current owner | Verified seam / required change |
|---|---|
| `work_items/owned_flow/preparation.rs:88–206` | replay/begin normalize body before source I/O; retained operation lock; BEGIN IMMEDIATE for immutable provisional IDs/token. `private_key_creation_allowed` only counts HMAC requests/private MCP. Extend the same operation, not a second bridge FSM. |
| Same `:306–369,397–520` | freeze startup currently requires private capability only for nonempty servers, then persists public snapshots; finalize commits accepted item/revision/attempt/receipt under retained launch Arc; Drop abandons without deleting objects. Add durable seed requirement/pins and storage association checks here. |
| `owned_flow/private_inputs.rs:19–85,90–164,194–289` | InputBinding binds operation/request/item/revision/run/workspace; canonical sorted JSON, HMAC domains, immutable objects, key checksum and marker. Empty MCP exits before opening private namespace. Key+marker are INSIDE removable directory. Reuse these primitives and preserve empty-MCP selection independently. |
| `owned_flow/private_files.rs:27–258,305–359` | Unix retained no-follow directory chain, UID/mode/ACL/device/inode/link checks, bounded reads, immutable publication+fsync. No portable persistent identity accessor yet. Extend this owner. |
| `state_home/windows/namespace/private_objects.rs:20–50,132–211` | Windows retained protected namespace, native file IDs/security, no-replace publication, flush/close barriers and fault hooks. `namespace.rs:49,606,1012` already owns native Identity/object_identity/file_identity. Reuse them without paths as authentication. |
| `start_preparation/secure_lock.rs:10–146` | Descriptor-checked nonblocking operation/task/launch locks. Add one typed private-establishment lock key in this owner, not a second lock implementation or mutex-only exclusion. |
| `owned_flow/launch.rs:63–149,333–367` | Independent accepted SQL association, current generation/claim checks before and after private hydration. Every manifest/read/hydration/claim/refusal path must also consult seed requirement and external establishment; public metadata is not a capability. |
| `runs/migrations/registry/0029_owned_flows.sql` | Existing provisional/preparing/abandoned/accepted owner, immutable first snapshots, pending/started/canceled launch intent. Preserve this lifecycle; add storage facts beside it. |
| `runs/storage.rs:583–615` | delete_run checks in-process writers, deletes registry row transactionally, then path remove_dir_all. No current pin check or durable deletion intent. An abandoned FS worker can outlive the caller. |
| `runs/registry.rs:243–257` | Public pool-only delete and raw connection helper bypass physical deletion admission. Only direct caller outside Storage is its registry test. Close this bypass in the same stage. |
| `runs/migrations/registry/0032_host_settlement.sql`, `host_settlement/mod.rs:107–119` | Permanent run tombstone and deletion inventory schema already exists; opening admission checks it. Survey found no production tombstone/inventory writer yet. Do not describe schema as an implemented deletion protocol. |
| `runs/registry_exec.rs:70–112` | Existing BEGIN IMMEDIATE blocking registry write and cancellation commit fence. Reuse for short deletion/pin decisions, never hold it across filesystem work. |
| `artifacts.rs:100–141`, private publication owners | Only unpublished staging cleanup exists; no production artifact/private-object GC API was found. Run-directory deletion removes artifacts; published private objects are retained. Do not invent a GC job. |

Affected actual ordinary callers include daemon `owned_flows.rs:40/46/90/107`,
daemon `work_items.rs:553`, orchestrator `engine/work_items.rs:35`, WorkItemStore
claim/validate_claim/claim_reconciliation, owned_flow launch/refusal/reconciliation
and tests, and daemon MCP ownership/recovery tests. Refresh exact rg inventory
before editing. Keep stable ordinary request/wire contracts; no transport/core
manifest change is necessary for storage-only ownership.

## Alternatives and selected boundary

1. **Extend owned_flow preparation plus registry establishment/pins (selected).**
   Existing operation/token/launch lock owns cancellation and retries. Private
   descriptor owners supply identities; SQL owns permanent dependency facts and
   serializes deletion. Three later extensions (semantic hydration, native read
   history, accepted bridge startup) reuse these same facts without acquiring
   authority from a path or DTO.
2. Separate seed directory with its own operation/key/pin service: rejected. It
   repeats request identity, orphan/rekey logic and cancellation, and makes two
   protocols coordinate acceptance. Merely renaming a copied key namespace does
   not survive whole-directory loss.
3. Use HMAC request/MCP presence or a marker within private-flow-inputs as the
   establishment oracle: rejected. Empty MCP and provisional-before-manifest
   states have no dependency signal; removing the directory removes the oracle.
   In-memory handles alone cannot remember established identity across restart.

No product fork is required. The following choices are tactical implementation
decisions within the accepted source/namespace protection contract.

## Registry schema and invariant ownership

Add next registry migration **0035**, registered through the existing migration
array (current max registry 0034; per-run max 0012 stays unchanged). Do not rewrite
0029/0032 or old operation JSON. Use constrained normalized tables, immutable
triggers, typed decode/integrity checks and transactional owners together:

- One runtime-home `owned_flow_private_establishment` row: version, immutable
  establishment nonce, state Establishing/Established, retained native ancestry/
  leaf identity encoding, key-file native identity and key digest, with each
  identity write-once. Established requires all fields. Raw secret key is never
  in SQL. No UPDATE back to Establishing, replacement, deletion, or reset API.
- Operation storage extension keyed by existing operation FK: seed-required flag,
  immutable source-bootstrap/source-implementation reservation identities,
  external establishment nonce, optional frozen source-retention binding,
  optional ContinuationSeedManifest and immutable seed-object association. No
  duplicate operation state/token/receipt columns. Source-implementation uniqueness
  lives here across abandoned and accepted operations, preserving exact retry.
- Normalized dependency rows keyed by operation and resource: run-history pins
  for planning/source implementation; private object pins for seed and retained
  preparation objects. Record establishment dependency from provisional creation,
  before seed reference exists. Source and object identities are typed; no path
  locator is a pin key. Deletion prohibition is checked even if no `runs` row is
  present. Do not ON DELETE CASCADE away pin/establishment facts.

Requirements are monotone: once seed-required, the operation can never clear it
because MCP is empty, preparation abandons, launch cancels or a process exits.
Freeze persists exact manifest/ref/hash/tag once; exact replay compares all facts,
changed source/operation/body conflicts. Accepted associations cannot replace
frozen seed or lose pins. Unknown version, half-present fields, missing
establishment, foreign object binding, corrupt pin or missing source registration
is a typed corruption/refusal, never an empty optional success.

Registry initialization can check structural SQL consistency without private I/O;
each sensitive replay/begin/freeze/read/claim boundary verifies native/key identity.
Old no-seed rows have no seed extension and retain their request semantics.
Malformed identity JSON must error, not disappear from a json_extract count.
`private_key_creation_allowed` becomes one owning typed policy over establishment
and ALL private dependencies (HMAC, private MCP, seed requirement, provisional/
frozen/abandoned/accepted references); no parallel caller-specific count queries.

## External establishment and native identity protocol

Use existing PreparationLock with a new typed PrivateEstablishment key under the
existing preparation-lock namespace OUTSIDE private-flow-inputs. Its retained
native identity also participates in the external record. A replaced lock is not
permission to establish again. Hold it only for namespace/key establishment or
verification, not semantic capture or future engine execution.

Order: operation lock, establishment lock, short registry transaction. Source run
pins are acquired before source reads in later stages; never acquire another
operation lock while holding the registry write transaction. Deletion owns its
run writer lock and then the same registry transaction domain; pin acquisition
never waits for that lock while holding a registry transaction. Busy returns typed
Busy rather than unbounded lock retry. No transaction spans file publication.

First private use with no prior establishment or dependency:

1. Under establishment lock, commit Establishing nonce BEFORE creating key/private
   directory or returning any preparation identity. For seed preparation commit
   seed-required operation reservation and source pins before exposing ownership;
   after external establishment/HMAC calculation, insert the ordinary operation
   row and its seed-extension/pins in ONE transaction. This avoids a child FK
   before its operation exists; do not persist a placeholder request identity or
   hold SQL open over key I/O. The earlier Establishing row already prevents rekey
   if the process dies before the operation transaction. A seed operation never
   uses an ordinary PlainPublic digest as its request authentication.
2. Open the protected native namespace with creation allowed only for this exact
   first establishment. Capture identities from held descriptors, validate
   ancestry/security, persist the native identity once, then publish/read the
   immutable key and marker through existing durability barriers. Bind the
   validated key-file identity and cryptographic digest into the same external
   establishment. Finally commit Established. Return request HMAC/owner only after
   this external durable publication and final revalidation.
3. If namespace/key publication or SQL completion is ambiguous, retain the
   Establishing row. Reconcile only the SAME recorded native namespace and exact
   recorded key identity/digest. A missing recorded identity, missing directory,
   incomplete key+marker or unprovable ownership returns RecoveryRequired; do not
   remove/recreate, adopt a replacement, regenerate a secret or allocate another
   accepted identity. Conservative refusal of an interrupted first establishment
   is allowed; automatic identity invention is not.

Existing-only reopen (including new private operations): fetch external identity,
open with create=false, compare retained namespace ancestry/leaf identity, read
and compare key native identity/digest/checksum and established marker before
request verification, object publication or authentication. Revalidate around
read/publication and SQL snapshot publication. A valid copied namespace containing
the same key/objects at new native identities still refuses. Never repair ACLs.

Legacy private migration is an explicit first-pinning path, not an absent-row
rekey exception. If pre-0035 HMAC/MCP dependencies exist without an external
establishment, key/directory creation is forbidden. Under the original retained
operation lock and establishment lock, open existing-only, validate the complete
native parent/leaf/key ownership and durability barriers, decode key checksum and
marker, then authenticate the existing operation's request HMAC or frozen MCP
binding/envelope against independent stored operation facts. A PlainPublic body
alone is insufficient to authenticate its private MCP material. Use the original
stored association; do not recapture mutable server defaults. Only after this
succeeds may the external record pin THAT retained native namespace/key identity
and digest. Hold native owners across both the short SQL publication and final
revalidation; no success or new private identity escapes before Established.

There is no historical native identity to recover from a pre-0035 row; the first
pin attests the currently descriptor-validated existing owner and authenticated
legacy key/objects under the established legacy trust policy. Do not claim it can
detect a byte-identical substitution completed before any external identity was
recorded. This evidentiary limit does not permit recreating missing legacy data.
A surviving dependency without a valid existing key+marker/envelope fails closed.

A crash before the first external intent leaves the same existing-only legacy
checks mandatory because persisted HMAC/MCP dependencies still forbid creation.
A crash after Establishing/native identity/key facts are published must reconcile
only those original facts. If publication failed before enough original identity
was durably recorded, return RecoveryRequired rather than adopt whatever is now
at the path. After Established, every cold request/freeze/read/claim compares the
external original native namespace and key identity; loss, valid copied directory,
foreign key, transplant or replacement refuses. Exact healthy ordinary MCP replay
and hydration retain their behavior and selections. Existing public no-seed/empty-MCP operations
still need no private namespace and do not acquire a private dependency merely
because a different operation once used one. All NEW private/seed operations must
honor an existing or incomplete external establishment.

`PrivateFiles` gains private identity/read-identity accessors using the same
retained handles as byte reads. Unix encodes version+platform+runtime-home and
private ancestry device/inode identities; key identity is separate from content
digest. Windows exports the existing native volume/file-ID encoding and retained
chain from Namespace, retaining security/flush/reparse/hardlink checks. No
canonical-path string, mtime or secret digest alone authenticates a namespace.
Unsupported platforms keep typed PrivateInputsUnsupported; tests must not fall
back to insecure filesystem access.

## Concrete owning API shape and complete caller migration

Replace the existing `explicit_private: bool` parameter of begin_owned_flow and
replay_owned_flow with one small owning `OwnedFlowPreparationRequirements` enum:
`Ordinary { private_request: bool }` and
`Continuation { source_bootstrap: RunId, source_run: RunId }`. This is public
host-storage metadata because the current public begin/replay methods consume
it; it carries no source authenticity, workspace adoption or launch permission.
Validate nonnil/distinct source facts and immutable equality with the stored
operation. Continuation always chooses HMAC and requires the private operation
lock independently of MCP. Ordinary retains the exact old bool behavior. Migrate
daemon owned_flows (both replay and begin), all direct daemon/persistence test
callers, exports and rustdoc together; no old bool overload/compatibility shim.
This enum's stage 2 consumer is the actual reservation/key/pin operation, not a
future endpoint. No transport client gains a new variant in stage 2.

Extend OwnedFlowPreparation with real storage methods, keeping source-capture
and execution proof outside their return types:

- `freeze_seed_storage(&mut self, source: &SeedStorageBinding, payload: &[u8])`
  returns the existing ContinuationSeedManifest. SeedStorageBinding is a
  public validated STORAGE header because this functional host-storage method
  consumes it now; its constructor checks structure only, not source authority.
  It carries the immutable source boundary/
  terminal/prefix/failure-tail/checkpoint/graph/workspace facts described below;
  it conveys storage association only. Method verifies seed requirement/token,
  compares reservation source, freezes reference/pins, seals bytes, authenticates
  read-back and persists the exact immutable manifest. It is used by the actual
  stage 2 storage owner and tests; its public structural constructor cannot certify source truth.
- `verify_seed_storage` authenticates a stored frozen association via the private
  AuthenticatedSeedObject path. Freeze read-back, cold operation replay checks and
  generic accepted-association validation call this same implementation. Missing
  required manifest/ref is distinguishable from an ordinary no-seed operation;
  no default empty payload or Option(None) hides a required seed failure.
- `finalize`, independent_binding/association, claim/reconciliation and ordinary
  runtime-input hydration recognize the durable requirement. Storage verification
  may succeed while executable continuation remains unavailable; existing generic
  ordinary-flow finalize/dispatch must refuse it until stages 3–5 supply their
  actual owners. There is no public boolean allowing callers to skip this refusal.

Keep SeedStorageBinding and AuthenticatedSeedObject owned by persistence; export
only the storage header needed by the real public freeze method, while the
authenticated read object remains private and is actually consumed by replay/
read-back validation. Do not leave an unused private publisher requiring dead-code
suppression, or expose a future semantic read/launch capability. Public storage
methods are tested through the host-storage boundary in stage 2; they do not
certify source content or dispatch an engine. Immutable source
reservation, byte authentication and cold integrity are fully usable stage 2
operations without pretending that storage is source approval.

## Seed object authentication and read/effect separation

Add private `owned_flow/seed_objects.rs` beside private_inputs, sharing its exact
canonicalization/HMAC/key owner (factor small private crypto helpers if needed,
not a generic public crypto/validation framework). Keep private files' 8 MiB
encoded-object cap, bounded decode and unknown/duplicate/version rejection. Enforce
the limit before publication and before large decode; distinguish object bytes
from semantic payload size because JSON encoding can expand them.

Seed envelope schema v1 authenticates a distinct domain, establishment nonce,
operation and request HMAC, successor run and WorkItemBinding, original workspace
owner and exact recorded workspace association, source Bootstrap and source run,
inclusive selected boundary, terminal/prefix/failure-tail identities and source
checkpoint, accepted graph hash, seed content identity, opaque object reference,
payload format/version and bounded payload bytes/hash. Source-retention metadata
is a private immutable record; stage 4 authenticates its truth against actual
source history. Stage 2 authenticates equality with stored association only.

Do not MAC an envelope containing its own tag: tag lives in the existing public
ContinuationSeedManifest, while the envelope contains its corresponding header
and object ref. SeedId derives from canonical payload identity; envelope HMAC binds
all context plus that identity. Random object refs are allocated once and stored
as a pending publication association before I/O; exact retry reuses them. Use
existing HmacSha256Tag, ContentHash, ContinuationSeedId, SourceEventRef and
OwnedFlowObjectRef. Private header structs validate through constructors/try_from;
derive Deserialize must not bypass invariants. No semantic RunMemory serialization.

Freeze: verify owning preparation token/source reservation/external establishment,
durably pin object reference, publish immutable bytes, authenticate/read them back,
then short transaction records exact frozen manifest/object dependency under the
same operation token. Failure retains reservation/pins; an orphan immutable object
is not authority. Lost object refuses on retry instead of silently recapturing
mutable source or generating another seed. MCP freeze still captures newly bound
successor MCP inputs with its existing separate domain, even when selection is
empty; an empty MCP manifest does not eliminate seed-private requirement.

Read-authentication yields a private nonserializable `AuthenticatedSeedObject`
(bounded owned bytes + immutable association + retained private namespace owner).
No public constructor/Deserialize/Deref to execution authority. The real stage 2
consumers are freeze read-back, cold storage integrity and acceptance-association
validation; they use this type now. Do not export a future launch seed type just
to satisfy an architecture diagram. Stage 4 can expose the narrowly needed read
capability to inspection and semantic validation; only its actual authenticated
source/workspace admission can mint AuthenticatedExecutionSeed, separately from
read access. Existing ordinary AuthenticatedOwnedFlowInputs is not renamed or
promoted into seed execution authority.

## Pin versus deletion: one transactional decision

All pin insertion and deletion admission use BEGIN IMMEDIATE in the SAME registry.
Pin insertion checks source registration, expected retained identity where frozen,
existing run_host_tombstones and missing/corrupt dependency facts before commit.
Planning/source pins persist through provisional/frozen/abandoned/accepted states.
Source lineage appends do not remove required captured facts; a pin grants no
write/launch authority and is not a global source-log freeze.

Delete admission checks every dependent operation regardless of its lifecycle,
then commits permanent deletion intent BEFORE unregistering or touching bytes.
If pin wins, deletion returns an explicit ReferencedRun refusal and leaves all
rows/bytes unchanged. If deletion wins, later pin fails on the tombstone even
after the run row is gone or a crash leaves files behind. Preexisting tombstones
never admit a new pin. Transaction rollback exposes neither half-pin nor half
deletion intent. A preliminary out-of-transaction `SELECT` is not the barrier.

Extend the existing host_settlement deletion owner rather than make a second
tombstone table: implement the missing inventory/intent producer required by
0032. Prepare bounded real deletion inventory and original run/database/outside
parent identities using existing retained native run namespace and existing-only
journal FileLock. Never put empty/dummy identity/inventory blobs into that schema.
Revalidate the prepared identity under deletion admission; do not claim that an
in-process ActiveWriters check alone fences another process. A live original
writer refuses; retained handles/worker own the original namespace through actual
cleanup. Native delete access/close ordering must account for Windows SQLite and
reader handles; failure leaves DeletePending and retained inventory for explicit
retry, not a new deletion identity or opening of a replacement path.

After intent+registry removal commit, only the admitted original deletion owner
may remove the inventoried run/artifact objects; substituted paths/new objects
refuse. Mark Deleted after durable completion. Retry resumes the same identity/
inventory, including already-missing original entries, and never recursively
deletes a replacement. This is storage deletion integrity, not process containment
or an execution-settlement certificate. No successful host settlement is minted.

Ordinary deletion compatibility is explicit: an unpinned healthy run remains
deletable through Storage, and repeated deletion resumes the SAME durable intent
rather than allocating another identity. A terminal run already absent with a
matching Deleted intent is idempotent success; a replacement namespace is never
an idempotent cleanup target. An absent run with no prior admitted identity cannot
justify arbitrary path cleanup. Once a real permanent 0032 deletion intent exists,
recreating that SAME RunId is refused by the existing host_settlement::admit rule;
no change weakens that existing rule. New ordinary work with a distinct RunId is
allowed subject to ordinary admission, and no workspace ownership/exclusion policy
is added here. Capture any existing test expecting same-ID resurrection explicitly
and correct its contract rather than silently bypassing tombstones. Preserve
legacy no-seed/MCP flows and unrelated physical deletion; do not keep unsafe
registry-only unregister as a compatibility shim.

Public pool-only `registry::delete_run` cannot prepare/hold physical ownership
(it has no home) and must be removed from the public API. Migrate its lone registry
test to the checked Storage surface or a private transaction fixture. Replace the
raw connection helper with a private admitted-deletion helper requiring the real
owning deletion preparation; no callable unchecked DELETE escape remains. Schema
triggers additionally reject deleting a pinned run and pinning a tombstoned run;
public API removal is intentional active-dev breaking change, not a deprecated
unchecked alias. Existing `Storage::delete_run` signature remains stable.

There is no current production artifact/private-object GC. Preserve unpublished
temporary cleanup, which may delete only its held original staging object. Never
treat an abandoned operation's published seed/key/object as a temporary. All
published private objects and establishment records are retained in stage 2;
there is no new public release-pin API. Future release requires an owner-held
operation lock after actual worker exit, no accepted lineage dependency, and an
explicit transaction; cancellation alone cannot prove that. Accepted pins remain
for history. This conservative policy covers abandoned-worker races without a
second worker registry or speculative GC feature.

## Error and API contract

Reuse WorkItemError::{Busy, Conflict, PrivateInputsUnsupported, PrivateInputsUnsafe,
PrivateInputsCorrupt, PrivateInputsRecoveryRequired, IndependentAcceptedDataInvalid}
according to existing meanings. Preserve typed native/SQL failures and redact
private bodies/keys/envelopes. Add a narrow typed run-dependency refusal to
StorageError for callers to distinguish referenced history from active writer or
I/O failure; map existing-only/tombstoned source refusal explicitly at the work
item boundary. Do not use string matching or swallow absent/corrupt as Ok(None).

New storage implementation types stay private/pub(crate), consumed by actual
reservation/freeze/reopen/deletion owners, apart from the explicit public
OwnedFlowPreparationRequirements discriminator required by current begin/replay
callers and SeedStorageBinding consumed by the actual public immutable freeze
method. These are storage metadata, never execution capabilities. Reuse public stage 1 manifest/identity types. The owning storage methods
perform real reservation, authentication and pin transactions; there is no raw
path/body-to-execution-capability constructor.
No new public seed transport request, endpoint or launch capability is exported in
stage 2. Record public registry-delete removal and any affected signature/errors
in the exact API review/caller inventory. No release or publication is authorized.

## Real RED strategy and acceptance matrix

Before production changes, add existing-signature behavioral REDs where current
APIs can express the defect, and preserve exact before-source/commands/results:

1. Existing owned-Flow private request/freeze/accept fixture; copy the complete
   valid key/marker/object namespace into a newly created protected directory,
   replace the original, drop/reopen Storage, replay the original request. Assert
   refusal and unchanged bytes/rows. Current authentication checks bytes but has
   no persisted native namespace identity. Valid original cold replay is the
   paired positive. On Windows settle handles before cold substitution; retain a
   separate live-owner substitution test proving native protections refuse it.
2. Existing Storage/registry deletion with a valid preexisting 0032 tombstone
   naming an original real fixture, followed by replacement of its namespace,
   must preserve the replacement instead of path-recursive deletion. Distinguish
   this identity mismatch from a real resumed matching DeletePending intent
   (positive), and already Deleted unchanged absence (idempotent positive). Existing raw registry API must not bypass
   permanent intent or pin exclusion. No forged successful settlement fixture.
3. Key establishment via existing internal request_identity before an operation
   row is exposed, followed by whole namespace loss and another private request:
   assert no new key/publication. Run in the current private_inputs test module,
   not through a fabricated future public continuation signature. Exact ordinary
   no-private/no-MCP first use remains a paired no-directory-creation control.

Seed-required empty-MCP reservation/pins do not exist at baseline. Their tests
are **new-feature inner REDs**, enabled after the minimal schema/type skeleton
compiles and before production protocol edits. A compile failure is setup, never
the behavioral RED. Do not relabel the existing empty-MCP legacy behavior as a
bug; the new seed requirement is an independent discriminator.

Required fixed-source cases (collect all matrix outcomes before final assertion):

- First no-MCP continuation reservation creates protected external establishment
  and durable private requirement/pins before returning owner, while preserving
  Explicit versus HostDefault empty MCP selection. Exact repeated request keeps
  original IDs/key/object refs; changed body/competing source refuses.
- At provisional pre-freeze, frozen abandoned and accepted association states:
  lose/replace WHOLE directory including key+marker, close/reopen, request old
  operation and a NEW private/continuation operation. Refuse without rekey,
  recapture, item/run duplication or new object publication. Test corruption and
  missing single key/marker/object too; preserve source files/history.
- Foreign valid namespace with copied valid key/object bytes, foreign valid
  namespace with another key, object transplant across operation/binding/run/
  workspace/source/establishment and selected prefix/terminal/checkpoint; wrong
  HMAC domain/version/reference, invalid canonical encoding and oversized body.
  Assert specific corruption/recovery category and no private bytes in errors.
- Crash around external intent/native identity/key/marker/Established commit,
  object pin/publication/freeze and accepted registry transition. Reopen either
  retains exact original identity or refuses conservatively; never creates a
  new secret after ambiguity. Use existing native observation/crash hooks and
  joined child workers; do not simulate durability by mocking every I/O call.
- Pin versus deletion with independently synchronized registry connections:
  pin-first refuses delete; delete-first refuses pin; rollback permits the other
  side; preexisting tombstone and missing/corrupt pin refuse. Exercise actual
  Storage::delete_run plus attempted raw SQL/API bypass, original artifact bytes,
  permanent tombstone facts and cold restart. No timing-only sleep oracle.
- Dropped/canceled preparation while real blocking worker holds original guards:
  no early pin release or published object cleanup; after actual exit abandoned
  reservations still retain dependencies. Accepted-not-started and canceled
  attempts likewise keep retained history; ordinary unrelated run deletion still
  succeeds with real current deletion inventory and cannot remove replacements.
- Existing ordinary private MCP request/freeze/hydration/cold reconciliation,
  HostDefault public request resolving private MCP, no-seed empty-MCP public
  flows, immutable operation retries, and native permission/reparse/symlink/
  hardlink/publication/flush errors remain supported under the documented policy.

Use fixed independent source IDs/hashes/byte oracles and direct SQL facts plus
returned records; do not generate the expected pin/envelope by invoking the same
serializer/validator being tested. Tests retain real native parents/locks until
workers and pools settle, then explicitly clean FixtureHome. No ignored new test,
lint suppression, generic crypto stub or disabled gate is an acceptance shortcut.

## Stage-sized implementation closure and gates

1. Characterization/behavior RED and exact current caller/deletion inventory;
   schema 0035 plus typed native identity extraction and external establishment
   owner, integrating all current MCP key consumers in the same completed slice.
   Commit only after migration/reopen/legacy and native affected checks pass.
2. Existing owned-Flow reservation/freeze owner extended with durable seed-private
   requirement, immutable storage binding, real envelope seal/read-back and cold
   authenticated storage inspection. Include all generic claim/reconciliation
   refusal paths and new-feature inner RED/GREEN; no dispatch of seed-required
   operations. Do not commit a bare public type without its consumers.
3. Pins and physical deletion closure must ship together: pin transactions,
   tombstone admission/inventory, native retained deletion, registry-only bypass
   removal and worker/crash/retry tests. Because reservation pins require this
   closure, slices 2/3 may be one intertwined dependency-complete checkpoint;
   never commit pins while a public deletion path still ignores them.

Expected paths: owned_flow/{preparation,private_inputs,private_files,launch,mod,
seed_objects,tests}.rs; work_items.rs and secure_lock.rs plus platform adapters;
state_home/windows/{namespace.rs,namespace/private_objects.rs} and existing Unix
private/run namespace owner; runs/{migrations.rs,registry.rs,storage.rs,error.rs,
host_settlement/mod.rs} plus a focused deletion module within that owner and next
registry SQL. Affected ordinary daemon/orchestrator/IPC/integration fixtures only
where the actual API/caller inventory requires. Core manifest remains unchanged
unless an independently reviewed concrete invariant demonstrates otherwise.

Gates are full-risk: persistence preparation/private/launch/recovery/registry/
migration/deletion tests; native Unix and Windows namespace/publication/deletion
fault suites; affected daemon/orchestrator integrations with current-build mock
identity; strict default/all-features all-target clippy, workspace fmt/diff,
doctests/docs baseline accounting, API review and complete affected dependency
closure using repo timeout/nextest conventions. Capture exact SHA/source inputs,
commands/env/exits, settled worker/native ownership and skipped tests separately.
MSRV and platform checks require actual execution or explicit pending limits;
local targeted green is not Windows/MSRV proof. Original outer remains genuinely
RED; stage 2 cannot claim semantic continuation, native all-eight-task/provider
acceptance or the full project goal.

## Maintainer pre-code verdict

**ACCEPTABLE for independent critic/security review.** Persistence owns private
storage identity, operations and pins; native state_home/private_files owns handle
identity; no dependency points upward to orchestrator/Git/UI. Reuse existing locks,
transactions, HMAC types/domains, immutable object publication and tombstone schema.
This deliberately changes the incomplete deletion owner rather than wrapping its
unsafe public bypass or inventing a parallel lifecycle. Active-dev public removal
and full callers are explicit. A strict maintainer should reject path-authenticated
establishment, marker-only rekey protection, half-ripple deletion checks, dummy
tombstones, premature launch seed capabilities, automatic abandoned-pin release,
or stage 2 tests presented as source/semantic/platform acceptance. No strategic
decision fork remains; root must freeze this exact scope and independent plans
must accept it before builder release. Counter remains **2/3**.


## Current checkpoint — Substep1 accepted locally, Stage2 OPEN

Substep1 external establishment/native key+marker identity/ordinary and legacy MCP
caller closure is accepted after repairs3–5. Frozen21-source digest
e1f08dce2310a96953db25ed7e00876a75e53ac2dd50d3129557bdebf612c069; current
4213/4213 affected tests and strict workspaceboth PASS. Independent5a and focused
5b quality/domain/security/API complete without blockers. Canonical shared count
5/5 exhausted with explicit human extensions; top historical2/3 is not current.
Full receipts and limits live in stage2/repair5/builder-handoff.md and gate-summary.json.
Substeps2/3 seed reservation/storage/pins/native deletion remainOPEN; separate
outer/deletion expectedREDs, empty-MCP HMAC core amendment and stages3–7 remain
required. Local commit is a Substep1 checkpoint, not complete Stage2 or habit app.
No push/merge/native provider/platform acceptance authorized or claimed here.


## Root-accepted Substeps2/3 legacy physical deletion criterion amendment

This appendix preserves the original first536 lines and explicitly narrows their
ordinary deletion compatibility criterion (lines360–374): positive physical deletion
is required only for NEW runs durably coordinated before first namespace creation.
Legacy runs retain history/read-only views, ordinary execution, continuation and
durable source/private pins. Their physical deletion returns typed
DeletionCoordinationUnavailable with zero filesystem or SQL mutation. A legacy
writer reopen never upgrades authority; missing/replaced NewControlled origin/control
never downgrades. Referenced deletion refuses and every deletion/cleanup bypass closes.
Root explicitly accepted this necessary-only scope under the latest human direction:
same habit/history/all8, codex-acp/exact gpt-6.1-sol, NO new UI. Stage6 new native UI
is deferred, not a prerequisite for CLI/daemon continuation; existing history support
and real same-app/all8/model evidence remain required. Exact closure is defined in
bootstrap-continuation-stage2-seed-storage-amendment.md; independent critic/security/API
pre-code re-review is required before writer release. Substeps2/3 remain OPEN.
No source/build/runtime/provider changes; canonical5/5 EXHAUSTED across stages/reviews,
not a reset or sixth repair dispatch.

### Controlled Unix database identity lifetime clarification

Before native-reader implementation, root/builder identified that the existing
Unix Database retains a raw events.sqlite File. Closing it while another SQLite
connection is live bypasses SQLite VFS deferred-close bookkeeping and can cancel
process POSIX locks. Owner and independent security accept a bounded NewControlled-only
correction: store validated native dev/ino facts under retained RunRoute parent and
shared outside control; use fstatat(AT_SYMLINK_NOFOLLOW), preserving original regular-file,
link-count, owner/security checks before and after SQLite-owned opening. No added
raw database descriptors for verification/identity. New database creation uses
SQLite APIs under original creation ownership outside SQL; metadata association
and admission must not expose a race before original identity is bound.
Legacy behavior and Windows retained native handles remain unchanged. No new
SQLite FD-layout FFI, global lifetime registry or permanent retention.
Pinned libsqlite3-sys0.30.1 sqlite3.c39488–39500 documents deferred close; primary
reference: https://www.sqlite.org/howtocorrupt.html section2.2.
Root accepts the focused security verdict under the explicitly cooperative Unix
threat model, not malicious same-user containment or native SQLite-FD attestation.
Actual-close,64-reader,replacement and cold-admission tests remain required.
This clarification is within initial feature implementation, not a post-review repair;
shared5/5 unchanged. Seed storage work was not paused.


### Human-approved registry read-only companion policy

The human explicitly approved only the SQLite registry companions described by
`bootstrap-continuation-registry-readonly-policy-proposal.md` (132 lines,
SHA256 `9cc9cf434e5865ad5e6155ebc4f51ef08863288870595f9c7fc48b44140ab64d`).
This resolves the initial Substeps2/3 policy fork; shared repair count remains5/5.
Only exact checked registry WAL without frames and SHM coordination additions are
permitted; original durable bytes/native identities, live-WAL visibility and all
production admission/locking checks remain. Test snapshots execute a no-SQL child
observer; prior failure evidence is retained. No VFS/flag/cleanup or runtime authority
is added. Original accepted document prefixes remain unchanged.
