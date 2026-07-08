//! Registry-level task-ledger index for cross-run CLI operations
//! (`surge ready` / `surge ledger`).
//!
//! Per-run event logs remain the source of truth for a run's ledger; the
//! per-run `task_ledger` view mirrors that run's folded [`LedgerState`]. This
//! registry index mirrors ledger rows across runs so the CLI can answer
//! "what is unblocked" without opening every run database.
//!
//! [`LedgerState`]: surge_core::LedgerState

use std::path::PathBuf;

use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::{OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};
use surge_core::{RoadmapStatus, RunId};

use crate::runs::error::StorageError;

/// Filter for registry-level task-ledger queries.
#[derive(Debug, Default, Clone)]
pub struct TaskLedgerIndexFilter {
    /// Only return tasks with this ledger status.
    pub status: Option<RoadmapStatus>,
    /// Only return tasks for this project path.
    pub project_path: Option<PathBuf>,
    /// Only return tasks attached to this run id.
    pub run_id: Option<RunId>,
    /// Only return tasks discovered mid-run (non-null `discovered_from`).
    pub discovered_only: bool,
    /// Limit returned rows.
    pub limit: Option<usize>,
}

/// Input used to upsert one registry-level task-ledger row.
#[derive(Debug, Clone)]
pub struct TaskLedgerIndexUpsert {
    /// Run that owns this task.
    pub run_id: RunId,
    /// Ledger task id.
    pub task_id: String,
    /// Project path this run belongs to.
    pub project_path: PathBuf,
    /// Current ledger status.
    pub status: RoadmapStatus,
    /// True once a verification-authority node confirmed the task.
    pub verified: bool,
    /// Task id this task was discovered from, when discovered mid-run.
    pub discovered_from: Option<String>,
    /// Node that last transitioned this task.
    pub last_authority_node: Option<String>,
    /// Seq of the last event that touched this task.
    pub updated_seq: u64,
    /// Observation timestamp in unix epoch milliseconds.
    pub observed_at_ms: i64,
}

/// One row from the registry-level task-ledger index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskLedgerIndexRecord {
    /// Run that owns this task.
    pub run_id: RunId,
    /// Ledger task id.
    pub task_id: String,
    /// Project path this run belongs to.
    pub project_path: PathBuf,
    /// Current ledger status.
    pub status: RoadmapStatus,
    /// True once a verification-authority node confirmed the task.
    pub verified: bool,
    /// Task id this task was discovered from, when discovered mid-run.
    pub discovered_from: Option<String>,
    /// Node that last transitioned this task.
    pub last_authority_node: Option<String>,
    /// Seq of the last event that touched this task.
    pub updated_seq: u64,
    /// Last time this row was updated in unix epoch milliseconds.
    pub updated_at_ms: i64,
}

/// Registry-backed store for cross-run task-ledger lookups.
#[derive(Clone)]
pub struct TaskLedgerStore {
    pool: Pool<SqliteConnectionManager>,
}

impl TaskLedgerStore {
    /// Create a store over the registry DB connection pool.
    #[must_use]
    pub fn new(pool: Pool<SqliteConnectionManager>) -> Self {
        Self { pool }
    }

