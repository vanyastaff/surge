//! SQLite-based storage for token usage data.

use crate::models::{CircuitBreakerState, SessionUsage, SpecUsage, SubtaskUsage};
use crate::{PersistenceError, Result};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::{Path, PathBuf};
use surge_core::id::{SpecId, SubtaskId, TaskId};
use surge_core::state::TaskState;

// ── Schema Constants ────────────────────────────────────────────────

const SCHEMA_VERSION: i32 = 1;

const CREATE_SCHEMA_VERSION_TABLE: &str = r#"
CREATE TABLE IF NOT EXISTS schema_version (
    version INTEGER PRIMARY KEY
)
"#;

const CREATE_SESSION_USAGE_TABLE: &str = r#"
CREATE TABLE IF NOT EXISTS session_usage (
    session_id TEXT PRIMARY KEY,
    agent_name TEXT NOT NULL,
    task_id TEXT NOT NULL,
    subtask_id TEXT,
    spec_id TEXT NOT NULL,
    timestamp_ms INTEGER NOT NULL,
    input_tokens INTEGER NOT NULL,
    output_tokens INTEGER NOT NULL,
    thought_tokens INTEGER,
    cached_read_tokens INTEGER,
    cached_write_tokens INTEGER,
    estimated_cost_usd REAL
)
"#;

const CREATE_SUBTASK_USAGE_TABLE: &str = r#"
CREATE TABLE IF NOT EXISTS subtask_usage (
    subtask_id TEXT NOT NULL,
    task_id TEXT NOT NULL,
    spec_id TEXT NOT NULL,
    input_tokens INTEGER NOT NULL,
    output_tokens INTEGER NOT NULL,
    thought_tokens INTEGER NOT NULL,
    cached_read_tokens INTEGER NOT NULL,
    cached_write_tokens INTEGER NOT NULL,
    estimated_cost_usd REAL NOT NULL,
    session_count INTEGER NOT NULL,
    PRIMARY KEY (subtask_id, task_id, spec_id)
)
"#;

const CREATE_SPEC_USAGE_TABLE: &str = r#"
CREATE TABLE IF NOT EXISTS spec_usage (
    spec_id TEXT PRIMARY KEY,
    input_tokens INTEGER NOT NULL,
    output_tokens INTEGER NOT NULL,
    thought_tokens INTEGER NOT NULL,
    cached_read_tokens INTEGER NOT NULL,
    cached_write_tokens INTEGER NOT NULL,
    estimated_cost_usd REAL NOT NULL,
    subtask_count INTEGER NOT NULL,
    session_count INTEGER NOT NULL
)
"#;

const CREATE_SESSION_SPEC_INDEX: &str =
    "CREATE INDEX IF NOT EXISTS idx_session_spec ON session_usage(spec_id)";

const CREATE_SESSION_SUBTASK_INDEX: &str =
    "CREATE INDEX IF NOT EXISTS idx_session_subtask ON session_usage(subtask_id)";

const CREATE_SESSION_TIMESTAMP_INDEX: &str =
    "CREATE INDEX IF NOT EXISTS idx_session_timestamp ON session_usage(timestamp_ms)";

const CREATE_SUBTASK_SPEC_INDEX: &str =
    "CREATE INDEX IF NOT EXISTS idx_subtask_spec ON subtask_usage(spec_id)";

const CREATE_TASK_STATE_TABLE: &str = r#"
CREATE TABLE IF NOT EXISTS task_state (
    task_id TEXT PRIMARY KEY,
    spec_id TEXT NOT NULL,
    state_json TEXT NOT NULL,
    updated_at INTEGER NOT NULL
)
"#;

const CREATE_TASK_STATE_SPEC_INDEX: &str =
    "CREATE INDEX IF NOT EXISTS idx_task_state_spec ON task_state(spec_id)";

const CREATE_CIRCUIT_BREAKER_TABLE: &str = r#"
CREATE TABLE IF NOT EXISTS circuit_breaker (
    task_id TEXT NOT NULL,
    subtask_id TEXT NOT NULL,
    consecutive_failures INTEGER NOT NULL,
    last_error TEXT,
    tripped_at INTEGER,
    next_retry_time INTEGER,
    PRIMARY KEY (task_id, subtask_id)
)
"#;

const CREATE_CIRCUIT_BREAKER_TASK_INDEX: &str =
    "CREATE INDEX IF NOT EXISTS idx_circuit_breaker_task ON circuit_breaker(task_id)";

const CREATE_CIRCUIT_BREAKER_TRIPPED_INDEX: &str =
    "CREATE INDEX IF NOT EXISTS idx_circuit_breaker_tripped ON circuit_breaker(tripped_at)";

// ── Store ───────────────────────────────────────────────────────────

/// SQLite-based storage for token usage data.
///
/// Provides persistent storage for session, subtask, and spec-level token
/// usage data using SQLite. Handles schema creation, migrations, and CRUD
/// operations.
pub struct Store {
    conn: Connection,
    path: PathBuf,
}

impl Store {
    /// Open or create a store at the given path.
    ///
    /// Creates the database file and initializes the schema if it doesn't exist.
    /// If the database exists, verifies the schema version.
    pub fn open(path: &Path) -> Result<Self> {
        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let conn = Connection::open(path)?;
        let mut store = Self {
            conn,
            path: path.to_path_buf(),
        };

        store.initialize_schema()?;
        Ok(store)
    }

    /// Create an in-memory store (for testing).
    pub fn in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        let mut store = Self {
            conn,
            path: PathBuf::from(":memory:"),
        };

