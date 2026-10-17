//! Retained exact-operation inspection; no execution or mutation authority.
use crate::runs::{
    StorageError,
    bootstrap_operations::{self, BootstrapOperationRecord},
    connection::RetainedConnection,
};
use std::path::{Path, PathBuf};
use surge_core::RunId;

/// A checked operation and its original read-only registry observation.
/// Journals are separate observations. Call `recheck` after reading them and
/// discard assembled facts if it returns false. This is not a cross-database lock.
#[derive(Debug)]
pub struct BootstrapOperationObservation {
    connection: RetainedConnection,
    identities: Vec<Identity>,
    record: BootstrapOperationRecord,
}
#[derive(Debug)]
struct Identity {
    path: PathBuf,
    #[cfg(unix)]
    held: std::fs::File,
    #[cfg(not(unix))]
    created: std::time::SystemTime,
}
impl Identity {
    fn capture(path: &Path) -> Result<Self, StorageError> {
        let metadata = std::fs::symlink_metadata(path)?;
        if metadata.file_type().is_symlink() || !(metadata.is_file() || metadata.is_dir()) {
            return Err(StorageError::OperationRejected(
                "registry namespace is not regular".into(),
            ));
        }
        let identity = Self {
            path: path.into(),
            #[cfg(unix)]
            held: std::fs::File::open(path)?,
            #[cfg(not(unix))]
            created: metadata.created()?,
        };
        if !identity.current()? {
            return Err(StorageError::OperationRejected(
                "registry changed during opening".into(),
            ));
        }
        Ok(identity)
    }
    fn current(&self) -> Result<bool, StorageError> {
        let metadata = std::fs::symlink_metadata(&self.path)?;
        if metadata.file_type().is_symlink() {
            return Ok(false);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt as _;
            let held = self.held.metadata()?;
            Ok(metadata.dev() == held.dev() && metadata.ino() == held.ino())
        }
        #[cfg(not(unix))]
        {
            Ok(metadata.created()? == self.created)
        }
    }
}
pub(super) fn observe(
    home: PathBuf,
    operation: RunId,
) -> Result<Option<BootstrapOperationObservation>, StorageError> {
    let path = home.join("db/registry.sqlite");
    if !super::exists_as(&path, false)? {
        return Ok(None);
    }
    let identities = [&home, &home.join("db"), &path]
        .into_iter()
        .map(|path| Identity::capture(path))
        .collect::<Result<Vec<_>, _>>()?;
    let connection = RetainedConnection::read_only(&path)?;
    let Some(record) = bootstrap_operations::read_inspection(&connection, operation)? else {
        return Ok(None);
    };
    let observed = BootstrapOperationObservation {
        connection,
        identities,
        record,
    };
    if !observed.current()? {
        return Err(StorageError::OperationRejected(
            "registry replaced during inspection".into(),
        ));
    }
    Ok(Some(observed))
}
impl BootstrapOperationObservation {
    /// Complete decoded record, including typed unsupported payloads.
    #[must_use]
    pub fn record(&self) -> &BootstrapOperationRecord {
        &self.record
    }
    fn current(&self) -> Result<bool, StorageError> {
        // On Windows this retained native namespace owner checks actual object
        // identities. Creation times below are only an additional metadata check.
        #[cfg(windows)]
        self.connection.verify_namespace()?;
        for identity in &self.identities {
            if !identity.current()? {
                return Ok(false);
            }
        }
        Ok(true)
    }
    /// Recheck the exact original connection, namespace identities and complete record.
    /// Returns false on observed identity/record inequality, including a removed record.
    /// Missing/unreadable namespaces, native identity refusals, invalid/oversized records
    /// (the same aggregate 4 MiB bound) and SQLite failures remain errors. A true result
    /// confirms this registry observation, not a cross-database snapshot. Consumes and settles the original retained
    /// read-only connection before returning; it never reopens a same-path replacement.
    #[must_use = "discard assembled journal facts when the original observation changed"]
    pub async fn recheck(self) -> Result<bool, StorageError> {
        tokio::task::spawn_blocking(move || {
            if !self.current()? {
                return Ok(false);
            }
            let record = bootstrap_operations::read_inspection(
                &self.connection,
                self.record.status.operation_id,
            )?;
            Ok(record.as_ref() == Some(&self.record) && self.current()?)
        })
        .await
        .map_err(|error| StorageError::Pool(error.to_string()))?
    }
}
