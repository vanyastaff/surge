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
    (
        "registry-0003-task-source-state",
        include_str!("migrations/registry/0003_task_source_state.sql"),
    ),
    (
        "registry-0004-inbox-callback-columns",
        include_str!("migrations/registry/0004_inbox_callback_columns.sql"),
    ),
    (
        "registry-0005-inbox-queues",
        include_str!("migrations/registry/0005_inbox_queues.sql"),
    ),
    (
        "registry-0006-roadmap-patch-index",
        include_str!("migrations/registry/0006_roadmap_patch_index.sql"),
    ),
    (
        "registry-0007-telegram-pairing-tokens",
        include_str!("migrations/registry/0007_telegram_pairing_tokens.sql"),
    ),
    (
        "registry-0008-telegram-pairings",
        include_str!("migrations/registry/0008_telegram_pairings.sql"),
    ),
    (
        "registry-0009-telegram-cards",
        include_str!("migrations/registry/0009_telegram_cards.sql"),
    ),
    (
        "registry-0010-snooze-subjects",
        include_str!("migrations/registry/0010_snooze_subjects.sql"),
    ),
    (
        "registry-0011-secrets",
        include_str!("migrations/registry/0011_secrets.sql"),
    ),
    (
        "registry-0012-inbox-action-policy-hint",
        include_str!("migrations/registry/0012_inbox_action_policy_hint.sql"),
    ),
    (
        "registry-0013-intake-emit-log",
        include_str!("migrations/registry/0013_intake_emit_log.sql"),
    ),
    (
        "registry-0014-task-ledger-index",
        include_str!("migrations/registry/0014_task_ledger_index.sql"),
    ),
    (
        "registry-0015-runtime-capacity",
        include_str!("migrations/registry/0015_runtime_capacity.sql"),
    ),
    (
        "registry-0016-runs-wake-at",
        include_str!("migrations/registry/0016_runs_wake_at.sql"),
    ),
    (
        "registry-0017-bootstrap-operations",
        include_str!("migrations/registry/0017_bootstrap_operations.sql"),
    ),
    (
        "registry-0018-bootstrap-continuation",
        include_str!("migrations/registry/0018_bootstrap_continuation.sql"),
    ),
    (
        "registry-0019-telegram-pairing-target",
        include_str!("migrations/registry/0019_telegram_pairing_target.sql"),
    ),
    (
        "registry-0020-terminal-comment-outbox",
        include_str!("migrations/registry/0020_terminal_comment_outbox.sql"),
    ),
    (
        "registry-0021-verification-binding",
        include_str!("migrations/registry/0021_verification_binding.sql"),
    ),
    (
        "registry-0022-work-items",
        include_str!("migrations/registry/0022_work_items.sql"),
    ),
    (
        "registry-0023-execution-controls",
        include_str!("migrations/registry/0023_execution_controls.sql"),
    ),
    (
        "registry-0024-recovery-cycles",
        include_str!("migrations/registry/0024_recovery_cycles.sql"),
    ),
    (
        "registry-0025-quota-handoffs",
        include_str!("migrations/registry/0025_quota_handoffs.sql"),
    ),
    (
        "registry-0026-recipe-admissions",
        include_str!("migrations/registry/0026_recipe_admissions.sql"),
    ),
    (
        "registry-0027-quota-plans",
        include_str!("migrations/registry/0027_quota_plans.sql"),
    ),
    (
        "registry-0028-start-preparations",
        include_str!("migrations/registry/0028_start_preparations.sql"),
    ),
    (
        "registry-0029-owned-flows",
        include_str!("migrations/registry/0029_owned_flows.sql"),
    ),
    (
        "registry-0030-owned-flow-wake-refusals",
        include_str!("migrations/registry/0030_owned_flow_wake_refusals.sql"),
    ),
    (
        "0031_task_ledger_human_overrides",
        include_str!("migrations/registry/0031_task_ledger_human_overrides.sql"),
    ),
    (
        "registry-0032-host-settlement",
        include_str!("migrations/registry/0032_host_settlement.sql"),
    ),
    (
        "registry-0033-graph-authority",
        include_str!("migrations/registry/0033_graph_authority.sql"),
    ),
    (
        "0034_inherited_provenance",
        include_str!("migrations/registry/0034_inherited_provenance.sql"),
    ),
    (
        "registry-0035-owned-flow-private-establishment",
        include_str!("migrations/registry/0035_owned_flow_private_establishment.sql"),
    ),
];