    /// Insert or update one task-ledger row, keyed by `(run_id, task_id)`.
    ///
    /// # Errors
    /// Returns [`StorageError`] when the registry DB cannot be read or written.
    pub fn upsert(
        &self,
        input: &TaskLedgerIndexUpsert,
    ) -> Result<TaskLedgerIndexRecord, StorageError> {
        let conn = self
            .pool
            .get()
            .map_err(|e| StorageError::Pool(e.to_string()))?;
        conn.execute(
            "INSERT INTO task_ledger_index
                (run_id, task_id, project_path, status, verified, discovered_from,
                 last_authority_node, updated_seq, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(run_id, task_id) DO UPDATE SET
                project_path = excluded.project_path,
                status = excluded.status,
                verified = excluded.verified,
                discovered_from = COALESCE(excluded.discovered_from, discovered_from),
                last_authority_node = excluded.last_authority_node,
                updated_seq = excluded.updated_seq,
                updated_at = excluded.updated_at",
            params![
                input.run_id.to_string(),
                input.task_id,
                input.project_path.to_string_lossy().to_string(),
                status_label(input.status),
                i64::from(input.verified),
                input.discovered_from.as_deref(),
                input.last_authority_node.as_deref(),
                input.updated_seq as i64,
                input.observed_at_ms,
            ],
        )?;
        self.get(input.run_id, &input.task_id)?.ok_or_else(|| {
            StorageError::MigrationFailed("task ledger upsert vanished".into())
        })
    }

    /// List task-ledger rows matching `filter`, newest update first.
    ///
    /// # Errors
    /// Returns [`StorageError`] when the registry DB cannot be read.
    pub fn list(
        &self,
        filter: &TaskLedgerIndexFilter,
    ) -> Result<Vec<TaskLedgerIndexRecord>, StorageError> {
        let conn = self
            .pool
            .get()
            .map_err(|e| StorageError::Pool(e.to_string()))?;
        let mut sql = String::from(
            "SELECT run_id, task_id, project_path, status, verified, discovered_from,
                    last_authority_node, updated_seq, updated_at
             FROM task_ledger_index WHERE 1=1",
        );
        let mut binds: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        if let Some(status) = filter.status {
            sql.push_str(" AND status = ?");
            binds.push(Box::new(status_label(status).to_owned()));
        }
        if let Some(project_path) = &filter.project_path {
            sql.push_str(" AND project_path = ?");
            binds.push(Box::new(project_path.to_string_lossy().to_string()));
        }
        if let Some(run_id) = filter.run_id {
            sql.push_str(" AND run_id = ?");
            binds.push(Box::new(run_id.to_string()));
        }
        if filter.discovered_only {
            sql.push_str(" AND discovered_from IS NOT NULL");
        }
        sql.push_str(" ORDER BY updated_at DESC, run_id, task_id");
        if let Some(limit) = filter.limit {
            sql.push_str(" LIMIT ?");
            binds.push(Box::new(limit as i64));
        }

        let bind_refs: Vec<&dyn rusqlite::ToSql> =
            binds.iter().map(std::convert::AsRef::as_ref).collect();
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(bind_refs), row_to_record)?;
        rows.collect::<rusqlite::Result<_>>().map_err(Into::into)
    }

    /// Read one task-ledger row by `(run_id, task_id)`.
    ///
    /// # Errors
    /// Returns [`StorageError`] when the registry DB cannot be read.
    pub fn get(
        &self,
        run_id: RunId,
        task_id: &str,
    ) -> Result<Option<TaskLedgerIndexRecord>, StorageError> {
        let conn = self
            .pool
            .get()
            .map_err(|e| StorageError::Pool(e.to_string()))?;
        conn.query_row(
            "SELECT run_id, task_id, project_path, status, verified, discovered_from,
                    last_authority_node, updated_seq, updated_at
             FROM task_ledger_index WHERE run_id = ? AND task_id = ?",
            params![run_id.to_string(), task_id],
            row_to_record,
        )
        .optional()
        .map_err(Into::into)
    }
}

fn row_to_record(row: &Row<'_>) -> rusqlite::Result<TaskLedgerIndexRecord> {
    let run_id_str: String = row.get(0)?;
    let status_str: String = row.get(3)?;
    let verified_int: i64 = row.get(4)?;
    Ok(TaskLedgerIndexRecord {
        run_id: run_id_str.parse().map_err(|_| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                format!("invalid run id {run_id_str:?}").into(),
            )
        })?,
        task_id: row.get(1)?,
        project_path: PathBuf::from(row.get::<_, String>(2)?),
        status: parse_status(&status_str).map_err(|_| {
            rusqlite::Error::FromSqlConversionFailure(
                3,
                rusqlite::types::Type::Text,
                format!("invalid ledger status {status_str:?}").into(),
            )
        })?,
        verified: verified_int != 0,
        discovered_from: row.get(5)?,
        last_authority_node: row.get(6)?,
        updated_seq: row.get::<_, i64>(7)? as u64,
        updated_at_ms: row.get(8)?,
    })
}

/// Stable snake-case label for a ledger status, matching `RoadmapStatus`'s
/// serde/Display form used across the codebase.
fn status_label(status: RoadmapStatus) -> &'static str {
    match status {
        RoadmapStatus::Pending => "pending",
        RoadmapStatus::Running => "running",
        RoadmapStatus::Paused => "paused",
        RoadmapStatus::ReadyForVerification => "ready_for_verification",
        RoadmapStatus::FailedVerification => "failed_verification",
        RoadmapStatus::Completed => "completed",
        RoadmapStatus::Failed => "failed",
        RoadmapStatus::Skipped => "skipped",
    }
}

