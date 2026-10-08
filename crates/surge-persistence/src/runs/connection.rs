//! SQLite connections retaining native ownership through checked final close.
#[cfg(windows)]
use rusqlite::TransactionBehavior;
use rusqlite::{Connection, OpenFlags, Transaction};
use std::{ops::Deref, path::Path};

/// An owned SQLite connection whose native namespace remains held until close.
/// Mutable raw connection extraction is deliberately unavailable.
pub struct RetainedConnection {
    connection: Option<Connection>,
    #[cfg(windows)]
    _namespace: Option<std::sync::Arc<crate::state_home::SqliteNamespaceOwner>>,
}
impl RetainedConnection {
    pub(crate) fn read_only(path: &Path) -> rusqlite::Result<Self> {
        Self::open_existing(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
    }
    pub(crate) fn read_only_no_mutex(path: &Path) -> rusqlite::Result<Self> {
        Self::open_existing(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
    }
    fn open_existing(path: &Path, flags: OpenFlags) -> rusqlite::Result<Self> {
        #[cfg(windows)]
        {
            let namespace =
                crate::state_home::SqliteNamespaceOwner::existing(path).map_err(native_error)?;
            Self::open_owned(path, flags, namespace)
        }
        #[cfg(not(windows))]
        {
            Ok(Self {
                connection: Some(Connection::open_with_flags(path, flags)?),
            })
        }
    }
    pub(crate) fn connection_mut(&mut self) -> &mut Connection {
        match &mut self.connection {
            Some(connection) => connection,
            None => std::process::abort(),
        }
    }
    /// Begin a transaction borrowing this complete owner.
    ///
    /// # Errors
    /// Returns SQLite errors without releasing namespace ownership.
    pub fn transaction(&mut self) -> rusqlite::Result<Transaction<'_>> {
        self.connection_mut().transaction()
    }
    /// Begin a transaction with explicit locking behavior, retaining its owner.
    ///
    /// # Errors
    /// Returns SQLite errors without releasing namespace ownership.
    #[cfg(windows)]
    pub fn transaction_with_behavior(
        &mut self,
        behavior: TransactionBehavior,
    ) -> rusqlite::Result<Transaction<'_>> {
        self.connection_mut().transaction_with_behavior(behavior)
    }
}
impl std::fmt::Debug for RetainedConnection {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OwnedSqliteConnection")
            .field("open", &self.connection.is_some())
            .finish_non_exhaustive()
    }
}
impl Deref for RetainedConnection {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        match &self.connection {
            Some(connection) => connection,
            None => std::process::abort(),
        }
    }
}
#[cfg(not(windows))]
impl std::ops::DerefMut for RetainedConnection {
    fn deref_mut(&mut self) -> &mut Connection {
        self.connection_mut()
    }
}

