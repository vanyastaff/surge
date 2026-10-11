//! Original-owner opening barriers. An opening is audit evidence, never successful settlement.
mod framing;
#[cfg(not(windows))]
mod unix;
#[cfg(windows)]
use crate::state_home::{RunRoot, RunRoute};
#[cfg(not(windows))]
use unix::{Database, RunRoot, RunRoute};
#[cfg(windows)]
type Database = Arc<crate::state_home::SqliteNamespaceOwner>;

use super::{
    error::{OpenError, StorageError},
    file_lock::FileLock,
    registry_exec,
    storage::Storage,
    writer_slot::{ActiveWriters, WriterLease, WriterToken},
};
use crate::SqliteConnectionManager;
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use surge_core::RunId;

pub(crate) fn storage_error<E>(error: E) -> StorageError
where
    E: Into<OpenError>,
{
    StorageError::OperationRejected(error.into().to_string())
}
fn opening_error(error: StorageError) -> OpenError {
    OpenError::Authority(error.to_string())
}

/// A unique reserved new-run opening. It cannot be cloned or reconstructed from a path.
/// Move this owner into the actual artifact/checkpoint worker and return it only after that worker finishes.
pub struct RunCreationReservation {
    storage: Arc<Storage>,
    run: RunId,
    prepared: PreparedOpening,
}
impl RunCreationReservation {
    /// The one run bound to this reservation.
    #[must_use]
    pub fn run_id(&self) -> &RunId {
        &self.run
    }
    pub(crate) fn artifacts_path(&self, runs_root: &Path) -> Result<PathBuf, OpenError> {
        if runs_root != self.storage.home.join("runs") {
            return Err(OpenError::Authority(
                "reservation belongs to another artifact root".into(),
            ));
        }
        self.prepared.verify()?;
        Ok(self.storage.artifacts_dir(&self.run))
    }
}
struct PreparedOpening {
    token: Arc<WriterToken>,
    lock: FileLock,
    route: Arc<RunRoute>,
    opening: Opening,
}
impl PreparedOpening {
    fn verify(&self) -> Result<(), OpenError> {
        self.route.verify()?;
        self.lock.identity()?;
        Ok(())
    }
    fn bind(self, database: Database) -> Arc<WriterLease> {
        Arc::new(WriterLease {
            _token: self.token,
            file_lock: self.lock,
            #[cfg(windows)]
            namespace: database.clone(),
            authority: HeldOpening {
                route: self.route,
                database,
                _opening: self.opening,
            },
        })
    }
}
/// All objects remain held through actor/pool/read-worker settlement.
pub(crate) struct HeldOpening {
    route: Arc<RunRoute>,
    database: Database,
    _opening: Opening,
}
impl HeldOpening {
    pub(crate) fn verify(&self) -> Result<(), OpenError> {
        self.route.verify()?;
        #[cfg(not(windows))]
        self.route.verify_database(&self.database)?;
        #[cfg(windows)]
        self.database.verify()?;
        Ok(())
    }
}
struct Opening {
    id: String,
    epoch: i64,
}

