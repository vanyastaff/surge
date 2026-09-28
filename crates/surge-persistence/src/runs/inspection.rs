//! Non-mutating startup evidence for an operation-owned run.
//!
//! Registry and event data have separate snapshot boundaries. The caller must
//! serialize inspection with its own starts/resumes; this is not a cross-DB lock.

use std::{path::Path, sync::Arc};

use rusqlite::{Connection, OpenFlags};
use surge_core::{RunId, VersionedEventPayload, migrate_payload};

use super::{ReadEvent, RunSummary, Storage, StorageError, seq::EventSeq};

/// Facts found without creating storage or repairing registry status.
#[derive(Debug)]
pub struct RunInspection {
    /// Registry evidence, including its possibly stale status.
    pub registry: Option<RunSummary>,
    /// Even an empty leftover directory is evidence of an attempted start.
    pub run_directory_present: bool,
    /// Event database evidence, independent of registry presence.
    pub database: RunDatabaseInspection,
}

/// Missing database differs from an existing database with no events.
#[derive(Debug)]
pub enum RunDatabaseInspection {
    /// No database existed at inspection.
    Absent,
    /// All decoded events from one SQLite read transaction.
    Present {
        /// Decoded rows in sequence order from one read transaction.
        events: Vec<ReadEvent>,
    },
}

impl Storage {
    /// Inspect the immutable bootstrap base for an exact owned run, without writes.
    /// Missing journals return `None`; invalid or unsupported records remain errors.
    pub async fn inspect_existing_run_capture(
        home: std::path::PathBuf,
        run_id: RunId,
    ) -> Result<Option<surge_core::bootstrap_operation::BootstrapCapture>, StorageError> {
        tokio::task::spawn_blocking(move || {
            let path = home.join("db/registry.sqlite");
            if !exists_as(&path, false)? {
                return Ok(None);
            }
            let mut connection =
                Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
            let transaction = connection.transaction()?;
            Ok(super::bootstrap_operations::read_capture_for_run(
                &transaction,
                run_id,
            )?)
        })
        .await
        .map_err(|error| StorageError::Pool(error.to_string()))?
    }

    /// Read one registry row without creating storage, migrations, or status repair.
    pub async fn inspect_existing_run_summary(
        home: std::path::PathBuf,
        run_id: RunId,
    ) -> Result<Option<RunSummary>, StorageError> {
        tokio::task::spawn_blocking(move || {
            let path = home.join("db/registry.sqlite");
            if !exists_as(&path, false)? {
                return Ok(None);
            }
            let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
            super::registry::get_run_connection(&connection, &run_id)
        })
        .await
        .map_err(|error| StorageError::Pool(error.to_string()))?
    }

    /// Read recent registry rows without creating storage, migrating, or repairing status.
    pub async fn inspect_existing_run_summaries(
        home: std::path::PathBuf,
        limit: std::num::NonZeroU32,
    ) -> Result<Vec<RunSummary>, StorageError> {
        tokio::task::spawn_blocking(move || {
            let path = home.join("db/registry.sqlite");
            if !exists_as(&path, false)? {
                return Ok(Vec::new());
            }
            let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
            super::registry::list_runs_connection(
                &connection,
                &super::registry::RunFilter {
                    limit: Some(limit.get() as usize),
                    ..Default::default()
                },
            )
        })
        .await
        .map_err(|error| StorageError::Pool(error.to_string()))?
    }

    /// Read an existing run history without opening or migrating the registry.
    pub async fn inspect_existing_run_events(
        root: std::path::PathBuf,
        run_id: RunId,
    ) -> Result<Vec<ReadEvent>, StorageError> {
        let path = root.join(run_id.to_string()).join("events.sqlite");
        tokio::task::spawn_blocking(move || {
            if !exists_as(&path, false)? {
                return Err(StorageError::MigrationFailed(
                    "event database is absent".into(),
                ));
            }
            read_events(&path)
        })
        .await
        .map_err(|error| StorageError::Pool(error.to_string()))?
    }

    /// Read at most `limit` events strictly after a durable sequence watermark.
    /// Opens an existing database read-only; absence, corruption and schema errors fail.
    pub async fn inspect_events_after(
        self: &Arc<Self>,
        run_id: RunId,
        after: EventSeq,
        limit: std::num::NonZeroU32,
    ) -> Result<Vec<ReadEvent>, StorageError> {
        let path = self.events_db_path(&run_id);
        tokio::task::spawn_blocking(move || {
            if !exists_as(&path, false)? {
                return Err(StorageError::MigrationFailed(
                    "event database disappeared".into(),
                ));
            }
            let after = i64::try_from(after.0).map_err(|_| {
                StorageError::MigrationFailed("event watermark exceeds SQLite range".into())
            })?;
            read_event_range(&path, Some(after), i64::from(limit.get()))
        })
        .await
        .map_err(|error| StorageError::Pool(error.to_string()))?
    }

