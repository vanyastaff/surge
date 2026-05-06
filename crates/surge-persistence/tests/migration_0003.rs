//! Smoke test for 0003_task_source_state.sql

use rusqlite::{Connection, params};

#[test]
fn migration_0003_creates_task_source_state_table() {
    let conn = Connection::open_in_memory().unwrap();
    let sql = include_str!("../src/runs/migrations/registry/0003_task_source_state.sql");
    conn.execute_batch(sql).unwrap();

    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master \
             WHERE type='table' AND name='task_source_state'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);

    // Insert + read-back round trip.
    conn.execute(
        "INSERT INTO task_source_state(source_id, last_seen_cursor, last_poll_at, consecutive_failures) \
         VALUES (?,?,?,?)",
        params!["linear:wsp1", "cursor_42", "2026-05-06T10:00:00Z", 0_i64],
    )
    .unwrap();

    let cursor: String = conn
        .query_row(
            "SELECT last_seen_cursor FROM task_source_state WHERE source_id = ?",
            ["linear:wsp1"],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(cursor, "cursor_42");

    // consecutive_failures default value
    let failures: i64 = conn
        .query_row(
            "SELECT consecutive_failures FROM task_source_state WHERE source_id = ?",
            ["linear:wsp1"],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(failures, 0);
}
