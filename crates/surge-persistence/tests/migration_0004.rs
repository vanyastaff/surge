//! Smoke test for 0004_inbox_callback_columns.sql
//!
//! Verifies that the migration adds the three new nullable columns and
//! that the partial UNIQUE index correctly rejects duplicate non-NULL
//! tokens while allowing multiple NULLs.

use rusqlite::{Connection, params};

fn db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    // 0001 creates `runs` (which 0002 has FK to).
    let m1 = include_str!("../src/runs/migrations/registry/0001_initial.sql");
    conn.execute_batch(m1).unwrap();
    let m2 = include_str!("../src/runs/migrations/registry/0002_ticket_index.sql");
    conn.execute_batch(m2).unwrap();
    let m4 = include_str!("../src/runs/migrations/registry/0004_inbox_callback_columns.sql");
    conn.execute_batch(m4).unwrap();
    conn
}

#[test]
fn migration_0004_adds_three_nullable_columns() {
    let conn = db();
    // PRAGMA table_info returns one row per column.
    let mut stmt = conn.prepare("PRAGMA table_info('ticket_index')").unwrap();
    let cols: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(1))
        .unwrap()
        .map(|c| c.unwrap())
        .collect();
    assert!(cols.contains(&"callback_token".to_string()));
    assert!(cols.contains(&"tg_chat_id".to_string()));
    assert!(cols.contains(&"tg_message_id".to_string()));
}

#[test]
fn migration_0004_partial_unique_index_rejects_duplicate_non_null_tokens() {
    let conn = db();
    let now = "2026-05-06T10:00:00Z";
    // Insert two rows with NULL callback_token — must not collide.
    conn.execute(
        "INSERT INTO ticket_index(task_id, source_id, provider, state, first_seen, last_seen) \
         VALUES (?, ?, ?, ?, ?, ?)",
        params!["linear:wsp1/A-1", "linear:wsp1", "linear", "Seen", now, now],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO ticket_index(task_id, source_id, provider, state, first_seen, last_seen) \
         VALUES (?, ?, ?, ?, ?, ?)",
        params!["linear:wsp1/A-2", "linear:wsp1", "linear", "Seen", now, now],
    )
    .unwrap();
    // Set the same callback_token on both — second SET must fail.
    conn.execute(
        "UPDATE ticket_index SET callback_token = ? WHERE task_id = ?",
        params!["tok-shared", "linear:wsp1/A-1"],
    )
    .unwrap();
    let dup_err = conn.execute(
        "UPDATE ticket_index SET callback_token = ? WHERE task_id = ?",
        params!["tok-shared", "linear:wsp1/A-2"],
    );
    assert!(dup_err.is_err(), "duplicate non-NULL callback_token must violate the UNIQUE index");
}

#[test]
fn migration_0004_partial_unique_index_allows_multiple_nulls() {
    let conn = db();
    let now = "2026-05-06T10:00:00Z";
    for id in ["linear:wsp1/B-1", "linear:wsp1/B-2", "linear:wsp1/B-3"] {
        conn.execute(
            "INSERT INTO ticket_index(task_id, source_id, provider, state, first_seen, last_seen) \
             VALUES (?, ?, ?, ?, ?, ?)",
            params![id, "linear:wsp1", "linear", "Seen", now, now],
        )
        .unwrap();
    }
    // All three rows have NULL callback_token — that's allowed.
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM ticket_index WHERE callback_token IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 3);
}
