//! Permanent outside-run coordination; an owner excludes deletion, not other readers.
#[cfg(unix)]
use super::unix::NativeRunControl;
use super::{OpenError, SqliteConnectionManager, Storage, admit};
#[cfg(windows)]
use crate::state_home::NativeRunControl;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use surge_core::RunId;

pub(crate) type NativeControlIdentities = (Vec<u8>, Vec<u8>, Vec<u8>);
type StoredOpeningRow = (String, i64, String, Vec<u8>, Option<Vec<u8>>);
type StoredControlRow = (String, i64, Vec<u8>, Vec<u8>, Vec<u8>);
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Record {
    nonce: String,
    home: Vec<u8>,
    parent: Vec<u8>,
    file: Vec<u8>,
}
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum Origin {
    Legacy,
    Controlled(Record),
}
fn invalid() -> OpenError {
    OpenError::Authority("run coordination origin/control changed or missing".into())
}
pub(super) fn origin(conn: &Connection, run: RunId) -> Result<Option<Origin>, OpenError> {
    let value: Option<(String, Option<String>)> = conn
        .query_row(
            "SELECT kind,creation_nonce FROM run_coordination_origins WHERE run_id=?",
            [run.to_string()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let control:Option<StoredControlRow>=conn.query_row(
        "SELECT creation_nonce,identity_version,home_identity,parent_identity,file_identity FROM run_host_controls WHERE run_id=?",
        [run.to_string()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional()?;
    match (value, control) {
        (Some((kind, None)), None) if kind == "Legacy" => Ok(Some(Origin::Legacy)),
        (Some((kind, Some(nonce))), Some((bound, 1, home, parent, file)))
            if kind == "NewControlled"
                && nonce == bound
                && !nonce.is_empty()
                && [home.len(), parent.len(), file.len()]
                    .iter()
                    .all(|length| matches!(length, 16 | 24)) =>
        {
            Ok(Some(Origin::Controlled(Record {
                nonce,
                home,
                parent,
                file,
            })))
        },
        (None, None) => {
            let recorded:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM run_host_openings WHERE run_id=? UNION ALL SELECT 1 FROM runs WHERE id=?)",
                params![run.to_string(),run.to_string()],|r|r.get(0))?;
            if recorded { Err(invalid()) } else { Ok(None) }
        },
        _ => Err(invalid()),
    }
}
fn native(
    home: &Path,
    run: RunId,
    create: bool,
    exclusive: bool,
) -> Result<NativeRunControl, OpenError> {
    #[cfg(unix)]
    {
        NativeRunControl::acquire(home, run, create, exclusive)
    }
    #[cfg(windows)]
    {
        NativeRunControl::acquire(home, run, create, exclusive).map_err(|error| match error {
            crate::state_home::NativeError::Busy => OpenError::RunInUse { run_id: run },
            other => other.into(),
        })
    }
}
/// The actual manager, connection and worker each retain this owner through close.
pub(crate) struct RunUseOwner {
    native: NativeRunControl,
    record: Record,
    run: RunId,
    registry: PathBuf,
}
impl RunUseOwner {
    pub(crate) fn verify(&self) -> Result<(), OpenError> {
        let (home, parent, file) = self.native.identities()?;
        if (home, parent, file)
            != (
                self.record.home.clone(),
                self.record.parent.clone(),
                self.record.file.clone(),
            )
        {
            return Err(invalid());
        }
        Ok(())
    }
    pub(crate) fn verify_reconnect(&self) -> Result<(), OpenError> {
        {
            let conn = crate::runs::connection::RetainedConnection::read_only(&self.registry)?;
            if origin(&conn, self.run)? != Some(Origin::Controlled(self.record.clone())) {
                return Err(invalid());
            }
            admit(&conn, self.run).map_err(|error| OpenError::Authority(error.to_string()))?;
        }
        self.verify()
    }
}
pub(crate) fn acquire(
    home: &Path,
    pool: &r2d2::Pool<SqliteConnectionManager>,
    run: RunId,
    exclusive: bool,
) -> Result<Option<Arc<RunUseOwner>>, OpenError> {
    let observed = {
        let conn = pool.get().map_err(|e| OpenError::Pool(e.to_string()))?;
        let value = origin(&conn, run)?;
        admit(&conn, run).map_err(|error| OpenError::Authority(error.to_string()))?;
        value
    };
    let Some(Origin::Controlled(record)) = observed else {
        return Ok(None);
    };
    let native = native(home, run, false, exclusive)?;
    let owner = Arc::new(RunUseOwner {
        native,
        record,
        run,
        registry: home.join("db/registry.sqlite"),
    });
    owner.verify_reconnect()?;
    Ok(Some(owner))
}
pub(crate) fn establish(storage: &Storage, run: RunId) -> Result<Arc<RunUseOwner>, OpenError> {
    {
        let conn = storage
            .registry_pool
            .get()
            .map_err(|e| OpenError::Pool(e.to_string()))?;
        if origin(&conn, run)?.is_some() {
            return Err(invalid());
        }
        admit(&conn, run).map_err(|error| OpenError::Authority(error.to_string()))?;
    }
    match std::fs::symlink_metadata(storage.run_dir(&run)) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
        Err(error) => return Err(error.into()),
        Ok(_) => return Err(invalid()),
    }
    let exclusive = native(&storage.home, run, true, true)?;
    let (home, parent, file) = exclusive.identities()?;
    let nonce = RunId::new().to_string();
    {
        let mut conn = storage
            .registry_pool
            .get()
            .map_err(|e| OpenError::Pool(e.to_string()))?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if origin(&tx, run)?.is_some() {
            return Err(invalid());
        }
        admit(&tx, run).map_err(|error| OpenError::Authority(error.to_string()))?;
        tx.execute("INSERT INTO run_coordination_origins(run_id,kind,creation_nonce) VALUES(?,'NewControlled',?)",params![run.to_string(),nonce])?;
        tx.execute("INSERT INTO run_host_controls(run_id,creation_nonce,identity_version,home_identity,parent_identity,file_identity) VALUES(?,?,1,?,?,?)",
            params![run.to_string(),nonce,home,parent,file])?;
        tx.commit()?;
    }
    exclusive.verify()?;
    drop(exclusive);
    acquire(&storage.home, &storage.registry_pool, run, false)?.ok_or_else(invalid)
}
pub(crate) fn record_legacy_opening(conn: &Connection, run: RunId) -> Result<(), OpenError> {
    if origin(conn, run)?.is_none() {
        conn.execute(
            "INSERT INTO run_coordination_origins(run_id,kind) VALUES(?,'Legacy')",
            [run.to_string()],
        )?;
    }
    Ok(())
}

/// Original route and scalar database identity accompany every actual SQLite owner.
pub(crate) struct RunDatabaseOwner {
    route: Arc<super::RunRoute>,
    database: super::Database,
    control: Arc<RunUseOwner>,
}
impl RunDatabaseOwner {
    pub(crate) fn verify(&self) -> Result<(), OpenError> {
        self.control.verify_reconnect()?;
        super::verify_database(&self.route, &self.database)?;
        Ok(())
    }
}
impl RunUseOwner {
    pub(crate) fn database(self: &Arc<Self>) -> Result<Arc<RunDatabaseOwner>, OpenError> {
        self.verify_reconnect()?;
        let root = super::RunRoot::existing(
            self.registry
                .parent()
                .and_then(Path::parent)
                .ok_or_else(invalid)?,
        )?;
        #[cfg(unix)]
        let route = Arc::new(root.existing_run(self.run)?);
        #[cfg(windows)]
        let route = Arc::new(root.run(self.run, false)?);
        let expected_route = route.reader_route_identity()?;
        let expected = {
            let conn = crate::runs::connection::RetainedConnection::read_only(&self.registry)?;
            let latest: Option<StoredOpeningRow> = conn.query_row(
                "SELECT state,identity_version,coordination_origin,route_identity,database_identity FROM run_host_openings WHERE run_id=? ORDER BY epoch DESC LIMIT 1",
                [self.run.to_string()], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?))).optional()?;
            let Some((state, 1, origin, recorded_route, database)) = latest else {
                return Err(invalid());
            };
            if origin != self.run.to_string()
                || recorded_route != expected_route
                || !matches!(
                    state.as_str(),
                    "Opening" | "JoinedWriter" | "ConfirmedExecution" | "Interrupted" | "Failed"
                )
            {
                return Err(invalid());
            }
            match database {
                Some(identity) => identity,
                None if state == "Opening" => {
                    return Err(OpenError::OpeningNotReady { run_id: self.run });
                },
                None => return Err(invalid()),
            }
        };
        #[cfg(unix)]
        let database = route.controlled_database(false)?;
        #[cfg(windows)]
        let database = route.database(false)?;
        if expected != database.identity()? {
            return Err(invalid());
        }
        Ok(Arc::new(RunDatabaseOwner {
            control: self.clone(),
            route,
            database,
        }))
    }
}
pub(crate) fn use_from_path(path: &Path) -> Result<Option<Arc<RunUseOwner>>, OpenError> {
    if path.file_name() != Some(std::ffi::OsStr::new("events.sqlite")) {
        return Ok(None);
    }
    let directory = path.parent().ok_or_else(invalid)?;
    let run = directory
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(invalid)?
        .parse()
        .map_err(|_| invalid())?;
    let runs = directory.parent().ok_or_else(invalid)?;
    if runs.file_name() != Some(std::ffi::OsStr::new("runs")) {
        return Err(invalid());
    }
    let home = runs.parent().ok_or_else(invalid)?;
    let registry = home.join("db/registry.sqlite");
    let record = {
        let conn = crate::runs::connection::RetainedConnection::read_only(&registry)?;
        let origin = origin(&conn, run)?;
        admit(&conn, run).map_err(|error| OpenError::Authority(error.to_string()))?;
        origin
    };
    let Some(Origin::Controlled(record)) = record else {
        return Ok(None);
    };
    let native = native(home, run, false, false)?;
    let owner = Arc::new(RunUseOwner {
        native,
        record,
        run,
        registry,
    });
    owner.verify_reconnect()?;
    Ok(Some(owner))
}

