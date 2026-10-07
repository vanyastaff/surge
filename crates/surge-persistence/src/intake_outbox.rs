//! Atomic terminal ticket completion and durable tracker comment delivery.
//!
//! A token fences acknowledgment/retry against a replacement claimant. The lease
//! exceeds the daemon's network timeout; uncertain provider success may still be
//! delivered again after expiry. This is not an exactly-once external protocol.

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::intake::TicketState;
use crate::intake_emit_log::{self, EmitEventKind, EmitKey};

/// Terminal outcomes eligible for durable tracker comments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalCommentKind {
    /// Successful completion.
    Completed,
    /// Failed execution.
    Failed,
    /// Aborted execution.
    Aborted,
}

impl TerminalCommentKind {
    /// Existing intake emission identity.
    #[must_use]
    pub fn emit_kind(self) -> EmitEventKind {
        match self {
            Self::Completed => EmitEventKind::RunCompleted,
            Self::Failed => EmitEventKind::RunFailed,
            Self::Aborted => EmitEventKind::RunAborted,
        }
    }

    fn ticket_state(self) -> TicketState {
        match self {
            Self::Completed => TicketState::Completed,
            Self::Failed => TicketState::Failed,
            Self::Aborted => TicketState::Aborted,
        }
    }
}

/// Immutable payload and private ownership token for one claimed delivery.
#[derive(Debug)]
pub struct ClaimedComment {
    id: i64,
    token: String,
    /// Configured tracker source.
    source_id: String,
    /// Original external task identity.
    task_id: String,
    /// Original run identity, independent of later ticket reassignment.
    run_id: String,
    /// Persisted terminal kind.
    event_kind: EmitEventKind,
    /// Persisted body, reused byte-for-byte on retry.
    body: String,
    /// Attempts including this claim.
    attempts: u32,
}

impl ClaimedComment {
    /// Configured source identity captured when enqueued.
    #[must_use]
    pub fn source_id(&self) -> &str {
        &self.source_id
    }
    /// External task captured when enqueued.
    #[must_use]
    pub fn task_id(&self) -> &str {
        &self.task_id
    }
    /// Run that produced the comment.
    #[must_use]
    pub fn run_id(&self) -> &str {
        &self.run_id
    }
    /// Exact persisted body to send.
    #[must_use]
    pub fn body(&self) -> &str {
        &self.body
    }

    fn key(&self) -> EmitKey<'_> {
        EmitKey {
            source_id: &self.source_id,
            task_id: &self.task_id,
            event_kind: self.event_kind,
            run_id: &self.run_id,
        }
    }
}

/// Complete only the still-correlated active ticket and enqueue its comment in
/// one transaction. Enqueue failure rolls back the terminal state. Returns true
/// when this call changed the ticket. Existing emission acknowledgment suppresses
/// enqueue but never suppresses repairing the ticket state. No historical backfill.
pub fn enqueue_terminal(
    conn: &Connection,
    task_id: &str,
    run_id: &str,
    kind: TerminalCommentKind,
    body: &str,
    now_ms: i64,
) -> rusqlite::Result<bool> {
    // SQLite still rejects BEGIN inside an active transaction. Borrowing only
    // &Connection keeps the caller's native owner intact without raw extraction.
    let tx = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    let source: Option<String> = tx
        .query_row(
            "SELECT source_id FROM ticket_index WHERE task_id = ?1 AND run_id = ?2
         AND state IN ('Active', 'RunStarted')",
            params![task_id, run_id],
            |row| row.get(0),
        )
        .optional()?;
    let Some(source) = source else {
        return Ok(false);
    };
    tx.execute(
        "UPDATE ticket_index SET state = ?1 WHERE task_id = ?2 AND run_id = ?3
         AND state IN ('Active', 'RunStarted')",
        params![kind.ticket_state().as_str(), task_id, run_id],
    )?;
    let key = EmitKey {
        source_id: &source,
        task_id,
        event_kind: kind.emit_kind(),
        run_id,
    };
    if !intake_emit_log::has(&tx, key)? {
        let body = format!("{body}\n\nSurge run: `{run_id}`.");
        tx.execute(
            "INSERT INTO terminal_comment_outbox
             (source_id, task_id, run_id, event_kind, body, created_at, next_attempt_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)
             ON CONFLICT(source_id, task_id, event_kind, run_id) DO NOTHING",
            params![
                source,
                task_id,
                run_id,
                kind.emit_kind().as_str(),
                body,
                now_ms
            ],
        )?;
    }
    tx.commit()?;
    Ok(true)
}

