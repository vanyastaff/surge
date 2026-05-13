//! Per-side-effect idempotency log for outbound intake actions.
//!
//! Every outbound action (tracker comment, label change, merge proposal)
//! is keyed by `(source_id, task_id, event_kind, run_id)`. Before emitting
//! the side-effect, call [`has`] — if it returns `true`, skip; otherwise
//! perform the side-effect and call [`record`] to mark the row.
//!
//! Backed by the `intake_emit_log` table (registry migration 0013). The
//! table uses `INSERT OR IGNORE` so concurrent emitters cannot duplicate
//! rows; the call site checks the boolean return to decide whether the
//! side-effect must run.
//!
//! This layer is additive on top of per-source idempotency the comment
//! poster already performs (GitHub exact-body match, Linear idempotency
//! keys). It catches retries that survive daemon restarts and that span
//! multiple side-effect channels.

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

/// What kind of side-effect a row in `intake_emit_log` records.
///
/// Values map to a stable on-disk string via [`EmitEventKind::as_str`]. The
/// enum is `#[non_exhaustive]` so future tiers can add side-effect kinds
/// without a workspace-wide match-arm churn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum EmitEventKind {
    /// Triage-decision comment (Enqueued / Duplicate / OOS / Unclear).
    TriageDecision,
    /// "Surge run #X started" comment posted at handle_start.
    RunStarted,
    /// Completion comment with PR link and summary.
    RunCompleted,
    /// Failure comment with stage and reason.
    RunFailed,
    /// Abort comment (user-cancel or external close).
    RunAborted,
    /// L3 auto-merge action posted to the tracker.
    MergeProposed,
    /// L3 merge gate blocked (red checks / no review).
    MergeBlocked,
}

impl EmitEventKind {
    /// Stable on-disk string form (snake_case). Inverse of
    /// [`EmitEventKind::parse`].
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TriageDecision => "triage_decision",
            Self::RunStarted => "run_started",
            Self::RunCompleted => "run_completed",
            Self::RunFailed => "run_failed",
            Self::RunAborted => "run_aborted",
            Self::MergeProposed => "merge_proposed",
            Self::MergeBlocked => "merge_blocked",
        }
    }

    /// Inverse of [`EmitEventKind::as_str`]. Returns `None` on unknown
    /// strings — callers should treat this as a corrupted row.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "triage_decision" => Some(Self::TriageDecision),
            "run_started" => Some(Self::RunStarted),
            "run_completed" => Some(Self::RunCompleted),
            "run_failed" => Some(Self::RunFailed),
            "run_aborted" => Some(Self::RunAborted),
            "merge_proposed" => Some(Self::MergeProposed),
            "merge_blocked" => Some(Self::MergeBlocked),
            _ => None,
        }
    }
}

/// Composite key identifying one outbound side-effect emission.
///
/// `source_id` and `task_id` are inherited from the originating event;
/// `run_id` is the run that produced the side-effect (empty string is
/// reserved for pre-run actions like `TriageDecision` on a `Skipped`
/// L0 short-circuit — the dedup is then keyed by source + task + kind).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EmitKey<'a> {
    /// Tracker source identifier (e.g. `"github_issues:user/repo"`).
    pub source_id: &'a str,
    /// External ticket id formatted by the source.
    pub task_id: &'a str,
    /// Side-effect kind being emitted.
    pub event_kind: EmitEventKind,
    /// Run id correlated with the side-effect, or empty for pre-run
    /// emissions.
    pub run_id: &'a str,
}

/// One stored row of the `intake_emit_log` table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmitLogRow {
    /// Tracker source identifier.
    pub source_id: String,
    /// External ticket id.
    pub task_id: String,
    /// Side-effect kind.
    pub event_kind: EmitEventKind,
    /// Correlated run id (empty for pre-run emissions).
    pub run_id: String,
    /// Time the row was inserted.
    pub recorded_at: DateTime<Utc>,
}

