//! Registry-level project task queue (ADR-0020).
//!
//! `.surge/roadmap.toml` in the repository is the planning truth. This store
//! mirrors it into `task_queue` so the daemon can dispatch, reconcile and
//! report without reading the file on every tick, and so the execution facts
//! a file cannot hold (dispatch state, run id, attempt, skip count) survive a
//! restart.
//!
//! Two write rules, by column family:
//!
//! - **Mirror** ([`TaskQueueStore::mirror`]) writes planning columns only
//!   (priority, dependencies, size, roadmap hash) and never touches
//!   execution columns. It is keyed by `(project_root, task_id)` and runs on
//!   every tick whose file hash differs from the stored one.
//! - **Scheduler** methods ([`TaskQueueStore::claim`],
//!   [`TaskQueueStore::record_skips`], [`TaskQueueStore::mark_done`], …)
//!   write execution columns only.
//!
//! The claim is the single-flight primitive: an `UPDATE ... WHERE
//! dispatch_state = 'queued'` whose affected-row count decides the race. Zero
//! rows is a lost race, not an error — the caller simply does not dispatch.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::{OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};
use surge_core::roadmap::TaskSize;
use surge_core::{ContentHash, Priority, RunId};

use crate::runs::error::StorageError;

/// Execution state of a queue row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DispatchState {
    /// Waiting for dispatch.
    Queued,
    /// A run was minted for it; `run_id` is set.
    Dispatched,
    /// The run finished successfully (verified).
    Done,
    /// The run failed, or the dispatch crashed past the retry rule.
    Failed,
}

impl DispatchState {
    /// Stable string form used in the `dispatch_state` column.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Dispatched => "dispatched",
            Self::Done => "done",
            Self::Failed => "failed",
        }
    }

    /// Parse the column form.
    ///
    /// # Errors
    /// [`StorageError::MigrationFailed`] when the label is unknown — a row
    /// written by a future schema must not be silently treated as queued.
    pub fn parse(label: &str) -> Result<Self, StorageError> {
        match label {
            "queued" => Ok(Self::Queued),
            "dispatched" => Ok(Self::Dispatched),
            "done" => Ok(Self::Done),
            "failed" => Ok(Self::Failed),
            other => Err(StorageError::MigrationFailed(format!(
                "unknown dispatch_state {other:?} in task_queue"
            ))),
        }
    }
}

/// One queue row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskQueueRow {
    /// Repository root — the project partition key.
    pub project_root: PathBuf,
    /// Roadmap task id (`m1-t1`).
    pub task_id: String,
    /// Hash of the roadmap file this row was mirrored from.
    pub roadmap_hash: ContentHash,
    /// Manual priority.
    pub priority: Priority,
    /// Task ids that must complete first.
    pub depends_on: Vec<String>,
    /// Context-budget class, when the roadmap carries one.
    pub size: Option<TaskSize>,
    /// Unix epoch milliseconds the row first entered the queue.
    pub enqueued_at: i64,
    /// How many dispatch decisions passed this row over.
    pub skipped_dispatches: u32,
    /// Execution state.
    pub dispatch_state: DispatchState,
    /// The run this row was dispatched to, when any.
    pub run_id: Option<RunId>,
    /// Attempt number; 1 for the first dispatch, incremented when a crashed
    /// dispatch is re-queued.
    pub attempt: u32,
}

/// One planning-side row as fed to [`TaskQueueStore::mirror`].
///
/// A re-export of [`surge_core::QueueMirrorEntry`] so callers can build mirror
/// input directly from a parsed roadmap (`RoadmapArtifact::to_queue_entries`)
/// without a conversion hop.
pub use surge_core::QueueMirrorEntry as TaskQueueMirrorEntry;

/// Filter for [`TaskQueueStore::list`].
#[derive(Debug, Default, Clone)]
pub struct TaskQueueFilter {
    /// Only rows for this project root.
    pub project_root: Option<PathBuf>,
    /// Only rows in this dispatch state.
    pub dispatch_state: Option<DispatchState>,
    /// Only the row attached to this run.
    pub run_id: Option<RunId>,
}

/// Registry-backed store for the project task queue.
#[derive(Clone)]
pub struct TaskQueueStore {
    pool: Pool<SqliteConnectionManager>,
}