/// Exclusively claim the oldest due delivery, including an expired claim.
/// Database ownership is released before the caller performs external I/O.
pub fn claim(conn: &Connection, now_ms: i64) -> rusqlite::Result<Option<ClaimedComment>> {
    // SQLite still rejects BEGIN inside an active transaction. Borrowing only
    // &Connection keeps the caller's native owner intact without raw extraction.
    let tx = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    let id: Option<i64> = tx
        .query_row(
            "SELECT id FROM terminal_comment_outbox WHERE delivered_at IS NULL
         AND next_attempt_at <= ?1 AND (lease_until IS NULL OR lease_until <= ?1)
         ORDER BY next_attempt_at, id LIMIT 1",
            [now_ms],
            |row| row.get(0),
        )
        .optional()?;
    let Some(id) = id else {
        return Ok(None);
    };
    tx.execute(
        "UPDATE terminal_comment_outbox SET lease_token = lower(hex(randomblob(16))),
         lease_until = ?1, attempts = attempts + 1 WHERE id = ?2",
        params![now_ms.saturating_add(30_000), id],
    )?;
    let comment = tx.query_row(
        "SELECT id, lease_token, source_id, task_id, run_id, event_kind, body, attempts
         FROM terminal_comment_outbox WHERE id = ?1",
        [id],
        |row| {
            let raw: String = row.get(5)?;
            let event_kind = EmitEventKind::parse(&raw)
                .filter(|kind| {
                    matches!(
                        kind,
                        EmitEventKind::RunCompleted
                            | EmitEventKind::RunFailed
                            | EmitEventKind::RunAborted
                    )
                })
                .ok_or_else(|| {
                    rusqlite::Error::FromSqlConversionFailure(
                        5,
                        rusqlite::types::Type::Text,
                        "invalid terminal comment kind".into(),
                    )
                })?;
            Ok(ClaimedComment {
                id: row.get(0)?,
                token: row.get(1)?,
                source_id: row.get(2)?,
                task_id: row.get(3)?,
                run_id: row.get(4)?,
                event_kind,
                body: row.get(6)?,
                attempts: row.get(7)?,
            })
        },
    );
    let comment = match comment {
        Ok(comment) => comment,
        Err(error) => {
            tx.execute(
                "UPDATE terminal_comment_outbox SET lease_token = NULL, lease_until = NULL,
                 next_attempt_at = ?1, last_error = ?2 WHERE id = ?3",
                params![
                    now_ms.saturating_add(300_000),
                    format!("invalid queued comment: {error}"),
                    id
                ],
            )?;
            tx.commit()?;
            return Ok(None);
        },
    };
    tx.commit()?;
    Ok(Some(comment))
}

/// Acknowledge delivery and emission identity atomically, only for a current,
/// unexpired lease. Existing emission rows also allow this delivery to settle.
pub fn acknowledge(
    conn: &Connection,
    comment: &ClaimedComment,
    now_ms: i64,
) -> rusqlite::Result<bool> {
    // SQLite still rejects BEGIN inside an active transaction. Borrowing only
    // &Connection keeps the caller's native owner intact without raw extraction.
    let tx = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    let changed = tx.execute(
        "UPDATE terminal_comment_outbox SET delivered_at = ?1, lease_token = NULL,
         lease_until = NULL, last_error = NULL WHERE id = ?2 AND lease_token = ?3
         AND lease_until > ?1 AND delivered_at IS NULL",
        params![now_ms, comment.id, comment.token],
    )?;
    if changed == 0 {
        return Ok(false);
    }
    intake_emit_log::record(&tx, comment.key())?;
    tx.commit()?;
    Ok(true)
}

