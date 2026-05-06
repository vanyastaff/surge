//! Smoke test that 0002_ticket_index.sql applies cleanly on a fresh DB
//! and creates the expected schema.

use rusqlite::Connection;

#[test]
fn migration_0002_creates_ticket_index_table() {
    let conn = Connection::open_in_memory().unwrap();
    // 0002 depends on `runs` table for FK; use minimal stub.
    conn.execute_batch(
        "CREATE TABLE runs (id TEXT PRIMARY KEY);",
    )
    .unwrap();

    let sql = include_str!("../src/runs/migrations/registry/0002_ticket_index.sql");
    conn.execute_batch(sql).unwrap();

    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='ticket_index'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1, "ticket_index table should exist");

    let idx_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND tbl_name='ticket_index' AND name LIKE 'idx_ticket_index_%'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(idx_count, 3, "should have 3 indexes on ticket_index");
}

#[test]
fn migration_0002_fk_constraints_are_valid() {
    let conn = Connection::open_in_memory().unwrap();
    
    conn.execute_batch(
        "PRAGMA foreign_keys = ON;
         CREATE TABLE runs (id TEXT PRIMARY KEY);",
    )
    .unwrap();

    let sql = include_str!("../src/runs/migrations/registry/0002_ticket_index.sql");
    conn.execute_batch(sql).unwrap();

    // Verify FK to runs(id) works
    let res = conn.execute(
        "INSERT INTO ticket_index (task_id, source_id, provider, state, first_seen, last_seen, run_id) 
         VALUES (?, ?, ?, ?, ?, ?, ?)",
        rusqlite::params!["t1", "src1", "github", "open", "2025-01-01", "2025-01-02", "nonexistent"],
    );
    assert!(res.is_err(), "FK constraint should reject run_id that doesn't exist");

    // Verify self-referential FK works by first inserting a valid ticket
    conn.execute(
        "INSERT INTO ticket_index (task_id, source_id, provider, state, first_seen, last_seen) 
         VALUES (?, ?, ?, ?, ?, ?)",
        rusqlite::params!["t1", "src1", "github", "open", "2025-01-01", "2025-01-02"],
    )
    .unwrap();

    let res = conn.execute(
        "INSERT INTO ticket_index (task_id, source_id, provider, state, first_seen, last_seen, duplicate_of) 
         VALUES (?, ?, ?, ?, ?, ?, ?)",
        rusqlite::params!["t2", "src2", "github", "open", "2025-01-01", "2025-01-02", "nonexistent"],
    );
    assert!(res.is_err(), "FK constraint should reject duplicate_of that doesn't exist");
}