impl TaskQueueStore {
    /// Create a store over the registry DB connection pool.
    #[must_use]
    pub fn new(pool: Pool<SqliteConnectionManager>) -> Self {
        Self { pool }
    }

    /// Mirror a roadmap's planning columns into the queue, leaving execution
    /// columns untouched.
    ///
    /// A row absent from `entries` is **kept**, not deleted: the roadmap may
    /// be mid-write, and dropping a dispatched task because a file was
    /// momentarily unreadable would lose the only record linking a live run
    /// to its queue row. Removal is an explicit operator action.
    ///
    /// Rows already present keep their `enqueued_at` and skip count; new rows
    /// start `queued`, attempt 1, at `now_ms`.
    ///
    /// # Errors
    /// Returns [`StorageError`] when the registry DB cannot be written.
    pub fn mirror(
        &self,
        project_root: &Path,
        roadmap_hash: &ContentHash,
        entries: &[TaskQueueMirrorEntry],
        now_ms: i64,
    ) -> Result<usize, StorageError> {
        let mut conn = self
            .pool
            .get()
            .map_err(|e| StorageError::Pool(e.to_string()))?;
        let tx = conn.transaction()?;
        let mut written = 0usize;
        for entry in entries {
            let depends_on_json = serde_json::to_string(&entry.depends_on)
                .map_err(|e| StorageError::MigrationFailed(format!("depends_on json: {e}")))?;
            tx.execute(
                "INSERT INTO task_queue
                    (project_root, task_id, roadmap_hash, priority, depends_on_json, size,
                     enqueued_at, skipped_dispatches, dispatch_state, run_id, attempt, updated_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, 0, 'queued', NULL, 1, ?)
                 ON CONFLICT(project_root, task_id) DO UPDATE SET
                    roadmap_hash = excluded.roadmap_hash,
                    priority = excluded.priority,
                    depends_on_json = excluded.depends_on_json,
                    size = excluded.size,
                    updated_at = excluded.updated_at",
                params![
                    project_root.to_string_lossy(),
                    entry.task_id,
                    roadmap_hash.to_string(),
                    entry.priority.to_string(),
                    depends_on_json,
                    entry.size.map(|s| s.to_string()),
                    now_ms,
                    now_ms,
                ],
            )?;
            written += 1;
        }
        tx.commit()?;
        Ok(written)
    }