pub(crate) fn admit(connection: &Connection, run: RunId) -> Result<(), StorageError> {
    let deleted: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM run_host_tombstones WHERE run_id=?)",
        [run.to_string()],
        |row| row.get(0),
    )?;
    if deleted {
        return Err(StorageError::OperationRejected(
            "run has a permanent deletion intent".into(),
        ));
    }
    Ok(())
}
fn record(
    tx: &Transaction<'_>,
    run: RunId,
    route_identity: Vec<u8>,
    database: Option<Vec<u8>>,
) -> Result<Opening, StorageError> {
    admit(tx, run)?;
    let previous:Option<(String,Option<String>,Option<String>)>=tx.query_row(
        "SELECT opening_id,unresolved_engine_opening,last_confirmed_engine_receipt FROM run_host_openings WHERE run_id=? ORDER BY epoch DESC LIMIT 1",
        [run.to_string()],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).optional()?;
    let (predecessor, unresolved, confirmed) =
        previous.map_or((None, None, None), |(id, u, c)| (Some(id), u, c));
    let id = ulid::Ulid::new().to_string();
    tx.execute("INSERT INTO run_host_openings(opening_id,run_id,owner_mode,state,predecessor,identity_version,route_identity,database_identity,unresolved_engine_opening,last_confirmed_engine_receipt) VALUES(?,?,'JournalOnly','Opening',?,1,?,?,?,?)",
        params![id,run.to_string(),predecessor,route_identity,database,unresolved,confirmed])?;
    Ok(Opening {
        id,
        epoch: tx.last_insert_rowid(),
    })
}
fn verify_database(route: &RunRoute, database: &Database) -> Result<(), OpenError> {
    route.verify()?;
    #[cfg(not(windows))]
    route.verify_database(database)?;
    #[cfg(windows)]
    database.verify()?;
    Ok(())
}
fn route_identity(route: &RunRoute, lock: &FileLock) -> Result<Vec<u8>, OpenError> {
    let (home, run) = route.identities()?;
    #[cfg(windows)]
    let tag = 2;
    #[cfg(not(windows))]
    let tag = 1;
    let mut bytes = vec![tag];
    bytes.extend(home);
    bytes.extend(run);
    bytes.extend(lock.identity()?);
    Ok(bytes)
}
fn token(writers: &ActiveWriters, run: RunId) -> Result<Arc<WriterToken>, OpenError> {
    writers
        .try_acquire_sync(run)
        .map_err(|_| OpenError::WriterOwnershipPoisoned)?
        .ok_or(OpenError::WriterAlreadyHeld { run_id: run })
}
fn existing_parts(
    home: &Path,
    writers: &ActiveWriters,
    run: RunId,
) -> Result<(Arc<WriterToken>, FileLock, Arc<RunRoute>, Database), OpenError> {
    let token = token(writers, run)?;
    let root = RunRoot::existing(home)?;
    let run_path = home.join("runs").join(run.to_string());
    let database_path = run_path.join("events.sqlite");
    let lock = match FileLock::try_acquire_existing(&run_path.join("events.sqlite.lock"), run) {
        Ok(lock) => lock,
        Err(OpenError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            if database_path.exists() {
                return Err(OpenError::Io(error));
            }
            return Err(OpenError::RunNotFound(run));
        },
        Err(error) => return Err(error),
    };
    if !database_path.exists() {
        return Err(OpenError::RunNotFound(run));
    }
    #[cfg(unix)]
    let route = {
        Arc::new(match root.run(run, false) {
            Err(OpenError::Io(error))
                if error.kind() == std::io::ErrorKind::NotFound
                    && (!run_path.exists() || !database_path.exists()) =>
            {
                return Err(OpenError::RunNotFound(run));
            },
            result => result?,
        })
    };
    #[cfg(windows)]
    let route = Arc::new(root.run(run, false).map_err(OpenError::from)?);
    let database = route.database(false)?;
    Ok((token, lock, route, database))
}
/// Called by the already-owned synchronous informational delivery worker; no journal SQL precedes it.
pub(crate) fn open_existing_sync(
    home: &Path,
    pool: &r2d2::Pool<SqliteConnectionManager>,
    writers: &ActiveWriters,
    run: RunId,
) -> Result<Arc<WriterLease>, OpenError> {
    {
        let connection = pool.get().map_err(|e| OpenError::Pool(e.to_string()))?;
        admit(&connection, run).map_err(opening_error)?;
    }
    let (token, lock, route, database) = existing_parts(home, writers, run)?;
    let identities = route_identity(&route, &lock)?;
    let database_identity = database.identity()?;
    let mut connection = pool.get().map_err(|e| OpenError::Pool(e.to_string()))?;
    let tx = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    verify_database(&route, &database)?;
    lock.identity()?;
    let opening = record(&tx, run, identities, Some(database_identity)).map_err(opening_error)?;
    tx.commit()?;
    Ok(PreparedOpening {
        token,
        lock,
        route,
        opening,
    }
    .bind(database))
}
impl Storage {
    /// Reserve exactly one fresh run and its original exclusion before any copied artifacts or journal database.
    pub async fn reserve_new_run(
        self: &Arc<Self>,
        run: RunId,
    ) -> Result<RunCreationReservation, OpenError> {
        let token = token(&self.active_writers, run)?;
        let root = RunRoot::existing(&self.home)?;
        root.verify()?;
        // The actual fenced worker owns token and route even when its caller is dropped.
        let (token, route) = registry_exec::write(&self.registry_pool, move |tx| {
            admit(tx, run)?;
            root.verify().map_err(storage_error)?;
            let route = root.run(run, true).map_err(storage_error)?;
            Ok((token, Arc::new(route)))
        })
        .await
        .map_err(opening_error)?;
        let lock = FileLock::try_acquire_existing(&self.lock_path(&run), run)?;
        let identity = route_identity(&route, &lock)?;
        let prepared = registry_exec::write(&self.registry_pool, move |tx| {
            route.verify().map_err(storage_error)?;
            lock.identity().map_err(storage_error)?;
            let opening = record(tx, run, identity, None)?;
            Ok(PreparedOpening {
                token,
                lock,
                route,
                opening,
            })
        })
        .await
        .map_err(opening_error)?;
        Ok(RunCreationReservation {
            storage: self.clone(),
            run,
            prepared,
        })
    }
    pub(crate) async fn acquire_opening(
        self: &Arc<Self>,
        run: RunId,
    ) -> Result<Arc<WriterLease>, OpenError> {
        registry_exec::read(&self.registry_pool, move |connection| {
            admit(connection, run)
        })
        .await
        .map_err(opening_error)?;
        let (token, lock, route, database) = existing_parts(&self.home, &self.active_writers, run)?;
        let identity = route_identity(&route, &lock)?;
        let db_identity = database.identity()?;
        registry_exec::write(&self.registry_pool, move |tx| {
            route.verify().map_err(storage_error)?;
            lock.identity().map_err(storage_error)?;
            verify_database(&route, &database).map_err(storage_error)?;
            let opening = record(tx, run, identity, Some(db_identity))?;
            Ok(PreparedOpening {
                token,
                lock,
                route,
                opening,
            }
            .bind(database))
        })
        .await
        .map_err(opening_error)
    }
    pub(crate) async fn bind_reserved(
        self: &Arc<Self>,
        reservation: RunCreationReservation,
    ) -> Result<(RunId, Arc<WriterLease>), OpenError> {
        if !Arc::ptr_eq(self, &reservation.storage) {
            return Err(OpenError::Authority(
                "reservation belongs to another Storage owner".into(),
            ));
        }
        let RunCreationReservation { run, prepared, .. } = reservation;
        registry_exec::write(&self.registry_pool,move|tx|{
            admit(tx,run)?;
            prepared.verify().map_err(storage_error)?;
            let latest:Option<String>=tx.query_row("SELECT opening_id FROM run_host_openings WHERE run_id=? ORDER BY epoch DESC LIMIT 1",[run.to_string()],|row|row.get(0)).optional()?;
            if latest.as_deref()!=Some(prepared.opening.id.as_str()){return Err(StorageError::OperationRejected("reserved opening superseded".into()));}
            let database=prepared.route.database(true).map_err(storage_error)?;
            let identity=database.identity().map_err(storage_error)?;
            let changed=tx.execute("UPDATE run_host_openings SET database_identity=? WHERE opening_id=? AND epoch=? AND database_identity IS NULL AND state='Opening'",params![identity,prepared.opening.id,prepared.opening.epoch])?;
            if changed!=1 {return Err(StorageError::OperationRejected("reserved database identity already bound".into()));}
            Ok((run,prepared.bind(database)))
        }).await.map_err(opening_error)
    }
}

pub(crate) fn verify_empty_initialized(
    run: RunId,
    path: &Path,
    lease: &WriterLease,
) -> Result<(), OpenError> {
    lease.verify()?;
    let mut identity = route_identity(&lease.authority.route, &lease.file_lock)?;
    identity.extend(lease.authority.database.identity()?);
    #[cfg(windows)]
    let mut connection = super::connection::RetainedConnection::open_owned(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        lease.namespace.clone(),
    )?;
    #[cfg(not(windows))]
    let mut connection =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    framing::snapshot(&mut connection, run, &identity, 0).map_err(opening_error)?;
    drop(connection);
    lease.verify()
}
