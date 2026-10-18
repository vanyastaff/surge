//! External private storage identity; directory loss never grants key creation.
use super::private_files::PrivateFiles;
use super::private_inputs::KEY_MARKER;
use crate::work_items::start_preparation::secure_lock::{PreparationLock, PreparationLockKey};
use crate::work_items::{Result, WorkItemError, WorkItemStore};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{fs::File, sync::Arc};
use surge_core::{RunId, id::WorkItemOperationId, work_item::*};

const KEY: &str = "host-key-v1";

#[derive(Clone, PartialEq, Eq, Serialize)]
struct NamespaceIdentity {
    version: u32,
    platform: String,
    ancestry: Vec<Vec<u8>>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawNamespaceIdentity {
    version: u32,
    platform: String,
    ancestry: Vec<Vec<u8>>,
}
impl NamespaceIdentity {
    fn decode(encoded: &str) -> Result<Self> {
        let raw: RawNamespaceIdentity =
            serde_json::from_str(encoded).map_err(|_| WorkItemError::PrivateInputsCorrupt)?;
        let width = match raw.platform.as_str() {
            "unix" => 16,
            "windows" => 24,
            _ => return Err(WorkItemError::PrivateInputsCorrupt),
        };
        if raw.version != 1
            || raw.ancestry.is_empty()
            || raw.ancestry.iter().any(|identity| identity.len() != width)
        {
            return Err(WorkItemError::PrivateInputsCorrupt);
        }
        Ok(Self {
            version: raw.version,
            platform: raw.platform,
            ancestry: raw.ancestry,
        })
    }
    fn capture(files: &PrivateFiles) -> Result<Self> {
        let platform = if cfg!(windows) { "windows" } else { "unix" };
        let value = Self {
            version: 1,
            platform: platform.into(),
            ancestry: files.identity()?,
        };
        Self::decode(&serde_json::to_string(&value)?)
    }
}
#[derive(Clone, PartialEq, Eq)]
struct Record {
    nonce: String,
    state: String,
    lock: String,
    namespace: Option<String>,
    key: Option<Vec<u8>>,
    digest: Option<Vec<u8>>,
    marker: Option<Vec<u8>>,
}
fn read(conn: &Connection) -> Result<Option<Record>> {
    let raw = conn.query_row(
        "SELECT schema_version,nonce,state,lock_identity,namespace_identity,key_identity,key_digest,marker_identity FROM owned_flow_private_establishment WHERE singleton=1",
        [], |row| Ok((row.get::<_,u32>(0)?, Record { nonce:row.get(1)?,state:row.get(2)?,lock:row.get(3)?,namespace:row.get(4)?,key:row.get(5)?,digest:row.get(6)?,marker:row.get(7)? })),
    ).optional()?;
    let Some((version, value)) = raw else {
        return Ok(None);
    };
    if version != 1
        || value.nonce.parse::<RunId>().is_err()
        || !matches!(value.state.as_str(), "Establishing" | "Established")
        || value.lock.is_empty()
        || value.lock.len() > 65536
        || value.key.is_some() != value.digest.is_some()
        || value
            .digest
            .as_ref()
            .is_some_and(|digest| digest.len() != 32)
    {
        return Err(WorkItemError::PrivateInputsCorrupt);
    }
    if value.state == "Established"
        && (value.namespace.is_none()
            || value.key.is_none()
            || value.digest.is_none()
            || value.marker.is_none())
    {
        return Err(WorkItemError::PrivateInputsCorrupt);
    }
    if let Some(namespace) = &value.namespace {
        let identity = NamespaceIdentity::decode(namespace)?;
        let width = if identity.platform == "windows" {
            24
        } else {
            16
        };
        if value.key.as_ref().is_some_and(|key| key.len() != width)
            || value
                .marker
                .as_ref()
                .is_some_and(|marker| marker.len() != width)
        {
            return Err(WorkItemError::PrivateInputsCorrupt);
        }
    } else if value.key.is_some() || value.marker.is_some() {
        return Err(WorkItemError::PrivateInputsCorrupt);
    }
    Ok(Some(value))
}
fn private_dependencies(conn: &Connection) -> Result<bool> {
    let mut query =
        conn.prepare("SELECT request_identity,inputs_manifest,operation,input_purpose FROM owned_flow_operations")?;
    let rows = query.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Option<String>>(3)?,
        ))
    })?;
    let mut found = false;
    for row in rows {
        let (identity, manifest, operation, encoded_purpose) = row?;
        let identity: OwnedFlowRequestIdentity =
            serde_json::from_str(&identity).map_err(|_| WorkItemError::PrivateInputsCorrupt)?;
        let manifest: Option<OwnedFlowInputsManifest> = manifest
            .map(|encoded| {
                serde_json::from_str(&encoded).map_err(|_| WorkItemError::PrivateInputsCorrupt)
            })
            .transpose()?;
        let operation = operation
            .parse()
            .map_err(|_| WorkItemError::PrivateInputsCorrupt)?;
        let purpose =
            super::purpose::decode(conn, operation, &identity, encoded_purpose.as_deref())?;
        found |= purpose.private()
            || matches!(identity, OwnedFlowRequestIdentity::Hmac { .. })
            || manifest
                .as_ref()
                .is_some_and(|value| matches!(value.mcp(), FrozenOwnedFlowMcp::Private { .. }));
    }
    Ok(found)
}

