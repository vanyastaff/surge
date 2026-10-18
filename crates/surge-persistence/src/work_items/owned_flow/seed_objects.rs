//! Bounded private seed storage, separate from semantic or execution authority.
use super::{private_establishment::PrivateOwner, private_inputs};
use crate::work_items::WorkItemStore;
use crate::work_items::{Result, WorkItemError};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use surge_core::execution_recovery::semantic_position::ContinuationSeedId;
use surge_core::execution_recovery::semantic_position::SourceEventRef;
use surge_core::work_item::{
    ContinuationSeedFields, ContinuationSeedManifest, HmacSha256Tag, OwnedFlowInputPurpose,
    OwnedFlowObjectRef, OwnedFlowRequestIdentity,
};
use surge_core::{
    ContentHash, RunId,
    id::{WorkItemId, WorkItemOperationId},
    work_item::{ContinuationInputRequirement, WorkItemBinding},
};

const DOMAIN: &[u8] = b"surge/owned-flow/seed-envelope/v1\0";
const LIMIT: usize = 8 * 1024 * 1024;

/// Correlated failure-tail metadata; it does not establish the source event kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeedFailureTail {
    through: SourceEventRef,
    hash: ContentHash,
}
impl SeedFailureTail {
    /// Retain already validated typed metadata, without authenticating its source.
    #[must_use]
    pub fn new(through: SourceEventRef, hash: ContentHash) -> Self {
        Self { through, hash }
    }
    /// Inclusive tail boundary.
    #[must_use]
    pub fn through(&self) -> SourceEventRef {
        self.through
    }
    /// Recorded tail digest, without a verification claim.
    #[must_use]
    pub fn hash(&self) -> ContentHash {
        self.hash
    }
}