pub(super) fn acquire_deletion(
    home: &Path,
    pool: &r2d2::Pool<SqliteConnectionManager>,
    run: RunId,
) -> Result<Arc<RunUseOwner>, OpenError> {
    let record = {
        let conn = pool
            .get()
            .map_err(|error| OpenError::Pool(error.to_string()))?;
        match origin(&conn, run)? {
            Some(Origin::Controlled(record)) => record,
            _ => return Err(invalid()),
        }
    };
    let native = native(home, run, false, true)?;
    let owner = Arc::new(RunUseOwner {
        native,
        record,
        run,
        registry: home.join("db/registry.sqlite"),
    });
    {
        let conn = pool
            .get()
            .map_err(|error| OpenError::Pool(error.to_string()))?;
        if origin(&conn, run)? != Some(Origin::Controlled(owner.record.clone())) {
            return Err(invalid());
        }
    }
    owner.verify()?;
    Ok(owner)
}
impl RunUseOwner {
    pub(super) fn control_identities(&self) -> Result<NativeControlIdentities, OpenError> {
        #[cfg(unix)]
        {
            self.native.identities()
        }
        #[cfg(windows)]
        {
            self.native.identities().map_err(Into::into)
        }
    }
}

impl RunUseOwner {
    pub(super) fn verify_stored(&self, conn: &Connection) -> Result<(), OpenError> {
        if origin(conn, self.run)? != Some(Origin::Controlled(self.record.clone())) {
            return Err(invalid());
        }
        Ok(())
    }
}
