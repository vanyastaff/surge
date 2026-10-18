# Stage 2 substeps 2/3 — seed-private metadata, storage and deletion closure

RESHAPED PROPOSAL; owner verdict **ACCEPTABLE** for independent pre-code re-review
under the root-accepted conservative legacy deletion criterion below. No writer release. Baseline independently
verified `77c5f1284f416c978738bf60e7ea8a989d7b2cdb`, clean before this document.
Reference original stage2 plan536 lines/SHA f1e267143979fce52d2a8a5e3bcdd3dc63660979c59141ff272b8b73802ec25d,
accepted parent first604 lines and stage2 allocation723–748; do not duplicate them.
This amendment resolves the empty-MCP invariant and completes their intertwined
storage/pin/deletion design. Substep1 establishment/key/marker/Attention stays intact.
Canonical **5/5 remains exhausted across all stages/reviews**. This is planning
original next-feature work, not a new repair allowance, sixth dispatch or reset.
Root must obtain independent critic/security/API acceptance before writer release.
Only this owning document is written; no source/tests/Cargo/runtime/provider/Git mutation.

## Source-bound decisions and ownership

Core flow_inputs.rs:454–468 rejects Empty+Hmac; ordinary daemon owned_flows.rs:34
derives private request from explicit nonempty MCP. Persistence begin/replay takes
that bool before any manifest exists; private_inputs::freeze returns early for
empty servers. Therefore persistence-only seed support cannot satisfy the parent.
Core owns public structural metadata; persistence owns authenticated bytes and
independent operation association. Neither owns semantic eligibility or original
workspace execution authority here. Those remain stages3/4; endpoint/atomic bridge
acceptance remains stage5; existing history support stays. New Stage6 native UI is
DEFERRED, not a CLI/daemon prerequisite; same habit/all8/exact Sol evidence remains.

| Option | Trade-off / decision |
|---|---|
| A: explicit validated continuation requirement, v2 continuation manifest | **Choose.** Expresses why privacy exists independently of MCP; legacy ordinary v1 stays exact. Requires complete constructor/serde/storage/caller migration now. |
| B: remove Empty+Hmac rejection globally | Reject: loses the continuation discriminator and permits unrelated ordinary combinations without the independently stored requirement. |
| C: second continuation startup wrapper/lifecycle | Reject: duplicates input association, hydration, idempotency and claim contracts; existing operation owner already supplies them. |

No dummy MCP entry, unchecked allow bool, bool compatibility overload, fabricated source
proof, public activation constructor, new dependency or verification/sandbox policy.

## Core metadata and compatibility API

Add core `ContinuationInputRequirement` with PRIVATE source_bootstrap/source_run
fields, getters and validated constructor/Raw+try_from Deserialize: both IDs nonnil;
unknown/duplicate fields reject. Do NOT invent source_bootstrap != source_run; their
roles are checked by the durable Bootstrap association. source_bootstrap is
the Bootstrap OPERATION identity, not planning RunId. Runtime manifest additionally
rejects successor run equaling either source identity and workspace_owner differing
from source_run. These are correlation checks, not authentic source/Git proof.

Add typed `OwnedFlowInputPurpose::{Ordinary, Continuation { source }}`. Preserve
existing seven-argument `OwnedFlowInputsManifest::new` as the legitimate ordinary
v1 constructor unchanged. Add `new_continuation(ContinuationInputsFields)` and
`purpose(&self) -> &OwnedFlowInputPurpose`; the raw field aggregate carries operation,
request_auth_tag: HmacSha256Tag, binding, run, workspace_owner, workspace, mcp and
source: ContinuationInputRequirement. It has no Deserialize/capability. This avoids
an eight-argument shim or lint exemption. Manifest fields remain private; its Raw
decoder validates through the same private rules. Use existing Identity and
RequestIdentity structural errors, not a second validation taxonomy. This refines
API drafta4062008: source-source equality is permitted, both sources nonnil.
Purpose/requirements enums deliberately stay closed for exhaustive security review;
future variants and field-literal growth are MAJOR-on-paper. Add #Errors/must_use
and structural-only docs; update Hmac identity docs beyond MCP without granting authority.