fn parse_status(label: &str) -> Result<RoadmapStatus, ()> {
    match label {
        "pending" => Ok(RoadmapStatus::Pending),
        "running" => Ok(RoadmapStatus::Running),
        "paused" => Ok(RoadmapStatus::Paused),
        "ready_for_verification" => Ok(RoadmapStatus::ReadyForVerification),
        "failed_verification" => Ok(RoadmapStatus::FailedVerification),
        "completed" => Ok(RoadmapStatus::Completed),
        "failed" => Ok(RoadmapStatus::Failed),
        "skipped" => Ok(RoadmapStatus::Skipped),
        _ => Err(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runs::clock::MockClock;
    use crate::runs::registry::open_registry_pool;

    fn store() -> TaskLedgerStore {
        let tmp = tempfile::tempdir().unwrap();
        let clock = MockClock::new(1_700_000_000_000);
        let pool = open_registry_pool(tmp.path(), &clock).unwrap();
        TaskLedgerStore::new(pool)
    }

    fn upsert(
        run: RunId,
        task: &str,
        status: RoadmapStatus,
        verified: bool,
        discovered_from: Option<&str>,
    ) -> TaskLedgerIndexUpsert {
        TaskLedgerIndexUpsert {
            run_id: run,
            task_id: task.into(),
            project_path: PathBuf::from("/proj"),
            status,
            verified,
            discovered_from: discovered_from.map(ToOwned::to_owned),
            last_authority_node: Some("verify_1".into()),
            updated_seq: 7,
            observed_at_ms: 1_700_000_000_000,
        }
    }

    #[test]
    fn upsert_then_get_roundtrips() {
        let store = store();
        let run = RunId::new();
        let record = store
            .upsert(&upsert(run, "m1-t1", RoadmapStatus::Completed, true, None))
            .unwrap();
        assert!(record.verified);
        assert_eq!(record.status, RoadmapStatus::Completed);

        let fetched = store.get(run, "m1-t1").unwrap().unwrap();
        assert_eq!(fetched, record);
    }

    #[test]
    fn upsert_is_idempotent_on_run_task_key() {
        let store = store();
        let run = RunId::new();
        store
            .upsert(&upsert(run, "t1", RoadmapStatus::ReadyForVerification, false, None))
            .unwrap();
        store
            .upsert(&upsert(run, "t1", RoadmapStatus::Completed, true, None))
            .unwrap();
        let all = store.list(&TaskLedgerIndexFilter::default()).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].status, RoadmapStatus::Completed);
        assert!(all[0].verified);
    }

    #[test]
    fn list_filters_by_status_and_discovered() {
        let store = store();
        let run = RunId::new();
        store
            .upsert(&upsert(run, "t1", RoadmapStatus::Pending, false, None))
            .unwrap();
        store
            .upsert(&upsert(run, "t2", RoadmapStatus::Pending, false, Some("t1")))
            .unwrap();
        store
            .upsert(&upsert(run, "t3", RoadmapStatus::Completed, true, None))
            .unwrap();

        let pending = store
            .list(&TaskLedgerIndexFilter {
                status: Some(RoadmapStatus::Pending),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(pending.len(), 2);

        let discovered = store
            .list(&TaskLedgerIndexFilter {
                discovered_only: true,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(discovered.len(), 1);
        assert_eq!(discovered[0].task_id, "t2");
        assert_eq!(discovered[0].discovered_from.as_deref(), Some("t1"));
    }

    #[test]
    fn discovered_from_is_preserved_across_later_status_upserts() {
        let store = store();
        let run = RunId::new();
        store
            .upsert(&upsert(run, "t2", RoadmapStatus::Pending, false, Some("t1")))
            .unwrap();
        // A later status-only upsert (COALESCE keeps the origin).
        store
            .upsert(&upsert(run, "t2", RoadmapStatus::ReadyForVerification, false, None))
            .unwrap();
        let record = store.get(run, "t2").unwrap().unwrap();
        assert_eq!(record.discovered_from.as_deref(), Some("t1"));
        assert_eq!(record.status, RoadmapStatus::ReadyForVerification);
    }
}