/// `true` if a row for the given key already exists.
///
/// Use this as a precheck before performing the outbound side-effect.
/// Always pair with [`record`] on the success path so the dedup row is
/// inserted.
///
/// # Errors
/// Returns the underlying SQLite error if the query fails.
pub fn has(conn: &Connection, key: EmitKey<'_>) -> rusqlite::Result<bool> {
    let exists: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM intake_emit_log \
             WHERE source_id = ?1 AND task_id = ?2 AND event_kind = ?3 AND run_id = ?4 \
             LIMIT 1",
            params![
                key.source_id,
                key.task_id,
                key.event_kind.as_str(),
                key.run_id,
            ],
            |r| r.get(0),
        )
        .optional()?;
    Ok(exists.is_some())
}

/// Insert (idempotently) a side-effect dedup row. Returns `true` when
/// the row was newly inserted, `false` when it already existed.
///
/// Uses `INSERT OR IGNORE` to make concurrent callers safe: at most one
/// of them sees a `true` return, the others see `false`. The recommended
/// idiom is to call [`has`] first and only run the side-effect when it
/// returns `false`, then call [`record`] on success. Calling [`record`]
/// without a preceding [`has`] check is also safe — the return value
/// tells you whether you actually inserted.
///
/// # Errors
/// Returns the underlying SQLite error if the insert fails.
pub fn record(conn: &Connection, key: EmitKey<'_>) -> rusqlite::Result<bool> {
    let changes = conn.execute(
        "INSERT OR IGNORE INTO intake_emit_log \
            (source_id, task_id, event_kind, run_id, recorded_at) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            key.source_id,
            key.task_id,
            key.event_kind.as_str(),
            key.run_id,
            Utc::now().timestamp_millis(),
        ],
    )?;
    let inserted = changes > 0;
    if inserted {
        tracing::debug!(
            target: "intake::emit_log",
            source_id = %key.source_id,
            task_id = %key.task_id,
            event_kind = %key.event_kind.as_str(),
            run_id = %key.run_id,
            "emit_log row recorded"
        );
    } else {
        tracing::debug!(
            target: "intake::emit_log",
            source_id = %key.source_id,
            task_id = %key.task_id,
            event_kind = %key.event_kind.as_str(),
            run_id = %key.run_id,
            "emit_log row already exists; dedup hit"
        );
    }
    Ok(inserted)
}

