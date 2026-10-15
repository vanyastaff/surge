//! Read-only keyset observations; their upper bound is not a cross-source snapshot.
use super::{RunSummary, row_to_summary};
use crate::runs::StorageError;
use rusqlite::OptionalExtension as _;
#[cfg(unix)]
use std::os::unix::fs::MetadataExt as _;
use std::{
    num::NonZeroU32,
    path::{Path, PathBuf},
};

/// Opaque continuation of one registry observation, bound to its database identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunSummaryCursor {
    identity: RegistryIdentity,
    upper: SummaryKey,
    after: SummaryKey,
}
/// A bounded newest-first registry page. An empty present page differs from absence.
#[derive(Debug)]
pub struct RunSummaryPage {
    /// Raw registry facts, without lifecycle repair or project guesses.
    pub entries: Vec<RunSummary>,
    /// Continue through global pages before deciding project membership.
    pub next_cursor: Option<RunSummaryCursor>,
}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct SummaryKey(i64, String);
#[derive(Debug, Clone, PartialEq, Eq)]
struct RegistryIdentity {
    path: PathBuf,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(not(unix))]
    created: std::time::SystemTime,
}
impl RegistryIdentity {
    fn read(path: &Path) -> Result<Self, StorageError> {
        let metadata = std::fs::symlink_metadata(path)?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(StorageError::OperationRejected(
                "inspection requires a regular registry".into(),
            ));
        }
        Ok(Self {
            path: path.canonicalize()?,
            #[cfg(unix)]
            device: metadata.dev(),
            #[cfg(unix)]
            inode: metadata.ino(),
            #[cfg(not(unix))]
            created: metadata.created()?,
        })
    }
}
/// Read at most 256 rows. Newly inserted keys above the first page are deferred.
/// Existing row contents may change between pages; this is not a frozen database.
pub(in crate::runs) fn read_page(
    path: &Path,
    limit: NonZeroU32,
    cursor: Option<RunSummaryCursor>,
) -> Result<RunSummaryPage, StorageError> {
    if limit.get() > 256 {
        return Err(StorageError::OperationRejected(
            "inspection page limit exceeds 256".into(),
        ));
    }
    let identity = RegistryIdentity::read(path)?;
    if cursor
        .as_ref()
        .is_some_and(|cursor| cursor.identity != identity || cursor.after > cursor.upper)
    {
        return Err(StorageError::OperationRejected(
            "registry continuation belongs to another observation".into(),
        ));
    }
    let mut connection = crate::runs::connection::RetainedConnection::read_only(path)?;
    let transaction = connection.transaction()?;
    let upper = if let Some(cursor) = &cursor {
        Some(cursor.upper.clone())
    } else {
        transaction
            .query_row(
                "SELECT started_at,id FROM runs ORDER BY started_at DESC,id DESC LIMIT 1",
                [],
                |row| Ok(SummaryKey(row.get(0)?, row.get(1)?)),
            )
            .optional()?
    };
    let Some(upper) = upper else {
        return Ok(RunSummaryPage {
            entries: Vec::new(),
            next_cursor: None,
        });
    };
    let after = cursor.as_ref().map(|cursor| &cursor.after);
    let mut statement = transaction.prepare("SELECT id,project_path,pipeline_template,status,started_at,ended_at,daemon_pid,wake_at FROM runs WHERE (started_at,id) <= (?1,?2) AND (?3 IS NULL OR (started_at,id) < (?3,?4)) ORDER BY started_at DESC,id DESC LIMIT ?5")?;
    let mut entries = statement
        .query_map(
            rusqlite::params![
                upper.0,
                upper.1,
                after.map(|key| key.0),
                after.map(|key| &key.1),
                i64::from(limit.get()) + 1
            ],
            row_to_summary,
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let more = entries.len() > limit.get() as usize;
    entries.truncate(limit.get() as usize);
    let next_cursor = entries
        .last()
        .filter(|_| more)
        .map(|last| RunSummaryCursor {
            identity,
            upper,
            after: SummaryKey(last.started_at_ms, last.id.to_string()),
        });
    Ok(RunSummaryPage {
        entries,
        next_cursor,
    })
}