/// A legacy proof names the independently stored operation; it cannot authorize a new one.
pub(super) struct LegacyProof<'a> {
    pub operation: WorkItemOperationId,
    pub body: Option<&'a [u8]>,
    pub manifest: Option<&'a OwnedFlowInputsManifest>,
}
struct LegacyFacts {
    identity: OwnedFlowRequestIdentity,
    manifest: Option<OwnedFlowInputsManifest>,
    lock_identity: String,
}
struct RawLegacyFacts {
    identity: String,
    manifest: Option<String>,
    lock: String,
    item: String,
    run: String,
    workspace_owner: String,
    source: Option<String>,
    receipt: Option<String>,
}
fn validate_legacy_binding(
    raw: &RawLegacyFacts,
    operation: WorkItemOperationId,
    identity: &OwnedFlowRequestIdentity,
    manifest: &OwnedFlowInputsManifest,
) -> Result<()> {
    let corrupt = || WorkItemError::PrivateInputsCorrupt;
    let item: surge_core::id::WorkItemId = raw.item.parse().map_err(|_| corrupt())?;
    let run: RunId = raw.run.parse().map_err(|_| corrupt())?;
    let owner: RunId = raw.workspace_owner.parse().map_err(|_| corrupt())?;
    let source: super::OwnedFlowSourceSnapshot =
        serde_json::from_str(raw.source.as_deref().ok_or_else(corrupt)?).map_err(|_| corrupt())?;
    let binding = WorkItemBinding {
        item,
        revision: 1,
        generation: 1,
        requirements_hash: source.contract.hash()?,
    };
    if manifest.operation() != operation
        || manifest.request_identity() != identity
        || manifest.binding() != &binding
        || manifest.run() != run
        || manifest.workspace_owner() != owner
        || manifest.workspace() != &source.workspace
        || source.workspace.ownership != owner.to_string()
    {
        return Err(corrupt());
    }
    if let Some(receipt) = &raw.receipt {
        let receipt: OwnedFlowReceipt = serde_json::from_str(receipt).map_err(|_| corrupt())?;
        if receipt.operation_id != operation
            || receipt.item != item
            || receipt.run != run
            || receipt.workspace_owner != owner
            || receipt.binding != binding
        {
            return Err(corrupt());
        }
    }
    Ok(())
}
fn legacy_facts(store: &WorkItemStore, proof: &LegacyProof<'_>) -> Result<LegacyFacts> {
    let raw = store.pool.get()?.query_row(
        "SELECT request_identity,inputs_manifest,operation_lock_identity,item,run,workspace_owner,source_snapshot,receipt FROM owned_flow_operations WHERE operation=?",
        [proof.operation.to_string()], |row| Ok(RawLegacyFacts {
            identity:row.get(0)?,manifest:row.get(1)?,lock:row.get(2)?,item:row.get(3)?,run:row.get(4)?,workspace_owner:row.get(5)?,source:row.get(6)?,receipt:row.get(7)?,
        }),
    ).optional()?.ok_or(WorkItemError::PrivateInputsRecoveryRequired)?;
    let identity =
        serde_json::from_str(&raw.identity).map_err(|_| WorkItemError::PrivateInputsCorrupt)?;
    let manifest: Option<OwnedFlowInputsManifest> = raw
        .manifest
        .as_ref()
        .map(|value| serde_json::from_str(value).map_err(|_| WorkItemError::PrivateInputsCorrupt))
        .transpose()?;
    if proof
        .manifest
        .is_some_and(|provided| manifest.as_ref() != Some(provided))
    {
        return Err(WorkItemError::PrivateInputsCorrupt);
    }
    if raw.receipt.is_some() && manifest.is_none() {
        return Err(WorkItemError::PrivateInputsCorrupt);
    }
    if let Some(manifest) = &manifest {
        validate_legacy_binding(&raw, proof.operation, &identity, manifest)?;
    }
    Ok(LegacyFacts {
        identity,
        manifest,
        lock_identity: raw.lock,
    })
}

