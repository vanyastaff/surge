# Stage2 input purpose — API/wire draft
DRAFT for owning maintainer and independent pre-code review. No Rust/test/build or
provider action is authorized by this note. This is planned Substeps2/3 design,
not a sixth repair; canonical shared repair budget remains **5/5 EXHAUSTED**.
History: supersedes draft52ad55626fede2764b376973ff47871e3d33b6e0ccee04f13c2eb0680abf7bc2.
Base:77c5f1284f416c978738bf60e7ea8a989d7b2cdb. Parent first604 lines/Stage2 remain binding.
Owning seed-storage-amendment takes precedence; it owns reservations/pins/deletion.
## Options and tactical choice
A. Explicit validated continuation purpose, v2 inputs projection, durable SQL seed
association: **choose**. Separates MCP selection from seed-private requirement and
preserves ordinary v1 bytes. Startup metadata correlates; storage authenticates.
B. Embed full ContinuationSeedManifest in inputs: coherent but duplicates frozen
reference/tag, couples freeze order and enlarges public projection unnecessarily.
C. Keep only SQL purpose and relax v1 Empty+Hmac: reject. It silently changes v1
ordinary invariants and gives no explicit public purpose. Dummy MCP, boolean bypass,
public authenticated capability and infallible compatibility overload are rejected.
## Concrete public core surface
Names below are proposed exact names; use existing RunId/WorkItemBinding/workspace
and HMAC types. Source Bootstrap is a semantic role of RunId, not a new ID framework.
```rust
pub struct ContinuationInputRequirement { /* private source_bootstrap/source_run */ }
impl ContinuationInputRequirement {
    pub fn new(source_bootstrap: RunId, source_run: RunId)
        -> Result<Self, OwnedFlowManifestError>;
    #[must_use] pub fn source_bootstrap(&self) -> RunId;
    #[must_use] pub fn source_run(&self) -> RunId;
}
pub enum OwnedFlowInputPurpose {
    Ordinary,
    Continuation { source: ContinuationInputRequirement },
}
/// Unvalidated construction fields; no capability and no Deserialize implementation.
pub struct ContinuationInputsFields {
    pub operation: WorkItemOperationId,
    pub request_auth_tag: HmacSha256Tag,
    pub binding: WorkItemBinding,
    pub run: RunId,
    pub workspace_owner: RunId,
    pub workspace: WorkItemWorkspace,
    pub mcp: FrozenOwnedFlowMcp,
    pub source: ContinuationInputRequirement,
}
impl OwnedFlowInputsManifest {
    // Existing seven-argument new(...) stays the ordinary constructor unchanged.
    pub fn new_continuation(fields: ContinuationInputsFields)
        -> Result<Self, OwnedFlowManifestError>;
    #[must_use] pub fn purpose(&self) -> &OwnedFlowInputPurpose;
}
```
The field aggregate avoids an eight-argument constructor/lint exemption without
creating a second validated binding framework. Its public fields are deliberately
raw constructor input, like existing ContinuationSeedFields; only the manifest's
private representation is invariant-bearing. Existing new is the ordinary owner,
not a compatibility shim. Internal shared validation may be factored privately.

Requirement.new rejects nil source IDs only. Equality between source Bootstrap and
source implementation IDs is structurally allowed; independent stored Bootstrap
association proves their semantic roles. Constructor/deserializer validates
successor differs from both source IDs,
workspace_owner equals source_run, and all existing identity/workspace/MCP rules.
The HMAC-tag constructor field makes PlainPublic impossible on this Rust path;
untrusted JSON must independently enforce Hmac. This validates correlation only:
no proof that either run exists, is Bootstrap/Failed, or owns the workspace.

Requirement derives serde through a private deny_unknown_fields raw struct and
validating TryFrom. Manifest retains private fields and version-aware validating
raw decoding. Raw construction carriers are never accepted as authenticated data.
Requirement.new and new_continuation need #Errors documenting structural refusal;
fallible Results are already must-use. All read accessors retain #[must_use]. Doc
stubs state structural-only validation, unchanged MCP selection and separation from
semantic/workspace authority. Update OwnedFlowRequestIdentity::Hmac docs: it covers
explicit private transport bytes OR continuation-private requirements; wire unchanged.

## Exact wire/compatibility policy

Ordinary: schema_version1 and all existing fields, no purpose field emitted.
Existing new always emits v1 and preserves existing request/MCP matrix:
Empty Explicit/HostDefault + PlainPublic valid; either Empty + Hmac invalid;
Private Explicit requires Hmac; Private HostDefault permits PlainPublic or Hmac.
Private entries remain nonempty/ordered/unique with ordinary policy checks.

Continuation: schema_version2, same common fields, plus required
`"purpose":{"kind":"continuation","source":{"source_bootstrap":RUN_ID,"source_run":RUN_ID}}`.
RUN_ID uses existing RunId serde, not Display spelling. request_identity is the
existing `{"kind":"hmac","tag":"hmac-sha256:..."}` shape. Genuine Empty Explicit
and HostDefault are valid here, as is normal nonempty Private MCP. Do not put seed
payload, key, private path, full frozen seed manifest or fabricated MCP into purpose.