        store.initialize_schema()?;
        Ok(store)
    }

    /// Get the path to the default store location (~/.surge/usage.db).
    pub fn default_path() -> Result<PathBuf> {
        let home = dirs::home_dir()
            .ok_or_else(|| PersistenceError::Storage("Cannot determine home directory".into()))?;
        Ok(home.join(".surge").join("usage.db"))
    }

    /// Initialize or verify the database schema.
    fn initialize_schema(&mut self) -> Result<()> {
        // Create schema version table
        self.conn.execute(CREATE_SCHEMA_VERSION_TABLE, [])?;

        // Check current schema version
        let current_version: Option<i32> = self
            .conn
            .query_row(
                "SELECT version FROM schema_version ORDER BY version DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()?;

        if let Some(version) = current_version {
            if version > SCHEMA_VERSION {
                return Err(PersistenceError::Storage(format!(
                    "Database schema version {version} is newer than supported version {SCHEMA_VERSION}"
                )));
            }
            // Future: handle migrations here if version < SCHEMA_VERSION
        } else {
            // Initialize new database
            self.conn.execute(CREATE_SESSION_USAGE_TABLE, [])?;
            self.conn.execute(CREATE_SUBTASK_USAGE_TABLE, [])?;
            self.conn.execute(CREATE_SPEC_USAGE_TABLE, [])?;
            self.conn.execute(CREATE_TASK_STATE_TABLE, [])?;
            self.conn.execute(CREATE_CIRCUIT_BREAKER_TABLE, [])?;
            self.conn.execute(CREATE_SESSION_SPEC_INDEX, [])?;
            self.conn.execute(CREATE_SESSION_SUBTASK_INDEX, [])?;
            self.conn.execute(CREATE_SESSION_TIMESTAMP_INDEX, [])?;
            self.conn.execute(CREATE_SUBTASK_SPEC_INDEX, [])?;
            self.conn.execute(CREATE_TASK_STATE_SPEC_INDEX, [])?;
            self.conn.execute(CREATE_CIRCUIT_BREAKER_TASK_INDEX, [])?;
            self.conn.execute(CREATE_CIRCUIT_BREAKER_TRIPPED_INDEX, [])?;

            self.conn.execute(
                "INSERT INTO schema_version (version) VALUES (?1)",
                [SCHEMA_VERSION],
            )?;
        }

        Ok(())
    }

    // ── Session Usage Operations ────────────────────────────────────

    /// Insert a new session usage record.
    ///
    /// If a record with the same session_id already exists, it will be replaced.
    pub fn insert_session(&mut self, session: &SessionUsage) -> Result<()> {
        self.conn.execute(
            r#"
            INSERT OR REPLACE INTO session_usage (
                session_id, agent_name, task_id, subtask_id, spec_id,
                timestamp_ms, input_tokens, output_tokens, thought_tokens,
                cached_read_tokens, cached_write_tokens, estimated_cost_usd
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
            "#,
            params![
                session.session_id,
                session.agent_name,
                session.task_id.to_string(),
                session.subtask_id.as_ref().map(ToString::to_string),
                session.spec_id.to_string(),
                session.timestamp_ms as i64,
                session.input_tokens as i64,
                session.output_tokens as i64,
                session.thought_tokens.map(|t| t as i64),
                session.cached_read_tokens.map(|t| t as i64),
                session.cached_write_tokens.map(|t| t as i64),
                session.estimated_cost_usd,
            ],
        )?;
        Ok(())
    }

    /// Get a session usage record by session ID.
    pub fn get_session(&self, session_id: &str) -> Result<Option<SessionUsage>> {
        self.conn
            .query_row(
                "SELECT * FROM session_usage WHERE session_id = ?1",
                [session_id],
                |row| {
                    Ok(SessionUsage {
                        session_id: row.get(0)?,
                        agent_name: row.get(1)?,
                        task_id: row.get::<_, String>(2)?.parse().unwrap(),
                        subtask_id: row.get::<_, Option<String>>(3)?.map(|s| s.parse().unwrap()),
                        spec_id: row.get::<_, String>(4)?.parse().unwrap(),
                        timestamp_ms: row.get::<_, i64>(5)? as u64,
                        input_tokens: row.get::<_, i64>(6)? as u64,
                        output_tokens: row.get::<_, i64>(7)? as u64,
                        thought_tokens: row.get::<_, Option<i64>>(8)?.map(|t| t as u64),
                        cached_read_tokens: row.get::<_, Option<i64>>(9)?.map(|t| t as u64),
                        cached_write_tokens: row.get::<_, Option<i64>>(10)?.map(|t| t as u64),
                        estimated_cost_usd: row.get(11)?,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }

    /// List all session usage records for a given spec.
    pub fn list_sessions_by_spec(&self, spec_id: SpecId) -> Result<Vec<SessionUsage>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM session_usage WHERE spec_id = ?1 ORDER BY timestamp_ms DESC")?;

        let rows = stmt.query_map([spec_id.to_string()], |row| {
            Ok(SessionUsage {
                session_id: row.get(0)?,
                agent_name: row.get(1)?,
                task_id: row.get::<_, String>(2)?.parse().unwrap(),
                subtask_id: row.get::<_, Option<String>>(3)?.map(|s| s.parse().unwrap()),
                spec_id: row.get::<_, String>(4)?.parse().unwrap(),
                timestamp_ms: row.get::<_, i64>(5)? as u64,
                input_tokens: row.get::<_, i64>(6)? as u64,
                output_tokens: row.get::<_, i64>(7)? as u64,
                thought_tokens: row.get::<_, Option<i64>>(8)?.map(|t| t as u64),
                cached_read_tokens: row.get::<_, Option<i64>>(9)?.map(|t| t as u64),
                cached_write_tokens: row.get::<_, Option<i64>>(10)?.map(|t| t as u64),
                estimated_cost_usd: row.get(11)?,
            })
        })?;

        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    /// List all session usage records for a given subtask.
    pub fn list_sessions_by_subtask(&self, subtask_id: SubtaskId) -> Result<Vec<SessionUsage>> {
        let mut stmt = self.conn.prepare(
            "SELECT * FROM session_usage WHERE subtask_id = ?1 ORDER BY timestamp_ms DESC",
        )?;

        let rows = stmt.query_map([subtask_id.to_string()], |row| {
            Ok(SessionUsage {
                session_id: row.get(0)?,
                agent_name: row.get(1)?,
                task_id: row.get::<_, String>(2)?.parse().unwrap(),
                subtask_id: row.get::<_, Option<String>>(3)?.map(|s| s.parse().unwrap()),
                spec_id: row.get::<_, String>(4)?.parse().unwrap(),
                timestamp_ms: row.get::<_, i64>(5)? as u64,
                input_tokens: row.get::<_, i64>(6)? as u64,
                output_tokens: row.get::<_, i64>(7)? as u64,
                thought_tokens: row.get::<_, Option<i64>>(8)?.map(|t| t as u64),
                cached_read_tokens: row.get::<_, Option<i64>>(9)?.map(|t| t as u64),
                cached_write_tokens: row.get::<_, Option<i64>>(10)?.map(|t| t as u64),
                estimated_cost_usd: row.get(11)?,
            })
        })?;

        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    /// Get total cost for sessions in a time range.
    ///
    /// Sums up all `estimated_cost_usd` values from sessions with timestamps
    /// between `start_ms` (inclusive) and `end_ms` (exclusive).
    ///
    /// # Arguments
    ///
    /// * `start_ms` - Start of time range (Unix timestamp in milliseconds, inclusive)
    /// * `end_ms` - End of time range (Unix timestamp in milliseconds, exclusive)
    ///
    /// # Returns
    ///
    /// Total cost in USD for all sessions in the time range. Returns 0.0 if no
    /// sessions are found or if all sessions have `estimated_cost_usd` as NULL.
    pub fn get_cost_in_time_range(&self, start_ms: u64, end_ms: u64) -> Result<f64> {
        let cost: f64 = self
            .conn
            .query_row(
                r#"
                SELECT COALESCE(SUM(estimated_cost_usd), 0.0)
                FROM session_usage
                WHERE timestamp_ms >= ?1 AND timestamp_ms < ?2
                "#,
                params![start_ms as i64, end_ms as i64],
                |row| row.get(0),
            )?;

        Ok(cost)
    }

    // ── Subtask Usage Operations ────────────────────────────────────

    /// Insert or update a subtask usage record.
    pub fn upsert_subtask(&mut self, subtask: &SubtaskUsage) -> Result<()> {
        self.conn.execute(
            r#"
            INSERT OR REPLACE INTO subtask_usage (
                subtask_id, task_id, spec_id, input_tokens, output_tokens,
                thought_tokens, cached_read_tokens, cached_write_tokens,
                estimated_cost_usd, session_count
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
            "#,
            params![
                subtask.subtask_id.to_string(),
                subtask.task_id.to_string(),
                subtask.spec_id.to_string(),
                subtask.input_tokens as i64,
                subtask.output_tokens as i64,
                subtask.thought_tokens as i64,
                subtask.cached_read_tokens as i64,
                subtask.cached_write_tokens as i64,
                subtask.estimated_cost_usd,
                subtask.session_count as i64,
            ],
        )?;
        Ok(())
    }

    /// Get a subtask usage record by subtask ID, task ID, and spec ID.
    pub fn get_subtask(
        &self,
        subtask_id: SubtaskId,
        task_id: TaskId,
        spec_id: SpecId,
    ) -> Result<Option<SubtaskUsage>> {
        self.conn
            .query_row(
                "SELECT * FROM subtask_usage WHERE subtask_id = ?1 AND task_id = ?2 AND spec_id = ?3",
                params![
                    subtask_id.to_string(),
                    task_id.to_string(),
                    spec_id.to_string()
                ],
                |row| {
                    Ok(SubtaskUsage {
                        subtask_id: row.get::<_, String>(0)?.parse().unwrap(),
                        task_id: row.get::<_, String>(1)?.parse().unwrap(),
                        spec_id: row.get::<_, String>(2)?.parse().unwrap(),
                        input_tokens: row.get::<_, i64>(3)? as u64,
                        output_tokens: row.get::<_, i64>(4)? as u64,
                        thought_tokens: row.get::<_, i64>(5)? as u64,
                        cached_read_tokens: row.get::<_, i64>(6)? as u64,
                        cached_write_tokens: row.get::<_, i64>(7)? as u64,
                        estimated_cost_usd: row.get(8)?,
                        session_count: row.get::<_, i64>(9)? as u32,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }

    /// List all subtask usage records for a given spec.
    pub fn list_subtasks_by_spec(&self, spec_id: SpecId) -> Result<Vec<SubtaskUsage>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM subtask_usage WHERE spec_id = ?1")?;

        let rows = stmt.query_map([spec_id.to_string()], |row| {
            Ok(SubtaskUsage {
                subtask_id: row.get::<_, String>(0)?.parse().unwrap(),
                task_id: row.get::<_, String>(1)?.parse().unwrap(),
                spec_id: row.get::<_, String>(2)?.parse().unwrap(),
                input_tokens: row.get::<_, i64>(3)? as u64,
                output_tokens: row.get::<_, i64>(4)? as u64,
                thought_tokens: row.get::<_, i64>(5)? as u64,
                cached_read_tokens: row.get::<_, i64>(6)? as u64,
                cached_write_tokens: row.get::<_, i64>(7)? as u64,
                estimated_cost_usd: row.get(8)?,
                session_count: row.get::<_, i64>(9)? as u32,
            })
        })?;

        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    // ── Spec Usage Operations ───────────────────────────────────────

    /// Insert or update a spec usage record.
    pub fn upsert_spec(&mut self, spec: &SpecUsage) -> Result<()> {
        self.conn.execute(
            r#"
            INSERT OR REPLACE INTO spec_usage (
                spec_id, input_tokens, output_tokens, thought_tokens,
                cached_read_tokens, cached_write_tokens, estimated_cost_usd,
                subtask_count, session_count
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
            "#,
            params![
                spec.spec_id.to_string(),
                spec.input_tokens as i64,
                spec.output_tokens as i64,
                spec.thought_tokens as i64,
                spec.cached_read_tokens as i64,
                spec.cached_write_tokens as i64,
                spec.estimated_cost_usd,
                spec.subtask_count as i64,
                spec.session_count as i64,
            ],
        )?;
        Ok(())
    }

    /// Get a spec usage record by spec ID.
    pub fn get_spec(&self, spec_id: SpecId) -> Result<Option<SpecUsage>> {
        self.conn
            .query_row(
                "SELECT * FROM spec_usage WHERE spec_id = ?1",
                [spec_id.to_string()],
                |row| {
                    Ok(SpecUsage {
                        spec_id: row.get::<_, String>(0)?.parse().unwrap(),
                        input_tokens: row.get::<_, i64>(1)? as u64,
                        output_tokens: row.get::<_, i64>(2)? as u64,
                        thought_tokens: row.get::<_, i64>(3)? as u64,
                        cached_read_tokens: row.get::<_, i64>(4)? as u64,
                        cached_write_tokens: row.get::<_, i64>(5)? as u64,
                        estimated_cost_usd: row.get(6)?,
                        subtask_count: row.get::<_, i64>(7)? as u32,
                        session_count: row.get::<_, i64>(8)? as u32,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }

    /// List all spec usage records.
    pub fn list_all_specs(&self) -> Result<Vec<SpecUsage>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM spec_usage ORDER BY estimated_cost_usd DESC")?;

        let rows = stmt.query_map([], |row| {
            Ok(SpecUsage {
                spec_id: row.get::<_, String>(0)?.parse().unwrap(),
                input_tokens: row.get::<_, i64>(1)? as u64,
                output_tokens: row.get::<_, i64>(2)? as u64,
                thought_tokens: row.get::<_, i64>(3)? as u64,
                cached_read_tokens: row.get::<_, i64>(4)? as u64,
                cached_write_tokens: row.get::<_, i64>(5)? as u64,
                estimated_cost_usd: row.get(6)?,
                subtask_count: row.get::<_, i64>(7)? as u32,
                session_count: row.get::<_, i64>(8)? as u32,
            })
        })?;

        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    // ── Task State Operations ───────────────────────────────────────

    /// Save a task state checkpoint.
    ///
    /// Creates or updates a checkpoint for the given task, storing its current
    /// state for later resumption. The state is serialized as JSON.
    pub fn checkpoint_task_state(
        &mut self,
        task_id: TaskId,
        spec_id: SpecId,
        state: &TaskState,
    ) -> Result<()> {
        let state_json = serde_json::to_string(state).map_err(|e| {
            PersistenceError::Storage(format!("Failed to serialize task state: {e}"))
        })?;

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| PersistenceError::Storage(format!("System time error: {e}")))?
            .as_millis() as i64;

        self.conn.execute(
            r#"
            INSERT OR REPLACE INTO task_state (
                task_id, spec_id, state_json, updated_at
            ) VALUES (?1, ?2, ?3, ?4)
            "#,
            params![task_id.to_string(), spec_id.to_string(), state_json, now,],
        )?;
        Ok(())
    }

    /// Resume a task from its last checkpoint.
    ///
    /// Retrieves the most recent task state for the given task ID. Returns
    /// `None` if no checkpoint exists for the task.
    pub fn resume_task_state(&self, task_id: TaskId) -> Result<Option<(SpecId, TaskState)>> {
        self.conn
            .query_row(
                "SELECT spec_id, state_json FROM task_state WHERE task_id = ?1",
                [task_id.to_string()],
                |row| {
                    let spec_id: String = row.get(0)?;
                    let state_json: String = row.get(1)?;
                    Ok((spec_id, state_json))
                },
            )
            .optional()?
            .map(|(spec_id_str, state_json)| {
                let spec_id = spec_id_str
                    .parse()
                    .map_err(|e| PersistenceError::Storage(format!("Invalid spec_id: {e}")))?;
                let state: TaskState = serde_json::from_str(&state_json).map_err(|e| {
                    PersistenceError::Storage(format!("Failed to deserialize task state: {e}"))
                })?;
                Ok((spec_id, state))
            })
            .transpose()
    }

    /// Get all task checkpoints for a given spec.
    pub fn list_task_states_by_spec(
        &self,
        spec_id: SpecId,
    ) -> Result<Vec<(TaskId, TaskState, u64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT task_id, state_json, updated_at FROM task_state WHERE spec_id = ?1 ORDER BY updated_at DESC",
        )?;

        let rows = stmt.query_map([spec_id.to_string()], |row| {
            let task_id_str: String = row.get(0)?;
            let state_json: String = row.get(1)?;
            let updated_at: i64 = row.get(2)?;
            Ok((task_id_str, state_json, updated_at as u64))
        })?;

        rows.map(|row| {
            let (task_id_str, state_json, updated_at) = row?;
            let task_id = task_id_str
                .parse()
                .map_err(|e| PersistenceError::Storage(format!("Invalid task_id: {e}")))?;
            let state: TaskState = serde_json::from_str(&state_json).map_err(|e| {
                PersistenceError::Storage(format!("Failed to deserialize task state: {e}"))
            })?;
            Ok((task_id, state, updated_at))
        })
        .collect()
    }

    // ── Circuit Breaker Operations ──────────────────────────────────

    /// Save circuit breaker state for a subtask.
    ///
    /// Creates or updates the circuit breaker state, allowing persistence
    /// across restarts to prevent infinite retry loops.
    pub fn save_circuit_breaker_state(&mut self, state: &CircuitBreakerState) -> Result<()> {
        self.conn.execute(
            r#"
            INSERT OR REPLACE INTO circuit_breaker (
                task_id, subtask_id, consecutive_failures, last_error,
                tripped_at, next_retry_time
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            "#,
            params![
                state.task_id.to_string(),
                state.subtask_id.to_string(),
                state.consecutive_failures as i64,
                state.last_error.as_ref(),
                state.tripped_at.map(|t| t as i64),
                state.next_retry_time.map(|t| t as i64),
            ],
        )?;
        Ok(())
    }

    /// Load circuit breaker state for a specific subtask.
    ///
    /// Returns `None` if no circuit breaker state exists for the given task/subtask.
    pub fn load_circuit_breaker_state(
        &self,
        task_id: TaskId,
        subtask_id: SubtaskId,
    ) -> Result<Option<CircuitBreakerState>> {
        self.conn
            .query_row(
                r#"
                SELECT consecutive_failures, last_error, tripped_at, next_retry_time
                FROM circuit_breaker
                WHERE task_id = ?1 AND subtask_id = ?2
                "#,
                params![task_id.to_string(), subtask_id.to_string()],
                |row| {
                    Ok(CircuitBreakerState {
                        task_id,
                        subtask_id,
                        consecutive_failures: row.get::<_, i64>(0)? as u32,
                        last_error: row.get(1)?,
                        tripped_at: row.get::<_, Option<i64>>(2)?.map(|t| t as u64),
                        next_retry_time: row.get::<_, Option<i64>>(3)?.map(|t| t as u64),
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }

    /// Delete circuit breaker state for a specific subtask.
    ///
    /// Used when resetting a circuit breaker after successful execution.
    pub fn delete_circuit_breaker_state(
        &mut self,
        task_id: TaskId,
        subtask_id: SubtaskId,
    ) -> Result<()> {
        self.conn.execute(
            "DELETE FROM circuit_breaker WHERE task_id = ?1 AND subtask_id = ?2",
            params![task_id.to_string(), subtask_id.to_string()],
        )?;
        Ok(())
    }

    /// List all circuit breaker states for a given task.
    ///
    /// Returns all circuit breaker states associated with the task,
    /// useful for debugging and monitoring.
    pub fn list_circuit_breaker_states_by_task(
        &self,
        task_id: TaskId,
    ) -> Result<Vec<CircuitBreakerState>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT subtask_id, consecutive_failures, last_error, tripped_at, next_retry_time
            FROM circuit_breaker
            WHERE task_id = ?1
            ORDER BY tripped_at DESC
            "#,
        )?;

        let rows = stmt.query_map([task_id.to_string()], |row| {
            let subtask_id_str: String = row.get(0)?;
            Ok((
                subtask_id_str,
                row.get::<_, i64>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<i64>>(3)?,
                row.get::<_, Option<i64>>(4)?,
            ))
        })?;

        rows.map(|row| {
            let (subtask_id_str, consecutive_failures, last_error, tripped_at, next_retry_time) =
                row?;
            let subtask_id = subtask_id_str
                .parse()
                .map_err(|e| PersistenceError::Storage(format!("Invalid subtask_id: {e}")))?;

            Ok(CircuitBreakerState {
                task_id,
                subtask_id,
                consecutive_failures: consecutive_failures as u32,
                last_error,
                tripped_at: tripped_at.map(|t| t as u64),
                next_retry_time: next_retry_time.map(|t| t as u64),
            })
        })
        .collect()
    }

    // ── Utility Operations ──────────────────────────────────────────

    /// Get the path to the store file.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Get total session count across all specs.
    pub fn total_session_count(&self) -> Result<u64> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM session_usage", [], |row| row.get(0))?;
        Ok(count as u64)
    }

    /// Get total estimated cost across all specs.
    pub fn total_estimated_cost(&self) -> Result<f64> {
        let cost: f64 = self.conn.query_row(
            "SELECT COALESCE(SUM(estimated_cost_usd), 0.0) FROM spec_usage",
            [],
            |row| row.get(0),
        )?;
        Ok(cost)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_session() -> SessionUsage {
        SessionUsage {
            session_id: "sess-1".to_string(),
            agent_name: "claude".to_string(),
            task_id: TaskId::new(),
            subtask_id: Some(SubtaskId::new()),
            spec_id: SpecId::new(),
            timestamp_ms: 1_700_000_000_000,
            input_tokens: 1000,
            output_tokens: 500,
            thought_tokens: Some(200),
            cached_read_tokens: Some(100),
            cached_write_tokens: Some(50),
            estimated_cost_usd: Some(0.005),
        }
    }

    #[test]
    fn test_store_in_memory() {
        let store = Store::in_memory().unwrap();
        assert_eq!(store.path(), Path::new(":memory:"));
    }

    #[test]
    fn test_insert_and_get_session() {
        let mut store = Store::in_memory().unwrap();
        let session = sample_session();

        store.insert_session(&session).unwrap();
        let retrieved = store.get_session(&session.session_id).unwrap().unwrap();

        assert_eq!(retrieved.session_id, session.session_id);
        assert_eq!(retrieved.agent_name, session.agent_name);
        assert_eq!(retrieved.input_tokens, session.input_tokens);
        assert_eq!(retrieved.output_tokens, session.output_tokens);
        assert_eq!(retrieved.thought_tokens, session.thought_tokens);
        assert_eq!(retrieved.estimated_cost_usd, session.estimated_cost_usd);
    }

    #[test]
    fn test_insert_duplicate_session_replaces() {
        let mut store = Store::in_memory().unwrap();
        let mut session = sample_session();

        store.insert_session(&session).unwrap();

        // Update and re-insert
        session.input_tokens = 2000;
        store.insert_session(&session).unwrap();

        let retrieved = store.get_session(&session.session_id).unwrap().unwrap();
        assert_eq!(retrieved.input_tokens, 2000);

        // Should only have one record
        let count = store.total_session_count().unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_list_sessions_by_spec() {
        let mut store = Store::in_memory().unwrap();
        let spec_id = SpecId::new();

        let mut session1 = sample_session();
        session1.session_id = "sess-1".to_string();
        session1.spec_id = spec_id;

        let mut session2 = sample_session();
        session2.session_id = "sess-2".to_string();
        session2.spec_id = spec_id;

        let mut session3 = sample_session();
        session3.session_id = "sess-3".to_string();
        session3.spec_id = SpecId::new(); // Different spec

        store.insert_session(&session1).unwrap();
        store.insert_session(&session2).unwrap();
        store.insert_session(&session3).unwrap();

        let sessions = store.list_sessions_by_spec(spec_id).unwrap();
        assert_eq!(sessions.len(), 2);
    }

    #[test]
    fn test_list_sessions_by_subtask() {
        let mut store = Store::in_memory().unwrap();
        let subtask_id = SubtaskId::new();

        let mut session1 = sample_session();
        session1.session_id = "sess-1".to_string();
        session1.subtask_id = Some(subtask_id);

        let mut session2 = sample_session();
        session2.session_id = "sess-2".to_string();
        session2.subtask_id = Some(subtask_id);

        let mut session3 = sample_session();
        session3.session_id = "sess-3".to_string();
        session3.subtask_id = Some(SubtaskId::new()); // Different subtask

        store.insert_session(&session1).unwrap();
        store.insert_session(&session2).unwrap();
        store.insert_session(&session3).unwrap();

        let sessions = store.list_sessions_by_subtask(subtask_id).unwrap();
        assert_eq!(sessions.len(), 2);
    }

    #[test]
    fn test_upsert_and_get_subtask() {
        let mut store = Store::in_memory().unwrap();
        let session = sample_session();
        let subtask = SubtaskUsage::from_session(&session);

        store.upsert_subtask(&subtask).unwrap();
        let retrieved = store
            .get_subtask(subtask.subtask_id, subtask.task_id, subtask.spec_id)
            .unwrap()
            .unwrap();

        assert_eq!(retrieved.subtask_id, subtask.subtask_id);
        assert_eq!(retrieved.input_tokens, subtask.input_tokens);
        assert_eq!(retrieved.output_tokens, subtask.output_tokens);
        assert_eq!(retrieved.session_count, subtask.session_count);
    }

    #[test]
    fn test_upsert_subtask_updates_existing() {
        let mut store = Store::in_memory().unwrap();
        let session = sample_session();
        let mut subtask = SubtaskUsage::from_session(&session);

        store.upsert_subtask(&subtask).unwrap();

        // Update and re-upsert
        subtask.input_tokens = 2000;
        subtask.session_count = 2;
        store.upsert_subtask(&subtask).unwrap();

        let retrieved = store
            .get_subtask(subtask.subtask_id, subtask.task_id, subtask.spec_id)
            .unwrap()
            .unwrap();
        assert_eq!(retrieved.input_tokens, 2000);
        assert_eq!(retrieved.session_count, 2);
    }

    #[test]
    fn test_list_subtasks_by_spec() {
        let mut store = Store::in_memory().unwrap();
        let spec_id = SpecId::new();

        let mut session1 = sample_session();
        session1.session_id = "sess-1".to_string();
        session1.spec_id = spec_id;
        session1.subtask_id = Some(SubtaskId::new());

        let mut session2 = sample_session();
        session2.session_id = "sess-2".to_string();
        session2.spec_id = spec_id;
        session2.subtask_id = Some(SubtaskId::new());

        let subtask1 = SubtaskUsage::from_session(&session1);
        let subtask2 = SubtaskUsage::from_session(&session2);

        store.upsert_subtask(&subtask1).unwrap();
        store.upsert_subtask(&subtask2).unwrap();

        let subtasks = store.list_subtasks_by_spec(spec_id).unwrap();
        assert_eq!(subtasks.len(), 2);
    }

    #[test]
    fn test_upsert_and_get_spec() {
        let mut store = Store::in_memory().unwrap();
        let session = sample_session();
        let spec = SpecUsage::from_session(&session);

        store.upsert_spec(&spec).unwrap();
        let retrieved = store.get_spec(spec.spec_id).unwrap().unwrap();

        assert_eq!(retrieved.spec_id, spec.spec_id);
        assert_eq!(retrieved.input_tokens, spec.input_tokens);
        assert_eq!(retrieved.output_tokens, spec.output_tokens);
        assert_eq!(retrieved.session_count, spec.session_count);
    }

    #[test]
    fn test_upsert_spec_updates_existing() {
        let mut store = Store::in_memory().unwrap();
        let session = sample_session();
        let mut spec = SpecUsage::from_session(&session);

        store.upsert_spec(&spec).unwrap();

        // Update and re-upsert
        spec.input_tokens = 3000;
        spec.session_count = 3;
        store.upsert_spec(&spec).unwrap();

        let retrieved = store.get_spec(spec.spec_id).unwrap().unwrap();
        assert_eq!(retrieved.input_tokens, 3000);
        assert_eq!(retrieved.session_count, 3);
    }

    #[test]
    fn test_list_all_specs() {
        let mut store = Store::in_memory().unwrap();

        let mut session1 = sample_session();
        session1.session_id = "sess-1".to_string();
        session1.spec_id = SpecId::new();
        session1.estimated_cost_usd = Some(0.01);

        let mut session2 = sample_session();
        session2.session_id = "sess-2".to_string();
        session2.spec_id = SpecId::new();
        session2.estimated_cost_usd = Some(0.05);

        let spec1 = SpecUsage::from_session(&session1);
        let spec2 = SpecUsage::from_session(&session2);

        store.upsert_spec(&spec1).unwrap();
        store.upsert_spec(&spec2).unwrap();

        let specs = store.list_all_specs().unwrap();
        assert_eq!(specs.len(), 2);
        // Should be ordered by cost descending
        assert_eq!(specs[0].estimated_cost_usd, 0.05);
        assert_eq!(specs[1].estimated_cost_usd, 0.01);
    }

    #[test]
    fn test_total_session_count() {
        let mut store = Store::in_memory().unwrap();
        let session1 = sample_session();
        let mut session2 = sample_session();
        session2.session_id = "sess-2".to_string();

        store.insert_session(&session1).unwrap();
        store.insert_session(&session2).unwrap();

        let count = store.total_session_count().unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn test_total_estimated_cost() {
        let mut store = Store::in_memory().unwrap();

        let mut session1 = sample_session();
        session1.session_id = "sess-1".to_string();
        session1.spec_id = SpecId::new();
        session1.estimated_cost_usd = Some(0.01);

        let mut session2 = sample_session();
        session2.session_id = "sess-2".to_string();
        session2.spec_id = SpecId::new();
        session2.estimated_cost_usd = Some(0.05);

        let spec1 = SpecUsage::from_session(&session1);
        let spec2 = SpecUsage::from_session(&session2);

        store.upsert_spec(&spec1).unwrap();
        store.upsert_spec(&spec2).unwrap();

        let total_cost = store.total_estimated_cost().unwrap();
        assert!((total_cost - 0.06).abs() < f64::EPSILON);
    }

    #[test]
    fn test_get_nonexistent_session_returns_none() {
        let store = Store::in_memory().unwrap();
        let result = store.get_session("nonexistent").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_get_nonexistent_subtask_returns_none() {
        let store = Store::in_memory().unwrap();
        let result = store
            .get_subtask(SubtaskId::new(), TaskId::new(), SpecId::new())
            .unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_get_nonexistent_spec_returns_none() {
        let store = Store::in_memory().unwrap();
        let result = store.get_spec(SpecId::new()).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_checkpoint_and_resume_task_state() {
        let mut store = Store::in_memory().unwrap();
        let task_id = TaskId::new();
        let spec_id = SpecId::new();
        let state = TaskState::Executing {
            completed: 3,
            total: 10,
        };

        // Checkpoint the task state
        store
            .checkpoint_task_state(task_id, spec_id, &state)
            .unwrap();

        // Resume the task state
        let resumed = store.resume_task_state(task_id).unwrap();
        assert!(resumed.is_some());

        let (resumed_spec_id, resumed_state) = resumed.unwrap();
        assert_eq!(resumed_spec_id, spec_id);
        assert_eq!(resumed_state, state);
    }

    #[test]
    fn test_checkpoint_replaces_existing_state() {
        let mut store = Store::in_memory().unwrap();
        let task_id = TaskId::new();
        let spec_id = SpecId::new();

        // First checkpoint
        let state1 = TaskState::Executing {
            completed: 3,
            total: 10,
        };
        store
            .checkpoint_task_state(task_id, spec_id, &state1)
            .unwrap();

        // Second checkpoint (should replace)
        let state2 = TaskState::Executing {
            completed: 7,
            total: 10,
        };
        store
            .checkpoint_task_state(task_id, spec_id, &state2)
            .unwrap();

        // Resume should get the latest state
        let (_, resumed_state) = store.resume_task_state(task_id).unwrap().unwrap();
        assert_eq!(resumed_state, state2);
    }

    #[test]
    fn test_resume_nonexistent_task_returns_none() {
        let store = Store::in_memory().unwrap();
        let result = store.resume_task_state(TaskId::new()).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_checkpoint_various_task_states() {
        let mut store = Store::in_memory().unwrap();
        let spec_id = SpecId::new();

        // Test different task states can be checkpointed and resumed
        let test_states = vec![
            TaskState::Draft,
            TaskState::Planning,
            TaskState::Planned { subtask_count: 5 },
            TaskState::Executing {
                completed: 2,
                total: 5,
            },
            TaskState::QaReview {
                verdict: None,
                reasoning: None,
            },
            TaskState::QaFix {
                iteration: 2,
                verdict: None,
                reasoning: None,
            },
            TaskState::HumanReview,
            TaskState::Merging,
            TaskState::Completed,
            TaskState::Failed {
                reason: "test error".to_string(),
            },
            TaskState::Cancelled,
        ];

        for state in test_states {
            let task_id = TaskId::new();
            store
                .checkpoint_task_state(task_id, spec_id, &state)
                .unwrap();
            let (_, resumed_state) = store.resume_task_state(task_id).unwrap().unwrap();
            assert_eq!(resumed_state, state);
        }
    }

    #[test]
    fn test_list_task_states_by_spec() {
        let mut store = Store::in_memory().unwrap();
        let spec_id = SpecId::new();

        let task1 = TaskId::new();
        let task2 = TaskId::new();
        let task3 = TaskId::new();

        // Checkpoint three tasks for the same spec
        store
            .checkpoint_task_state(
                task1,
                spec_id,
                &TaskState::Executing {
                    completed: 1,
                    total: 5,
                },
            )
            .unwrap();
        store
            .checkpoint_task_state(task2, spec_id, &TaskState::Completed)
            .unwrap();
        store
            .checkpoint_task_state(task3, spec_id, &TaskState::Planning)
            .unwrap();

        // Also checkpoint a task for a different spec (should not be included)
        let other_spec = SpecId::new();
        store
            .checkpoint_task_state(TaskId::new(), other_spec, &TaskState::Draft)
            .unwrap();

        // List tasks for the first spec
        let tasks = store.list_task_states_by_spec(spec_id).unwrap();
        assert_eq!(tasks.len(), 3);

        // Verify all task IDs are present
        let task_ids: Vec<TaskId> = tasks.iter().map(|(id, _, _)| *id).collect();
        assert!(task_ids.contains(&task1));
        assert!(task_ids.contains(&task2));
        assert!(task_ids.contains(&task3));
    }

    // ── Circuit Breaker Tests ───────────────────────────────────────

    #[test]
    fn test_save_and_load_circuit_breaker_state() {
        let mut store = Store::in_memory().unwrap();
        let task_id = TaskId::new();
        let subtask_id = SubtaskId::new();

        let mut state = CircuitBreakerState::new(task_id, subtask_id);
        state.record_failure("Test error".to_string(), Some(1_700_000_000_000));
        state.trip(1_700_000_000_000);

        store.save_circuit_breaker_state(&state).unwrap();

        let loaded = store
            .load_circuit_breaker_state(task_id, subtask_id)
            .unwrap()
            .unwrap();

        assert_eq!(loaded.task_id, task_id);
        assert_eq!(loaded.subtask_id, subtask_id);
        assert_eq!(loaded.consecutive_failures, 1);
        assert_eq!(loaded.last_error, Some("Test error".to_string()));
        assert_eq!(loaded.tripped_at, Some(1_700_000_000_000));
        assert_eq!(loaded.next_retry_time, Some(1_700_000_000_000));
        assert!(loaded.is_tripped());
    }

    #[test]
    fn test_load_nonexistent_circuit_breaker_returns_none() {
        let store = Store::in_memory().unwrap();
        let task_id = TaskId::new();
        let subtask_id = SubtaskId::new();

        let loaded = store
            .load_circuit_breaker_state(task_id, subtask_id)
            .unwrap();
        assert!(loaded.is_none());
    }

    #[test]
    fn test_save_circuit_breaker_replaces_existing() {
        let mut store = Store::in_memory().unwrap();
        let task_id = TaskId::new();
        let subtask_id = SubtaskId::new();

        // Save initial state
        let mut state = CircuitBreakerState::new(task_id, subtask_id);
        state.record_failure("First error".to_string(), None);
        store.save_circuit_breaker_state(&state).unwrap();

        // Update and save again
        state.record_failure("Second error".to_string(), Some(1_700_000_100_000));
        store.save_circuit_breaker_state(&state).unwrap();

        let loaded = store
            .load_circuit_breaker_state(task_id, subtask_id)
            .unwrap()
            .unwrap();

        assert_eq!(loaded.consecutive_failures, 2);
        assert_eq!(loaded.last_error, Some("Second error".to_string()));
        assert_eq!(loaded.next_retry_time, Some(1_700_000_100_000));
    }

    #[test]
    fn test_delete_circuit_breaker_state() {
        let mut store = Store::in_memory().unwrap();
        let task_id = TaskId::new();
        let subtask_id = SubtaskId::new();

        let state = CircuitBreakerState::new(task_id, subtask_id);
        store.save_circuit_breaker_state(&state).unwrap();

        // Verify it exists
        assert!(store
            .load_circuit_breaker_state(task_id, subtask_id)
            .unwrap()
            .is_some());

        // Delete it
        store
            .delete_circuit_breaker_state(task_id, subtask_id)
            .unwrap();

        // Verify it's gone
        assert!(store
            .load_circuit_breaker_state(task_id, subtask_id)
            .unwrap()
            .is_none());
    }

    #[test]
    fn test_list_circuit_breaker_states_by_task() {
        let mut store = Store::in_memory().unwrap();
        let task_id = TaskId::new();
        let subtask1 = SubtaskId::new();
        let subtask2 = SubtaskId::new();
        let subtask3 = SubtaskId::new();

        // Save circuit breaker states for multiple subtasks
        let mut state1 = CircuitBreakerState::new(task_id, subtask1);
        state1.trip(1_700_000_000_000);
        store.save_circuit_breaker_state(&state1).unwrap();

        let mut state2 = CircuitBreakerState::new(task_id, subtask2);
        state2.trip(1_700_000_100_000);
        store.save_circuit_breaker_state(&state2).unwrap();

        let state3 = CircuitBreakerState::new(task_id, subtask3);
        store.save_circuit_breaker_state(&state3).unwrap();

        // Also save state for a different task (should not be included)
        let other_task = TaskId::new();
        let state4 = CircuitBreakerState::new(other_task, SubtaskId::new());
        store.save_circuit_breaker_state(&state4).unwrap();

        // List states for the first task
        let states = store.list_circuit_breaker_states_by_task(task_id).unwrap();
        assert_eq!(states.len(), 3);

        // Verify all subtask IDs are present
        let subtask_ids: Vec<SubtaskId> = states.iter().map(|s| s.subtask_id).collect();
        assert!(subtask_ids.contains(&subtask1));
        assert!(subtask_ids.contains(&subtask2));
        assert!(subtask_ids.contains(&subtask3));
    }

    #[test]
    fn test_circuit_breaker_state_methods() {
        let task_id = TaskId::new();
        let subtask_id = SubtaskId::new();

        let mut state = CircuitBreakerState::new(task_id, subtask_id);
        assert!(!state.is_tripped());
        assert_eq!(state.consecutive_failures, 0);

        // Record failures
        state.record_failure("Error 1".to_string(), None);
        assert_eq!(state.consecutive_failures, 1);
        assert_eq!(state.last_error, Some("Error 1".to_string()));
        assert!(!state.is_tripped());

        state.record_failure("Error 2".to_string(), Some(1_700_000_000_000));
        assert_eq!(state.consecutive_failures, 2);
        assert_eq!(state.last_error, Some("Error 2".to_string()));
        assert_eq!(state.next_retry_time, Some(1_700_000_000_000));

        // Trip the circuit
        state.trip(1_700_000_000_000);
        assert!(state.is_tripped());
        assert_eq!(state.tripped_at, Some(1_700_000_000_000));

        // Reset the circuit
        state.reset();
        assert!(!state.is_tripped());
        assert_eq!(state.consecutive_failures, 0);
        assert_eq!(state.last_error, None);
        assert_eq!(state.tripped_at, None);
        assert_eq!(state.next_retry_time, None);
    }
}