#[cfg(windows)]
fn native_error(error: crate::state_home::NativeError) -> rusqlite::Error {
    rusqlite::Error::InvalidParameterName(error.to_string())
}
#[cfg(windows)]
impl RetainedConnection {
    fn owned(
        connection: Connection,
        namespace: Option<std::sync::Arc<crate::state_home::SqliteNamespaceOwner>>,
    ) -> Self {
        surge_process::owner_panic::install_owner_panic_protection();
        Self {
            connection: Some(connection),
            _namespace: namespace,
        }
    }
    pub(crate) fn open_owned(
        path: &Path,
        flags: OpenFlags,
        namespace: std::sync::Arc<crate::state_home::SqliteNamespaceOwner>,
    ) -> rusqlite::Result<Self> {
        namespace.verify().map_err(native_error)?;
        let connection = Connection::open_with_flags(path, flags)?;
        Ok(Self::owned(connection, Some(namespace)))
    }
    pub(crate) fn in_memory() -> rusqlite::Result<Self> {
        Ok(Self::owned(Connection::open_in_memory()?, None))
    }
    /// Close SQLite completely before releasing the native namespace.
    ///
    /// # Errors
    /// An unsuccessful SQLite close returns this entire owner and the error.
    /// Outstanding statements must settle before retrying. Dropping an owner
    /// whose close still fails terminates the process to preserve ownership.
    pub fn close(mut self) -> Result<(), (Box<Self>, rusqlite::Error)> {
        let Some(connection) = self.connection.take() else {
            std::process::abort()
        };
        let result = surge_process::owner_panic::abort_on_owner_panic(|| {
            connection
                .close()
                .map_err(|(connection, error)| (Box::new(connection), error))
        });
        match result {
            Ok(()) => Ok(()),
            Err((connection, error)) => {
                self.connection = Some(*connection);
                Err((Box::new(self), error))
            },
        }
    }
}
#[cfg(windows)]
impl Drop for RetainedConnection {
    fn drop(&mut self) {
        // The namespace stays in the outer owner throughout close, logging and
        // panic containment. A failed close cannot release it and return.
        surge_process::owner_panic::abort_on_owner_panic(|| {
            let Some(connection) = self.connection.take() else {
                return;
            };
            if let Err((connection, error)) = connection.close() {
                std::mem::forget(connection);
                tracing::error!(code = ?error.sqlite_error_code(), "owned SQLite close failed; terminating protected owner");
                std::process::abort();
            }
        });
    }
}

/// Pool manager holding both its own namespace and one owner per connection.
#[cfg(windows)]
pub struct OwnedSqliteConnectionManager {
    inner: r2d2_sqlite::SqliteConnectionManager,
    namespace: std::sync::Arc<crate::state_home::SqliteNamespaceOwner>,
    init: Option<Box<ConnectionInit>>,
}
#[cfg(windows)]
type ConnectionInit = dyn Fn(&mut Connection) -> rusqlite::Result<()> + Send + Sync;
#[cfg(windows)]
impl OwnedSqliteConnectionManager {
    pub(crate) fn file(
        path: &Path,
        namespace: std::sync::Arc<crate::state_home::SqliteNamespaceOwner>,
    ) -> Self {
        Self {
            inner: r2d2_sqlite::SqliteConnectionManager::file(path),
            namespace,
            init: None,
        }
    }
    pub(crate) fn with_init(
        mut self,
        init: impl Fn(&mut Connection) -> rusqlite::Result<()> + Send + Sync + 'static,
    ) -> Self {
        self.init = Some(Box::new(init));
        self
    }
}
#[cfg(windows)]
impl std::fmt::Debug for OwnedSqliteConnectionManager {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OwnedSqliteConnectionManager")
            .finish_non_exhaustive()
    }
}
#[cfg(windows)]
impl r2d2::ManageConnection for OwnedSqliteConnectionManager {
    type Connection = RetainedConnection;
    type Error = rusqlite::Error;
    fn connect(&self) -> rusqlite::Result<RetainedConnection> {
        self.namespace.verify().map_err(native_error)?;
        let raw = self.inner.connect()?;
        // No callback/SQL runs until checked close and native ownership are bound.
        let mut connection = RetainedConnection::owned(raw, Some(self.namespace.clone()));
        if let Some(init) = &self.init {
            init(connection.connection_mut())?;
        }
        Ok(connection)
    }
    fn is_valid(&self, connection: &mut RetainedConnection) -> rusqlite::Result<()> {
        self.namespace.verify().map_err(native_error)?;
        self.inner.is_valid(connection.connection_mut())
    }
    fn has_broken(&self, connection: &mut RetainedConnection) -> bool {
        self.inner.has_broken(connection.connection_mut())
    }
}

#[cfg(windows)]
pub(crate) type ManagedConnection = RetainedConnection;
#[cfg(not(windows))]
pub(crate) type ManagedConnection = Connection;

pub(crate) fn raw_mut(connection: &mut ManagedConnection) -> &mut Connection {
    #[cfg(windows)]
    {
        connection.connection_mut()
    }
    #[cfg(not(windows))]
    {
        connection
    }
}

#[cfg(all(test, windows))]
#[path = "connection/windows_tests.rs"]
mod windows_tests;
