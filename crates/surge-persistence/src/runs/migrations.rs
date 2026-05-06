//! Forward-only migration runner with one-transaction-per-migration semantics.
//!
//! Multi-process safety: each migration runs inside its own `BEGIN EXCLUSIVE`
//! transaction. If two processes call `Storage::open` simultaneously, one
//! acquires the exclusive lock first and applies all pending migrations; the
//! second waits and then sees them already-applied.
//!
//! Resumability: if migration N fails, migrations 1..N-1 stay committed.
//! On retry the runner picks up at N.

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::runs::clock::Clock;

/// Static list of `(id, sql)` pairs applied in declaration order.
pub type MigrationSet = &'static [(&'static str, &'static str)];

/// Migrations applied to the registry DB.
pub const REGISTRY_MIGRATIONS: MigrationSet = &[
    (
        "registry-0001-initial",
        include_str!("migrations/registry/0001_initial.sql"),
    ),
    (
        "registry-0002-ticket-index",
        include_str!("migrations/registry/0002_ticket_index.sql"),
    ),
];

/// Migrations applied to each per-run DB.
pub const PER_RUN_MIGRATIONS: MigrationSet = &[(
    "per-run-0001-initial",
    include_str!("migrations/per_run/0001_initial.sql"),
)];

/// Errors from the migration runner.
#[derive(Debug, thiserror::Error)]
pub enum MigrationError {
    /// A specific migration's SQL failed to apply.
    #[error("migration {id} failed: {source}")]
    Apply {
        /// The migration id that failed.
        id: String,
        /// Underlying SQLite error.
        #[source]
        source: rusqlite::Error,
    },

    /// The `_migrations` table itself could not be created.
    #[error("could not initialize _migrations table: {0}")]
    InitTable(#[source] rusqlite::Error),
}

/// Apply the given migration set to the connection.
///
/// Idempotent: migrations already in `_migrations` are skipped. Each migration
/// runs in its own `BEGIN EXCLUSIVE` transaction for multi-process safety and
/// partial-failure resumability.
pub fn apply(
    conn: &mut Connection,
    migrations: MigrationSet,
    clock: &dyn Clock,
) -> Result<(), MigrationError> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS _migrations (
            id          TEXT    PRIMARY KEY,
            applied_at  INTEGER NOT NULL
        )",
    )
    .map_err(MigrationError::InitTable)?;

    for (id, sql) in migrations {
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Exclusive)
            .map_err(|e| MigrationError::Apply {
                id: (*id).to_string(),
                source: e,
            })?;

        let already: bool = tx
            .query_row(
                "SELECT 1 FROM _migrations WHERE id = ?",
                params![id],
                |_| Ok(true),
            )
            .optional()
            .map_err(|e| MigrationError::Apply {
                id: (*id).to_string(),
                source: e,
            })?
            .unwrap_or(false);

        if already {
            tx.commit().map_err(|e| MigrationError::Apply {
                id: (*id).to_string(),
                source: e,
            })?;
            continue;
        }

        tx.execute_batch(sql).map_err(|e| MigrationError::Apply {
            id: (*id).to_string(),
            source: e,
        })?;

        tx.execute(
            "INSERT INTO _migrations (id, applied_at) VALUES (?, ?)",
            params![id, clock.now_ms()],
        )
        .map_err(|e| MigrationError::Apply {
            id: (*id).to_string(),
            source: e,
        })?;

        tx.commit().map_err(|e| MigrationError::Apply {
            id: (*id).to_string(),
            source: e,
        })?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runs::clock::MockClock;

    #[test]
    fn apply_registry_to_fresh_db() {
        let mut conn = Connection::open_in_memory().unwrap();
        let clock = MockClock::new(1_700_000_000_000);

        apply(&mut conn, REGISTRY_MIGRATIONS, &clock).unwrap();

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='runs'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);

        let applied: i64 = conn
            .query_row("SELECT COUNT(*) FROM _migrations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(applied, REGISTRY_MIGRATIONS.len() as i64);
    }

    #[test]
    fn apply_per_run_to_fresh_db() {
        let mut conn = Connection::open_in_memory().unwrap();
        let clock = MockClock::new(1_700_000_000_000);

        apply(&mut conn, PER_RUN_MIGRATIONS, &clock).unwrap();

        for table in [
            "events",
            "stage_executions",
            "artifacts",
            "pending_approvals",
            "cost_summary",
            "graph_snapshots",
        ] {
            let n: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?",
                    params![table],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(n, 1, "table {table} missing");
        }
    }

    #[test]
    fn apply_is_idempotent() {
        let mut conn = Connection::open_in_memory().unwrap();
        let clock = MockClock::new(1_700_000_000_000);

        apply(&mut conn, REGISTRY_MIGRATIONS, &clock).unwrap();
        apply(&mut conn, REGISTRY_MIGRATIONS, &clock).unwrap();
        apply(&mut conn, REGISTRY_MIGRATIONS, &clock).unwrap();

        let applied: i64 = conn
            .query_row("SELECT COUNT(*) FROM _migrations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(applied, REGISTRY_MIGRATIONS.len() as i64);
    }

    #[test]
    fn append_only_trigger_blocks_update_on_events() {
        let mut conn = Connection::open_in_memory().unwrap();
        let clock = MockClock::new(1_700_000_000_000);

        apply(&mut conn, PER_RUN_MIGRATIONS, &clock).unwrap();

        conn.execute(
            "INSERT INTO events (timestamp, kind, payload, schema_version) VALUES (?, ?, ?, 1)",
            params![1, "Test", vec![0u8; 4]],
        )
        .unwrap();

        let err = conn
            .execute("UPDATE events SET kind = 'X' WHERE seq = 1", [])
            .unwrap_err();
        assert!(err.to_string().contains("append-only"));
    }

    #[test]
    fn append_only_trigger_blocks_delete_on_events() {
        let mut conn = Connection::open_in_memory().unwrap();
        let clock = MockClock::new(1_700_000_000_000);

        apply(&mut conn, PER_RUN_MIGRATIONS, &clock).unwrap();

        conn.execute(
            "INSERT INTO events (timestamp, kind, payload, schema_version) VALUES (?, ?, ?, 1)",
            params![1, "Test", vec![0u8; 4]],
        )
        .unwrap();

        let err = conn
            .execute("DELETE FROM events WHERE seq = 1", [])
            .unwrap_err();
        assert!(err.to_string().contains("append-only"));
    }
}