Ordinary serializes EXACT legacy schema1 shape/field order/invariants: purpose is
absent, Empty requires PlainPublic, Private Explicit requires Hmac, Private
HostDefault retains its existing public/private-request choices. V1 decode cannot
smuggle a continuation purpose. Continuation serializes schema2 with explicit typed
purpose; schema2 requires Continuation/Hmac, independently of Empty versus Private
MCP. Preserve Explicit/HostDefault empty selection. No full seed manifest, payload,
storage key or source proof is duplicated in this startup projection. V2 purpose
contains only the validated requirement; actual seed manifest stays separate.
Custom wire/Raw validation rejects schema2 missing purpose, schema1 with purpose,
unknown versions/variants and mismatched run/source ownership. Old readers cleanly
reject v2; ordinary v1 serialized bytes/canonical HMAC inputs stay unchanged.
No new EventPayload is needed; existing payload migrator tests must prove this
manifest-version boundary and old-reader refusal without silently upgrading v1.

## Provisional reservation before manifest: actual host-storage API

Replace begin/replay's bool with persistence `OwnedFlowPreparationPurpose`:
`Ordinary(OrdinaryRequestPrivacy::{Public,Private})` or
`Continuation(ContinuationInputRequirement)`. This metadata is consumed by the
functional reservation API now; ordinary daemon maps its existing selection to
the equivalent Ordinary value. This SUPERSEDES the API draft's Requirements/bool
section: there is ONE canonical surface (both closed enums, no bool alias):
```
replay_owned_flow(&self, operation: WorkItemOperationId, canonical_body: &[u8],
    purpose: OwnedFlowPreparationPurpose) -> Result<Option<OwnedFlowReceipt>, WorkItemError>
begin_owned_flow(&self, operation: WorkItemOperationId, canonical_body: &[u8],
    purpose: OwnedFlowPreparationPurpose, now: i64)
    -> Result<OwnedFlowPreparationResult, WorkItemError>
```
No daemon continuation transport endpoint is added.

Continuation always acquires the existing PRIVATE operation lock, opens original
PrivateOwner and HMACs a canonical preimage containing typed purpose+requirement+
exact canonical request bytes under `surge/owned-flow/continuation-request/v1\0`.
Canonical sorted JSON preimage is schema_version:1, purpose:{kind:continuation,
source:{source_bootstrap,source_run}}, body_digest:ContentHash::compute(body), with
existing RunId/ContentHash serde. Freeze exact bytes independently; terminal NUL
is a domain byte. No alternate ordinary digest is accepted for continuation.
Ordinary request/MCP HMAC domains and bytes remain unchanged. Verify using immutable
SQL purpose, never the supplied public manifest as the discriminator. Do not create
or expose item/run/preparation identity before durable private-required reservation.

Under existing operation lock resolve the durable BootstrapOperationRecord using
the operation ID, validate supported record and source_run == actual implementation
run, and obtain ACTUAL planning_run. Reuse its typed decoder with a supplied checked
connection; do not call pool-borrowing BootstrapOperationStore::get inside a held
transaction. Source snapshot/version/fingerprints are frozen correlation facts;
stage4 still authenticates terminal/prefix/checkpoint/writer eligibility.

Use next append-only registry migration **0036+**; committed0035 is NEVER rewritten.
Extend owned_flow_operations with immutable typed purpose (legacy absence=Ordinary
with original identity semantics); add seed reservation/association and normalized
dependency tables. Unique source implementation maps to one operation through
abandonment/acceptance. Store resolved planning/implementation run IDs, Bootstrap
identity/fingerprints and establishment nonce. No second operation state/token FSM.
In ONE BEGIN IMMEDIATE insert original operation/seed requirement and source pins,
checking source registration/association/tombstones before committing IDs to caller.
HMAC/key I/O is outside SQL; existing external Establishing record prevents rekey
if death occurs before this transaction. Exact replay checks purpose+body+source;
changed purpose or body conflicts before source/configuration effects.

private_dependencies must decode every typed row and count seed-required rows in
provisional/frozen/abandoned/accepted states, even with no startup manifest yet.
Corrupt/partial rows refuse, not json_extract-false. Original PrivateOwner and
operation lock travel with preparation through actual worker exit. Legacy no-seed
public empty-MCP behavior remains independent; key/marker permanence is unchanged.

## Functional storage sequence without circular startup binding