Reject v1 with any purpose field (including null), v2 missing/null/ordinary purpose,
unknown version/kind, contradictory IDs, PlainPublic v2, duplicate/unknown fields,
mixed Empty/private MCP fields and malformed existing references/tags. Preserve
presence versus absence in raw decoding; a default Option alone cannot enforce
v1-null rejection. Do not use permissive JSON Value roundtrips that erase duplicates.
Old readers refuse v2; new readers accept and reserialize ordinary v1 unchanged.
Never rewrite legacy ordinary manifests or MCP authenticated bytes as v2.

## Public persistence entry signatures and immutable request identity

```rust
pub enum OrdinaryRequestPrivacy { Public, Private }
pub enum OwnedFlowPreparationPurpose {
    Ordinary(OrdinaryRequestPrivacy),
    Continuation(ContinuationInputRequirement),
}
// Replace explicit_private: bool in existing methods, same remaining arguments:
pub fn replay_owned_flow(&self, operation: WorkItemOperationId, canonical_body: &[u8],
    purpose: OwnedFlowPreparationPurpose) -> Result<Option<OwnedFlowReceipt>>;
pub fn begin_owned_flow(&self, operation: WorkItemOperationId, canonical_body: &[u8],
    purpose: OwnedFlowPreparationPurpose, now: i64)
    -> Result<OwnedFlowPreparationResult>;
```

These are host-storage purposes, not transport request variants. Ordinary Public/Private
map exactly to previous public/private request, OS-lock and identity behavior. Continuation requires
private operation lock and retained establishment independently of MCP. Persist
immutable requirement/source association and source pins before exposing identity;
exact retries compare requirements, source IDs and body identity. Legacy rows are
Ordinary only under explicit migration semantics; required data never defaults away.

Ordinary request v1 domain/preimage remain byte-identical. Continuation HMAC uses
new domain `surge/owned-flow/continuation-request/v1\0` and canonical sorted JSON:
`{"schema_version":1,"purpose":{"kind":"continuation","source":{"source_bootstrap":RUN_ID,"source_run":RUN_ID}},"body_digest":CONTENT_HASH}`.
CONTENT_HASH is existing ContentHash::compute(exact canonical_body), encoded by its
existing serde. Domain bytes include terminal NUL; the displayed backslash is notation.
Freeze these exact keys/serde forms with fixed bytes, not a mirrored MAC oracle.
This binds purpose/source/body without changing the existing public Hmac identity.
Replay selects preimage from immutable stored requirements and compares caller
requirements; never select a weaker preimage from untrusted current manifest data.

MCP InputBinding must bind continuation purpose for v2 server/envelope objects;
ordinary v1 objects omit it exactly. Seed envelope separately authenticates required
purpose/source plus existing operation/request/run/binding/workspace and frozen seed
association. Its separate domain is owned by the broader storage amendment.

## Storage/authentication versus dispatch

Persist required seed independently of startup metadata and frozen manifest. Reuse
existing ContinuationSeedManifest for frozen metadata; do not duplicate that DTO.
SeedStorageBinding construction/fields, freeze_seed_storage Result and shared private
verifier follow ONLY [the owning amendment](bootstrap-continuation-stage2-seed-storage-amendment.md).
Required manifest/ref/pins absent, corrupt or mismatched is refusal, not ordinary success. Marker absence cannot erase the durable requirement.
Empty MCP freeze/hydrate may return zero servers but must retain/verify private
owner and seed-storage association for continuation. No successful storage API
mints semantic seed/workspace adoption/execution authority. Generic ordinary
finalize/claim/reconciliation/dispatch must refuse continuation until actual later
owners exist; no public skip flag. AuthenticatedSeedObject remains private.

## Errors, semver and exact migration closure

Reuse OwnedFlowManifestError::Identity for malformed source/version correlation,
RequestIdentity for disallowed purpose/request/MCP combination, and existing
Snapshot/Workspace/ObjectReference/AuthenticationTag checks. Broader error-owner
proposal may name narrowly required new errors; this draft authorizes none itself.
Storage uses existing typed Conflict/private/native/independent-association errors;
required-seed execution refusal must stay distinguishable from ordinary absence.

Replacing public bool signatures is intentional MAJOR-on-paper; no bool overload.
New types/constructor/accessor are additive. Keep core OwnedFlowInputPurpose and
persistence OwnedFlowPreparationPurpose/OrdinaryRequestPrivacy CLOSED: their complete alternatives are a reviewed security
matrix; new purposes require explicit consumer decisions and another MAJOR-on-paper
change, not wildcard fallthrough. No #[non_exhaustive] is proposed for these enums.
ContinuationInputsFields deliberately permits external struct literals; it cannot be
#[non_exhaustive] without another constructor. New required fields would therefore
be MAJOR-on-paper under the active-development policy. The invariant-bearing
Requirement/manifest fields stay private. No release/version action is implied.
Migrate daemon owned_flows replay/begin and daemon mcp_runtime_ownership fixture;
persistence preparation/private_inputs/private_establishment/launch/refusal and
all direct persistence callers; core manifest/migration fixed fixtures; persistence
inspection; orchestrator work_items/admission startup; daemon work-item manifest
comparison. Inventory every constructor/exhaustive purpose consumer after edits.

Compile/check public constructor examples, all callers/benches/doctests and both
feature modes. Fixed ordinary v1 JSON/MAC fixtures plus v2 valid Empty selections,
private MCP, hostile serde, cross-purpose/source replay and storage-success versus
execution-refusal tests are required. This draft does not claim those tests exist
or pass. API review must include later public registry-delete removal separately.
