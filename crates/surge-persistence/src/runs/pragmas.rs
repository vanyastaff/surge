//! SQLite PRAGMA application for run-DB and registry-DB connections.

use rusqlite::Connection;

/// PRAGMAs applied to every connection (writer and readers) on a per-run database.
pub const PER_RUN_PRAGMAS: &[&str] = &[
    "PRAGMA journal_mode = WAL",
    "PRAGMA synchronous = NORMAL",
    "PRAGMA temp_store = MEMORY",
    "PRAGMA mmap_size = 30000000000",
    "PRAGMA cache_size = -32000",
    "PRAGMA foreign_keys = ON",
    "PRAGMA wal_autocheckpoint = 1000",
];

/// PRAGMAs applied to the registry DB connection.
pub const REGISTRY_PRAGMAS: &[&str] = &[
    "PRAGMA journal_mode = WAL",
    "PRAGMA synchronous = NORMAL",
    "PRAGMA foreign_keys = ON",
];

/// PRAGMAs applied to the cross-run memory store.
///
/// WAL for the same reason the other two lists carry it: the store is opened
/// by more than one component — `engine::hooks::memory_writeback` on a stage
/// outcome, `project_context` at run start, `surge memory audit` from the CLI
/// — and the default rollback journal serialises them through an exclusive
/// file lock. `busy_timeout` is the piece the other lists can do without and
/// this one cannot: those connections are pooled and coordinated, these are
/// independent opens that can genuinely collide, and without it a collision
/// is an immediate `SQLITE_BUSY` rather than a short wait.
///
/// The absence of both is why a run configured with a memory store could
/// stall on Windows, where file locking is mandatory rather than advisory.
pub const MEMORY_STORE_PRAGMAS: &[&str] = &[
    "PRAGMA journal_mode = WAL",
    "PRAGMA synchronous = NORMAL",
    "PRAGMA foreign_keys = ON",
    "PRAGMA busy_timeout = 5000",
];

/// Apply the given PRAGMAs to a connection.
///
/// PRAGMA may return a row (e.g., `journal_mode`); `execute_batch` handles that.
pub fn apply(conn: &Connection, pragmas: &[&str]) -> rusqlite::Result<()> {
    for p in pragmas {
        conn.execute_batch(p)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pragmas_apply_to_in_memory_db() {
        let conn = Connection::open_in_memory().unwrap();
        apply(&conn, PER_RUN_PRAGMAS).unwrap();

        let mode: String = conn
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .unwrap();
        // :memory: returns "memory" not "wal" — call shouldn't error.
        assert!(mode == "memory" || mode == "wal");
    }

    /// The memory store's list must carry the two properties its
    /// independent openers depend on. Asserting the constants, not just that
    /// `apply` succeeds: a silent removal of `busy_timeout` would leave
    /// every collision an immediate `SQLITE_BUSY`.
    #[test]
    fn memory_store_pragmas_carry_wal_and_a_busy_timeout() {
        assert!(
            MEMORY_STORE_PRAGMAS
                .iter()
                .any(|p| p.contains("journal_mode = WAL")),
            "{MEMORY_STORE_PRAGMAS:?}"
        );
        assert!(
            MEMORY_STORE_PRAGMAS
                .iter()
                .any(|p| p.contains("busy_timeout")),
            "{MEMORY_STORE_PRAGMAS:?}"
        );
        let conn = Connection::open_in_memory().unwrap();
        apply(&conn, MEMORY_STORE_PRAGMAS).unwrap();
        let busy: i64 = conn
            .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
            .unwrap();
        assert_eq!(busy, 5000, "busy_timeout must actually take effect");
    }

    #[test]
    fn registry_pragmas_apply() {
        let conn = Connection::open_in_memory().unwrap();
        apply(&conn, REGISTRY_PRAGMAS).unwrap();
    }
}