Order: reserve purpose/pins → freeze source/storage binding → seal seed/read-back
→ freeze MCP/startup projection. InputBinding gains conditional continuation-purpose
correlation omitted from ordinary v1 bytes; it does not require the startup manifest
or seed tag to exist. Thus sealing the seed never MACs a manifest containing its
own tag. Existing core ContinuationSeedManifest/Fields, SourceEventRef, SeedId,
ContentHash, OwnedFlowObjectRef and HmacSha256Tag remain the shared identities.

Public persistence `SeedStorageBinding` has PRIVATE fields/getters, constructed by
`SeedStorageBinding::new(SeedStorageBindingFields) -> Result<Self, WorkItemError>`.
Raw fields (no Serialize/Deserialize) are exactly operation:WorkItemOperationId,
run:RunId, binding:WorkItemBinding, workspace_owner:RunId,
workspace_identity:ContentHash, source:ContinuationInputRequirement,
boundary:SourceEventRef, prefix_hash:ContentHash, terminal:SourceEventRef,
terminal_hash:ContentHash, failure_tail:Option<SeedFailureTail>,
source_checkpoint:ContentHash, graph_hash:ContentHash. SeedFailureTail has PRIVATE
through:SourceEventRef/hash:ContentHash, validated new/getters; no semantic proof.
All references have source.source_run(); terminal.seq >= boundary.seq; a tail is
required exactly when terminal.seq > boundary.seq, through == terminal; nil IDs,
zero binding versions, successor/source collision and owner mismatch reject Invalid.
Hashes use existing ContentHash validation; fields do not assert actual event kind.
No public wire decoder is needed. Private stored Raw decodes deny_unknown_fields
through these constructors; source association is independently checked against SQL.
No object_ref/auth_tag/SeedId is caller input: original private storage generates
reference/tag and derives SeedId from canonical header+payload, then validates the
existing ContinuationSeedManifest output (whose Fields cannot replace this input).
```
OwnedFlowPreparation::freeze_seed_storage(&mut self, binding: &SeedStorageBinding,
    payload: &[u8]) -> Result<ContinuationSeedManifest, WorkItemError>
```
This is real immutable bounded storage; Stage4 authenticates source eligibility.
Shared `seed_objects::verify_storage(...original PrivateOwner..., stored association)`
is PRIVATE and returns private AuthenticatedSeedObject; used by freeze read-back,
cold integrity and startup freeze. No public proof type or unused verify facade.
Allocate/persist pending object ref and its dependency before publication; immutable
retry reuses it. Separate seed-envelope HMAC binds domain/version, establishment,
purpose/request HMAC, operation/run/binding/workspace/source facts, object ref,
payload version/hash/bytes and SeedId. Cap ENCODED object at existing8MiB, bounded
decode/canonicality/unknown-field checks, read-back under original PrivateOwner,
then freeze exact manifest/association in short SQL transaction after revalidation.
Missing frozen/pending object refuses; do not recapture mutable source or mint a
replacement ref/key. Store no semantic RunMemory, secret transport or private body
in registry/public JSON. New successor MCP objects retain their separate domains.
Private AuthenticatedSeedObject is nonserializable, holds bounded bytes+original
PrivateOwner and is consumed by freeze read-back/cold storage integrity. Stage4
later exposes actual read hydration separately from AuthenticatedExecutionSeed;
neither a structural SeedStorageBinding nor public manifest is either capability.
No unused public activation facade or dummy source-approved token is added.
Startup freeze for Continuation requires the exact stored sealed seed, HMAC purpose,
original PrivateOwner and matching correlation, even when servers is empty. Attach
that same owner to preparation; empty-MCP early return cannot discard it. Existing
ordinary freeze/manifest/replay behavior remains exact. Source bytes/checkpoints/
HEAD/index are not restored, rewritten or opened as an executable workspace here.
## Independent refusal until actual later authorities exist
Add truthful `WorkItemError::ContinuationExecutionUnavailable` for HEALTHY stored
continuation lacking later execution owners, distinct from corruption/private auth,
Busy and permanent InputsAssociationMismatch receipts. Missing key/marker/object
uses PrivateInputsRecoveryRequired; bad MAC/object/association uses PrivateInputsCorrupt;
unsafe/unsupported native state retains its actual typed private error. Missing
registered source is NotFound, changed immutable retry Conflict, contradictory
accepted SQL IndependentAcceptedDataInvalid. Keep private_unavailable::known
whitelist unchanged; healthy unavailable never becomes Attention. Core metadata validation
does not become stage3 workspace admission or stage4 semantic validation.
Read immutable SQL purpose BEFORE ordinary finalize, claim/claim_reconciliation,
quota-wake Ready classification, pending/effect admission, manifest/startup
acknowledgment or runtime hydration can grant execution. If continuation-required,
authenticate storage for inspection where appropriate but return typed unavailable
from GENERIC execution paths; never Ready, Legacy fallback, unauthenticated claim,
ordinary receipt/outbox, or source session authority. Check independently of
whether a seed marker/manifest was omitted or replaced with ordinary v1 metadata.
Ordinary finalize cannot accept a continuation in stage2; stage5 extends its SAME
atomic acceptance owner after real stages3/4 owners exist. Test accepted-shaped
seed rows only via private fixtures to prove generic refusal, never a production
acceptance bypass. No continuation marker writer or engine fold hydration yet.
## Pins plus actual physical deletion: one inseparable change
Retain dependency rows for actual planning/source run history and pending/frozen
private refs through abandoned/canceled workers and accepted lineage. No release
API/automatic GC in this stage. Current artifact/private cleanup only removes held
unpublished staging objects; published refs are never inferred temporary. A pin
confers neither source writer exclusion nor execution authority.
Pin creation and deletion admission use SAME registry BEGIN IMMEDIATE. Pin checks
real source association, expected identities and run_host_tombstones; deletion checks
all dependent operations without lifecycle filtering. Pin-first explicitly refuses
delete; tombstone-first refuses pin, including absent run rows and cold restart.
Use immutable triggers plus typed transaction checks; no out-of-transaction oracle.
Current storage.rs:583–615 unregisters then recursive path-deletes; registry.rs:243–257
exposes unchecked unregister. Host_settlement::admit only READS0032 tombstones;
there is NO production inventory/intent writer. RunRoot/RunRoute/Database in
host_settlement/unix.rs and native Windows RunRoute supply retained identities,
but do not implement inventory-bound deletion. Extend these owners, not a parallel
tombstone protocol. Remove public pool-only registry::delete_run and restrict
Storage's direct private delete_run_connection call to the admitted-owner
transaction helper. Preserve Storage::delete_run's public signature. Add typed
StorageError::RunReferenced { run_id } and DeletionOwnership { run_id, source: OpenError };
OpenError is already publicly reachable through runs/error and re-export. Keep
NativeError private; do not flatten policy/native ownership through existing string
adapters. WriterStillActive is ONLY a proven in-process writer; shared-use/exclusive
delete contention with unknown reader/process holder is StorageError::RunInUse { run_id }.
Pool/SQL/I/O errors
retain their actual meanings, not fake private corruption.
Existing RunCreationReservation/PreparedOpening carries only the IN-ROOT journal
FileLock; it does not survive deletion. Add ONE host_settlement per-run control
owner OUTSIDE the removable run, at retained home/run-controls/<RunId>.lock,
using existing descriptor/native primitives with SHARED normal-use / EXCLUSIVE
deletion modes. Journal FileLock remains separately exclusive for the sole writer.
This control is use/deletion exclusion,
not a second tombstone or lifecycle. Keep its file permanently; never unlink/rekey.
Next0036+ immutable run_host_controls binds run_id, runtime/controls-parent native
identity and control-file native identity. Tombstone binds that control plus existing
original opening, outside-parent, root/journal identities and versioned inventory.
Native IDs, not path spelling, authenticate. Windows retains actual native handles;
Unix relies on cooperative participants, not malicious same-user containment.
First control establishment: create-exclusive+sync file/parent outside SQL, hold
its EXCLUSIVE native lock, then bind exact descriptor identities in IMMEDIATE transaction
before ANY run namespace creation/mutation. Existing control row requires existing
original file/name/parent match; loss/replacement refuses, no recreate. A first
publication crash leaving unbound existing control refuses ambiguous adoption.
Immutable coordination origin is recorded BEFORE first namespace creation, not
inferred from current files or an ordinary writer opening. Next migration explicitly
classifies all existing registered/opened runs Legacy; immutable origin record and
control binding for NewControlled are inserted under original creation reservation.
Origin rows survive deletion; immutable creation/opening rows reference their origin.
Triggers/FKs prohibit downgrade/delete; crosscheck opening/creation origin before fallback.
Unknown/ambiguous historical roots are Legacy for reads, NEVER NewControlled.
NewControlled requires original control row/file/name/parent on every admission;
missing/replaced/malformed facts refuse, never fall back to legacy/no-owner behavior.
Crash before durable origin exposes no run namespace/identity; orphan control first
publication refuses. Crash after origin before namespace allows only original
creation reservation/recovery semantics, not arbitrary adoption. A Legacy writer
opening does not upgrade origin. Registry-only/no-intent is not physical-success;
wholly unknown run NotFound. No filesystem probe mints deletion-safe origin.
Concurrent first establishment uses one create-exclusive original control owner.
For NewControlled, ALL opening paths acquire SHARED before SQL admit/run-FS access:
reserve_new_run, acquire_opening, open_existing_sync, bind_reserved and their
create/open/fork/info/recovery callers. Move owner through RunCreationReservation,
PreparedOpening, WriterLease and actual workers/pools; bind_reserved consumes SAME
control (no reacquire). Initial exclusive creation releases after durable binding;
then reacquire shared and recheck before opening. Hold shared to actual final close.
Recheck tombstone inside final transaction; stale pre-admit cannot create a namespace
after deletion wins. Legacy uses existing writer protocol and remains nondeletable;
no healthy NewControlled path may enter via that legacy route.
No SQL transaction spans file enumeration/hash/create/delete; split existing
reserve/bind filesystem work out of registry closures under the retained control,
then short SQL commit with native verification before/after it under SAME control.
No native syscall inside SQL. Inventory every direct run FS creator.
Initial deletion: resolve immutable origin using SQL read only. Legacy returns
StorageError::DeletionCoordinationUnavailable { run_id } with ZERO filesystem or
SQL mutation, BEFORE owner creation/unregister/intent. Pins/history stay intact.
NewControlled acquires EXCLUSIVE outside control, reads tombstone (connection
dropped), then acquire authentic existing root/journal writer exclusion and bounded
inventory WITHOUT RunRoot.run(false), which creates missing artifacts. Add an
existing-only deletion route that permits genuinely absent recorded directories.
No intent + absent artifacts/lock/root is uncertain original ownership, not normal
resumption; refuse without side effects. Prepare inventory outside SQL; IMMEDIATE
pin-check + persist actual0032 intent/inventory + unregister using private helper,
then release SQL before cleanup. Both public registry wrapper and Storage's direct
private delete_run_connection bypass must be removed/restricted to admitted owner.
Cleanup order: delete inventoried contents, close journal/database handles as native
platform requires, remove original journal lock late, remove original root last;
outside control remains locked throughout and permanently exists afterward. Persist
Deleted in a short transaction after identity-bound root absence observed under
original retained outside parent/control. Every partial step is resumable. Caller
cancellation transfers owner to actual blocking worker until join/exit; retryable
errors do not mint fresh authority. Competing process cannot retry/open until release.

