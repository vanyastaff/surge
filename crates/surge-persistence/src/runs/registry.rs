//! Registry DB — cross-run metadata lives here.
//!
//! Schema: see `migrations/registry/0001_initial.sql`. M2 ships only the `runs`
//! table; profiles/templates/trust come in M3+.

use std::path::{Path, PathBuf};

use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use surge_core::{RunId, RunStatus};

use crate::runs::clock::Clock;
use crate::runs::error::{OpenError, StorageError};
use crate::runs::migrations::{apply as apply_migrations, REGISTRY_MIGRATIONS};
use crate::runs::pragmas::{apply as apply_pragmas, REGISTRY_PRAGMAS};

/// Filter applied to `Storage::list_runs`.
#[derive(Debug, Default, Clone)]
pub struct RunFilter {
    /// Only return runs with this status.
    pub status: Option<RunStatus>,
    /// Only return runs whose project_path matches exactly.
    pub project_path: Option<PathBuf>,
    /// Limit on the number of returned rows.
    pub limit: Option<usize>,
}

/// Lightweight summary returned by `list_runs` and `get_run`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunSummary {
    /// Full RunId.
    pub id: RunId,
    /// Absolute path to the project this run was created against.
    pub project_path: PathBuf,
    /// Optional pipeline template id (e.g., "rust-crate-tdd@1.0").
    pub pipeline_template: Option<String>,
    /// Coarse lifecycle status.
    pub status: RunStatus,
    /// Unix epoch ms of run creation.
    pub started_at_ms: i64,
    /// Unix epoch ms of run termination, NULL if still active.
    pub ended_at_ms: Option<i64>,
    /// Daemon process id, NULL if cleanly terminated or never started.
    pub daemon_pid: Option<i32>,
}

/// Open the registry pool, creating `~/.surge/db/` and applying migrations if needed.
pub fn open_registry_pool(
    home: &Path,
    clock: &dyn Clock,
) -> Result<Pool<SqliteConnectionManager>, OpenError> {
    let db_dir = home.join("db");
    std::fs::create_dir_all(&db_dir)?;
    let db_path = db_dir.join("registry.sqlite");

    // Apply migrations on a dedicated connection.
    let mut conn = rusqlite::Connection::open(&db_path)?;
    apply_pragmas(&conn, REGISTRY_PRAGMAS)?;
    apply_migrations(&mut conn, REGISTRY_MIGRATIONS, clock)
        .map_err(|e| OpenError::MigrationFailed(e.to_string()))?;
    drop(conn);

    let manager = SqliteConnectionManager::file(&db_path)
        .with_init(|c| apply_pragmas(c, REGISTRY_PRAGMAS));
    let pool = Pool::builder()
        .max_size(8)
        .build(manager)
        .map_err(|e| OpenError::Pool(e.to_string()))?;

    Ok(pool)
}

/// Insert a new run row.
pub fn insert_run(
    pool: &Pool<SqliteConnectionManager>,
    summary: &RunSummary,
) -> Result<(), StorageError> {
    let conn = pool.get().map_err(|e| StorageError::Pool(e.to_string()))?;
    conn.execute(
        "INSERT INTO runs (id, project_path, pipeline_template, status, started_at, ended_at, daemon_pid)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
        params![
            summary.id.to_string(),
            summary.project_path.to_string_lossy(),
            summary.pipeline_template,
            summary.status.as_str(),
            summary.started_at_ms,
            summary.ended_at_ms,
            summary.daemon_pid,
        ],
    )?;
    Ok(())
}