/// Migrations applied to each per-run DB.
pub const PER_RUN_MIGRATIONS: MigrationSet = &[
    (
        "per-run-0001-initial",
        include_str!("migrations/per_run/0001_initial.sql"),
    ),
    (
        "per-run-0002-roadmap-patches",
        include_str!("migrations/per_run/0002_roadmap_patches.sql"),
    ),
    (
        "per-run-0003-task-ledger",
        include_str!("migrations/per_run/0003_task_ledger.sql"),
    ),
    (
        "per-run-0004-verification-binding",
        include_str!("migrations/per_run/0004_verification_binding.sql"),
    ),
    (
        "per-run-0005-stage-outcome-commits",
        include_str!("migrations/per_run/0005_stage_outcome_commits.sql"),
    ),
    (
        "per-run-0006-gate-stage-commits",
        include_str!("migrations/per_run/0006_gate_stage_commits.sql"),
    ),
    (
        "per-run-0007-stage-session-attribution",
        include_str!("migrations/per_run/0007_stage_session_attribution.sql"),
    ),
    (
        "per-run-0008-stage-known-cost",
        include_str!("migrations/per_run/0008_stage_known_cost.sql"),
    ),
    (
        "per-run-0009-task-ledger-human-overrides",
        include_str!("migrations/per_run/0009_task_ledger_human_overrides.sql"),
    ),
    (
        "per-run-0010-stage-tokens-unknown",
        include_str!("migrations/per_run/0010_stage_tokens_unknown.sql"),
    ),
    (
        "per-run-0011-graph-authority",
        include_str!("migrations/per_run/0011_graph_authority.sql"),
    ),
    (
        "0012_inherited_provenance",
        include_str!("migrations/per_run/0012_inherited_provenance.sql"),
    ),
];

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

    const OLD_TASK_LEDGER_UPSERT: &str = r"INSERT INTO task_ledger_index
                (run_id, task_id, project_path, status, verified, discovered_from,
                 last_authority_node, updated_seq, updated_at, accepted_by_human,
                 requirement_revised)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(run_id, task_id) DO UPDATE SET
                project_path = excluded.project_path,
                status = excluded.status,
                verified = excluded.verified,
                accepted_by_human = excluded.accepted_by_human,
                requirement_revised = excluded.requirement_revised,
                discovered_from = COALESCE(excluded.discovered_from, discovered_from),
                last_authority_node = excluded.last_authority_node,
                updated_seq = excluded.updated_seq,
                updated_at = excluded.updated_at";

    #[test]
    fn graph_authority_registry_rejects_queued_old_upsert_on_two_connections() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("registry.sqlite");
        let mut old = rusqlite::Connection::open(&path).unwrap();
        let clock = MockClock::new(100);
        let boundary = REGISTRY_MIGRATIONS
            .iter()
            .position(|(id, _)| *id == "registry-0033-graph-authority")
            .unwrap_or(REGISTRY_MIGRATIONS.len());
        apply(&mut old, &REGISTRY_MIGRATIONS[..boundary], &clock).unwrap();
        let legacy_sql = OLD_TASK_LEDGER_UPSERT;
        let bind = |task: &str| {
            vec![
                "legacy".to_owned(),
                task.to_owned(),
                "/repo".to_owned(),
                "completed".to_owned(),
                "1".to_owned(),
                "source".to_owned(),
                "verifier".to_owned(),
                "7".to_owned(),
                "100".to_owned(),
                "1".to_owned(),
                "1".to_owned(),
            ]
        };
        let mut queued = old.prepare(legacy_sql).unwrap();
        queued
            .execute(rusqlite::params_from_iter(bind("existing")))
            .unwrap();
        let old_snapshot: bool = old
            .query_row(
                "SELECT verified FROM task_ledger_index WHERE task_id='existing'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(old_snapshot);
        let mut current = rusqlite::Connection::open(&path).unwrap();
        apply(&mut current, REGISTRY_MIGRATIONS, &clock).unwrap();
        let preserved: (bool, String, String, i64, i64, bool, bool) = current.query_row("SELECT verified,discovered_from,last_authority_node,updated_seq,updated_at,accepted_by_human,requirement_revised FROM task_ledger_index", [], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?,row.get(6)?))).unwrap();
        assert_eq!(
            preserved,
            (
                false,
                "source".into(),
                "verifier".into(),
                7,
                100,
                true,
                true
            )
        );
        current
            .execute(
                crate::task_ledger::UPSERT_SQL,
                rusqlite::params![
                    "legacy",
                    "existing",
                    "/repo",
                    "completed",
                    1,
                    "source",
                    "verifier",
                    9,
                    100,
                    1,
                    1,
                    Option::<String>::None
                ],
            )
            .unwrap();
        let row_bytes = |conn: &rusqlite::Connection| {
            conn.query_row(
                "SELECT * FROM task_ledger_index WHERE task_id='existing'",
                [],
                |row| {
                    (0..12)
                        .map(|index| row.get::<_, rusqlite::types::Value>(index))
                        .collect::<rusqlite::Result<Vec<_>>>()
                },
            )
            .unwrap()
        };
        let before = row_bytes(&current);
        for task in ["absent", "existing"] {
            let error = queued
                .execute(rusqlite::params_from_iter(bind(task)))
                .unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("NOT NULL constraint failed: task_ledger_index.projection_semantics"),
                "{error}"
            );
        }
        assert_eq!(
            before,
            row_bytes(&current),
            "legacy conflict update cannot change any current row bytes"
        );
        let row: (i64, bool) = current
            .query_row(
                "SELECT updated_seq,verified FROM task_ledger_index WHERE task_id='existing'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(row, (9, true));
        let indexes: i64 = current.query_row("SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND tbl_name='task_ledger_index' AND name LIKE 'idx_task_ledger_index_%'",[],|row|row.get(0)).unwrap();
        assert_eq!(indexes, 4);
        let columns: Vec<String> = current
            .prepare("SELECT name FROM pragma_table_info('task_ledger_index') ORDER BY cid")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(
            columns,
            [
                "run_id",
                "task_id",
                "project_path",
                "status",
                "verified",
                "discovered_from",
                "last_authority_node",
                "updated_seq",
                "updated_at",
                "accepted_by_human",
                "requirement_revised",
                "projection_semantics",
                "inherited_provenance_json"
            ]
        );
        let primary: Vec<String> = current
            .prepare(
                "SELECT name FROM pragma_table_info('task_ledger_index') WHERE pk>0 ORDER BY pk",
            )
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(primary, ["run_id", "task_id"]);
        let mandatory: (bool,Option<String>) = current.query_row("SELECT [notnull],dflt_value FROM pragma_table_info('task_ledger_index') WHERE name='projection_semantics'",[],|row|Ok((row.get(0)?,row.get(1)?))).unwrap();
        assert_eq!(mandatory, (true, None));
        let provenance_column: (bool, Option<String>) = current.query_row(
            "SELECT [notnull],dflt_value FROM pragma_table_info('task_ledger_index') WHERE name='inherited_provenance_json'",
            [],
            |row| Ok((row.get(0)?,row.get(1)?)),
        ).unwrap();
        assert_eq!(provenance_column, (false, None));
        for (name, expected) in [
            ("idx_task_ledger_index_status", vec!["status"]),
            (
                "idx_task_ledger_index_project_status",
                vec!["project_path", "status"],
            ),
            ("idx_task_ledger_index_run", vec!["run_id"]),
            ("idx_task_ledger_index_discovered", vec!["discovered_from"]),
        ] {
            let actual: Vec<String> = current
                .prepare("SELECT name FROM pragma_index_info(?) ORDER BY seqno")
                .unwrap()
                .query_map([name], |row| row.get(0))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap();
            assert_eq!(actual, expected);
        }
        let status: (String, String) = current
            .query_row(
                "SELECT project_path,status FROM task_ledger_index WHERE task_id='existing'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, ("/repo".into(), "completed".into()));
    }

    #[test]
    fn graph_authority_registry_downgrades_writer_that_finishes_before_migration() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("registry.sqlite");
        let mut old = rusqlite::Connection::open(&path).unwrap();
        let clock = MockClock::new(100);
        let boundary = REGISTRY_MIGRATIONS
            .iter()
            .position(|(id, _)| *id == "registry-0033-graph-authority")
            .unwrap_or(REGISTRY_MIGRATIONS.len());
        apply(&mut old, &REGISTRY_MIGRATIONS[..boundary], &clock).unwrap();
        old.execute_batch("BEGIN IMMEDIATE").unwrap();
        old.execute(
            OLD_TASK_LEDGER_UPSERT,
            rusqlite::params![
                "legacy",
                "held",
                "/repo",
                "completed",
                1,
                "source",
                "verify",
                7,
                100,
                1,
                1
            ],
        )
        .unwrap();
        let (ready, waiting) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let mut conn = rusqlite::Connection::open(path).unwrap();
            conn.busy_timeout(std::time::Duration::from_secs(5))
                .unwrap();
            ready.send(()).unwrap();
            apply(&mut conn, REGISTRY_MIGRATIONS, &MockClock::new(100)).unwrap();
        });
        waiting.recv().unwrap();
        // SQLite's exclusive migration cannot obtain writer ownership until
        // this old transaction commits; its positive joins the downgraded copy.
        old.execute_batch("COMMIT").unwrap();
        worker.join().unwrap();
        let verified: bool = old
            .query_row(
                "SELECT verified FROM task_ledger_index WHERE task_id='held'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!verified);
        let error = old
            .execute(
                OLD_TASK_LEDGER_UPSERT,
                rusqlite::params![
                    "legacy",
                    "held",
                    "/repo",
                    "completed",
                    1,
                    "source",
                    "verify",
                    7,
                    100,
                    1,
                    1
                ],
            )
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("NOT NULL constraint failed: task_ledger_index.projection_semantics")
        );
    }

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

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type='table' AND name='roadmap_patch_index'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type='table' AND name='runtime_capacity'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "0015 must create runtime_capacity");

        let has_wake_at: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('runs') WHERE name = 'wake_at'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(has_wake_at, 1, "0016 must add runs.wake_at");

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
            "roadmap_patches",
            "task_ledger",
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
    fn stage_usage_migrations_preserve_existing_attempt_rows() {
        let mut conn = Connection::open_in_memory().unwrap();
        let clock = MockClock::new(1_700_000_000_000);
        apply(&mut conn, &PER_RUN_MIGRATIONS[..6], &clock).unwrap();
        conn.execute(
            "INSERT INTO stage_executions (node_id, attempt, started_seq, started_at) VALUES ('worker', 1, 1, 10)",
            [],
        )
        .unwrap();

        apply(&mut conn, &PER_RUN_MIGRATIONS[6..], &clock).unwrap();

        let row: (i64, Option<String>, Option<f64>, i64) = conn
            .query_row(
                "SELECT attempt, session_id, known_cost_usd, cost_unknown FROM stage_executions WHERE node_id='worker'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(row, (1, None, None, 0));
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

#[cfg(test)]
mod verification_upgrade_tests {
    use super::*;
    use crate::runs::clock::MockClock;
    #[test]
    fn upgrade_downgrades_old_registry_proof_without_any_new_run_event() {
        let mut conn = Connection::open_in_memory().unwrap();
        let binding_migration = REGISTRY_MIGRATIONS
            .iter()
            .position(|(id, _)| *id == "registry-0021-verification-binding")
            .expect("verification downgrade migration is registered");
        let previous: MigrationSet = Box::leak(
            REGISTRY_MIGRATIONS[..binding_migration]
                .to_vec()
                .into_boxed_slice(),
        );
        let clock = MockClock::new(100);
        apply(&mut conn, previous, &clock).unwrap();
        assert!(!conn.query_row("SELECT EXISTS(SELECT 1 FROM _migrations WHERE id='registry-0021-verification-binding')",[],|row|row.get::<_,bool>(0)).unwrap());
        conn.execute("INSERT INTO task_ledger_index(run_id,task_id,project_path,status,verified,updated_seq,updated_at) VALUES ('legacy','t1','/repo','completed',1,1,1)",[]).unwrap();
        apply(&mut conn, REGISTRY_MIGRATIONS, &clock).unwrap();
        let verified: bool = conn
            .query_row("SELECT verified FROM task_ledger_index", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert!(!verified);
        apply(&mut conn, REGISTRY_MIGRATIONS, &clock).unwrap();
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM task_ledger_index", [], |row| row
                .get::<_, u64>(0))
                .unwrap(),
            1
        );
    }
}

#[cfg(test)]
mod work_item_upgrade_tests {
    use super::*;
    use crate::runs::clock::MockClock;
    #[test]
    fn adding_persistent_tasks_preserves_existing_registry_rows_without_association() {
        let mut conn = Connection::open_in_memory().unwrap();
        let work_items_migration = REGISTRY_MIGRATIONS
            .iter()
            .position(|(id, _)| *id == "registry-0022-work-items")
            .expect("persistent task migration is registered");
        let previous: MigrationSet = &REGISTRY_MIGRATIONS[..work_items_migration];
        let clock = MockClock::new(100);
        apply(&mut conn, previous, &clock).unwrap();
        conn.execute("INSERT INTO runs(id,project_path,status,started_at) VALUES ('legacy-run','/repo','completed',1)",[]).unwrap();
        conn.execute("INSERT INTO ticket_index(task_id,source_id,provider,run_id,state,first_seen,last_seen) VALUES ('tracker-7','source','github','legacy-run','completed','1','1')",[]).unwrap();
        apply(&mut conn, REGISTRY_MIGRATIONS, &clock).unwrap();
        apply(&mut conn, REGISTRY_MIGRATIONS, &clock).unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT project_path FROM runs WHERE id='legacy-run'",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
            "/repo"
        );
        assert_eq!(
            conn.query_row(
                "SELECT run_id FROM ticket_index WHERE task_id='tracker-7'",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
            "legacy-run"
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM work_items", [], |row| row
                .get::<_, u64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM work_item_attempts", [], |row| row
                .get::<_, u64>(0))
                .unwrap(),
            0
        );
    }
}
