//! One original-control-owned inventory producer and resumable physical deletion.
#[cfg(unix)]
use super::unix::DeletionNamespace;
use super::{OpenError, Storage, control};
use crate::runs::error::StorageError;
#[cfg(windows)]
use crate::state_home::DeletionNamespace;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
};
use surge_core::RunId;
const MAX_ENTRIES: usize = 100_000;
const MAX_BYTES: usize = 67_108_864;
const KIND: &str = "surge-controlled-delete-v1";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NativeEntry {
    pub(crate) identity: Vec<u8>,
    pub(crate) directory: bool,
    size: u64,
    hash: Option<Vec<u8>>,
}
pub(crate) fn read_native_entry(
    file: &File,
    identity: Vec<u8>,
    directory: bool,
) -> Result<NativeEntry, OpenError> {
    if directory {
        return Ok(NativeEntry {
            identity,
            directory,
            size: 0,
            hash: None,
        });
    }
    let before = file.metadata()?;
    let mut input = file;
    let mut hash = Sha256::new();
    let mut chunk = [0_u8; 8192];
    loop {
        let count = input.read(&mut chunk)?;
        if count == 0 {
            break;
        }
        hash.update(&chunk[..count]);
    }
    let after = file.metadata()?;
    if before.len() != after.len() || before.modified()? != after.modified()? {
        return Err(invalid());
    }
    Ok(NativeEntry {
        identity,
        directory,
        size: after.len(),
        hash: Some(hash.finalize().to_vec()),
    })
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    path: Vec<u8>,
    native: NativeEntry,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Outside {
    kind: String,
    platform: u8,
    run: RunId,
    opening: String,
    home: Vec<u8>,
    runs_parent: Vec<u8>,
    controls_parent: Vec<u8>,
    control_file: Vec<u8>,
    root: Vec<u8>,
    route: Vec<u8>,
    database: Vec<u8>,
}
struct Intent {
    id: String,
    state: String,
    outside: Outside,
    entries: Vec<Entry>,
}
fn invalid() -> OpenError {
    OpenError::Authority("original deletion namespace/inventory changed or unsupported".into())
}
fn ownership(run: RunId, error: OpenError) -> StorageError {
    match error {
        OpenError::RunInUse { .. } | OpenError::WriterAlreadyHeld { .. } => {
            StorageError::RunInUse { run_id: run }
        },
        source => StorageError::DeletionOwnership {
            run_id: run,
            source,
        },
    }
}
#[cfg(unix)]
fn platform() -> u8 {
    1
}
#[cfg(windows)]
fn platform() -> u8 {
    2
}
#[cfg(unix)]
fn encoded(path: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str().as_bytes().to_vec()
}
#[cfg(windows)]
fn encoded(path: &Path) -> Vec<u8> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str()
        .encode_wide()
        .flat_map(u16::to_le_bytes)
        .collect()
}
fn decoded(bytes: &[u8]) -> Result<PathBuf, OpenError> {
    #[cfg(unix)]
    let path = {
        use std::os::unix::ffi::OsStringExt;
        if bytes.contains(&0) {
            return Err(invalid());
        }
        PathBuf::from(std::ffi::OsString::from_vec(bytes.to_vec()))
    };
    #[cfg(windows)]
    let path = {
        use std::os::windows::ffi::OsStringExt;
        let (chunks, remainder) = bytes.as_chunks::<2>();
        if !remainder.is_empty() {
            return Err(invalid());
        }
        let units: Vec<u16> = chunks.iter().copied().map(u16::from_le_bytes).collect();
        if units.contains(&0) {
            return Err(invalid());
        }
        PathBuf::from(std::ffi::OsString::from_wide(&units))
    };
    let mut normalized = PathBuf::new();
    for component in path.components() {
        let std::path::Component::Normal(name) = component else {
            return Err(invalid());
        };
        normalized.push(name);
    }
    if encoded(&normalized) != bytes {
        return Err(invalid());
    }
    Ok(normalized)
}
fn referenced(conn: &Connection, run: RunId) -> Result<bool, StorageError> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM owned_flow_source_pins WHERE source_run=?)",
        [run.to_string()],
        |row| row.get(0),
    )?)
}
fn initial(conn: &Connection, run: RunId) -> Result<(), StorageError> {
    if referenced(conn, run)? {
        return Err(StorageError::RunReferenced { run_id: run });
    }
    match control::origin(conn, run).map_err(|error| ownership(run, error))? {
        Some(control::Origin::Controlled(_)) => Ok(()),
        Some(control::Origin::Legacy) => {
            Err(StorageError::DeletionCoordinationUnavailable { run_id: run })
        },
        None => Err(ownership(run, OpenError::RunNotFound(run))),
    }
}
pub(crate) async fn delete(storage: Arc<Storage>, run: RunId) -> Result<(), StorageError> {
    {
        let conn = storage
            .registry_pool
            .get()
            .map_err(|error| StorageError::Pool(error.to_string()))?;
        initial(&conn, run)?;
    }
    if storage
        .active_writers
        .is_held(&run)
        .await
        .map_err(|_| StorageError::WriterOwnershipPoisoned)?
    {
        return Err(StorageError::WriterStillActive { run_id: run });
    }
    tokio::task::spawn_blocking(move || delete_owned(&storage, run))
        .await
        .map_err(|error| StorageError::Pool(format!("owned deletion worker: {error}")))?
}
fn inventory(namespace: &DeletionNamespace) -> Result<Vec<Entry>, OpenError> {
    let mut pending = vec![PathBuf::new()];
    let mut entries = Vec::new();
    while let Some(path) = pending.pop() {
        if entries.len() >= MAX_ENTRIES {
            return Err(invalid());
        }
        let native = namespace.entry(&path)?.ok_or_else(invalid)?;
        if native.directory {
            for child in std::fs::read_dir(namespace.path().join(&path))? {
                let child = child?;
                if pending.len() + entries.len() + 1 >= MAX_ENTRIES {
                    return Err(invalid());
                }
                pending.push(path.join(child.file_name()));
            }
        }
        entries.push(Entry {
            path: encoded(&path),
            native,
        });
    }
    namespace.verify()?;
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    digest(&entries)?;
    Ok(entries)
}
fn digest(entries: &[Entry]) -> Result<(usize, Vec<u8>), OpenError> {
    let mut bytes = 0_usize;
    let mut hash = Sha256::new();
    for entry in entries {
        let value = serde_json::to_vec(entry).map_err(|_| invalid())?;
        bytes = bytes.checked_add(value.len()).ok_or_else(invalid)?;
        if bytes > MAX_BYTES {
            return Err(invalid());
        }
        hash.update(
            u64::try_from(entry.path.len())
                .map_err(|_| invalid())?
                .to_le_bytes(),
        );
        hash.update(&entry.path);
        hash.update(
            u64::try_from(value.len())
                .map_err(|_| invalid())?
                .to_le_bytes(),
        );
        hash.update(value);
    }
    Ok((bytes, hash.finalize().to_vec()))
}
fn validate_entries(entries: &[Entry]) -> Result<(), OpenError> {
    if entries.is_empty() || entries.len() > MAX_ENTRIES {
        return Err(invalid());
    }
    let indexed: BTreeMap<_, _> = entries
        .iter()
        .map(|entry| (entry.path.clone(), &entry.native))
        .collect();
    if indexed.len() != entries.len()
        || !indexed
            .get(&Vec::new())
            .is_some_and(|entry| entry.directory)
    {
        return Err(invalid());
    }
    for entry in entries {
        let path = decoded(&entry.path)?;
        if !matches!(entry.native.identity.len(), 16 | 24)
            || (entry.native.directory && (entry.native.size != 0 || entry.native.hash.is_some()))
            || (!entry.native.directory
                && entry
                    .native
                    .hash
                    .as_ref()
                    .is_none_or(|hash| hash.len() != 32))
        {
            return Err(invalid());
        }
        if !entry.path.is_empty()
            && !indexed
                .get(&encoded(path.parent().ok_or_else(invalid)?))
                .is_some_and(|value| value.directory)
        {
            return Err(invalid());
        }
    }
    Ok(())
}
type IntentRow = (
    String,
    String,
    Option<String>,
    Vec<u8>,
    i64,
    i64,
    i64,
    Vec<u8>,
    Vec<u8>,
);
fn read_intent(conn: &Connection, run: RunId) -> Result<Option<Intent>, StorageError> {
    let row:Option<IntentRow>=conn.query_row(
        "SELECT deletion_id,state,original_opening,journal_identity,inventory_version,inventory_count,inventory_bytes,inventory_digest,outside_parent FROM run_host_tombstones WHERE run_id=?",
        [run.to_string()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?,r.get(8)?))).optional()?;
    let Some(row) = row else {
        return Ok(None);
    };
    let outside: Outside = serde_json::from_slice(&row.8).map_err(|_| ownership(run, invalid()))?;
    if outside.kind != KIND
        || outside.platform != platform()
        || outside.run != run
        || row.2.as_deref() != Some(outside.opening.as_str())
        || row.4 != 1
        || serde_json::to_vec(&outside)? != row.8
        || row.3 != [outside.route.clone(), outside.database.clone()].concat()
    {
        return Err(ownership(run, invalid()));
    }
    let original:(Vec<u8>,Vec<u8>)=conn.query_row("SELECT route_identity,database_identity FROM run_host_openings WHERE opening_id=? AND run_id=? AND coordination_origin=run_id",
        params![outside.opening,run.to_string()],|row|Ok((row.get(0)?,row.get(1)?)))?;
    if original != (outside.route.clone(), outside.database.clone()) {
        return Err(ownership(run, invalid()));
    }
    let mut query=conn.prepare("SELECT relative_path,encoded_entry FROM run_host_deletion_inventory WHERE deletion_id=? ORDER BY relative_path")?;
    let raw = query.query_map([&row.0], |r| {
        Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, Vec<u8>>(1)?))
    })?;
    let mut entries = Vec::new();
    let mut encoded_bytes = 0_usize;
    for value in raw {
        let (path, bytes) = value?;
        encoded_bytes = encoded_bytes
            .checked_add(bytes.len())
            .ok_or_else(|| ownership(run, invalid()))?;
        if entries.len() >= MAX_ENTRIES || encoded_bytes > MAX_BYTES {
            return Err(ownership(run, invalid()));
        }
        let entry: Entry = serde_json::from_slice(&bytes).map_err(|_| ownership(run, invalid()))?;
        if path != entry.path || serde_json::to_vec(&entry)? != bytes {
            return Err(ownership(run, invalid()));
        }
        entries.push(entry);
    }
    validate_entries(&entries).map_err(|error| ownership(run, error))?;
    let (bytes, hash) = digest(&entries).map_err(|error| ownership(run, error))?;
    if i64::try_from(entries.len()).ok() != Some(row.5)
        || i64::try_from(bytes).ok() != Some(row.6)
        || hash != row.7
    {
        return Err(ownership(run, invalid()));
    }
    Ok(Some(Intent {
        id: row.0,
        state: row.1,
        outside,
        entries,
    }))
}
fn opening(conn: &Connection, run: RunId) -> Result<(String, Vec<u8>, Vec<u8>), StorageError> {
    Ok(conn.query_row("SELECT opening_id,route_identity,database_identity FROM run_host_openings WHERE run_id=? AND coordination_origin=run_id ORDER BY epoch DESC LIMIT 1",
        [run.to_string()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?)
}
fn prepare(
    storage: &Storage,
    run: RunId,
    owner: &control::RunUseOwner,
    namespace: &DeletionNamespace,
) -> Result<(Intent, super::FileLock), StorageError> {
    if namespace.absent().map_err(|error| ownership(run, error))? {
        return Err(ownership(run, invalid()));
    }
    let lock = super::FileLock::try_acquire_existing(&storage.lock_path(&run), run)
        .map_err(|error| ownership(run, error))?;
    let entries = inventory(namespace).map_err(|error| ownership(run, error))?;
    validate_entries(&entries).map_err(|error| ownership(run, error))?;
    let observed = {
        let conn = storage
            .registry_pool
            .get()
            .map_err(|error| StorageError::Pool(error.to_string()))?;
        opening(&conn, run)?
    };
    let root = entries
        .iter()
        .find(|entry| entry.path.is_empty())
        .ok_or_else(|| ownership(run, invalid()))?
        .native
        .identity
        .clone();
    let database = entries
        .iter()
        .find(|entry| entry.path == encoded(Path::new("events.sqlite")))
        .filter(|entry| !entry.native.directory)
        .ok_or_else(|| ownership(run, invalid()))?
        .native
        .identity
        .clone();
    if !entries
        .iter()
        .any(|entry| entry.path == encoded(Path::new("artifacts")) && entry.native.directory)
    {
        return Err(ownership(run, invalid()));
    }
    let (home, runs_parent) = namespace
        .outside_parent()
        .map_err(|error| ownership(run, error))?;
    let (control_home, controls_parent, control_file) = owner
        .control_identities()
        .map_err(|error| ownership(run, error))?;
    let mut route = vec![platform()];
    route.extend(&home);
    route.extend(&root);
    route.extend(lock.identity().map_err(|error| ownership(run, error))?);
    if home != control_home || observed.1 != route || observed.2 != database {
        return Err(ownership(run, invalid()));
    }
    let outside = Outside {
        kind: KIND.into(),
        platform: platform(),
        run,
        opening: observed.0,
        home,
        runs_parent,
        controls_parent,
        control_file,
        root,
        route,
        database,
    };
    Ok((
        Intent {
            id: RunId::new().to_string(),
            state: "DeletePending".into(),
            outside,
            entries,
        },
        lock,
    ))
}
fn publish(
    storage: &Storage,
    run: RunId,
    owner: &control::RunUseOwner,
    intent: &Intent,
) -> Result<(), StorageError> {
    owner.verify().map_err(|error| ownership(run, error))?;
    let (bytes, hash) = digest(&intent.entries).map_err(|error| ownership(run, error))?;
    let outside = serde_json::to_vec(&intent.outside)?;
    if outside.len() > 65536 {
        return Err(ownership(run, invalid()));
    }
    {
        let mut conn = storage
            .registry_pool
            .get()
            .map_err(|error| StorageError::Pool(error.to_string()))?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        initial(&tx, run)?;
        owner
            .verify_stored(&tx)
            .map_err(|error| ownership(run, error))?;
        if read_intent(&tx, run)?.is_some()
            || opening(&tx, run)?
                != (
                    intent.outside.opening.clone(),
                    intent.outside.route.clone(),
                    intent.outside.database.clone(),
                )
        {
            return Err(ownership(run, invalid()));
        }
        tx.execute("INSERT INTO run_host_tombstones(run_id,deletion_id,original_opening,journal_identity,provenance,inventory_version,inventory_count,inventory_bytes,inventory_digest,outside_parent,state) VALUES(?,?,?,?,'ExplicitStorageDelete',1,?,?,?,?,'DeletePending')",
            params![run.to_string(),intent.id,intent.outside.opening,[intent.outside.route.clone(),intent.outside.database.clone()].concat(),
                i64::try_from(intent.entries.len()).map_err(|_|ownership(run,invalid()))?,i64::try_from(bytes).map_err(|_|ownership(run,invalid()))?,hash,outside])?;
        for entry in &intent.entries {
            tx.execute("INSERT INTO run_host_deletion_inventory(deletion_id,relative_path,encoded_entry) VALUES(?,?,?)",
            params![intent.id,entry.path,serde_json::to_vec(entry)?])?;
        }
        // Only this admitted original-owner transaction can unregister usable history.
        tx.execute("DELETE FROM runs WHERE id=?", [run.to_string()])?;
        tx.commit()?;
    }
    owner.verify().map_err(|error| ownership(run, error))
}
fn verify_namespace(
    namespace: &DeletionNamespace,
    owner: &control::RunUseOwner,
    intent: &Intent,
) -> Result<(), OpenError> {
    let (home, runs) = namespace.outside_parent()?;
    let (control_home, controls, control_file) = owner.control_identities()?;
    if home != intent.outside.home
        || runs != intent.outside.runs_parent
        || control_home != home
        || controls != intent.outside.controls_parent
        || control_file != intent.outside.control_file
    {
        return Err(invalid());
    }
    if let Some(root) = namespace.entry(Path::new(""))?
        && root.identity != intent.outside.root
    {
        return Err(invalid());
    }
    Ok(())
}
fn cleanup(
    namespace: &mut DeletionNamespace,
    owner: &control::RunUseOwner,
    intent: &Intent,
    mut lock: Option<super::FileLock>,
) -> Result<(), OpenError> {
    verify_namespace(namespace, owner, intent)?;
    if namespace.absent()? {
        return Ok(());
    }
    let indexed: BTreeMap<_, _> = intent
        .entries
        .iter()
        .map(|entry| (entry.path.clone(), entry))
        .collect();
    for current in inventory(namespace)? {
        if indexed
            .get(&current.path)
            .is_none_or(|expected| expected.native != current.native)
        {
            return Err(invalid());
        }
    }
    let mut ordered: Vec<_> = intent.entries.iter().collect();
    ordered.sort_by_key(|entry| {
        let path = decoded(&entry.path).map_or(0, |path| path.components().count());
        (
            entry.path == encoded(Path::new("events.sqlite.lock")),
            std::cmp::Reverse(path),
            entry.path.clone(),
        )
    });
    // Root is always last; journal lock is the last non-root object.
    ordered.sort_by_key(|entry| entry.path.is_empty());
    for entry in ordered {
        let path = decoded(&entry.path)?;
        let Some(current) = namespace.entry(&path)? else {
            continue;
        };
        if current != entry.native {
            return Err(invalid());
        }
        if path == Path::new("events.sqlite.lock") {
            drop(lock.take());
        }
        let parent = if path.as_os_str().is_empty() {
            &intent.outside.runs_parent
        } else {
            &indexed
                .get(&encoded(path.parent().ok_or_else(invalid)?))
                .ok_or_else(invalid)?
                .native
                .identity
        };
        owner.verify()?;
        namespace.remove(&path, &entry.native, parent)?;
        #[cfg(test)]
        {
            if !entry.native.directory {
                process_boundary(intent.outside.run, "contents-removed")?;
            }
            if path == Path::new("artifacts") {
                process_boundary(intent.outside.run, "artifacts-removed")?;
            }
            if path == Path::new("events.sqlite.lock") {
                process_boundary(intent.outside.run, "lock-removed")?;
            }
        }
    }
    if !namespace.absent()? {
        return Err(invalid());
    }
    owner.verify()
}
fn mark_deleted(
    storage: &Storage,
    run: RunId,
    owner: &control::RunUseOwner,
    intent: &Intent,
) -> Result<(), StorageError> {
    owner.verify().map_err(|error| ownership(run, error))?;
    let mut conn = storage
        .registry_pool
        .get()
        .map_err(|error| StorageError::Pool(error.to_string()))?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if referenced(&tx, run)? {
        return Err(StorageError::RunReferenced { run_id: run });
    }
    owner
        .verify_stored(&tx)
        .map_err(|error| ownership(run, error))?;
    let current = read_intent(&tx, run)?.ok_or_else(|| ownership(run, invalid()))?;
    if current.id != intent.id {
        return Err(ownership(run, invalid()));
    }
    if current.state == "DeletePending" {
        if tx.execute("UPDATE run_host_tombstones SET state='Deleted' WHERE run_id=? AND deletion_id=? AND state='DeletePending'",params![run.to_string(),intent.id])?!=1 {return Err(ownership(run,invalid()));}
    } else if current.state != "Deleted" {
        return Err(ownership(run, invalid()));
    }
    tx.commit()?;
    drop(conn);
    owner.verify().map_err(|error| ownership(run, error))
}
fn delete_owned(storage: &Storage, run: RunId) -> Result<(), StorageError> {
    let owner = control::acquire_deletion(&storage.home, &storage.registry_pool, run)
        .map_err(|error| ownership(run, error))?;
    let existing = {
        let conn = storage
            .registry_pool
            .get()
            .map_err(|error| StorageError::Pool(error.to_string()))?;
        initial(&conn, run)?;
        read_intent(&conn, run)?
    };
    let mut namespace =
        DeletionNamespace::existing(&storage.home, run).map_err(|error| ownership(run, error))?;
    let (intent, lock) = if let Some(intent) = existing {
        verify_namespace(&namespace, &owner, &intent).map_err(|error| ownership(run, error))?;
        if intent.state == "Deleted"
            && !namespace.absent().map_err(|error| ownership(run, error))?
        {
            return Err(ownership(run, invalid()));
        }
        let lock = if namespace.absent().map_err(|error| ownership(run, error))? {
            None
        } else {
            match super::FileLock::try_acquire_existing(&storage.lock_path(&run), run) {
                Ok(lock) => Some(lock),
                Err(OpenError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => return Err(ownership(run, error)),
            }
        };
        (intent, lock)
    } else {
        let (intent, lock) = prepare(storage, run, &owner, &namespace)?;
        publish(storage, run, &owner, &intent)?;
        (intent, Some(lock))
    };
    #[cfg(test)]
    process_boundary(run, "published").map_err(|error| ownership(run, error))?;
    cleanup(&mut namespace, &owner, &intent, lock).map_err(|error| ownership(run, error))?;
    #[cfg(test)]
    process_boundary(run, "root-removed").map_err(|error| ownership(run, error))?;
    mark_deleted(storage, run, &owner, &intent)
}

#[cfg(test)]
fn process_boundary(run: RunId, phase: &str) -> Result<(), OpenError> {
    if std::env::var("SURGE_TEST_DELETE_RUN").ok().as_deref() != Some(run.to_string().as_str())
        || std::env::var("SURGE_TEST_DELETE_PHASE").ok().as_deref() != Some(phase)
    {
        return Ok(());
    }
    let marker = std::env::var_os("SURGE_TEST_DELETE_MARKER").ok_or_else(invalid)?;
    let marker = PathBuf::from(marker);
    let staging = marker.with_extension("publishing");
    std::fs::write(&staging, phase.as_bytes())?;
    std::fs::rename(staging, marker)?;
    let mut byte = [0_u8];
    std::io::stdin().read_exact(&mut byte)?;
    Ok(())
}

#[cfg(all(test, windows))]
mod windows_inventory_tests {
    use super::*;
    use crate::runtime_home_fixture::FixtureHome;
    use std::time::{Duration, Instant};

    async fn actual_locked_entry(
        bytes: &[u8],
        artifact: bool,
    ) -> (Result<Option<NativeEntry>, OpenError>, Vec<u8>, Vec<u8>) {
        let home = FixtureHome::new().unwrap();
        let storage = Storage::open(home.path()).await.unwrap();
        let run = RunId::new();
        let writer = storage.create_run(run, home.path(), None).await.unwrap();
        writer.close().await.unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let owner = loop {
            match control::acquire(storage.home(), &storage.registry_pool, run, true) {
                Ok(owner) => break owner,
                Err(OpenError::RunInUse { .. }) if Instant::now() < deadline => {
                    std::thread::yield_now();
                },
                Err(error) => panic!("producer owners did not settle: {error:?}"),
            }
        };
        let lock_path = storage.lock_path(&run);
        let leaf = if artifact {
            "artifacts/nonempty-control"
        } else {
            "events.sqlite.lock"
        };
        let path = storage.run_dir(&run).join(leaf);
        std::fs::write(&path, bytes).unwrap();
        let lock = super::super::FileLock::try_acquire_existing(&lock_path, run).unwrap();
        let identity = lock.identity().unwrap();
        let namespace = DeletionNamespace::existing(home.path(), run).unwrap();
        let observed = namespace.entry(Path::new(leaf));
        eprintln!(
            "WG7 inventory phase=held-journal-leaf path={path:?} length={} result={observed:?}",
            bytes.len()
        );
        drop(namespace);
        drop(lock);
        let unchanged = std::fs::read(&path).unwrap();
        drop(owner);
        let witness = storage.registry_pool_witness().unwrap();
        drop(storage);
        tokio::time::timeout(Duration::from_secs(10), async {
            while witness.is_retained() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        home.close().unwrap();
        (observed, identity, unchanged)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn actual_owned_empty_journal_inventory_does_not_read_locked_region() {
        let (observed, identity, bytes) = actual_locked_entry(b"", false).await;
        assert!(bytes.is_empty());
        let entry = observed
            .expect("actual zero-length owned journal must not issue locked ReadFile")
            .expect("actual producer journal must exist");
        assert_eq!(entry.identity, identity);
        assert!(!entry.directory);
        assert_eq!(entry.size, 0);
        assert_eq!(
            entry.hash,
            Some(vec![
                0xe3, 0xb0, 0xc4, 0x42, 0x98, 0xfc, 0x1c, 0x14, 0x9a, 0xfb, 0xf4, 0xc8, 0x99, 0x6f,
                0xb9, 0x24, 0x27, 0xae, 0x41, 0xe4, 0x64, 0x9b, 0x93, 0x4c, 0xa4, 0x95, 0x99, 0x1b,
                0x78, 0x52, 0xb8, 0x55,
            ])
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn actual_nonempty_artifacts_keep_complete_hash_and_changed_content() {
        let (first, _, first_bytes) = actual_locked_entry(b"fixed artifact A", true).await;
        let (second, _, second_bytes) = actual_locked_entry(b"fixed artifact B", true).await;
        assert_eq!(first_bytes, b"fixed artifact A");
        assert_eq!(second_bytes, b"fixed artifact B");
        let first = first.unwrap().unwrap();
        let second = second.unwrap().unwrap();
        assert_eq!(first.size, 16);
        assert_eq!(second.size, 16);
        assert_ne!(first.hash, second.hash);
        assert_eq!(
            first.hash,
            Some(sha2::Sha256::digest(b"fixed artifact A").to_vec())
        );
        assert_eq!(
            second.hash,
            Some(sha2::Sha256::digest(b"fixed artifact B").to_vec())
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn actual_owned_nonempty_journal_keeps_locked_read_refusal_and_bytes() {
        let (observed, _, bytes) = actual_locked_entry(b"fixed nonempty journal", false).await;
        assert_eq!(bytes, b"fixed nonempty journal");
        assert!(matches!(observed, Err(OpenError::Io(error)) if error.raw_os_error() == Some(33)));
    }
}