    /// List rows matching `filter`, oldest enqueue first.
    ///
    /// # Errors
    /// Returns [`StorageError`] when the registry DB cannot be read.
    pub fn list(&self, filter: &TaskQueueFilter) -> Result<Vec<TaskQueueRow>, StorageError> {
        let conn = self
            .pool
            .get()
            .map_err(|e| StorageError::Pool(e.to_string()))?;
        let mut sql = String::from(
            "SELECT project_root, task_id, roadmap_hash, priority, depends_on_json, size,
                    enqueued_at, skipped_dispatches, dispatch_state, run_id, attempt
             FROM task_queue WHERE 1=1",
        );
        let mut binds: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        if let Some(root) = &filter.project_root {
            sql.push_str(" AND project_root = ?");
            binds.push(Box::new(root.to_string_lossy().into_owned()));
        }
        if let Some(state) = filter.dispatch_state {
            sql.push_str(" AND dispatch_state = ?");
            binds.push(Box::new(state.as_str().to_owned()));
        }
        if let Some(run_id) = filter.run_id {
            sql.push_str(" AND run_id = ?");
            binds.push(Box::new(run_id.to_string()));
        }
        sql.push_str(" ORDER BY enqueued_at ASC, task_id ASC");

        let bind_refs: Vec<&dyn rusqlite::ToSql> =
            binds.iter().map(std::convert::AsRef::as_ref).collect();
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(bind_refs), row_to_queue_row)?;
        rows.collect::<rusqlite::Result<_>>().map_err(Into::into)
    }

    /// Read one row by `(project_root, task_id)`.
    ///
    /// # Errors
    /// Returns [`StorageError`] when the registry DB cannot be read.
    pub fn get(
        &self,
        project_root: &Path,
        task_id: &str,
    ) -> Result<Option<TaskQueueRow>, StorageError> {
        let conn = self
            .pool
            .get()
            .map_err(|e| StorageError::Pool(e.to_string()))?;
        conn.query_row(
            "SELECT project_root, task_id, roadmap_hash, priority, depends_on_json, size,
                    enqueued_at, skipped_dispatches, dispatch_state, run_id, attempt
             FROM task_queue WHERE project_root = ? AND task_id = ?",
            params![project_root.to_string_lossy(), task_id],
            row_to_queue_row,
        )
        .optional()
        .map_err(Into::into)
    }

    /// Claim a queued row for a freshly minted run.
    ///
    /// The single-flight primitive: sets `dispatched`, records `run_id`, and
    /// leaves `attempt` and `enqueued_at` alone. `false` means the update
    /// matched no `queued` row — another tick won, or the row is paused/done —
    /// and the caller must **not** start the run.
    ///
    /// # Errors
    /// Returns [`StorageError`] when the registry DB cannot be written.
    pub fn claim(
        &self,
        project_root: &Path,
        task_id: &str,
        run_id: RunId,
        now_ms: i64,
    ) -> Result<bool, StorageError> {
        let conn = self
            .pool
            .get()
            .map_err(|e| StorageError::Pool(e.to_string()))?;
        let affected = conn.execute(
            "UPDATE task_queue
             SET dispatch_state = 'dispatched', run_id = ?, updated_at = ?
             WHERE project_root = ? AND task_id = ? AND dispatch_state = 'queued'",
            params![
                run_id.to_string(),
                now_ms,
                project_root.to_string_lossy(),
                task_id
            ],
        )?;
        Ok(affected == 1)
    }

    /// Add one skip to every queued row that was not chosen this tick.
    ///
    /// Takes the ids the policy chose, so the caller does not have to
    /// enumerate the losers.
    ///
    /// # Errors
    /// Returns [`StorageError`] when the registry DB cannot be written.
    pub fn record_skips(
        &self,
        project_root: &Path,
        chosen: &[String],
        now_ms: i64,
    ) -> Result<usize, StorageError> {
        let mut conn = self
            .pool
            .get()
            .map_err(|e| StorageError::Pool(e.to_string()))?;
        let tx = conn.transaction()?;
        // SQLite has no array parameter; a NOT IN list built from the chosen
        // ids is safe because ids come from our own `list` read, not from
        // free-form input. The list is bounded by the queue length.
        let mut sql = String::from(
            "UPDATE task_queue SET skipped_dispatches = skipped_dispatches + 1, updated_at = ?
             WHERE project_root = ? AND dispatch_state = 'queued'",
        );
        let mut binds: Vec<Box<dyn rusqlite::ToSql>> = vec![
            Box::new(now_ms),
            Box::new(project_root.to_string_lossy().into_owned()),
        ];
        if !chosen.is_empty() {
            let placeholders = vec!["?"; chosen.len()].join(", ");
            sql.push_str(&format!(" AND task_id NOT IN ({placeholders})"));
            for id in chosen {
                binds.push(Box::new(id.clone()));
            }
        }
        let bind_refs: Vec<&dyn rusqlite::ToSql> =
            binds.iter().map(std::convert::AsRef::as_ref).collect();
        let affected = tx.execute(&sql, rusqlite::params_from_iter(bind_refs))?;
        tx.commit()?;
        Ok(affected)
    }

    /// Mark a dispatched row done or failed.
    ///
    /// `false` when no row matched (already settled, or never dispatched).
    ///
    /// # Errors
    /// Returns [`StorageError`] when the registry DB cannot be written.
    pub fn settle(
        &self,
        project_root: &Path,
        task_id: &str,
        state: DispatchState,
        now_ms: i64,
    ) -> Result<bool, StorageError> {
        debug_assert!(
            matches!(state, DispatchState::Done | DispatchState::Failed),
            "settle is for terminal states; claim is for dispatch"
        );
        let conn = self
            .pool
            .get()
            .map_err(|e| StorageError::Pool(e.to_string()))?;
        let affected = conn.execute(
            "UPDATE task_queue SET dispatch_state = ?, updated_at = ?
             WHERE project_root = ? AND task_id = ? AND dispatch_state = 'dispatched'",
            params![
                state.as_str(),
                now_ms,
                project_root.to_string_lossy(),
                task_id
            ],
        )?;
        Ok(affected == 1)
    }

    /// Return a `dispatched` row to `queued` and bump its attempt.
    ///
    /// Used by the reconciliation sweep for a dispatch that never produced a
    /// run, and by `surge task requeue` for a failed dependency. The attempt
    /// counter is what makes an unbounded crash loop visible.
    ///
    /// # Errors
    /// Returns [`StorageError`] when the registry DB cannot be written.
    pub fn requeue(
        &self,
        project_root: &Path,
        task_id: &str,
        now_ms: i64,
    ) -> Result<bool, StorageError> {
        let conn = self
            .pool
            .get()
            .map_err(|e| StorageError::Pool(e.to_string()))?;
        let affected = conn.execute(
            "UPDATE task_queue
             SET dispatch_state = 'queued', run_id = NULL, attempt = attempt + 1, updated_at = ?
             WHERE project_root = ? AND task_id = ? AND dispatch_state IN ('dispatched', 'failed')",
            params![now_ms, project_root.to_string_lossy(), task_id],
        )?;
        Ok(affected == 1)
    }

    /// Force a row's planning priority, without rewriting the roadmap file.
    ///
    /// The CLI writes the file *and* calls this; the file remains the truth a
    /// later mirror would restore, so this is only meaningful until the next
    /// hash change. Returns `false` when the row does not exist.
    ///
    /// # Errors
    /// Returns [`StorageError`] when the registry DB cannot be written.
    pub fn set_priority(
        &self,
        project_root: &Path,
        task_id: &str,
        priority: Priority,
        now_ms: i64,
    ) -> Result<bool, StorageError> {
        let conn = self
            .pool
            .get()
            .map_err(|e| StorageError::Pool(e.to_string()))?;
        let affected = conn.execute(
            "UPDATE task_queue SET priority = ?, updated_at = ?
             WHERE project_root = ? AND task_id = ?",
            params![
                priority.to_string(),
                now_ms,
                project_root.to_string_lossy(),
                task_id
            ],
        )?;
        Ok(affected == 1)
    }

    /// Set or clear the project's pause flag, creating the row on first use.
    ///
    /// # Errors
    /// Returns [`StorageError`] when the registry DB cannot be written.
    pub fn set_project_paused(
        &self,
        project_root: &Path,
        paused: bool,
        now_ms: i64,
    ) -> Result<(), StorageError> {
        let conn = self
            .pool
            .get()
            .map_err(|e| StorageError::Pool(e.to_string()))?;
        conn.execute(
            "INSERT INTO project_queue (project_root, paused, updated_at)
             VALUES (?, ?, ?)
             ON CONFLICT(project_root) DO UPDATE SET
                paused = excluded.paused,
                updated_at = excluded.updated_at",
            params![project_root.to_string_lossy(), i64::from(paused), now_ms],
        )?;
        Ok(())
    }

    /// Register a project with the queue (unpaused), creating its
    /// `project_queue` row if absent.
    ///
    /// A project is known to the scheduler through this table even before
    /// its roadmap has been mirrored — otherwise the first tick would have
    /// to already know about a task_queue row to discover the project that
    /// produces it. `surge project start` (T7) calls this; re-registering
    /// never clears a pause.
    ///
    /// # Errors
    /// Returns [`StorageError`] when the registry DB cannot be written.
    pub fn register_project(&self, project_root: &Path, now_ms: i64) -> Result<(), StorageError> {
        let conn = self
            .pool
            .get()
            .map_err(|e| StorageError::Pool(e.to_string()))?;
        conn.execute(
            "INSERT INTO project_queue (project_root, paused, updated_at)
             VALUES (?, 0, ?)
             ON CONFLICT(project_root) DO UPDATE SET updated_at = excluded.updated_at",
            params![project_root.to_string_lossy(), now_ms],
        )?;
        Ok(())
    }

    /// Whether the project's queue is paused. Absent row = not paused.
    ///
    /// # Errors
    /// Returns [`StorageError`] when the registry DB cannot be read.
    pub fn is_project_paused(&self, project_root: &Path) -> Result<bool, StorageError> {
        let conn = self
            .pool
            .get()
            .map_err(|e| StorageError::Pool(e.to_string()))?;
        let paused: Option<i64> = conn
            .query_row(
                "SELECT paused FROM project_queue WHERE project_root = ?",
                params![project_root.to_string_lossy()],
                |row| row.get(0),
            )
            .optional()?;
        Ok(paused.unwrap_or(0) != 0)
    }

    /// Projects known to the queue, with their pause flag.
    ///
    /// The scheduler's outer loop. A project is known once it is registered
    /// (or has any `task_queue` row), so the first tick after
    /// `surge project start` sees it before any task is mirrored.
    ///
    /// # Errors
    /// Returns [`StorageError`] when the registry DB cannot be read.
    pub fn projects(&self) -> Result<Vec<(PathBuf, bool)>, StorageError> {
        let conn = self
            .pool
            .get()
            .map_err(|e| StorageError::Pool(e.to_string()))?;
        let mut stmt = conn.prepare(
            "SELECT roots.project_root, COALESCE(p.paused, 0)
             FROM (
                SELECT project_root FROM project_queue
                UNION
                SELECT DISTINCT project_root FROM task_queue
             ) AS roots
             LEFT JOIN project_queue p ON p.project_root = roots.project_root
             ORDER BY roots.project_root",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                PathBuf::from(row.get::<_, String>(0)?),
                row.get::<_, i64>(1)? != 0,
            ))
        })?;
        rows.collect::<rusqlite::Result<_>>().map_err(Into::into)
    }

    /// Dependency states for every task in `project_root`, as the policy
    /// wants them: for each task id, the state of each of its dependencies.
    ///
    /// Derived by joining the queue itself: a dependency's state is
    /// `Completed` when its row is `done`, and its roadmap status otherwise
    /// (which the scheduler projects into `failed`/waiting). This keeps the
    /// queue self-contained — a task's readiness never depends on a run log
    /// being readable.
    ///
    /// # Errors
    /// Returns [`StorageError`] when the registry DB cannot be read.
    pub fn dependency_states(
        &self,
        project_root: &Path,
    ) -> Result<BTreeMap<String, BTreeMap<String, surge_core::RoadmapStatus>>, StorageError> {
        let rows = self.list(&TaskQueueFilter {
            project_root: Some(project_root.to_path_buf()),
            ..Default::default()
        })?;
        let states: BTreeMap<&str, surge_core::RoadmapStatus> = rows
            .iter()
            .map(|row| {
                (
                    row.task_id.as_str(),
                    queue_state_as_roadmap(row.dispatch_state),
                )
            })
            .collect();
        let mut out = BTreeMap::new();
        for row in &rows {
            let deps = row
                .depends_on
                .iter()
                .filter_map(|dep| states.get(dep.as_str()).map(|state| (dep.clone(), *state)))
                .collect();
            out.insert(row.task_id.clone(), deps);
        }
        Ok(out)
    }
}