    /// Inspect a reserved run without creating/migrating its database.
    ///
    /// Filesystem, SQLite, schema and decoding errors are propagated. Existing
    /// WAL content is visible; this deliberately does not use SQLite immutable mode.
    pub async fn inspect_run(
        self: &Arc<Self>,
        run_id: RunId,
    ) -> Result<RunInspection, StorageError> {
        let storage = Arc::clone(self);
        tokio::task::spawn_blocking(move || {
            let registry = super::registry::get_run(&storage.registry_pool, &run_id)?;
            let run_directory_present = exists_as(&storage.run_dir(&run_id), true)?;
            let path = storage.events_db_path(&run_id);
            let database = if exists_as(&path, false)? {
                RunDatabaseInspection::Present {
                    events: read_events(&path)?,
                }
            } else {
                RunDatabaseInspection::Absent
            };
            Ok(RunInspection {
                registry,
                run_directory_present,
                database,
            })
        })
        .await
        .map_err(|error| StorageError::Pool(error.to_string()))?
    }
}

fn exists_as(path: &Path, directory: bool) -> Result<bool, StorageError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if (directory && metadata.is_dir()) || (!directory && metadata.is_file()) => {
            Ok(true)
        },
        Ok(_) => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "inspection requires a real directory and regular database, not a symlink",
        )
        .into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn read_events(path: &Path) -> Result<Vec<ReadEvent>, StorageError> {
    read_event_range(path, None, i64::MAX)
}

