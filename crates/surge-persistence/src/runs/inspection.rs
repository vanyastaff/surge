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

/// Complete trusted fold with only the immutable startup facts retained as rows.
#[derive(Debug)]
pub struct FoldedRunEvidence {
    /// Graph, required artifacts and binding from the prefix before execution/lifecycle results.
    pub startup: Vec<ReadEvent>,
    /// Final state after every event from one read snapshot.
    pub state: surge_core::RunState,
    /// Total validated event count; no arbitrary total-history ceiling.
    pub event_count: u64,
}
/// Nonmutating evidence whose SQL reads are bounded pages.
#[derive(Debug)]
pub struct FoldedRunInspection {
    /// Independent registry observation.
    pub registry: Option<RunSummary>,
    /// None means no database; Some may have zero events.
    pub database: Option<FoldedRunEvidence>,
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

    /// Inspect an exact accepted gate answer without reopening a writer.
    /// The complete journal is validated in bounded pages in the same read snapshot.
    /// A conflicting request occurrence or response remains an error.
    pub async fn inspect_gate_answer(
        self: &Arc<Self>,
        run_id: RunId,
        node: surge_core::NodeKey,
        request: surge_core::id::GateRequestId,
    ) -> Result<Option<serde_json::Value>, StorageError> {
        let path = self.events_db_path(&run_id);
        tokio::task::spawn_blocking(move || {
            if !exists_as(&path, false)? {
                return Ok(None);
            }
            let (_, receipt) = read_folded_with_gate_receipt(&path, run_id, Some((node, request)))?;
            Ok(receipt.and_then(|record| record.response))
        })
        .await
        .map_err(|error| StorageError::Pool(error.to_string()))?
    }