/// Project a queue row's dispatch state into the roadmap vocabulary the
/// policy speaks. `queued`/`dispatched` are both "not completed yet"; the
/// distinction that matters to readiness is only whether a dependency is
/// finished, failed, or still in flight.
fn queue_state_as_roadmap(state: DispatchState) -> surge_core::RoadmapStatus {
    use surge_core::RoadmapStatus;
    match state {
        DispatchState::Done => RoadmapStatus::Completed,
        DispatchState::Failed => RoadmapStatus::Failed,
        DispatchState::Queued | DispatchState::Dispatched => RoadmapStatus::Pending,
    }
}

fn row_to_queue_row(row: &Row<'_>) -> rusqlite::Result<TaskQueueRow> {
    let roadmap_hash_str: String = row.get(2)?;
    let priority_str: String = row.get(3)?;
    let depends_on_json: String = row.get(4)?;
    let size_str: Option<String> = row.get(5)?;
    let state_str: String = row.get(8)?;
    let run_id_str: Option<String> = row.get(9)?;

    Ok(TaskQueueRow {
        project_root: PathBuf::from(row.get::<_, String>(0)?),
        task_id: row.get(1)?,
        roadmap_hash: roadmap_hash_str.parse().map_err(
            |e: surge_core::content_hash::ContentHashParseError| {
                rusqlite::Error::FromSqlConversionFailure(
                    2,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            },
        )?,
        priority: priority_str
            .parse()
            .map_err(|e: surge_core::PriorityParseError| {
                rusqlite::Error::FromSqlConversionFailure(
                    3,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })?,
        depends_on: serde_json::from_str(&depends_on_json).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(4, rusqlite::types::Type::Text, Box::new(e))
        })?,
        size: size_str
            .as_deref()
            .map(|s| match s {
                "s" => Ok(TaskSize::S),
                "m" => Ok(TaskSize::M),
                "l" => Ok(TaskSize::L),
                other => Err(rusqlite::Error::FromSqlConversionFailure(
                    5,
                    rusqlite::types::Type::Text,
                    format!("unknown task size {other:?}").into(),
                )),
            })
            .transpose()?,
        enqueued_at: row.get(6)?,
        skipped_dispatches: u32::try_from(row.get::<_, i64>(7)?).unwrap_or(u32::MAX),
        dispatch_state: DispatchState::parse(&state_str).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(8, rusqlite::types::Type::Text, Box::new(e))
        })?,
        run_id: run_id_str
            .map(|s| {
                s.parse().map_err(|e: ulid::DecodeError| {
                    rusqlite::Error::FromSqlConversionFailure(
                        9,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    )
                })
            })
            .transpose()?,
        attempt: u32::try_from(row.get::<_, i64>(10)?).unwrap_or(u32::MAX),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runs::clock::MockClock;
    use crate::runs::registry::open_registry_pool;

    fn store() -> TaskQueueStore {
        let tmp = tempfile::tempdir().unwrap();
        let clock = MockClock::new(1_700_000_000_000);
        let pool = open_registry_pool(tmp.path(), &clock).unwrap();
        TaskQueueStore::new(pool)
    }

    fn hash(s: &str) -> ContentHash {
        ContentHash::compute(s.as_bytes())
    }

    fn entry(id: &str, deps: &[&str]) -> TaskQueueMirrorEntry {
        TaskQueueMirrorEntry {
            task_id: id.into(),
            priority: Priority::Medium,
            depends_on: deps.iter().map(|d| (*d).to_string()).collect(),
            size: Some(TaskSize::M),
        }
    }

    #[test]
    fn mirror_inserts_then_updates_planning_columns_only() {
        let store = store();
        let root = PathBuf::from("/proj");
        store
            .mirror(&root, &hash("v1"), &[entry("t1", &[])], 100)
            .unwrap();
        let run = RunId::new();
        assert!(store.claim(&root, "t1", run, 200).unwrap());

        let mut e = entry("t1", &[]);
        e.priority = Priority::Critical;
        store.mirror(&root, &hash("v2"), &[e], 300).unwrap();

        let row = store.get(&root, "t1").unwrap().unwrap();
        assert_eq!(row.priority, Priority::Critical);
        assert_eq!(row.roadmap_hash, hash("v2"));
        // Execution columns survived the mirror.
        assert_eq!(row.dispatch_state, DispatchState::Dispatched);
        assert_eq!(row.run_id, Some(run));
        assert_eq!(row.enqueued_at, 100, "enqueue time is first-write-wins");
        assert_eq!(row.attempt, 1);
    }

    #[test]
    fn mirror_absent_row_is_kept() {
        let store = store();
        let root = PathBuf::from("/proj");
        store
            .mirror(
                &root,
                &hash("v1"),
                &[entry("t1", &[]), entry("t2", &[])],
                100,
            )
            .unwrap();
        store
            .mirror(&root, &hash("v2"), &[entry("t1", &[])], 200)
            .unwrap();
        assert!(store.get(&root, "t2").unwrap().is_some());
    }

    #[test]
    fn claim_is_single_flight() {
        let store = store();
        let root = PathBuf::from("/proj");
        store
            .mirror(&root, &hash("v1"), &[entry("t1", &[])], 100)
            .unwrap();
        let first = RunId::new();
        let second = RunId::new();
        assert!(store.claim(&root, "t1", first, 200).unwrap());
        assert!(
            !store.claim(&root, "t1", second, 201).unwrap(),
            "second claim must lose the race"
        );
        let row = store.get(&root, "t1").unwrap().unwrap();
        assert_eq!(row.run_id, Some(first));
    }

    #[test]
    fn failed_dependency_blocks_and_done_completes() {
        let store = store();
        let root = PathBuf::from("/proj");
        store
            .mirror(
                &root,
                &hash("v1"),
                &[entry("t1", &[]), entry("t2", &["t1"])],
                100,
            )
            .unwrap();
        let states = store.dependency_states(&root).unwrap();
        assert_eq!(states["t2"]["t1"], surge_core::RoadmapStatus::Pending);

        store.claim(&root, "t1", RunId::new(), 200).unwrap();
        store
            .settle(&root, "t1", DispatchState::Failed, 300)
            .unwrap();
        let states = store.dependency_states(&root).unwrap();
        assert_eq!(states["t2"]["t1"], surge_core::RoadmapStatus::Failed);
    }

    #[test]
    fn requeue_bumps_attempt_and_clears_run() {
        let store = store();
        let root = PathBuf::from("/proj");
        store
            .mirror(&root, &hash("v1"), &[entry("t1", &[])], 100)
            .unwrap();
        let run = RunId::new();
        store.claim(&root, "t1", run, 200).unwrap();
        assert!(store.requeue(&root, "t1", 300).unwrap());
        let row = store.get(&root, "t1").unwrap().unwrap();
        assert_eq!(row.dispatch_state, DispatchState::Queued);
        assert_eq!(row.run_id, None);
        assert_eq!(row.attempt, 2);
        assert!(store.claim(&root, "t1", RunId::new(), 400).unwrap());
    }

    #[test]
    fn settle_only_matches_dispatched_rows() {
        let store = store();
        let root = PathBuf::from("/proj");
        store
            .mirror(&root, &hash("v1"), &[entry("t1", &[])], 100)
            .unwrap();
        assert!(
            !store.settle(&root, "t1", DispatchState::Done, 200).unwrap(),
            "a queued row was never dispatched and must not settle"
        );
        store.claim(&root, "t1", RunId::new(), 300).unwrap();
        assert!(store.settle(&root, "t1", DispatchState::Done, 400).unwrap());
        assert!(
            !store
                .settle(&root, "t1", DispatchState::Failed, 500)
                .unwrap()
        );
    }

    #[test]
    fn record_skips_increments_all_but_the_chosen() {
        let store = store();
        let root = PathBuf::from("/proj");
        store
            .mirror(
                &root,
                &hash("v1"),
                &[entry("t1", &[]), entry("t2", &[]), entry("t3", &[])],
                100,
            )
            .unwrap();
        let skipped = store.record_skips(&root, &["t1".to_string()], 200).unwrap();
        assert_eq!(skipped, 2);
        let t1 = store.get(&root, "t1").unwrap().unwrap();
        let t2 = store.get(&root, "t2").unwrap().unwrap();
        assert_eq!(t1.skipped_dispatches, 0);
        assert_eq!(t2.skipped_dispatches, 1);
    }

    #[test]
    fn pause_flag_round_trips_and_defaults_false() {
        let store = store();
        let root = PathBuf::from("/proj");
        assert!(!store.is_project_paused(&root).unwrap());
        store.set_project_paused(&root, true, 100).unwrap();
        assert!(store.is_project_paused(&root).unwrap());
        store.set_project_paused(&root, false, 200).unwrap();
        assert!(!store.is_project_paused(&root).unwrap());
    }

    #[test]
    fn projects_lists_distinct_roots_with_pause_state() {
        let store = store();
        let a = PathBuf::from("/a");
        let b = PathBuf::from("/b");
        store
            .mirror(&a, &hash("v1"), &[entry("t1", &[])], 100)
            .unwrap();
        store
            .mirror(&b, &hash("v1"), &[entry("t1", &[])], 100)
            .unwrap();
        store.set_project_paused(&b, true, 200).unwrap();
        let projects = store.projects().unwrap();
        assert_eq!(
            projects,
            vec![(a, false), (b, true)],
            "ordered by root, pause state joined"
        );
    }

    #[test]
    fn list_filters_by_state_and_run() {
        let store = store();
        let root = PathBuf::from("/proj");
        store
            .mirror(
                &root,
                &hash("v1"),
                &[entry("t1", &[]), entry("t2", &[])],
                100,
            )
            .unwrap();
        let run = RunId::new();
        store.claim(&root, "t1", run, 200).unwrap();

        let dispatched = store
            .list(&TaskQueueFilter {
                project_root: Some(root.clone()),
                dispatch_state: Some(DispatchState::Dispatched),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(dispatched.len(), 1);
        assert_eq!(dispatched[0].task_id, "t1");

        let by_run = store
            .list(&TaskQueueFilter {
                run_id: Some(run),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(by_run.len(), 1);
    }
}
