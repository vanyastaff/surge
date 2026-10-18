//! Pool manager and reconnect workers retain actual controlled SQLite ownership.
use super::{RetainedConnection, control_error};
use crate::runs::host_settlement::control::{RunDatabaseOwner, RunUseOwner};
use rusqlite::{Connection, OpenFlags};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
type ConnectionInit = dyn Fn(&mut Connection) -> rusqlite::Result<()> + Send + Sync;

/// SQLite pool manager retaining a shared original run owner through actual close.
pub struct OwnedSqliteConnectionManager {
    inner: r2d2_sqlite::SqliteConnectionManager,
    path: PathBuf,
    flags: OpenFlags,
    owner: Mutex<Option<Arc<RunDatabaseOwner>>>,
    init: Option<Box<ConnectionInit>>,
}
impl std::fmt::Debug for OwnedSqliteConnectionManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OwnedSqliteConnectionManager")
            .finish_non_exhaustive()
    }
}
impl OwnedSqliteConnectionManager {
    /// Configure an existing SQLite path; run ownership is checked before connect.
    #[must_use]
    pub fn file(path: impl AsRef<Path>) -> Self {
        Self {
            inner: r2d2_sqlite::SqliteConnectionManager::file(path.as_ref()),
            path: path.as_ref().to_path_buf(),
            flags: OpenFlags::default(),
            owner: Mutex::new(None),
            init: None,
        }
    }
    /// Configure SQLite open flags without changing native admission.
    #[must_use]
    pub fn with_flags(mut self, flags: OpenFlags) -> Self {
        self.inner = self.inner.with_flags(flags);
        self.flags = flags;
        self
    }
    /// Configure initialization after native ownership is attached to each connection.
    #[must_use]
    pub(crate) fn with_init(
        mut self,
        init: impl Fn(&mut Connection) -> rusqlite::Result<()> + Send + Sync + 'static,
    ) -> Self {
        self.init = Some(Box::new(init));
        self
    }
    pub(crate) fn with_run_owner(self, owner: Option<Arc<RunUseOwner>>) -> rusqlite::Result<Self> {
        *self.owner.lock().map_err(|_| {
            rusqlite::Error::InvalidParameterName("run manager owner poisoned".into())
        })? = owner
            .as_ref()
            .map(crate::runs::host_settlement::control::RunUseOwner::database)
            .transpose()
            .map_err(control_error)?;
        Ok(self)
    }
    fn retained_owner(&self) -> rusqlite::Result<Option<Arc<RunDatabaseOwner>>> {
        let mut stored = self.owner.lock().map_err(|_| {
            rusqlite::Error::InvalidParameterName("run manager owner poisoned".into())
        })?;
        if stored.is_none() {
            let owner = crate::runs::host_settlement::control::use_from_path(&self.path)
                .map_err(control_error)?;
            *stored = owner
                .as_ref()
                .map(crate::runs::host_settlement::control::RunUseOwner::database)
                .transpose()
                .map_err(control_error)?;
        }
        if let Some(owner) = stored.as_ref() {
            owner.verify().map_err(control_error)?;
        }
        Ok(stored.clone())
    }
}
impl r2d2::ManageConnection for OwnedSqliteConnectionManager {
    type Connection = RetainedConnection;
    type Error = rusqlite::Error;
    fn connect(&self) -> rusqlite::Result<RetainedConnection> {
        let owner = self.retained_owner()?;
        let raw = if owner.is_some() {
            Connection::open_with_flags(&self.path, self.flags & !OpenFlags::SQLITE_OPEN_CREATE)?
        } else {
            self.inner.connect()?
        };
        let mut connection = RetainedConnection {
            connection: Some(raw),
            run_owner: owner,
        };
        if let Some(owner) = &connection.run_owner {
            owner.verify().map_err(control_error)?;
        }
        if let Some(init) = &self.init {
            init(connection.connection_mut())?;
        }
        Ok(connection)
    }
    fn is_valid(&self, connection: &mut RetainedConnection) -> rusqlite::Result<()> {
        if let Some(owner) = &connection.run_owner {
            owner.verify().map_err(control_error)?;
        }
        self.inner.is_valid(connection.connection_mut())
    }
    fn has_broken(&self, connection: &mut RetainedConnection) -> bool {
        self.inner.has_broken(connection.connection_mut())
    }
}