Cold resume: acquire EXCLUSIVE existing control authenticated against durable record;
load immutable DeletePending and inventory before touching run path. Existing root
must match recorded identity; missing inventoried children/lock/artifacts are allowed
as already removed, never created. Missing entire root under original parent/control
means cleanup reached root removal: verify absence and mark Deleted without journal
lock reacquisition. Thus crash after lock/root removal but before SQL converges.
Replacement root/entry is refused and preserved. Unknown extra entries, malformed
inventory/control or changed parent refuse; no recursive path delete or reject-all.
Concurrent retries serialize on SAME permanent native control; second sees current
intent/state after first settles. Deleted+authentic absence is idempotent; permanent
tombstone blocks same-RunId resurrection; ordinary distinct RunIds remain unaffected.

### Controlled readers and conservative legacy deletion criterion

For NewControlled, shared use is cross-process, compatible with writer plus64
reader pools; deletion exclusive conflicts with ALL live use, not just writers.
Existing writer journal FileLock still enforces one writer. No shared-to-exclusive
upgrade while retaining shared owner: deletion returns typed contention; close real
readers/writer first. Outside control acquisition is nonblocking, not busy spinning.

Storage::open_run_reader314–369 resolves origin; NewControlled acquires shared BEFORE run
path/SQLite access, rechecks typed tombstone after acquisition, then passes original
owner to blocking pool creation. start_opened_writer409 instead calls PRIVATE
open_run_reader_owned(run, Arc<RunUseOwner>) using SAME WriterLease admission owner;
no public unowned token, lock reacquire or selfBusy. reserve/bind uses same owner.
Reader manager AND each RetainedConnection/reconnect/init/validation closure retain
Arc owner independently through actual connection close (Windows fences included),
not merely RunReader/drop. Windows OwnedSqliteConnectionManager already retains
namespace independently; Unix currently aliases raw r2d2_sqlite/Connection and needs
a real retained wrapper, not a with_init temporary. Existing checked-close failure
returns/retains the ENTIRE owner. Manager reconnect verifies native original identity and
tombstone before opening any run SQLite handle. Registry connection is short-lived,
released before run FS/SQL; manager must not nest registry-pool borrows. Canceled
spawn_blocking and delayed manager reconnect retain owner until actual settlement.
Delete cannot win while manager/connection exists; post-release stale async work
must reacquire/recheck and sees tombstone, never recreate/open replacement database.

