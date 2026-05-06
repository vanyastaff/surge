//! Smoke test for 0005_inbox_queues.sql

use rusqlite::{Connection, params};

fn db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    let sql = include_str!("../src/runs/migrations/registry/0005_inbox_queues.sql");
    conn.execute_batch(sql).unwrap();
    conn
}

#[test]
fn migration_0005_creates_inbox_action_queue_table() {
    let conn = db();
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master \
             WHERE type='table' AND name='inbox_action_queue'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);

    let now = "2026-05-06T10:00:00Z";
    conn.execute(
        "INSERT INTO inbox_action_queue \
            (kind, task_id, callback_token, decided_via, snooze_until, enqueued_at) \
         VALUES (?, ?, ?, ?, ?, ?)",
        params!["start", "linear:wsp1/T-1", "tok1", "telegram", Option::<String>::None, now],
    )
    .unwrap();
    let kind: String = conn
        .query_row(
            "SELECT kind FROM inbox_action_queue WHERE task_id = ?",
            ["linear:wsp1/T-1"],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(kind, "start");
}

#[test]
fn migration_0005_creates_inbox_delivery_queue_table() {
    let conn = db();
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master \
             WHERE type='table' AND name='inbox_delivery_queue'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);

    let now = "2026-05-06T10:00:00Z";
    conn.execute(
        "INSERT INTO inbox_delivery_queue \
            (task_id, callback_token, payload_json, enqueued_at) \
         VALUES (?, ?, ?, ?)",
        params!["linear:wsp1/T-2", "tok2", r#"{"x":1}"#, now],
    )
    .unwrap();
    let token: String = conn
        .query_row(
            "SELECT callback_token FROM inbox_delivery_queue WHERE task_id = ?",
            ["linear:wsp1/T-2"],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(token, "tok2");
}

#[test]
fn migration_0005_seq_is_monotonic_autoincrement() {
    let conn = db();
    let now = "2026-05-06T10:00:00Z";
    let mut seqs = vec![];
    for i in 1..=3 {
        conn.execute(
            "INSERT INTO inbox_action_queue \
                (kind, task_id, callback_token, decided_via, snooze_until, enqueued_at) \
             VALUES (?, ?, ?, ?, ?, ?)",
            params![
                "start",
                format!("linear:wsp1/M-{i}"),
                format!("tok-{i}"),
                "telegram",
                Option::<String>::None,
                now
            ],
        )
        .unwrap();
        let s: i64 = conn.last_insert_rowid();
        seqs.push(s);
    }
    assert!(seqs[0] < seqs[1] && seqs[1] < seqs[2]);
}
