//! Tests for `0010_snooze_subjects.sql`.
//!
//! Verifies the additive backfill: legacy rows survive the migration and
//! receive `subject_kind = 'inbox_ticket'` plus `subject_ref = callback_token`,
//! while new rows can land with `subject_kind = 'cockpit_card'`.

use rusqlite::{Connection, params};

fn fresh_db_through_0009() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    for sql in [
        include_str!("../src/runs/migrations/registry/0001_initial.sql"),
        include_str!("../src/runs/migrations/registry/0002_ticket_index.sql"),
        include_str!("../src/runs/migrations/registry/0003_task_source_state.sql"),
        include_str!("../src/runs/migrations/registry/0004_inbox_callback_columns.sql"),
        include_str!("../src/runs/migrations/registry/0005_inbox_queues.sql"),
        include_str!("../src/runs/migrations/registry/0006_roadmap_patch_index.sql"),
        include_str!("../src/runs/migrations/registry/0007_telegram_pairing_tokens.sql"),
        include_str!("../src/runs/migrations/registry/0008_telegram_pairings.sql"),
        include_str!("../src/runs/migrations/registry/0009_telegram_cards.sql"),
    ] {
        conn.execute_batch(sql).unwrap();
    }
    conn
}

fn apply_0010(conn: &Connection) {
    let sql = include_str!("../src/runs/migrations/registry/0010_snooze_subjects.sql");
    conn.execute_batch(sql).unwrap();
}

#[test]
fn legacy_rows_backfilled_to_inbox_ticket_with_callback_token_ref() {
    let conn = fresh_db_through_0009();

    // Insert a legacy snooze row before the migration runs.
    let now = "2026-05-13T10:00:00Z";
    conn.execute(
        "INSERT INTO inbox_action_queue \
            (kind, task_id, callback_token, decided_via, snooze_until, enqueued_at) \
         VALUES (?, ?, ?, ?, ?, ?)",
        params![
            "snooze",
            "linear:wsp1/T-1",
            "legacy-token-1",
            "telegram",
            "2026-05-13T11:00:00Z",
            now
        ],
    )
    .unwrap();

    apply_0010(&conn);

    let (kind, subject_ref): (String, Option<String>) = conn
        .query_row(
            "SELECT subject_kind, subject_ref \
             FROM inbox_action_queue \
             WHERE callback_token = ?",
            ["legacy-token-1"],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();

    assert_eq!(kind, "inbox_ticket");
    assert_eq!(subject_ref.as_deref(), Some("legacy-token-1"));
}

#[test]
fn new_inbox_ticket_rows_default_to_inbox_ticket_kind() {
    let conn = fresh_db_through_0009();
    apply_0010(&conn);

    // A producer that does NOT mention subject_kind/subject_ref must still
    // succeed and pick up the DEFAULT — important for inbox code paths that
    // were written before this migration existed.
    conn.execute(
        "INSERT INTO inbox_action_queue \
            (kind, task_id, callback_token, decided_via, enqueued_at) \
         VALUES (?, ?, ?, ?, ?)",
        params!["start", "linear:wsp1/T-2", "tok-2", "telegram", "now"],
    )
    .unwrap();

    let kind: String = conn
        .query_row(
            "SELECT subject_kind FROM inbox_action_queue WHERE callback_token = ?",
            ["tok-2"],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(kind, "inbox_ticket");
}

#[test]
fn cockpit_card_rows_carry_card_id_ref() {
    let conn = fresh_db_through_0009();
    apply_0010(&conn);

    conn.execute(
        "INSERT INTO inbox_action_queue \
            (kind, task_id, callback_token, decided_via, snooze_until, \
             enqueued_at, subject_kind, subject_ref) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        params![
            "snooze",
            "",
            "",
            "telegram",
            "2026-05-13T11:00:00Z",
            "2026-05-13T10:00:00Z",
            "cockpit_card",
            "01HQ7K2X8YZTPDR5G3FNAVCBMW"
        ],
    )
    .unwrap();

    let (kind, subject_ref): (String, Option<String>) = conn
        .query_row(
            "SELECT subject_kind, subject_ref \
             FROM inbox_action_queue \
             WHERE subject_kind = 'cockpit_card'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();

    assert_eq!(kind, "cockpit_card");
    assert_eq!(subject_ref.as_deref(), Some("01HQ7K2X8YZTPDR5G3FNAVCBMW"));
}

#[test]
fn index_subject_lookup_by_kind_and_ref() {
    let conn = fresh_db_through_0009();
    apply_0010(&conn);

    // Index existence smoke check: query the SQLite catalog directly.
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master \
             WHERE type='index' AND name='idx_inbox_action_queue_subject'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
}
