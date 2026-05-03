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

    #[test]
    fn registry_pragmas_apply() {
        let conn = Connection::open_in_memory().unwrap();
        apply(&conn, REGISTRY_PRAGMAS).unwrap();
    }
}