/// Get a single run summary by id, or None if not found.
pub fn get_run(
    pool: &Pool<SqliteConnectionManager>,
    run_id: &RunId,
) -> Result<Option<RunSummary>, StorageError> {
    let conn = pool.get().map_err(|e| StorageError::Pool(e.to_string()))?;
    match conn.query_row(
        "SELECT id, project_path, pipeline_template, status, started_at, ended_at, daemon_pid
         FROM runs WHERE id = ?",
        params![run_id.to_string()],
        row_to_summary,
    ) {
        Ok(s) => Ok(Some(s)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// List runs matching the filter, sorted by started_at DESC.
pub fn list_runs(
    pool: &Pool<SqliteConnectionManager>,
    filter: &RunFilter,
) -> Result<Vec<RunSummary>, StorageError> {
    let conn = pool.get().map_err(|e| StorageError::Pool(e.to_string()))?;

    let mut sql = String::from(
        "SELECT id, project_path, pipeline_template, status, started_at, ended_at, daemon_pid \
         FROM runs WHERE 1=1",
    );
    let mut binds: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    if let Some(s) = filter.status {
        sql.push_str(" AND status = ?");
        binds.push(Box::new(s.as_str().to_string()));
    }
    if let Some(p) = &filter.project_path {
        sql.push_str(" AND project_path = ?");
        binds.push(Box::new(p.to_string_lossy().into_owned()));
    }
    sql.push_str(" ORDER BY started_at DESC");
    if let Some(lim) = filter.limit {
        sql.push_str(" LIMIT ?");
        binds.push(Box::new(lim as i64));
    }

    let mut stmt = conn.prepare(&sql)?;
    let bind_refs: Vec<&dyn rusqlite::ToSql> = binds.iter().map(std::convert::AsRef::as_ref).collect();
    let rows = stmt
        .query_map(rusqlite::params_from_iter(bind_refs), row_to_summary)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Delete a run row.
pub fn delete_run(
    pool: &Pool<SqliteConnectionManager>,
    run_id: &RunId,
) -> Result<(), StorageError> {
    let conn = pool.get().map_err(|e| StorageError::Pool(e.to_string()))?;
    conn.execute("DELETE FROM runs WHERE id = ?", params![run_id.to_string()])?;
    Ok(())
}

/// Update status (and optionally ended_at_ms) for a run.
pub fn update_status(
    pool: &Pool<SqliteConnectionManager>,
    run_id: &RunId,
    status: RunStatus,
    ended_at_ms: Option<i64>,
) -> Result<(), StorageError> {
    let conn = pool.get().map_err(|e| StorageError::Pool(e.to_string()))?;
    conn.execute(
        "UPDATE runs SET status = ?, ended_at = ? WHERE id = ?",
        params![status.as_str(), ended_at_ms, run_id.to_string()],
    )?;
    Ok(())
}

fn row_to_summary(row: &rusqlite::Row<'_>) -> rusqlite::Result<RunSummary> {
    let id_str: String = row.get(0)?;
    let id: RunId = id_str.parse().map_err(|e: ulid::DecodeError| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })?;
    let status_str: String = row.get(3)?;
    let status: RunStatus = status_str.parse().map_err(|e: surge_core::ParseRunStatusError| {
        rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Text, Box::new(e))
    })?;

    Ok(RunSummary {
        id,
        project_path: PathBuf::from(row.get::<_, String>(1)?),
        pipeline_template: row.get(2)?,
        status,
        started_at_ms: row.get(4)?,
        ended_at_ms: row.get(5)?,
        daemon_pid: row.get(6)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runs::clock::MockClock;
    use tempfile::TempDir;

    fn fixture_summary(id: RunId, pid: Option<i32>) -> RunSummary {
        RunSummary {
            id,
            project_path: "/tmp/proj".into(),
            pipeline_template: Some("t@1".into()),
            status: RunStatus::Running,
            started_at_ms: 1_700_000_000_000,
            ended_at_ms: None,
            daemon_pid: pid,
        }
    }

    #[test]
    fn insert_get_list_delete_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let clock = MockClock::new(1_700_000_000_000);
        let pool = open_registry_pool(tmp.path(), &clock).unwrap();
        let s = fixture_summary(RunId::new(), Some(1234));

        insert_run(&pool, &s).unwrap();
        let got = get_run(&pool, &s.id).unwrap().unwrap();
        assert_eq!(got.id, s.id);
        assert_eq!(got.daemon_pid, Some(1234));

        let listed = list_runs(&pool, &RunFilter::default()).unwrap();
        assert_eq!(listed.len(), 1);

        delete_run(&pool, &s.id).unwrap();
        assert!(get_run(&pool, &s.id).unwrap().is_none());
    }

    #[test]
    fn list_filter_by_status() {
        let tmp = TempDir::new().unwrap();
        let clock = MockClock::new(1_700_000_000_000);
        let pool = open_registry_pool(tmp.path(), &clock).unwrap();

        let mut a = fixture_summary(RunId::new(), Some(1));
        a.status = RunStatus::Running;
        let mut b = fixture_summary(RunId::new(), None);
        b.status = RunStatus::Completed;

        insert_run(&pool, &a).unwrap();
        insert_run(&pool, &b).unwrap();

        let running = list_runs(
            &pool,
            &RunFilter {
                status: Some(RunStatus::Running),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(running.len(), 1);
        assert_eq!(running[0].id, a.id);
    }

    #[test]
    fn update_status_transitions() {
        let tmp = TempDir::new().unwrap();
        let clock = MockClock::new(1_700_000_000_000);
        let pool = open_registry_pool(tmp.path(), &clock).unwrap();
        let s = fixture_summary(RunId::new(), Some(1));
        insert_run(&pool, &s).unwrap();

        update_status(&pool, &s.id, RunStatus::Crashed, Some(1_700_000_000_500)).unwrap();
        let got = get_run(&pool, &s.id).unwrap().unwrap();
        assert_eq!(got.status, RunStatus::Crashed);
        assert_eq!(got.ended_at_ms, Some(1_700_000_000_500));
    }
}