inspection::read_event_range381 and read_folded_with_gate_receipt882 and ALL other
production direct read-only sites resolve origin, taking controlled shared admission
and retain it through transaction/statements/connection close; path-only signature
becomes private origin/owner-aware helper. Registry-only inspection95/116/133/179 remains
UNCHANGED: no per-run lease, run FS access or migration for registry queries.

Root EXPLICITLY ACCEPTED criterion amendment: the original Stage2 healthy unpinned
deletable promise is restricted to NewControlled runs durably coordinated BEFORE
first namespace creation. Legacy physical deletion refuses truthfully, while legacy
history/reads/continuation/storage/source pins retain behavior. This is conservative
scope narrowing, not a claim old readers are coordinated. No writer adoption upgrades
Legacy to safe-delete; no automatic/manual adoption helper is introduced here.
Legacy readers need NO new owner and perform no control/row/file creation. They
continue original read-only behavior because physical deletion can never admit for
their origin. No new UI. NewControlled missing origin/control must not downgrade:
immutable creation/opening associations distinguish corrupt missing rows from true
legacy; retain normalized origin/tombstone facts permanently and validate them.

All public/private deletion and cleanup entry points consult origin AND dependencies;
referenced deletion remains explicit RunReferenced, unreferenced Legacy uses exact
DeletionCoordinationUnavailable. Neither removes bytes, unregisters, creates control,
adds tombstone or changes origin. Ordinary distinct newly created runs can delete;
Legacy source continuation is allowed and pins remain durable across all states.
Unsupported historic DeletePending/test-format intent is typed refusal, not invented
resumable authority. Genuine new producer intents retain positive cold resumption.