fn read_event_range(
    path: &Path,
    after: Option<i64>,
    limit: i64,
) -> Result<Vec<ReadEvent>, StorageError> {
    let mut connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let transaction = connection.transaction()?;
    let query = if after.is_some() {
        "SELECT seq, timestamp, kind, payload, schema_version FROM events WHERE seq > ?1 ORDER BY seq LIMIT ?2"
    } else {
        "SELECT seq, timestamp, kind, payload, schema_version FROM events WHERE ?1 IS NULL ORDER BY seq LIMIT ?2"
    };
    let mut statement = transaction.prepare(query)?;
    let rows = statement.query_map(rusqlite::params![after, limit], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Vec<u8>>(3)?,
            row.get::<_, u32>(4)?,
        ))
    })?;
    let mut events = Vec::new();
    for row in rows {
        let (seq, timestamp_ms, kind, bytes, schema_version) = row?;
        let seq = u64::try_from(seq)
            .map_err(|_| StorageError::MigrationFailed("negative event sequence".into()))?;
        if seq == 0 {
            return Err(StorageError::MigrationFailed("zero event sequence".into()));
        }
        let payload = migrate_payload(schema_version, &bytes)
            .map_err(|error| StorageError::MigrationFailed(format!("seq={seq}: {error}")))?;
        events.push(ReadEvent {
            seq: EventSeq(seq),
            timestamp_ms,
            kind,
            payload: VersionedEventPayload {
                schema_version,
                payload,
            },
        });
    }
    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn exact_summary_inspection_preserves_custom_path_and_handles_absence() {
        let root = tempfile::tempdir().unwrap();
        let missing = root.path().join("absent");
        let run = RunId::new();
        assert!(
            Storage::inspect_existing_run_summary(missing.clone(), run)
                .await
                .unwrap()
                .is_none()
        );
        assert!(!missing.exists());
        let storage = Storage::open(root.path()).await.unwrap();
        let writer = storage
            .create_run(run, "/custom/output with spaces", None)
            .await
            .unwrap();
        writer.close().await.unwrap();
        let row = Storage::inspect_existing_run_summary(root.path().into(), run)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.id, run);
        assert_eq!(row.project_path, Path::new("/custom/output with spaces"));
        assert_eq!(row.status, surge_core::RunStatus::Bootstrapping);
        assert!(
            Storage::inspect_existing_run_summary(root.path().into(), RunId::new())
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn exact_summary_inspection_reports_invalid_database() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("db")).unwrap();
        let path = root.path().join("db/registry.sqlite");
        std::fs::write(&path, "broken database").unwrap();
        assert!(
            Storage::inspect_existing_run_summary(root.path().into(), RunId::new())
                .await
                .is_err()
        );
        assert_eq!(std::fs::read_to_string(path).unwrap(), "broken database");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn summary_inspection_does_not_create_missing_home() {
        let root = tempfile::tempdir().unwrap();
        let missing = root.path().join("absent");
        let rows =
            Storage::inspect_existing_run_summaries(missing.clone(), std::num::NonZeroU32::MIN)
                .await
                .unwrap();
        assert!(rows.is_empty());
        assert!(!missing.exists());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn summary_inspection_reads_existing_rows_with_limit() {
        let root = tempfile::tempdir().unwrap();
        let storage = Storage::open(root.path()).await.unwrap();
        let run = RunId::new();
        let writer = storage.create_run(run, "/worktree", None).await.unwrap();
        writer
            .append_event(VersionedEventPayload::new(
                surge_core::EventPayload::RunAborted {
                    reason: "test".into(),
                },
            ))
            .await
            .unwrap();
        writer.close().await.unwrap();
        let rows =
            Storage::inspect_existing_run_summaries(root.path().into(), std::num::NonZeroU32::MIN)
                .await
                .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, run);
        // Inspection returns raw registry evidence; it must not repair it from events.
        assert_eq!(rows[0].status, surge_core::RunStatus::Bootstrapping);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn full_snapshot_rejects_nonpositive_rows_before_valid_history() {
        for invalid in [0, -1] {
            let root = tempfile::tempdir().unwrap();
            let storage = Storage::open(root.path()).await.unwrap();
            let id = RunId::new();
            let writer = storage.create_run(id, "/worktree", None).await.unwrap();
            for _ in 0..3 {
                writer
                    .append_event(VersionedEventPayload::new(
                        surge_core::EventPayload::RunAborted {
                            reason: "fixture".into(),
                        },
                    ))
                    .await
                    .unwrap();
            }
            let connection = Connection::open(storage.events_db_path(&id)).unwrap();
            // Simulate corruption outside the normal append-only writer contract.
            connection
                .execute_batch("DROP TRIGGER trg_events_no_update")
                .unwrap();
            connection
                .execute("UPDATE events SET seq = ? WHERE seq = 3", [invalid])
                .unwrap();
            assert!(
                storage.inspect_run(id).await.is_err(),
                "full snapshot hid sequence {invalid}"
            );
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn incremental_snapshot_limits_rows_and_does_not_decode_old_payloads() {
        let root = tempfile::tempdir().unwrap();
        let storage = Storage::open(root.path()).await.unwrap();
        let id = RunId::new();
        let writer = storage.create_run(id, "/worktree", None).await.unwrap();
        for reason in ["one", "two", "three"] {
            writer
                .append_event(VersionedEventPayload::new(
                    surge_core::EventPayload::RunAborted {
                        reason: reason.into(),
                    },
                ))
                .await
                .unwrap();
        }
        let connection = Connection::open(storage.events_db_path(&id)).unwrap();
        // Simulate old corrupted bytes; incremental queries must not decode them.
        connection
            .execute_batch("DROP TRIGGER trg_events_no_update")
            .unwrap();
        connection
            .execute("UPDATE events SET payload = x'00' WHERE seq = 1", [])
            .unwrap();
        let limit = std::num::NonZeroU32::new(1).unwrap();
        let events = storage
            .inspect_events_after(id, EventSeq(1), limit)
            .await
            .unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].seq, EventSeq(2));
        assert!(storage.inspect_run(id).await.is_err());
        assert!(
            storage
                .inspect_events_after(id, EventSeq::ZERO, limit)
                .await
                .is_err()
        );
        let next = storage
            .inspect_events_after(id, EventSeq(2), limit)
            .await
            .unwrap();
        assert_eq!(next[0].seq, EventSeq(3));
        assert!(
            storage
                .inspect_events_after(id, EventSeq(3), limit)
                .await
                .unwrap()
                .is_empty()
        );
        let missing = RunId::new();
        assert!(
            storage
                .inspect_events_after(missing, EventSeq::ZERO, limit)
                .await
                .is_err()
        );
        assert!(!storage.run_dir(&missing).exists());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn database_without_registry_is_present_not_absent() {
        let root = tempfile::tempdir().unwrap();
        let storage = Storage::open(root.path()).await.unwrap();
        let id = RunId::new();
        std::fs::create_dir_all(storage.run_dir(&id)).unwrap();
        let conn = Connection::open(storage.events_db_path(&id)).unwrap();
        conn.execute_batch("CREATE TABLE events(seq INTEGER, timestamp INTEGER, kind TEXT, payload BLOB, schema_version INTEGER)").unwrap();
        drop(conn);
        let inspected = storage.inspect_run(id).await.unwrap();
        assert!(inspected.registry.is_none());
        assert!(
            matches!(inspected.database, RunDatabaseInspection::Present { events } if events.is_empty())
        );
        assert!(inspected.run_directory_present);
    }
    #[tokio::test(flavor = "multi_thread")]
    async fn absent_and_empty_database_are_not_created_or_migrated() {
        let root = tempfile::tempdir().unwrap();
        let storage = Storage::open(root.path()).await.unwrap();
        let id = RunId::new();
        let inspected = storage.inspect_run(id).await.unwrap();
        assert!(!inspected.run_directory_present);
        assert!(matches!(inspected.database, RunDatabaseInspection::Absent));
        assert!(!storage.run_dir(&id).exists());
        std::fs::create_dir_all(storage.run_dir(&id)).unwrap();
        let path = storage.events_db_path(&id);
        std::fs::write(&path, []).unwrap();
        assert!(storage.inspect_run(id).await.is_err());
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 0);
        assert_eq!(std::fs::read_dir(storage.run_dir(&id)).unwrap().count(), 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn wal_events_are_read_without_repairing_registry_status() {
        let root = tempfile::tempdir().unwrap();
        let storage = Storage::open(root.path()).await.unwrap();
        let id = RunId::new();
        let writer = storage.create_run(id, "/saved/custom", None).await.unwrap();
        writer
            .append_event(VersionedEventPayload::new(
                surge_core::EventPayload::RunAborted {
                    reason: "persisted cancellation".into(),
                },
            ))
            .await
            .unwrap();
        storage
            .acquire_registry_conn()
            .unwrap()
            .execute(
                "UPDATE runs SET daemon_pid = 2147483647 WHERE id = ?",
                [id.to_string()],
            )
            .unwrap();
        let before = super::super::registry::get_run(&storage.registry_pool, &id)
            .unwrap()
            .unwrap();
        let inspected = storage.inspect_run(id).await.unwrap();
        let RunDatabaseInspection::Present { events } = inspected.database else {
            panic!("missing DB")
        };
        assert_eq!(events.len(), 1);
        assert!(
            matches!(&events[0].payload.payload, surge_core::EventPayload::RunAborted { reason } if reason == "persisted cancellation")
        );
        let after = super::super::registry::get_run(&storage.registry_pool, &id)
            .unwrap()
            .unwrap();
        assert_eq!(before.status, after.status);
        assert_eq!(before.ended_at_ms, after.ended_at_ms);
        assert_eq!(before.daemon_pid, after.daemon_pid);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn malformed_event_is_an_error_not_an_empty_log() {
        let root = tempfile::tempdir().unwrap();
        let storage = Storage::open(root.path()).await.unwrap();
        let id = RunId::new();
        std::fs::create_dir_all(storage.run_dir(&id)).unwrap();
        let conn = Connection::open(storage.events_db_path(&id)).unwrap();
        conn.execute_batch("CREATE TABLE events(seq INTEGER, timestamp INTEGER, kind TEXT, payload BLOB, schema_version INTEGER); INSERT INTO events VALUES(1,0,'RunStarted',x'00',1)").unwrap();
        assert!(matches!(
            storage.inspect_run(id).await,
            Err(StorageError::MigrationFailed(_))
        ));
        assert_eq!(
            conn.query_row("SELECT count(*) FROM events", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread")]
    async fn symlink_database_is_rejected_without_touching_target() {
        let root = tempfile::tempdir().unwrap();
        let storage = Storage::open(root.path()).await.unwrap();
        let id = RunId::new();
        std::fs::create_dir_all(storage.run_dir(&id)).unwrap();
        let foreign = root.path().join("foreign");
        std::fs::write(&foreign, b"untouched").unwrap();
        std::os::unix::fs::symlink(&foreign, storage.events_db_path(&id)).unwrap();
        assert!(matches!(
            storage.inspect_run(id).await,
            Err(StorageError::Io(_))
        ));
        assert_eq!(std::fs::read(&foreign).unwrap(), b"untouched");
    }
    #[test]
    fn readonly_open_cannot_recreate_a_database_that_disappeared() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("events.sqlite");
        assert!(read_events(&path).is_err());
        assert!(!path.exists());
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    }
}