/// Native files remain owned independently of the short establishment lock.
/// Clones retain the same original descriptors through callers and worker exit.
pub(in crate::work_items) struct PrivateOwner {
    // Both original files close before their retained parent namespace is released.
    key_file: File,
    marker_file: File,
    files: PrivateFiles,
    key_identity: Vec<u8>,
    marker_identity: Vec<u8>,
    namespace: NamespaceIdentity,
    key_bytes: Vec<u8>,
    key: [u8; 32],
    record: Record,
    store: WorkItemStore,
}
impl PrivateOwner {
    fn capture(
        store: &WorkItemStore,
        files: PrivateFiles,
        key: [u8; 32],
        record: Record,
    ) -> Result<Self> {
        let namespace = NamespaceIdentity::capture(&files)?;
        let key_file = files.retain(KEY)?;
        let key_identity = files.retained_identity(&key_file)?;
        let key_bytes = files.read(KEY)?;
        let marker_file = files.retain(KEY_MARKER)?;
        let marker_identity = files.retained_identity(&marker_file)?;
        let owner = Self {
            files,
            key_file,
            marker_file,
            key_identity,
            marker_identity,
            namespace,
            key_bytes,
            key,
            record,
            store: store.clone(),
        };
        owner.verify_native()?;
        Ok(owner)
    }
    fn verify_native(&self) -> Result<()> {
        self.files.verify()?;
        let linked = self.files.retain(KEY)?;
        let marker_linked = self.files.retain(KEY_MARKER)?;
        if NamespaceIdentity::capture(&self.files)? != self.namespace
            || self.files.retained_identity(&self.key_file)? != self.key_identity
            || self.files.retained_identity(&linked)? != self.key_identity
            || self.files.retained_identity(&self.marker_file)? != self.marker_identity
            || self.files.retained_identity(&marker_linked)? != self.marker_identity
            || self.files.read(KEY)? != self.key_bytes
            || super::private_inputs::host_key(&self.files, false)? != self.key
        {
            return Err(WorkItemError::PrivateInputsRecoveryRequired);
        }
        Ok(())
    }
    pub(in crate::work_items) fn verify(&self) -> Result<()> {
        self.verify_in(&*self.store.pool.get()?)
    }
    pub(in crate::work_items) fn verify_in(&self, conn: &Connection) -> Result<()> {
        self.verify_native()?;
        self.verify_stored(conn)?;
        self.verify_native()
    }
    fn verify_stored(&self, conn: &Connection) -> Result<()> {
        if read(conn)?.as_ref() != Some(&self.record) {
            return Err(WorkItemError::PrivateInputsRecoveryRequired);
        }
        Ok(())
    }
    pub(super) fn files(&self) -> &PrivateFiles {
        &self.files
    }
    pub(super) fn key(&self) -> &[u8; 32] {
        &self.key
    }
    pub(super) fn establishment_nonce(&self) -> &str {
        &self.record.nonce
    }
    fn check_record(&self, lock: &PreparationLock, conn: &Connection) -> Result<()> {
        if self.record.state != "Established"
            || self.record.lock != lock.identity()?
            || self.record.namespace.as_deref()
                != Some(serde_json::to_string(&self.namespace)?.as_str())
            || self.record.key.as_ref() != Some(&self.key_identity)
            || self.record.marker.as_ref() != Some(&self.marker_identity)
            || self.record.digest.as_deref() != Some(Sha256::digest(&self.key_bytes).as_slice())
        {
            return Err(WorkItemError::PrivateInputsRecoveryRequired);
        }
        self.verify_in(conn)
    }
}