/// Fetch a row by composite key (mostly for diagnostics + tests).
///
/// # Errors
/// Returns the underlying SQLite error if the query fails.
pub fn fetch(conn: &Connection, key: EmitKey<'_>) -> rusqlite::Result<Option<EmitLogRow>> {
    conn.query_row(
        "SELECT source_id, task_id, event_kind, run_id, recorded_at \
         FROM intake_emit_log \
         WHERE source_id = ?1 AND task_id = ?2 AND event_kind = ?3 AND run_id = ?4",
        params![
            key.source_id,
            key.task_id,
            key.event_kind.as_str(),
            key.run_id,
        ],
        |r| {
            let kind_str: String = r.get(2)?;
            let kind = EmitEventKind::parse(&kind_str).ok_or_else(|| {
                rusqlite::Error::FromSqlConversionFailure(
                    2,
                    rusqlite::types::Type::Text,
                    format!("unknown emit event kind: {kind_str}").into(),
                )
            })?;
            let recorded_ms: i64 = r.get(4)?;
            let recorded_at =
                DateTime::<Utc>::from_timestamp_millis(recorded_ms).ok_or_else(|| {
                    rusqlite::Error::FromSqlConversionFailure(
                        4,
                        rusqlite::types::Type::Integer,
                        format!("invalid timestamp ms: {recorded_ms}").into(),
                    )
                })?;
            Ok(EmitLogRow {
                source_id: r.get(0)?,
                task_id: r.get(1)?,
                event_kind: kind,
                run_id: r.get(3)?,
                recorded_at,
            })
        },
    )
    .optional()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        let sql = include_str!("runs/migrations/registry/0013_intake_emit_log.sql");
        conn.execute_batch(sql).unwrap();
        conn
    }

    fn key<'a>(source: &'a str, task: &'a str, kind: EmitEventKind, run: &'a str) -> EmitKey<'a> {
        EmitKey {
            source_id: source,
            task_id: task,
            event_kind: kind,
            run_id: run,
        }
    }

    #[test]
    fn has_returns_false_for_empty_table() {
        let conn = db();
        let exists = has(
            &conn,
            key(
                "github",
                "github:user/repo#1",
                EmitEventKind::RunStarted,
                "r1",
            ),
        )
        .unwrap();
        assert!(!exists);
    }

    #[test]
    fn record_then_has_round_trip() {
        let conn = db();
        let k = key(
            "github",
            "github:u/r#7",
            EmitEventKind::RunCompleted,
            "run-42",
        );
        let inserted = record(&conn, k.clone()).unwrap();
        assert!(inserted);
        assert!(has(&conn, k).unwrap());
    }

    #[test]
    fn record_is_idempotent() {
        let conn = db();
        let k = key("lin", "linear:t/X-1", EmitEventKind::TriageDecision, "");
        assert!(record(&conn, k.clone()).unwrap());
        // Second call: row already exists, returns false.
        assert!(!record(&conn, k.clone()).unwrap());
        assert!(has(&conn, k).unwrap());
    }

    #[test]
    fn different_kinds_are_separate_rows() {
        let conn = db();
        let started = key("gh", "gh:o/r#1", EmitEventKind::RunStarted, "run-1");
        let completed = key("gh", "gh:o/r#1", EmitEventKind::RunCompleted, "run-1");
        assert!(record(&conn, started.clone()).unwrap());
        assert!(!has(&conn, completed.clone()).unwrap());
        assert!(record(&conn, completed.clone()).unwrap());
        assert!(has(&conn, started).unwrap());
        assert!(has(&conn, completed).unwrap());
    }

    #[test]
    fn different_runs_are_separate_rows() {
        let conn = db();
        let k1 = key("gh", "gh:o/r#1", EmitEventKind::MergeProposed, "run-1");
        let k2 = key("gh", "gh:o/r#1", EmitEventKind::MergeProposed, "run-2");
        assert!(record(&conn, k1.clone()).unwrap());
        assert!(record(&conn, k2.clone()).unwrap());
        assert!(has(&conn, k1).unwrap());
        assert!(has(&conn, k2).unwrap());
    }

    #[test]
    fn fetch_returns_row_with_kind_round_trip() {
        let conn = db();
        let k = key("gh", "gh:o/r#9", EmitEventKind::MergeBlocked, "run-9");
        record(&conn, k.clone()).unwrap();
        let row = fetch(&conn, k).unwrap().expect("row present");
        assert_eq!(row.event_kind, EmitEventKind::MergeBlocked);
        assert_eq!(row.source_id, "gh");
        assert_eq!(row.task_id, "gh:o/r#9");
        assert_eq!(row.run_id, "run-9");
    }

    #[test]
    fn fetch_returns_none_for_missing() {
        let conn = db();
        let result = fetch(
            &conn,
            key("gh", "gh:o/r#404", EmitEventKind::RunFailed, "run-x"),
        )
        .unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn event_kind_serde_round_trip() {
        let cases = [
            EmitEventKind::TriageDecision,
            EmitEventKind::RunStarted,
            EmitEventKind::RunCompleted,
            EmitEventKind::RunFailed,
            EmitEventKind::RunAborted,
            EmitEventKind::MergeProposed,
            EmitEventKind::MergeBlocked,
        ];
        for kind in cases {
            let s = kind.as_str();
            assert_eq!(EmitEventKind::parse(s), Some(kind));
        }
        assert_eq!(EmitEventKind::parse("bogus"), None);
    }
}