Discarded extra database-FD lock bridge stays FORBIDDEN: SQLite documents that close
of any descriptor for a database can clear its POSIX advisory locks, even if another
connection still uses it: https://www.sqlite.org/howtocorrupt.html#posix_advisory_locks_are_canceled_by_a_close
No flock-independence assumption or speculative Windows reserved-byte mechanism.

Required positives: nested writer startup;64 controlled reader pools/live writer;
legacy viewer/history/live writer unchanged with no new bytes/files/rows. Required
negative controls: legacy physical deletion zero-side-effects, including after writer
reopen; NewControlled missing/corrupt/deleted-origin/control cannot downgrade; pins
on legacy/new source block bypasses. Real competing process tests retain reader
manager/checked-out connection/canceled worker through close; cold reader/delete both
orders; stale reconnect; actual native close ordering. No sleep-only/unowned fallback.
Direct-reader inventory includes runs/verification, work_items::read_usage,
control::confirm_suspension, recovery_cycles handoffs/rate_limit/predispatch/opening,
owned_flow/refusal_delivery startup inspection; task claim is not read ownership.

The existing Unix characterization records REAL native identities but uses
`test_inventory_version` fixture encoding. Preserve its negative assertions; it
is NOT future producer/consumer interoperability proof. Add actual-producer positive
delete/reopen/retry tests AND actual-producer inventory challenged by replacement.
Passing by rejecting every inventory is not completion. Existing test-format rows
must fail closed rather than be mistaken for executable native deletion evidence.