fn existing(
    store: &WorkItemStore,
    lock: &PreparationLock,
    record: Record,
) -> Result<Arc<PrivateOwner>> {
    if record.state != "Established" {
        return Err(WorkItemError::PrivateInputsRecoveryRequired);
    }
    let files = PrivateFiles::open(&store.home, false)?;
    let key = super::private_inputs::host_key(&files, false)?;
    let owner = PrivateOwner::capture(store, files, key, record)?;
    owner.check_record(lock, &*store.pool.get()?)?;
    Ok(Arc::new(owner))
}
fn record_namespace(
    store: &WorkItemStore,
    nonce: &str,
    namespace: &NamespaceIdentity,
) -> Result<()> {
    let mut conn = store.pool.get()?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if tx.execute("UPDATE owned_flow_private_establishment SET namespace_identity=? WHERE singleton=1 AND nonce=? AND state='Establishing' AND namespace_identity IS NULL",
        params![serde_json::to_string(namespace)?,nonce])? != 1
    { return Err(WorkItemError::PrivateInputsRecoveryRequired) }
    tx.commit()?;
    Ok(())
}
fn publish_established(owner: &mut PrivateOwner) -> Result<()> {
    owner.verify_native()?;
    let mut conn = owner.store.pool.get()?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if read(&tx)?.as_ref() != Some(&owner.record) {
        return Err(WorkItemError::PrivateInputsRecoveryRequired);
    }
    let digest = Sha256::digest(&owner.key_bytes).to_vec();
    tx.execute("UPDATE owned_flow_private_establishment SET state='Established',key_identity=?,key_digest=?,marker_identity=? WHERE singleton=1 AND nonce=? AND state='Establishing'",
        params![owner.key_identity,digest,owner.marker_identity,owner.record.nonce])?;
    tx.commit()?;
    #[cfg(all(test, windows))]
    crate::state_home::private_observation::observe(
        crate::state_home::private_observation::Phase::PrivateEstablishmentCommitted,
        &owner.key_file,
    )
    .map_err(|_| WorkItemError::PrivateInputsRecoveryRequired)?;
    owner.record.state = "Established".into();
    owner.record.key = Some(owner.key_identity.clone());
    owner.record.digest = Some(digest);
    owner.record.marker = Some(owner.marker_identity.clone());
    owner.verify_in(&conn)
}
fn first_establishment(store: &WorkItemStore, lock: &PreparationLock) -> Result<Arc<PrivateOwner>> {
    let nonce = RunId::new().to_string();
    let mut conn = store.pool.get()?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if read(&tx)?.is_some() || private_dependencies(&tx)? {
        return Err(WorkItemError::PrivateInputsRecoveryRequired);
    }
    tx.execute("INSERT INTO owned_flow_private_establishment(singleton,schema_version,nonce,state,lock_identity) VALUES(1,1,?,'Establishing',?)",
        params![nonce,lock.identity()?])?;
    tx.commit()?;
    drop(conn);
    let files = PrivateFiles::open(&store.home, true)?;
    if !files.is_empty()? {
        return Err(WorkItemError::PrivateInputsRecoveryRequired);
    }
    let namespace = NamespaceIdentity::capture(&files)?;
    record_namespace(store, &nonce, &namespace)?;
    let key = super::private_inputs::host_key(&files, true)?;
    let record = read(&*store.pool.get()?)?.ok_or(WorkItemError::PrivateInputsRecoveryRequired)?;
    let mut owner = PrivateOwner::capture(store, files, key, record)?;
    lock.verify()?;
    publish_established(&mut owner)?;
    owner.check_record(lock, &*store.pool.get()?)?;
    Ok(Arc::new(owner))
}
fn pin_legacy(
    store: &WorkItemStore,
    lock: &PreparationLock,
    operation_lock: &PreparationLock,
    proof: &LegacyProof<'_>,
) -> Result<Arc<PrivateOwner>> {
    let facts = legacy_facts(store, proof)?;
    if operation_lock.identity()? != facts.lock_identity {
        return Err(WorkItemError::PrivateInputsRecoveryRequired);
    }
    let files = PrivateFiles::open(&store.home, false)?;
    let key = super::private_inputs::host_key(&files, false)?;
    super::private_inputs::authenticate_legacy(
        &files,
        &key,
        proof.body,
        &facts.identity,
        facts.manifest.as_ref(),
    )?;
    let record = Record {
        nonce: RunId::new().to_string(),
        state: "Established".into(),
        lock: lock.identity()?,
        namespace: None,
        key: None,
        digest: None,
        marker: None,
    };
    let mut owner = PrivateOwner::capture(store, files, key, record)?;
    owner.record.namespace = Some(serde_json::to_string(&owner.namespace)?);
    owner.record.key = Some(owner.key_identity.clone());
    owner.record.digest = Some(Sha256::digest(&owner.key_bytes).to_vec());
    owner.record.marker = Some(owner.marker_identity.clone());
    operation_lock.verify()?;
    owner.verify_native()?;
    let mut conn = store.pool.get()?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if read(&tx)?.is_some() || !private_dependencies(&tx)? {
        return Err(WorkItemError::PrivateInputsRecoveryRequired);
    }
    tx.execute("INSERT INTO owned_flow_private_establishment(singleton,schema_version,nonce,state,lock_identity,namespace_identity,key_identity,key_digest,marker_identity) VALUES(1,1,?,'Established',?,?,?,?,?)",
        params![owner.record.nonce,owner.record.lock,owner.record.namespace,owner.record.key,owner.record.digest,owner.record.marker])?;
    tx.commit()?;
    operation_lock.verify()?;
    owner.check_record(lock, &conn)?;
    Ok(Arc::new(owner))
}