/// Retain a failure and schedule capped exponential backoff. A stale token cannot
/// clear a replacement lease. Malformed task/source diagnostics are retryable too.
pub fn retry(
    conn: &Connection,
    comment: &ClaimedComment,
    now_ms: i64,
    error: &str,
) -> rusqlite::Result<bool> {
    let exponent = comment.attempts.saturating_sub(1).min(6);
    let delay_ms = (5_000_i64 * (1_i64 << exponent)).min(300_000);
    let error: String = error.chars().take(1_000).collect();
    Ok(conn.execute(
        "UPDATE terminal_comment_outbox SET next_attempt_at = ?1, last_error = ?2,
         lease_token = NULL, lease_until = NULL WHERE id = ?3 AND lease_token = ?4
         AND delivered_at IS NULL AND lease_until > ?5",
        params![
            now_ms.saturating_add(delay_ms),
            error,
            comment.id,
            comment.token,
            now_ms
        ],
    )? == 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runs::{Clock, MockClock};

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE ticket_index(task_id TEXT PRIMARY KEY, source_id TEXT, run_id TEXT, state TEXT);
            INSERT INTO ticket_index VALUES ('mock:test#1', 'mock:test', 'run-1', 'RunStarted');").unwrap();
        conn.execute_batch(include_str!(
            "runs/migrations/registry/0013_intake_emit_log.sql"
        ))
        .unwrap();
        conn.execute_batch(include_str!(
            "runs/migrations/registry/0020_terminal_comment_outbox.sql"
        ))
        .unwrap();
        conn
    }

    fn enqueue(conn: &Connection, now_ms: i64) {
        assert!(
            enqueue_terminal(
                conn,
                "mock:test#1",
                "run-1",
                TerminalCommentKind::Completed,
                "completed",
                now_ms
            )
            .unwrap()
        );
    }

    #[test]
    fn shared_connection_entry_preserves_nested_transaction_refusal() {
        let conn = db();
        let outer = conn.unchecked_transaction().unwrap();
        let result = enqueue_terminal(
            &conn,
            "mock:test#1",
            "run-1",
            TerminalCommentKind::Completed,
            "completed",
            1000,
        );
        assert!(result.is_err());
        assert!(
            !conn.is_autocommit(),
            "failed nested admission cannot end the outer transaction"
        );
        assert_eq!(
            conn.query_row("SELECT state FROM ticket_index", [], |row| row
                .get::<_, String>(0))
                .unwrap(),
            "RunStarted"
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM terminal_comment_outbox", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
            0
        );
        outer.rollback().unwrap();
        assert!(
            enqueue_terminal(
                &conn,
                "mock:test#1",
                "run-1",
                TerminalCommentKind::Completed,
                "completed",
                1000
            )
            .unwrap()
        );
    }

    #[test]
    fn enqueue_failure_rolls_back_ticket_and_identity_is_immutable() {
        let conn = db();
        conn.execute_batch("CREATE TRIGGER reject_outbox BEFORE INSERT ON terminal_comment_outbox BEGIN SELECT RAISE(ABORT, 'full'); END;").unwrap();
        assert!(
            enqueue_terminal(
                &conn,
                "mock:test#1",
                "run-1",
                TerminalCommentKind::Completed,
                "completed",
                1000
            )
            .is_err()
        );
        assert_eq!(
            conn.query_row("SELECT state FROM ticket_index", [], |row| row
                .get::<_, String>(0))
                .unwrap(),
            "RunStarted"
        );
        conn.execute_batch("DROP TRIGGER reject_outbox;").unwrap();
        enqueue(&conn, 1000);
        assert!(
            !enqueue_terminal(
                &conn,
                "mock:test#1",
                "run-1",
                TerminalCommentKind::Failed,
                "other",
                1001
            )
            .unwrap()
        );
        assert!(
            conn.execute("UPDATE terminal_comment_outbox SET body = 'changed'", [])
                .is_err()
        );
        let comment = claim(&conn, 1000).unwrap().unwrap();
        assert_eq!(comment.body, "completed\n\nSurge run: `run-1`.");
        assert!(claim(&conn, 1000).unwrap().is_none());
    }

    #[test]
    fn retry_backoff_expiry_and_stale_owners_are_fenced() {
        let conn = db();
        let clock = MockClock::new(1000);
        enqueue(&conn, clock.now_ms());
        let first = claim(&conn, clock.now_ms()).unwrap().unwrap();
        clock.advance(30_000);
        assert!(!acknowledge(&conn, &first, clock.now_ms()).unwrap());
        assert!(!retry(&conn, &first, clock.now_ms(), "expired failure").unwrap());
        let replacement = claim(&conn, clock.now_ms()).unwrap().unwrap();
        assert!(!acknowledge(&conn, &first, clock.now_ms()).unwrap());
        assert!(!retry(&conn, &first, clock.now_ms(), "stale failure").unwrap());
        assert!(retry(&conn, &replacement, clock.now_ms(), "network down").unwrap());
        assert!(claim(&conn, clock.now_ms()).unwrap().is_none());
        clock.advance(10_000);
        let third = claim(&conn, clock.now_ms()).unwrap().unwrap();
        assert_eq!(third.attempts, 3);
        assert_eq!(third.body, first.body);
        assert!(acknowledge(&conn, &third, clock.now_ms()).unwrap());
        assert!(intake_emit_log::has(&conn, third.key()).unwrap());
        assert!(claim(&conn, clock.now_ms() + 1_000_000).unwrap().is_none());
    }

    #[test]
    fn prior_ack_repairs_active_state_without_enqueue_and_ack_is_idempotent() {
        let conn = db();
        let key = EmitKey {
            source_id: "mock:test",
            task_id: "mock:test#1",
            run_id: "run-1",
            event_kind: EmitEventKind::RunCompleted,
        };
        intake_emit_log::record(&conn, key).unwrap();
        enqueue(&conn, 1000);
        assert!(claim(&conn, 1000).unwrap().is_none());
        assert_eq!(
            conn.query_row("SELECT state FROM ticket_index", [], |row| row
                .get::<_, String>(0))
                .unwrap(),
            "Completed"
        );
        conn.execute_batch(
            "DELETE FROM intake_emit_log; UPDATE ticket_index SET state = 'Active';",
        )
        .unwrap();
        enqueue(&conn, 1000);
        let comment = claim(&conn, 1000).unwrap().unwrap();
        intake_emit_log::record(&conn, comment.key()).unwrap();
        assert!(acknowledge(&conn, &comment, 1001).unwrap());
        assert!(!acknowledge(&conn, &comment, 1002).unwrap());
    }

    #[test]
    fn concurrent_claims_and_reopen_preserve_one_payload() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("registry.sqlite");
        let source = db();
        let conn = Connection::open(&path).unwrap();
        drop(source);
        conn.execute_batch("CREATE TABLE ticket_index(task_id TEXT PRIMARY KEY, source_id TEXT, run_id TEXT, state TEXT);
            INSERT INTO ticket_index VALUES ('mock:test#1', 'mock:test', 'run-1', 'RunStarted');").unwrap();
        conn.execute_batch(include_str!(
            "runs/migrations/registry/0013_intake_emit_log.sql"
        ))
        .unwrap();
        conn.execute_batch(include_str!(
            "runs/migrations/registry/0020_terminal_comment_outbox.sql"
        ))
        .unwrap();
        enqueue(&conn, 1000);
        drop(conn);
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let threads: Vec<_> = (0..2)
            .map(|_| {
                let path = path.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    let conn = Connection::open(path).unwrap();
                    conn.busy_timeout(std::time::Duration::from_secs(2))
                        .unwrap();
                    barrier.wait();
                    claim(&conn, 1000).unwrap().is_some()
                })
            })
            .collect();
        let winners = threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .filter(|won| *won)
            .count();
        assert_eq!(winners, 1);
        let conn = Connection::open(path).unwrap();
        assert!(claim(&conn, 1001).unwrap().is_none());
        let recovered = claim(&conn, 31_000).unwrap().unwrap();
        assert_eq!(recovered.body, "completed\n\nSurge run: `run-1`.");
        assert!(acknowledge(&conn, &recovered, 31_001).unwrap());
    }
    #[test]
    fn malformed_due_row_is_retained_without_starving_the_next_job() {
        let conn = db();
        enqueue(&conn, 1000);
        conn.execute_batch(
            "DROP TRIGGER terminal_comment_outbox_immutable;
            PRAGMA ignore_check_constraints = ON;
            UPDATE terminal_comment_outbox SET event_kind = 'unknown';
            INSERT INTO ticket_index VALUES ('mock:test#2', 'mock:test', 'run-2', 'Active');",
        )
        .unwrap();
        assert!(
            enqueue_terminal(
                &conn,
                "mock:test#2",
                "run-2",
                TerminalCommentKind::Aborted,
                "cancelled",
                1000
            )
            .unwrap()
        );
        assert!(claim(&conn, 1000).unwrap().is_none());
        let healthy = claim(&conn, 1000).unwrap().unwrap();
        assert_eq!(healthy.run_id(), "run-2");
        let diagnostic: String = conn
            .query_row(
                "SELECT last_error FROM terminal_comment_outbox WHERE run_id = 'run-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(diagnostic.contains("invalid queued comment"));
    }

    #[test]
    fn migration_upgrades_existing_ack_and_ticket_without_backfill() {
        use crate::runs::migrations::{self, REGISTRY_MIGRATIONS};
        let mut conn = Connection::open_in_memory().unwrap();
        let clock = MockClock::new(1000);
        // Actual prior-schema migration set, rather than a hand-written fake schema.
        let prior: &'static [(&'static str, &'static str)] =
            &REGISTRY_MIGRATIONS[..REGISTRY_MIGRATIONS.len() - 1];
        migrations::apply(&mut conn, prior, &clock).unwrap();
        conn.execute("INSERT INTO ticket_index (task_id, source_id, provider, state, first_seen, last_seen) VALUES ('mock:test#old', 'mock:test', 'mock', 'Completed', '2026-09-30T00:00:00Z', '2026-09-30T00:00:00Z')", []).unwrap();
        intake_emit_log::record(
            &conn,
            EmitKey {
                source_id: "mock:test",
                task_id: "mock:test#old",
                event_kind: EmitEventKind::RunCompleted,
                run_id: "old-run",
            },
        )
        .unwrap();
        migrations::apply(&mut conn, REGISTRY_MIGRATIONS, &clock).unwrap();
        migrations::apply(&mut conn, REGISTRY_MIGRATIONS, &clock).unwrap();
        assert_eq!(
            conn.query_row("SELECT state FROM ticket_index", [], |row| row
                .get::<_, String>(0))
                .unwrap(),
            "Completed"
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM intake_emit_log", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert!(claim(&conn, clock.now_ms()).unwrap().is_none());
    }
}