## Complete write zones, tests and gates

Core: work_item/flow_inputs.rs (types/fields/wire), exports and migration/serde tests.
Persistence: owned_flow preparation/private_inputs/seed_objects/launch/refusal/
private_establishment dependency scan; work_items errors/exports; bootstrap decoder
connection seam; migrations0036+; runs registry/storage/error/host_settlement deletion
owner, outside-control migration/opening/reader/inspection/pool/connection closure
and Unix/native Windows adapters.
Actual consumers include daemon owned_flows,
daemon work_items wake match, orchestrator engine/{work_items,admission/startup},
refusal_delivery startup comparison, all manifest constructors/fixtures/doctests.
Refresh exact symbols, including external/integration callers; no silent old bool.

Before production changes preserve current genuine rejection/deletion REDs. Add
fixed current-signature core Empty+Hmac constructor/serde expectation tests, clearly
separating intended new-purpose tests requiring type skeleton (compile is setup,
not RED). Functional inner REDs cover public reserve→seal/read-back→empty startup,
immutable SQL purpose, distinct request domains, all malformed/missing purpose/version
forms, v1 exact bytes, both selections, no IDs before durable pins and no generic
execution with omitted/ordinary marker. Use independent fixed bytes/SQL facts.

Require no-MCP first establishment; whole namespace/key/marker/object loss/replacement
at provisional/frozen/abandoned/accepted-shaped cold states, old/new operations;
foreign valid envelope/ref/purpose/source transplant and size/canonicality failures;
crash before/after object pin/publication/freeze; real workers canceled before exit.
Pin/delete races must synchronize real independent connections, test both commit
orders/rollback/preexisting tombstone, source association substitution and actual
native cleanup crash/retry/replacement. Actual producer tests stop after contents,
after artifacts, after journal lock, and after root removal BEFORE Deleted SQL;
cold retries must converge with no mkdir/recreated lock. Test simultaneous cold
retries, original/replaced outside control and parent/root, no-intent registry-only
states, stale opening-admit racing delete and stale delete racing pin. Prove healthy
original delete and already-removed original absence, not just refusal. Retain source
checkpoints/index/HEAD and truthful lifetime/group policy. Fresh-open max2/held1
public regressions remain mandatory; no nested pool getter while connection held.

Substeps2+3 are ONE dependency-complete implementation checkpoint: no bare core type,
pins-only commit or published storage method while deletion bypass remains. Tests
can be developed incrementally, but no completed slice/commit until reservation,
seal/read-back, all generic refusals AND physical deletion closure agree. Run full
affected eight-package nextest, strict workspace all-target default/all-features,
doctests/docs baseline accounting, current mock/native/recovery/IPC/migration gates,
fmt/diff and independent5a+5b security/domain/API. Public bool signature migration,
new exhaustive error variants and removed registry-delete API are deliberate
active-dev MAJOR-on-paper breaks; ordinary constructor stays unchanged, new
continuation constructor/types are additive. Do not incidentally add non_exhaustive; v2 is old-reader-incompatible, v1 remains exact. No release/publication.
MSRV/Windows/Linux need real evidence or explicit pending limits; macOS green is
not portability proof. Outer remains realRED; deletion RED must become genuinely
producer-backed GREEN. No skips/retry masks/weakened gates or blanket TDD claims.

Owner **ACCEPTABLE** for independent re-review under the explicit narrowed legacy
delete criterion; metadata/storage/use/deletion boundaries are concrete, not released.
Full stage2 and same habit/history/all eight tasks/exact Sol/model/native acceptance
remain open. Root release is still required; canonical5/5 remains exhausted.

Final focused erratum (root/API accepted): RunReferenced takes precedence for any
referenced target; unreferenced Legacy returns DeletionCoordinationUnavailable.
Unknown shared-use/reader/cross-process contention returns RunInUse { run_id };
WriterStillActive is reserved for a proven in-process writer. Coordination origin
is privately validated storage metadata, never a public authority/capability.


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