pub(super) fn open(
    store: &WorkItemStore,
    proof: Option<LegacyProof<'_>>,
    original_operation_lock: Option<&PreparationLock>,
) -> Result<Arc<PrivateOwner>> {
    // Inspect before locking only to choose lock order. All decisions are repeated
    // under the actual external lock and short immediate transactions.
    let needs_legacy =
        read(&*store.pool.get()?)?.is_none() && private_dependencies(&*store.pool.get()?)?;
    let acquired_operation;
    let operation_lock = if needs_legacy {
        let proof = proof
            .as_ref()
            .ok_or(WorkItemError::PrivateInputsRecoveryRequired)?;
        if let Some(guard) = original_operation_lock {
            Some(guard)
        } else {
            acquired_operation = PreparationLock::acquire_private(
                &store.home,
                PreparationLockKey::FlowOperation(proof.operation),
            )?;
            Some(&acquired_operation)
        }
    } else {
        None
    };
    let lock =
        PreparationLock::acquire_private(&store.home, PreparationLockKey::PrivateEstablishment)?;
    let record = {
        let conn = store.pool.get()?;
        read(&conn)?
    };
    if let Some(record) = record {
        return existing(store, &lock, record);
    }
    if private_dependencies(&*store.pool.get()?)? {
        let proof = proof
            .as_ref()
            .ok_or(WorkItemError::PrivateInputsRecoveryRequired)?;
        let guard = operation_lock.ok_or(WorkItemError::PrivateInputsRecoveryRequired)?;
        pin_legacy(store, &lock, guard, proof)
    } else {
        first_establishment(store, &lock)
    }
}

#[cfg(all(test, any(target_os = "linux", target_os = "macos", windows)))]
#[path = "private_establishment_tests.rs"]
mod tests;
