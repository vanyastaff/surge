//! Tests for `0017_task_queue.sql`.
//!
//! The queue mirror is the daemon's scheduling table; these tests pin the
//! schema facts the store relies on: the two-table split (`task_queue` per
//! row, `project_queue` per project), the `(project_root, task_id)` primary
//! key, the default dispatch state, and the two indexes the scheduler scans.

use rusqlite::Connection;

fn fresh_db_through_0016() -> Connection {
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
        include_str!("../src/runs/migrations/registry/0010_snooze_subjects.sql"),
        include_str!("../src/runs/migrations/registry/0011_secrets.sql"),
        include_str!("../src/runs/migrations/registry/0012_inbox_action_policy_hint.sql"),
        include_str!("../src/runs/migrations/registry/0013_intake_emit_log.sql"),
        include_str!("../src/runs/migrations/registry/0014_task_ledger_index.sql"),
        include_str!("../src/runs/migrations/registry/0015_runtime_capacity.sql"),
        include_str!("../src/runs/migrations/registry/0016_runs_wake_at.sql"),
    ] {
        conn.execute_batch(sql).unwrap();
    }
    conn
}

fn apply_0017(conn: &Connection) {
    let sql = include_str!("../src/runs/migrations/registry/0017_task_queue.sql");
    conn.execute_batch(sql).unwrap();
}

#[test]
fn task_queue_columns_and_defaults() {
    let conn = fresh_db_through_0016();
    apply_0017(&conn);

    conn.execute(
        "INSERT INTO task_queue (project_root, task_id, roadmap_hash, enqueued_at, updated_at)
         VALUES ('/proj', 'm1-t1', 'sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855', 1, 1)",
        [],
    )
    .unwrap();

    let (priority, deps, state, attempt, skips, size, run_id): (
        String,
        String,
        String,
        i64,
        i64,
        Option<String>,
        Option<String>,
    ) = conn
        .query_row(
            "SELECT priority, depends_on_json, dispatch_state, attempt, skipped_dispatches, size, run_id
             FROM task_queue WHERE project_root = '/proj' AND task_id = 'm1-t1'",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                ))
            },
        )
        .unwrap();

    assert_eq!(priority, "medium", "default priority is medium");
    assert_eq!(deps, "[]", "default dependency list is empty");
    assert_eq!(state, "queued", "default dispatch state is queued");
    assert_eq!(attempt, 1, "first dispatch is attempt 1");
    assert_eq!(skips, 0);
    assert_eq!(size, None);
    assert_eq!(run_id, None);
}

#[test]
fn task_queue_primary_key_is_project_and_task() {
    let conn = fresh_db_through_0016();
    apply_0017(&conn);

    let insert = |root: &str, task: &str| {
        conn.execute(
            "INSERT INTO task_queue (project_root, task_id, roadmap_hash, enqueued_at, updated_at)
             VALUES (?1, ?2, 'x', 1, 1)",
            rusqlite::params![root, task],
        )
    };

    insert("/a", "t1").unwrap();
    insert("/b", "t1").unwrap();
    assert!(
        insert("/a", "t1").is_err(),
        "same (project, task) must violate the primary key"
    );
}

#[test]
fn project_queue_defaults_unpaused() {
    let conn = fresh_db_through_0016();
    apply_0017(&conn);

    conn.execute(
        "INSERT INTO project_queue (project_root, updated_at) VALUES ('/proj', 1)",
        [],
    )
    .unwrap();
    let paused: i64 = conn
        .query_row(
            "SELECT paused FROM project_queue WHERE project_root = '/proj'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(paused, 0);
}

#[test]
fn scheduler_indexes_exist() {
    let conn = fresh_db_through_0016();
    apply_0017(&conn);

    for index in ["idx_task_queue_project_state", "idx_task_queue_run"] {
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name = ?1",
                [index],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "{index} must exist");
    }
}

#[test]
fn migration_is_additive_on_an_existing_registry() {
    let conn = fresh_db_through_0016();
    // A run recorded before the queue existed.
    conn.execute(
        "INSERT INTO runs (id, project_path, pipeline_template, status, started_at)
         VALUES ('r1', '/proj', 'linear-3', 'running', 1)",
        [],
    )
    .unwrap();
    apply_0017(&conn);
    let runs: i64 = conn
        .query_row("SELECT COUNT(*) FROM runs", [], |row| row.get(0))
        .unwrap();
    assert_eq!(runs, 1, "existing registry rows survive 0017");
}