/// Raw construction fields; no deserialization or authenticated authority exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeedStorageBindingFields {
    /// Original host continuation operation.
    pub operation: WorkItemOperationId,
    /// Fresh reserved successor run.
    pub run: RunId,
    /// Exact reserved requirements and generation.
    pub binding: WorkItemBinding,
    /// Original source workspace creator.
    pub workspace_owner: RunId,
    /// Correlated workspace digest.
    pub workspace_identity: ContentHash,
    /// Independently stored Bootstrap/source requirement.
    pub source: ContinuationInputRequirement,
    /// Inclusive semantic boundary, before a possible failure tail.
    pub boundary: SourceEventRef,
    /// Exact selected source prefix digest.
    pub prefix_hash: ContentHash,
    /// Original recorded terminal occurrence.
    pub terminal: SourceEventRef,
    /// Recorded terminal digest.
    pub terminal_hash: ContentHash,
    /// Required exactly when the terminal follows the selected boundary.
    pub failure_tail: Option<SeedFailureTail>,
    /// Source checkpoint correlation digest.
    pub source_checkpoint: ContentHash,
    /// Registered successor graph correlation digest.
    pub graph_hash: ContentHash,
}
/// Structurally validated storage binding; never a source or workspace capability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeedStorageBinding {
    fields: SeedStorageBindingFields,
}
impl SeedStorageBinding {
    /// Validate immutable correlation before a host stores private bytes.
    ///
    /// # Errors
    /// Rejects nil identities, invalid generations, unrelated source occurrences,
    /// successor/source collisions, owner mismatch, and inconsistent tail metadata.
    pub fn new(fields: SeedStorageBindingFields) -> Result<Self> {
        let source = fields.source.source_run();
        if fields.operation == WorkItemOperationId::nil()
            || fields.run == RunId::nil()
            || fields.run == source
            || fields.run == fields.source.source_bootstrap()
            || fields.binding.item == WorkItemId::nil()
            || fields.binding.revision == 0
            || fields.binding.generation == 0
            || fields.workspace_owner != source
            || fields.boundary.run() != source
            || fields.terminal.run() != source
            || fields.terminal.seq() < fields.boundary.seq()
            || fields.failure_tail.is_some() != (fields.terminal.seq() > fields.boundary.seq())
            || fields
                .failure_tail
                .as_ref()
                .is_some_and(|tail| tail.through != fields.terminal)
        {
            return Err(WorkItemError::Invalid(
                "invalid continuation seed storage binding".into(),
            ));
        }
        Ok(Self { fields })
    }
    /// Original operation.
    #[must_use]
    pub fn operation(&self) -> WorkItemOperationId {
        self.fields.operation
    }
    /// Reserved successor.
    #[must_use]
    pub fn run(&self) -> RunId {
        self.fields.run
    }
    /// Exact reserved binding.
    #[must_use]
    pub fn binding(&self) -> &WorkItemBinding {
        &self.fields.binding
    }
    /// Original workspace creator.
    #[must_use]
    pub fn workspace_owner(&self) -> RunId {
        self.fields.workspace_owner
    }
    /// Workspace correlation digest.
    #[must_use]
    pub fn workspace_identity(&self) -> ContentHash {
        self.fields.workspace_identity
    }
    /// Bootstrap/source correlation requirement.
    #[must_use]
    pub fn source(&self) -> &ContinuationInputRequirement {
        &self.fields.source
    }
    /// Inclusive source prefix boundary.
    #[must_use]
    pub fn boundary(&self) -> SourceEventRef {
        self.fields.boundary
    }
    /// Source prefix digest.
    #[must_use]
    pub fn prefix_hash(&self) -> ContentHash {
        self.fields.prefix_hash
    }
    /// Recorded terminal occurrence.
    #[must_use]
    pub fn terminal(&self) -> SourceEventRef {
        self.fields.terminal
    }
    /// Recorded terminal digest.
    #[must_use]
    pub fn terminal_hash(&self) -> ContentHash {
        self.fields.terminal_hash
    }
    /// Optional tail correlation.
    #[must_use]
    pub fn failure_tail(&self) -> Option<&SeedFailureTail> {
        self.fields.failure_tail.as_ref()
    }
    /// Source checkpoint digest.
    #[must_use]
    pub fn source_checkpoint(&self) -> ContentHash {
        self.fields.source_checkpoint
    }
    /// Registered successor graph digest.
    #[must_use]
    pub fn graph_hash(&self) -> ContentHash {
        self.fields.graph_hash
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTail {
    through: SourceEventRef,
    hash: ContentHash,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBinding {
    operation: WorkItemOperationId,
    run: RunId,
    binding: WorkItemBinding,
    workspace_owner: RunId,
    workspace_identity: ContentHash,
    source: ContinuationInputRequirement,
    boundary: SourceEventRef,
    prefix_hash: ContentHash,
    terminal: SourceEventRef,
    terminal_hash: ContentHash,
    failure_tail: Option<RawTail>,
    source_checkpoint: ContentHash,
    graph_hash: ContentHash,
}
impl From<&SeedStorageBinding> for RawBinding {
    fn from(value: &SeedStorageBinding) -> Self {
        Self {
            operation: value.operation(),
            run: value.run(),
            binding: value.binding().clone(),
            workspace_owner: value.workspace_owner(),
            workspace_identity: value.workspace_identity(),
            source: value.source().clone(),
            boundary: value.boundary(),
            prefix_hash: value.prefix_hash(),
            terminal: value.terminal(),
            terminal_hash: value.terminal_hash(),
            failure_tail: value.failure_tail().map(|tail| RawTail {
                through: tail.through(),
                hash: tail.hash(),
            }),
            source_checkpoint: value.source_checkpoint(),
            graph_hash: value.graph_hash(),
        }
    }
}
impl RawBinding {
    fn validated(&self) -> Result<SeedStorageBinding> {
        SeedStorageBinding::new(SeedStorageBindingFields {
            operation: self.operation,
            run: self.run,
            binding: self.binding.clone(),
            workspace_owner: self.workspace_owner,
            workspace_identity: self.workspace_identity,
            source: self.source.clone(),
            boundary: self.boundary,
            prefix_hash: self.prefix_hash,
            terminal: self.terminal,
            terminal_hash: self.terminal_hash,
            failure_tail: self
                .failure_tail
                .as_ref()
                .map(|tail| SeedFailureTail::new(tail.through, tail.hash)),
            source_checkpoint: self.source_checkpoint,
            graph_hash: self.graph_hash,
        })
        .map_err(|_| WorkItemError::PrivateInputsCorrupt)
    }
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Header {
    schema_version: u32,
    establishment: String,
    purpose: OwnedFlowInputPurpose,
    request_identity: OwnedFlowRequestIdentity,
    binding: RawBinding,
    object_ref: OwnedFlowObjectRef,
    payload_version: u32,
    payload_hash: ContentHash,
    payload_length: usize,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PendingSeed {
    header: Header,
    seed: ContinuationSeedId,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SeedObject {
    schema_version: u32,
    pending: PendingSeed,
    payload: Vec<u8>,
    auth_tag: HmacSha256Tag,
}
#[derive(Serialize)]
struct Authentication<'a> {
    pending: &'a PendingSeed,
    payload: &'a [u8],
}
pub(super) struct StoredSeed {
    pub(super) pending: PendingSeed,
    pub(super) manifest: Option<ContinuationSeedManifest>,
}
/// Private read-back result; it carries no graph, workspace or dispatch authority.
pub(super) struct AuthenticatedSeedObject {
    _payload: Vec<u8>,
    _owner: Arc<PrivateOwner>,
    pub(super) manifest: ContinuationSeedManifest,
}
fn object_name(reference: &OwnedFlowObjectRef) -> String {
    format!("seed-{}.json", reference.as_str())
}
fn seed_id(header: &Header, payload: &[u8]) -> Result<ContinuationSeedId> {
    let mut hash = Sha256::new();
    hash.update(private_inputs::canonical(header)?);
    hash.update(payload);
    Ok(ContinuationSeedId::new(ContentHash::from_bytes(
        hash.finalize().into(),
    )))
}
fn manifest(pending: &PendingSeed, tag: HmacSha256Tag) -> Result<ContinuationSeedManifest> {
    let value = &pending.header.binding;
    ContinuationSeedFields {
        schema_version: 1,
        operation: value.operation,
        run: value.run,
        binding: value.binding.clone(),
        workspace_owner: value.workspace_owner,
        workspace_identity: value.workspace_identity,
        source_bootstrap: value.source.source_bootstrap(),
        source: value.boundary,
        source_checkpoint: value.source_checkpoint,
        graph_hash: value.graph_hash,
        seed: pending.seed,
        object_ref: pending.header.object_ref.clone(),
        object_auth_tag: tag,
    }
    .try_into()
    .map_err(|_| WorkItemError::PrivateInputsCorrupt)
}
pub(super) fn prepare(
    owner: &PrivateOwner,
    binding: &SeedStorageBinding,
    identity: &OwnedFlowRequestIdentity,
    payload: &[u8],
    reference: OwnedFlowObjectRef,
) -> Result<(PendingSeed, Vec<u8>)> {
    if payload.len() > LIMIT {
        return Err(WorkItemError::Invalid(
            "encoded seed object exceeds bound".into(),
        ));
    }
    let header = Header {
        schema_version: 1,
        establishment: owner.establishment_nonce().into(),
        purpose: OwnedFlowInputPurpose::Continuation {
            source: binding.source().clone(),
        },
        request_identity: identity.clone(),
        binding: RawBinding::from(binding),
        object_ref: reference,
        payload_version: 1,
        payload_hash: ContentHash::compute(payload),
        payload_length: payload.len(),
    };
    let pending = PendingSeed {
        seed: seed_id(&header, payload)?,
        header,
    };
    let authenticated = private_inputs::canonical(&Authentication {
        pending: &pending,
        payload,
    })?;
    let object = SeedObject {
        schema_version: 1,
        auth_tag: private_inputs::authentication(owner.key(), DOMAIN, &authenticated)?,
        pending: pending.clone(),
        payload: payload.to_vec(),
    };
    let bytes = private_inputs::canonical(&object)?;
    if bytes.len() > LIMIT {
        return Err(WorkItemError::Invalid(
            "encoded seed object exceeds bound".into(),
        ));
    }
    Ok((pending, bytes))
}
pub(super) fn load(
    conn: &Connection,
    operation: WorkItemOperationId,
    owner: &PrivateOwner,
) -> Result<Option<StoredSeed>> {
    load_metadata(conn, operation, owner.establishment_nonce())
}
type StoredSeedRow = (
    Option<String>,
    Option<String>,
    Option<String>,
    String,
    String,
    String,
    String,
    String,
);
fn load_metadata(
    conn: &Connection,
    operation: WorkItemOperationId,
    nonce: &str,
) -> Result<Option<StoredSeed>> {
    let row: Option<StoredSeedRow> = conn.query_row(
        "SELECT c.seed_binding,c.pending_ref,c.seed_manifest,o.request_identity,o.run,o.workspace_owner,c.source_bootstrap,c.source_run FROM owned_flow_continuation_requirements c JOIN owned_flow_operations o USING(operation) WHERE operation=?",
        [operation.to_string()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?))).optional()?;
    let row = row.ok_or(WorkItemError::PrivateInputsCorrupt)?;
    let (encoded, reference) = match (&row.0, &row.1, &row.2) {
        (None, None, None) => return Ok(None),
        (Some(binding), Some(reference), _) => (binding, reference),
        _ => return Err(WorkItemError::PrivateInputsCorrupt),
    };
    let pending: PendingSeed =
        serde_json::from_str(encoded).map_err(|_| WorkItemError::PrivateInputsCorrupt)?;
    let value = &pending.header;
    let binding = value.binding.validated()?;
    let identity: OwnedFlowRequestIdentity =
        serde_json::from_str(&row.3).map_err(|_| WorkItemError::PrivateInputsCorrupt)?;
    if value.schema_version != 1
        || value.payload_version != 1
        || value.payload_length > LIMIT
        || value.establishment != nonce
        || value.request_identity != identity
        || !matches!(identity, OwnedFlowRequestIdentity::Hmac { .. })
        || binding.operation() != operation
        || binding.run().to_string() != row.4
        || binding.workspace_owner().to_string() != row.5
        || binding.source().source_bootstrap().to_string() != row.6
        || binding.source().source_run().to_string() != row.7
        || value.object_ref.as_str() != reference
        || value.purpose
            != (OwnedFlowInputPurpose::Continuation {
                source: binding.source().clone(),
            })
    {
        return Err(WorkItemError::PrivateInputsCorrupt);
    }
    let pinned:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM owned_flow_private_object_pins WHERE operation=? AND object_ref=? AND kind='seed')",
        params![operation.to_string(),reference],|r|r.get(0))?;
    if !pinned {
        return Err(WorkItemError::PrivateInputsCorrupt);
    }
    let stored = row
        .2
        .map(|value| serde_json::from_str(&value).map_err(|_| WorkItemError::PrivateInputsCorrupt))
        .transpose()?;
    Ok(Some(StoredSeed {
        pending,
        manifest: stored,
    }))
}
pub(super) fn allocate(
    conn: &Connection,
    operation: WorkItemOperationId,
    pending: &PendingSeed,
) -> Result<()> {
    if conn.execute("UPDATE owned_flow_continuation_requirements SET seed_binding=?,pending_ref=? WHERE operation=? AND seed_binding IS NULL AND pending_ref IS NULL AND seed_manifest IS NULL",
        params![String::from_utf8(private_inputs::canonical(pending)?).map_err(|_|WorkItemError::PrivateInputsCorrupt)?,pending.header.object_ref.as_str(),operation.to_string()])?!=1 {
        return Err(WorkItemError::Conflict("continuation seed already reserved".into()));
    }
    conn.execute(
        "INSERT INTO owned_flow_private_object_pins(operation,object_ref,kind) VALUES(?,?,'seed')",
        params![operation.to_string(), pending.header.object_ref.as_str()],
    )?;
    Ok(())
}
pub(super) fn publish(owner: &PrivateOwner, pending: &PendingSeed, bytes: &[u8]) -> Result<()> {
    owner.verify()?;
    owner
        .files()
        .publish(&object_name(&pending.header.object_ref), bytes)?;
    owner.verify()
}
pub(super) fn check_retry(
    pending: &PendingSeed,
    binding: &SeedStorageBinding,
    payload: &[u8],
) -> Result<()> {
    if pending.header.binding != RawBinding::from(binding)
        || pending.header.payload_hash != ContentHash::compute(payload)
        || pending.header.payload_length != payload.len()
        || pending.seed != seed_id(&pending.header, payload)?
    {
        return Err(WorkItemError::Conflict(
            "continuation seed immutable retry changed".into(),
        ));
    }
    Ok(())
}
pub(super) fn verify_storage(
    owner: Arc<PrivateOwner>,
    stored: &StoredSeed,
) -> Result<AuthenticatedSeedObject> {
    owner.verify()?;
    let bytes = owner
        .files()
        .read(&object_name(&stored.pending.header.object_ref))?;
    let object: SeedObject =
        serde_json::from_slice(&bytes).map_err(|_| WorkItemError::PrivateInputsCorrupt)?;
    if object.schema_version != 1
        || object.pending != stored.pending
        || bytes != private_inputs::canonical(&object)?
        || object.payload.len() != stored.pending.header.payload_length
        || ContentHash::compute(&object.payload) != stored.pending.header.payload_hash
        || seed_id(&object.pending.header, &object.payload)? != stored.pending.seed
    {
        return Err(WorkItemError::PrivateInputsCorrupt);
    }
    private_inputs::verify(
        owner.key(),
        DOMAIN,
        &private_inputs::canonical(&Authentication {
            pending: &object.pending,
            payload: &object.payload,
        })?,
        &object.auth_tag,
    )?;
    let manifest = manifest(&object.pending, object.auth_tag)?;
    if stored
        .manifest
        .as_ref()
        .is_some_and(|value| value != &manifest)
    {
        return Err(WorkItemError::PrivateInputsCorrupt);
    }
    owner.verify()?;
    Ok(AuthenticatedSeedObject {
        manifest,
        _payload: object.payload,
        _owner: owner,
    })
}
pub(super) fn seal(
    conn: &Connection,
    operation: WorkItemOperationId,
    owner: &PrivateOwner,
    expected: &PendingSeed,
    manifest: &ContinuationSeedManifest,
) -> Result<()> {
    let current = load(conn, operation, owner)?.ok_or(WorkItemError::PrivateInputsCorrupt)?;
    if current.pending != *expected {
        return Err(WorkItemError::PrivateInputsCorrupt);
    }
    if let Some(current) = current.manifest {
        if current != *manifest {
            return Err(WorkItemError::PrivateInputsCorrupt);
        }
        return Ok(());
    }
    if conn.execute("UPDATE owned_flow_continuation_requirements SET seed_manifest=? WHERE operation=? AND seed_manifest IS NULL",
        params![serde_json::to_string(manifest)?,operation.to_string()])?!=1 {
        return Err(WorkItemError::Conflict("continuation seed freeze changed".into()));
    }
    Ok(())
}

pub(super) fn require_sealed(
    store: &WorkItemStore,
    operation: WorkItemOperationId,
    owner: Arc<PrivateOwner>,
) -> Result<AuthenticatedSeedObject> {
    let stored = {
        let conn = store.pool.get()?;
        load(&conn, operation, &owner)?.ok_or(WorkItemError::PrivateInputsRecoveryRequired)?
    };
    if stored.manifest.is_none() {
        return Err(WorkItemError::PrivateInputsRecoveryRequired);
    }
    verify_storage(owner, &stored)
}

pub(super) fn validate_metadata(
    conn: &Connection,
    operation: WorkItemOperationId,
    nonce: &str,
) -> Result<()> {
    load_metadata(conn, operation, nonce).map(|_| ())
}
pub(super) fn inspect_bound(
    store: &WorkItemStore,
    operation: WorkItemOperationId,
    owner: Arc<PrivateOwner>,
) -> Result<()> {
    let stored = {
        let conn = store.pool.get()?;
        load(&conn, operation, &owner)?
    };
    if let Some(stored) = stored {
        let _verified = verify_storage(owner, &stored)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn seed_envelope_matches_independent_fixed_wire_id_and_mac() {
        let fixed = include_bytes!("seed_objects/fixed-envelope.json");
        let object: SeedObject = serde_json::from_slice(fixed).unwrap();
        assert_eq!(private_inputs::canonical(&object).unwrap(), fixed);
        let binding = object.pending.header.binding.validated().unwrap();
        assert_eq!(binding.boundary().seq(), 7);
        assert_eq!(binding.terminal().seq(), 9);
        assert_eq!(binding.failure_tail().unwrap().through().seq(), 9);
        let actual = seed_id(&object.pending.header, &object.payload).unwrap();
        assert_eq!(actual, object.pending.seed);
        let authenticated = private_inputs::canonical(&Authentication {
            pending: &object.pending,
            payload: &object.payload,
        })
        .unwrap();
        let tag = private_inputs::authentication(&[7; 32], DOMAIN, &authenticated).unwrap();
        assert_eq!(tag, object.auth_tag);
        let wrong_domain = private_inputs::authentication(
            &[7; 32],
            b"surge/owned-flow/mcp-envelope/v1\0",
            &authenticated,
        )
        .unwrap();
        assert_ne!(wrong_domain, object.auth_tag);
    }
}