    /// Validate and fold an existing run in bounded SQL pages without retaining history rows.
    /// One read transaction sees a consistent prefix. Missing, corrupt or gapped history fails.
    pub async fn inspect_folded_run(
        self: &Arc<Self>,
        run_id: RunId,
    ) -> Result<FoldedRunInspection, StorageError> {
        let storage = Arc::clone(self);
        tokio::task::spawn_blocking(move || {
            let registry = super::registry::get_run(&storage.registry_pool, &run_id)?;
            let path = storage.events_db_path(&run_id);
            let database = if exists_as(&path, false)? {
                Some(read_folded_events(&path, run_id)?)
            } else {
                None
            };
            Ok(FoldedRunInspection { registry, database })
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

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn folded_history_rejects_orphan_stage_route() {
        use surge_core::run_event::{EventPayload as E, VersionedEventPayload as V};
        let home = tempfile::tempdir().unwrap();
        let storage = Storage::open(home.path()).await.unwrap();
        let run = RunId::new();
        let writer = storage.create_run(run, home.path(), None).await.unwrap();
        writer
            .append_events(vec![V::new(E::RunStarted {
                pipeline_template: None,
                project_path: home.path().into(),
                initial_prompt: "fixture".into(),
                config: surge_core::run_event::RunConfig {
                    bootstrap_edit_loop_cap: None,
                    sandbox_default: surge_core::sandbox::SandboxMode::WorkspaceWrite,
                    approval_default: surge_core::approvals::ApprovalPolicy::OnRequest,
                    auto_pr: false,
                    mcp_servers: vec![],
                    budget: surge_core::budget::BudgetGuard::default(),
                },
            })])
            .await
            .unwrap();
        writer.close().await.unwrap();
        let conn = Connection::open(storage.events_db_path(&run)).unwrap();
        let payload = V::new(E::StageRouteCommitted {
            invocation: surge_core::id::StageInvocationId::new(),
            outcome_commit_seq: 1,
        });
        conn.execute(
            "INSERT INTO events(seq,timestamp,kind,payload,schema_version) VALUES(2,0,?1,?2,?3)",
            rusqlite::params![
                "StageRouteCommitted",
                serde_json::to_vec(&payload).unwrap(),
                payload.schema_version
            ],
        )
        .unwrap();
        let error = read_folded_events(&storage.events_db_path(&run), run).err();
        assert!(
            error.is_some_and(|error| error
                .to_string()
                .contains("stage route has no matching accepted commit")),
            "orphan route must not become trusted replay state"
        );
    }

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

pub(crate) fn read_folded_events(
    path: &Path,
    run_id: RunId,
) -> Result<FoldedRunEvidence, StorageError> {
    read_folded_with_gate_receipt(path, run_id, None).map(|(evidence, _)| evidence)
}

fn read_folded_with_gate_receipt(
    path: &Path,
    run_id: RunId,
    target: Option<(surge_core::NodeKey, surge_core::id::GateRequestId)>,
) -> Result<
    (
        FoldedRunEvidence,
        Option<surge_core::run_state::RecoveredGateDecision>,
    ),
    StorageError,
> {
    use surge_core::EventPayload as E;
    let mut receipt = None;
    let mut connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let transaction = connection.transaction()?;
    let mut state = surge_core::RunState::NotStarted;
    let mut startup = Vec::new();
    let mut sequence = 0;
    let mut bindings = 0;
    let mut startup_open = true;
    let mut definitive_terminal_seen = false;
    let mut recent = std::collections::VecDeque::new();
    let mut opened_connections = std::collections::HashMap::new();
    let mut seen_connections = std::collections::HashSet::new();
    let mut authorities = std::collections::HashMap::new();
    let mut committed_invocations = std::collections::HashMap::new();
    let mut routed_invocations = std::collections::HashSet::new();
    let mut active_node = None;
    let mut gate_commits = std::collections::HashMap::new();
    let mut routed_gates = std::collections::HashSet::new();
    let mut bootstrap_edit_cap = None;
    let mut route_evidence = super::inspection_route::RouteEvidence::default();
    loop {
        let mut statement=transaction.prepare("SELECT seq,timestamp,payload,schema_version FROM events WHERE seq>? ORDER BY seq LIMIT 256")?;
        let rows = statement
            .query_map([sequence], |row| {
                Ok((
                    row.get::<_, u64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                    row.get::<_, u32>(3)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        if rows.is_empty() {
            break;
        }
        for (seq, timestamp, bytes, version) in rows {
            let payload = migrate_payload(version, &bytes)
                .map_err(|error| StorageError::MigrationFailed(error.to_string()))?;
            if seq != sequence + 1 || (sequence == 0 && !matches!(payload, E::RunStarted { .. })) {
                return Err(StorageError::MigrationFailed(
                    "owned history has no trusted contiguous origin".into(),
                ));
            }
            let time = chrono::DateTime::from_timestamp_millis(timestamp).ok_or_else(|| {
                StorageError::MigrationFailed("owned event timestamp is invalid".into())
            })?;
            if let E::RunStarted { config, .. } = &payload {
                bootstrap_edit_cap = config.bootstrap_edit_loop_cap;
            }
            if let E::StageEntered { node, .. } = &payload {
                active_node = Some(node.clone());
                opened_connections.clear();
                authorities.clear();
            }
            if let E::SessionEstablishmentRequested {
                node,
                invocation,
                authority: Some(authority),
                ..
            } = &payload
            {
                if active_node.as_ref() != Some(node)
                    || authority.node != *node
                    || authority.run != run_id
                {
                    return Err(StorageError::MigrationFailed(
                        "stage authority has no matching active stage".into(),
                    ));
                }
                authorities.insert(*invocation, authority.clone());
            }
            if let E::SessionOpened {
                node,
                session,
                opened: Some(opened),
                ..
            } = &payload
            {
                if *session != opened.session || !seen_connections.insert(*session) {
                    return Err(StorageError::MigrationFailed(
                        "provider connection metadata mismatch".into(),
                    ));
                }
                opened_connections.insert(*session, (node.clone(), opened.descriptor.invocation()));
            }
            if let E::RunSuspended { fence } = &payload
                && let surge_core::execution_recovery::PendingStagePhase::WaitingHumanGate {
                    node,
                    request,
                    stage_entry_seq,
                    requested_seq,
                } = &fence.pending_stage
            {
                let surge_core::RunState::Pipeline { memory, .. } = &state else {
                    return Err(StorageError::MigrationFailed(
                        "waiting human gate outside a pipeline".into(),
                    ));
                };
                let valid = memory.gate_decisions.get(request).is_some_and(|record| {
                    !record.conflicting
                        && record.purpose == surge_core::run_state::GateDecisionPurpose::HumanGate
                        && record.node == *node
                        && record.stage_entry_seq == *stage_entry_seq
                        && record.requested_seq == *requested_seq
                        && active_node.as_ref() == Some(node)
                        && memory.stage_occurrences.get(node).copied() == Some(*stage_entry_seq)
                });
                if !valid {
                    return Err(StorageError::MigrationFailed(
                        "waiting human gate contradicts its original decision".into(),
                    ));
                }
            }
            if let E::GateStageOutcomeCommitted { commit } = &payload {
                if version < 13 {
                    return Err(StorageError::MigrationFailed(
                        "gate stage commit predates its schema".into(),
                    ));
                }
                let surge_core::RunState::Pipeline { memory, .. } = &state else {
                    return Err(StorageError::MigrationFailed(
                        "gate commit outside a pipeline".into(),
                    ));
                };
                validate_gate_stage_commit(
                    commit,
                    memory,
                    &recent,
                    active_node.as_ref(),
                    bootstrap_edit_cap,
                )?;
                let request = commit.answer().request().request();
                if gate_commits
                    .insert(request, (seq, commit.clone()))
                    .is_some()
                {
                    return Err(StorageError::MigrationFailed(
                        "gate decision effects already committed".into(),
                    ));
                }
            }
            if let E::GateStageRouteCommitted {
                request,
                stage_entry_seq,
                outcome_commit_seq,
            } = &payload
            {
                if version < 13 {
                    return Err(StorageError::MigrationFailed(
                        "gate stage route predates its schema".into(),
                    ));
                }
                let Some((accepted_seq, accepted)) = gate_commits.get(request) else {
                    return Err(StorageError::MigrationFailed(
                        "gate route has no matching accepted commit".into(),
                    ));
                };
                let original = accepted.answer().request();
                if *accepted_seq != *outcome_commit_seq
                    || original.stage_entry_seq() != *stage_entry_seq
                    || active_node.as_ref() != Some(original.node())
                    || accepted.disposition()
                        != surge_core::execution_recovery::gate_commit::GateCommitDisposition::Route
                    || !routed_gates.insert(*request)
                {
                    return Err(StorageError::MigrationFailed(
                        "gate route repeats or mismatches routable acceptance".into(),
                    ));
                }
                let completed = recent.back().map(VersionedEventPayload::payload);
                let edge = recent
                    .iter()
                    .rev()
                    .nth(1)
                    .map(VersionedEventPayload::payload);
                if !matches!(completed, Some(E::StageCompleted { node, outcome }) if node == original.node() && outcome == accepted.answer().outcome())
                    || !matches!(edge, Some(E::EdgeTraversed { from, .. }) if from == original.node())
                {
                    return Err(StorageError::MigrationFailed(
                        "gate route is not adjacent to its exact edge and completion".into(),
                    ));
                }
            }
            if let E::StageOutcomeCommitted { commit } = &payload {
                if active_node.as_ref() != Some(&commit.context().node)
                    || authorities.get(&commit.invocation()) != Some(commit.context())
                {
                    return Err(StorageError::MigrationFailed(
                        "stage commit authenticated authority mismatch".into(),
                    ));
                }
                if committed_invocations
                    .insert(commit.invocation(), (seq, commit.clone()))
                    .is_some()
                {
                    return Err(StorageError::MigrationFailed(
                        "stage invocation outcome was already committed".into(),
                    ));
                }
                validate_stage_commit(run_id, commit, &recent, &opened_connections)?;
            }
            if let E::StageRouteCommitted {
                invocation,
                outcome_commit_seq,
            } = &payload
            {
                let Some((accepted_seq, accepted)) = committed_invocations.get(invocation) else {
                    return Err(StorageError::MigrationFailed(
                        "stage route has no matching accepted commit".into(),
                    ));
                };
                if active_node.as_ref() != Some(&accepted.context().node)
                    || *outcome_commit_seq != *accepted_seq
                    || !routed_invocations.insert(*invocation)
                {
                    return Err(StorageError::MigrationFailed(
                        "stage route repeats or mismatches accepted commit".into(),
                    ));
                }
                let completed = recent
                    .back()
                    .map(surge_core::VersionedEventPayload::payload);
                let edge = recent
                    .iter()
                    .rev()
                    .nth(1)
                    .map(surge_core::VersionedEventPayload::payload);
                if !matches!(completed, Some(E::StageCompleted { node, outcome }) if node == &accepted.context().node && outcome == accepted.outcome())
                    || !matches!(edge, Some(E::EdgeTraversed { from, .. }) if from == &accepted.context().node)
                {
                    return Err(StorageError::MigrationFailed(
                        "stage route lacks its adjacent owning routing batch".into(),
                    ));
                }
            }
            if matches!(
                payload,
                E::RunCompleted { .. } | E::RunFailed { .. } | E::RunAborted { .. }
            ) {
                if definitive_terminal_seen {
                    return Err(StorageError::MigrationFailed(
                        "owned history contains ambiguous definitive terminal events".into(),
                    ));
                }
                definitive_terminal_seen = true;
            }
            startup_open &= matches!(
                &payload,
                E::RunStarted { .. }
                    | E::PipelineMaterialized { .. }
                    | E::ArtifactProduced { .. }
                    | E::WorkItemAttemptBound { .. }
            );
            if matches!(payload, E::WorkItemAttemptBound { .. }) {
                bindings += 1;
                if bindings > 1 {
                    return Err(StorageError::MigrationFailed(
                        "owned history repeats attempt binding".into(),
                    ));
                }
            }
            let keep=startup_open && match &payload {
                E::RunStarted{..}=>sequence==0,
                E::PipelineMaterialized{..}=>!startup.iter().any(|row:&ReadEvent|matches!(row.payload.payload,E::PipelineMaterialized{..})),
                E::ArtifactProduced{name,..} if name=="accepted_requirements" || name=="user_prompt"=>!startup.iter().any(|row:&ReadEvent|matches!(&row.payload.payload,E::ArtifactProduced{name:seen,..} if seen==name)),
                E::WorkItemAttemptBound{..}=>true,
                _=>false,
            };
            if keep {
                startup.push(ReadEvent {
                    seq: EventSeq(seq),
                    timestamp_ms: timestamp,
                    kind: payload.discriminant_str().to_owned(),
                    payload: VersionedEventPayload {
                        schema_version: version,
                        payload: payload.clone(),
                    },
                });
            }
            recent.push_back(VersionedEventPayload {
                schema_version: version,
                payload: payload.clone(),
            });
            if recent.len() > 4096 {
                recent.pop_front();
            }
            if let surge_core::RunState::Pipeline { graph, .. } = &state {
                route_evidence.observe(graph, &payload)?;
            }
            state = surge_core::run_state::apply(
                state,
                &surge_core::RunEvent {
                    run_id,
                    seq,
                    timestamp: time,
                    payload,
                },
            )
            .map_err(|error| StorageError::MigrationFailed(error.to_string()))?;
            if let (Some((node, request)), surge_core::RunState::Pipeline { memory, .. }) =
                (&target, &state)
            {
                if memory.gate_recovery_error.is_some() {
                    return Err(StorageError::MigrationFailed(
                        "gate receipt has contradictory request authority".into(),
                    ));
                }
                if let Some(record) = memory
                    .gate_decisions
                    .get(request)
                    .filter(|record| record.node == *node)
                {
                    if record.conflicting
                        || record.purpose == surge_core::run_state::GateDecisionPurpose::Unbound
                    {
                        return Err(StorageError::MigrationFailed(
                            "gate receipt has no matching authoritative occurrence".into(),
                        ));
                    }
                    if let Some(response) = &record.response {
                        let outcome = response
                            .get("outcome")
                            .and_then(serde_json::Value::as_str)
                            .and_then(|value| surge_core::OutcomeKey::try_from(value).ok());
                        let allowed = match &record.purpose {
                            surge_core::run_state::GateDecisionPurpose::HumanGate => {
                                record.gate_config.as_ref().is_some_and(|config| {
                                    outcome.as_ref().is_some_and(|value| {
                                        config.allow_freetext
                                            || config
                                                .options
                                                .iter()
                                                .any(|option| &option.outcome == value)
                                    })
                                })
                            },
                            surge_core::run_state::GateDecisionPurpose::SkillTrust(_) => matches!(
                                outcome.as_ref().map(surge_core::OutcomeKey::as_str),
                                Some("approve" | "reject")
                            ),
                            surge_core::run_state::GateDecisionPurpose::Unbound => false,
                        };
                        if !allowed {
                            return Err(StorageError::MigrationFailed(
                                "gate receipt response violates the original allowed outcomes"
                                    .into(),
                            ));
                        }
                        receipt = Some(record.clone());
                    }
                }
            }
            sequence = seq;
        }
    }
    Ok((
        FoldedRunEvidence {
            startup,
            state,
            event_count: sequence,
        },
        receipt,
    ))
}

fn validate_gate_stage_commit(
    commit: &surge_core::execution_recovery::gate_commit::GateStageCommit,
    memory: &surge_core::RunMemory,
    recent: &std::collections::VecDeque<VersionedEventPayload>,
    active_node: Option<&surge_core::NodeKey>,
    bootstrap_edit_cap: Option<u32>,
) -> Result<(), StorageError> {
    use surge_core::execution_recovery::gate_commit::{gate_effects_hash, gate_response_hash};
    let error = |message: &str| StorageError::MigrationFailed(message.into());
    let original = commit.answer().request();
    let record = memory
        .gate_decisions
        .get(&original.request())
        .ok_or_else(|| error("gate commit has no original authoritative decision"))?;
    if record.conflicting
        || record.purpose != surge_core::run_state::GateDecisionPurpose::HumanGate
        || record.node != *original.node()
        || active_node != Some(original.node())
        || record.stage_entry_seq != original.stage_entry_seq()
        || memory.stage_occurrences.get(original.node()).copied()
            != Some(original.stage_entry_seq())
        || record.requested_seq != original.requested_seq()
        || record.resolved_seq != Some(commit.answer().resolved_seq())
    {
        return Err(error("gate commit decision occurrence mismatch"));
    }
    let response = record
        .response
        .as_ref()
        .ok_or_else(|| error("gate commit has no accepted answer"))?;
    if gate_response_hash(response).map_err(|failure| error(&failure.to_string()))?
        != *commit.answer().response_hash()
        || response.get("outcome").and_then(serde_json::Value::as_str)
            != Some(commit.answer().outcome().as_str())
    {
        return Err(error("gate commit accepted response mismatch"));
    }
    let config = record
        .gate_config
        .as_ref()
        .ok_or_else(|| error("gate commit has no original graph contract"))?;
    if !config.allow_freetext
        && !config
            .options
            .iter()
            .any(|option| &option.outcome == commit.answer().outcome())
    {
        return Err(error(
            "gate commit response violates original allowed outcomes",
        ));
    }
    let count = commit.effects_count() as usize;
    if count > recent.len() {
        return Err(error("gate commit adjacent effects are missing"));
    }
    let effects: Vec<_> = recent.iter().skip(recent.len() - count).cloned().collect();
    if gate_effects_hash(&effects).map_err(|failure| error(&failure.to_string()))?
        != *commit.effects_hash()
    {
        return Err(error("gate commit adjacent effects digest mismatch"));
    }
    validate_gate_effects(commit, config, response, &effects)?;
    if let surge_core::human_gate_config::HumanGateMode::Bootstrap { stage } = config.mode {
        let cap = bootstrap_edit_cap
            .ok_or_else(|| error("bootstrap gate commit has no immutable edit cap"))?;
        if commit.answer().outcome().as_str() == "edit" {
            let added = u32::from(effects.iter().any(|effect| matches!(effect.payload(), surge_core::EventPayload::BootstrapEditRequested { stage: actual, .. } if *actual == stage)));
            let prior = memory
                .bootstrap_edit_counts
                .get(&stage)
                .copied()
                .unwrap_or(0)
                .checked_sub(added)
                .ok_or_else(|| {
                    error(&format!(
                        "bootstrap gate edit count contradicts its adjacent effects: stage={stage:?}, observed={}, adjacent={added}",
                        memory.bootstrap_edit_counts.get(&stage).copied().unwrap_or(0)
                    ))
                })?;
            let escalated = cap > 0 && prior >= cap;
            if escalated != (commit.disposition() == surge_core::execution_recovery::gate_commit::GateCommitDisposition::BootstrapEscalated) {
                return Err(error("bootstrap gate disposition violates immutable edit cap"));
            }
        }
    }
    Ok(())
}

fn validate_gate_effects(
    commit: &surge_core::execution_recovery::gate_commit::GateStageCommit,
    config: &surge_core::human_gate_config::HumanGateConfig,
    response: &serde_json::Value,
    effects: &[VersionedEventPayload],
) -> Result<(), StorageError> {
    use surge_core::execution_recovery::gate_commit::GateCommitDisposition as D;
    use surge_core::{EventPayload as E, human_gate_config::HumanGateMode};
    let error = |message: &str| StorageError::MigrationFailed(message.into());
    let original = commit.answer().request();
    if !matches!(effects.last().map(VersionedEventPayload::payload), Some(E::OutcomeReported { node, outcome, .. })
        if node == original.node() && outcome == commit.answer().outcome())
    {
        return Err(error("gate commit outcome effect mismatch"));
    }
    match &config.mode {
        HumanGateMode::Generic => {
            if effects.len() != 1 || commit.disposition() != D::Route {
                return Err(error("generic gate commit contains unrelated effects"));
            }
        },
        HumanGateMode::Bootstrap { stage } => {
            let decision = match commit.answer().outcome().as_str() {
                "approve" => surge_core::run_event::BootstrapDecision::Approve,
                "reject" => surge_core::run_event::BootstrapDecision::Reject,
                "edit" => surge_core::run_event::BootstrapDecision::Edit,
                _ => return Err(error("bootstrap gate outcome is invalid")),
            };
            let comment = response
                .get("comment")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned);
            if !matches!(effects.first().map(VersionedEventPayload::payload), Some(E::BootstrapApprovalDecided { stage: actual, decision: actual_decision, comment: actual_comment })
                if actual == stage && actual_decision == &decision && actual_comment == &comment)
            {
                return Err(error("bootstrap gate decision effect mismatch"));
            }
            let valid = match decision {
                surge_core::run_event::BootstrapDecision::Approve => {
                    effects.len() == 2 && commit.disposition() == D::Route
                },
                surge_core::run_event::BootstrapDecision::Reject => {
                    effects.len() == 2 && commit.disposition() == D::BootstrapRejected
                },
                surge_core::run_event::BootstrapDecision::Edit => {
                    effects.len() == 3 && match (commit.disposition(), effects[1].payload()) {
                        (
                            D::Route,
                            E::BootstrapEditRequested {
                                stage: actual,
                                feedback,
                            },
                        ) => actual == stage && feedback == comment.as_deref().unwrap_or_default(),
                        (
                            D::BootstrapEscalated,
                            E::EscalationRequested {
                                stage: Some(actual),
                                cause:
                                    surge_core::run_event::EscalationCause::BootstrapEditLoopExhausted,
                                ..
                            },
                        ) => actual == stage,
                        _ => false,
                    }
                },
            };
            if !valid {
                return Err(error("bootstrap gate required effects mismatch"));
            }
        },
    }
    Ok(())
}

fn validate_stage_commit(
    run: RunId,
    commit: &surge_core::execution_recovery::commit::StageOutcomeCommit,
    recent: &std::collections::VecDeque<VersionedEventPayload>,
    opened: &std::collections::HashMap<
        surge_core::SessionId,
        (surge_core::NodeKey, surge_core::id::StageInvocationId),
    >,
) -> Result<(), StorageError> {
    use surge_core::EventPayload as E;
    let error = |message: &str| StorageError::MigrationFailed(message.into());
    if commit.context().run != run
        || opened.get(&commit.provider_connection())
            != Some(&(commit.context().node.clone(), commit.invocation()))
    {
        return Err(error("stage commit provider invocation identity mismatch"));
    }
    let count = commit.effects_count() as usize;
    if count > recent.len() {
        return Err(error("stage commit adjacent effects are missing"));
    }
    let effects: Vec<_> = recent.iter().skip(recent.len() - count).cloned().collect();
    let encoded = serde_json::to_vec(&effects)
        .map_err(|failure| StorageError::MigrationFailed(failure.to_string()))?;
    if surge_core::ContentHash::compute(&encoded) != *commit.effects_hash() {
        return Err(error("stage commit effects digest mismatch"));
    }
    if !matches!(effects.first().map(VersionedEventPayload::payload), Some(E::OutcomeReported { node, outcome, .. })
        if node == &commit.context().node && outcome == commit.outcome())
    {
        return Err(error("stage commit accepted outcome mismatch"));
    }
    if effects.iter().skip(1).any(|event| !matches!(event.payload(),
        E::TaskStatusChanged { authority_node, .. } if authority_node == &commit.context().node)
        && !matches!(event.payload(), E::TaskVerified { node, .. } if node == &commit.context().node)
        && !matches!(event.payload(), E::TaskDiscovered { .. })) {
        return Err(error("stage commit contains unrelated effects"));
    }
    Ok(())
}

#[cfg(test)]
mod paged_owned_history_tests {
    use super::*;
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn folded_history_rejects_forged_stage_effects_digest() {
        use surge_core::execution_recovery::{
            OpenedSession, ProviderSessionDescriptor, ProviderSessionId, SessionOpenMode,
            SessionRestoreCapabilities, commit::StageOutcomeCommit,
        };
        use surge_core::{EventPayload as E, VersionedEventPayload as V};
        for forged in [false, true] {
            let home = tempfile::tempdir().unwrap();
            let storage = Storage::open(home.path()).await.unwrap();
            let run = RunId::new();
            let writer = storage.create_run(run, home.path(), None).await.unwrap();
            let node = surge_core::NodeKey::try_from("impl_1").unwrap();
            let outcome = surge_core::OutcomeKey::try_from("done").unwrap();
            let session = surge_core::SessionId::new();
            let invocation = surge_core::id::StageInvocationId::new();
            let descriptor = ProviderSessionDescriptor::new(
                ProviderSessionId::new("actual-saved-fixture".into()).unwrap(),
                invocation,
                "fixture-runtime".into(),
                surge_core::ContentHash::compute(b"launch"),
                home.path().into(),
                SessionRestoreCapabilities {
                    resume: true,
                    load: true,
                },
            )
            .unwrap();
            let opened = OpenedSession::new(session, descriptor, SessionOpenMode::New).unwrap();
            let graph: surge_core::Graph =
                toml::from_str(include_str!("../../../../examples/flow_minimal_agent.toml"))
                    .unwrap();
            let serialized = toml::to_string(&graph).unwrap();
            let mut effects = vec![V::new(E::OutcomeReported {
                node: node.clone(),
                outcome: outcome.clone(),
                summary: "accepted".into(),
            })];
            for index in 0..280 {
                effects.push(V::new(E::TaskDiscovered {
                    task_id: surge_core::roadmap::RoadmapTaskId::from(format!("task-{index}")),
                    discovered_from: surge_core::roadmap::RoadmapTaskId::from("origin-task"),
                    title: format!("Fixed page-crossing task {index}"),
                }));
            }
            let digest = if forged {
                surge_core::ContentHash::compute(b"forged-effects")
            } else {
                surge_core::ContentHash::compute(&serde_json::to_vec(&effects).unwrap())
            };
            let authority = surge_core::stage_tool::StageToolContext {
                run,
                node: node.clone(),
                session: surge_core::SessionId::new(),
                generation: surge_core::id::StageGenerationId::new(),
            };
            let commit = StageOutcomeCommit::new(
                authority.clone(),
                session,
                invocation,
                outcome.clone(),
                effects.len() as u32,
                digest,
            )
            .unwrap();
            let mut batch = vec![
                V::new(E::RunStarted {
                    pipeline_template: None,
                    project_path: home.path().into(),
                    initial_prompt: "fixture".into(),
                    config: surge_core::run_event::RunConfig {
                        bootstrap_edit_loop_cap: None,
                        sandbox_default: surge_core::sandbox::SandboxMode::WorkspaceWrite,
                        approval_default: surge_core::approvals::ApprovalPolicy::OnRequest,
                        auto_pr: false,
                        mcp_servers: vec![],
                        budget: surge_core::budget::BudgetGuard::default(),
                    },
                }),
                V::new(E::PipelineMaterialized {
                    graph: Box::new(graph),
                    graph_hash: surge_core::ContentHash::compute(serialized.as_bytes()),
                }),
                V::new(E::StageEntered {
                    node: node.clone(),
                    attempt: 1,
                }),
                V::new(E::SessionEstablishmentRequested {
                    node: node.clone(),
                    invocation,
                    restore: false,
                    authority: Some(authority),
                }),
                V::new(E::SessionOpened {
                    handoff: None,
                    node: node.clone(),
                    session,
                    agent: "fixture".into(),
                    agent_id: None,
                    opened: Some(opened),
                }),
            ];
            batch.extend(effects);
            batch.push(V::new(E::StageOutcomeCommitted { commit }));
            writer.append_events(batch).await.unwrap();
            writer.close().await.unwrap();
            let inspected = read_folded_events(&storage.events_db_path(&run), run);
            if forged {
                assert!(
                    inspected.err().is_some_and(|error| error
                        .to_string()
                        .contains("stage commit effects digest mismatch")),
                    "a forged marker must report its exact effects corruption"
                );
            } else {
                assert!(
                    inspected.is_ok(),
                    "the identical valid adjacent effects group must remain readable"
                );
            }
        }
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn owned_history_folds_beyond_ten_thousand_events_and_checks_late_gaps() {
        use surge_core::{
            EventPayload as E, VersionedEventPayload, approvals::ApprovalPolicy,
            run_event::RunConfig, sandbox::SandboxMode,
        };
        let home = tempfile::tempdir().unwrap();
        let storage = Storage::open(home.path()).await.unwrap();
        let run = RunId::new();
        let writer = storage.create_run(run, home.path(), None).await.unwrap();
        writer.close().await.unwrap();
        let mut conn = Connection::open(storage.events_db_path(&run)).unwrap();
        let txn = conn.transaction().unwrap();
        let origin = E::RunStarted {
            pipeline_template: None,
            project_path: home.path().into(),
            initial_prompt: String::new(),
            config: RunConfig {
                bootstrap_edit_loop_cap: None,
                sandbox_default: SandboxMode::WorkspaceWrite,
                approval_default: ApprovalPolicy::OnRequest,
                auto_pr: false,
                mcp_servers: vec![],
                budget: surge_core::budget::BudgetGuard::default(),
            },
        };
        for seq in 1..=10_025u64 {
            let payload = if seq == 1 {
                origin.clone()
            } else if seq == 10_025 {
                E::RunAborted {
                    reason: "Fixed late terminal oracle".into(),
                }
            } else {
                E::PipelineMaterialized {
                    graph: Box::new(
                        toml::from_str(include_str!(
                            "../../../../examples/flow_terminal_only.toml"
                        ))
                        .unwrap(),
                    ),
                    graph_hash: surge_core::ContentHash::compute(b"oracle"),
                }
            };
            txn.execute(
                "INSERT INTO events(seq,timestamp,kind,payload,schema_version) VALUES(?,?,?,?,?)",
                rusqlite::params![
                    seq,
                    1,
                    payload.discriminant_str(),
                    serde_json::to_vec(&VersionedEventPayload::new(payload.clone())).unwrap(),
                    VersionedEventPayload::new(payload.clone()).schema_version
                ],
            )
            .unwrap();
        }
        txn.commit().unwrap();
        let inspected = storage
            .inspect_folded_run(run)
            .await
            .unwrap()
            .database
            .unwrap();
        assert_eq!(inspected.event_count, 10_025);
        assert_eq!(inspected.startup.len(), 2);
        assert!(matches!(
            inspected.state,
            surge_core::RunState::Terminal {
                kind: surge_core::run_state::TerminalReason::Aborted,
                ..
            }
        ));
        conn.execute("INSERT INTO events(seq,timestamp,kind,payload,schema_version) SELECT 10027,timestamp,kind,payload,schema_version FROM events WHERE seq=10025",[]).unwrap();
        assert!(storage.inspect_folded_run(run).await.is_err());
        assert_eq!(
            conn.query_row("SELECT count(*) FROM events", [], |row| row
                .get::<_, u64>(0))
                .unwrap(),
            10_026
        );
    }
}
