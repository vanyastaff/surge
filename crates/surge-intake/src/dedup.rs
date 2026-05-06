//! Tier-1 PreFilter: computational deduplication. No LLM, no network.
//!
//! MVP step: active-run lookup against `ticket_index`. Other steps
//! (embedding similarity for B/C in RFC-0010) are deferred to RFC-0014.

use crate::types::{TaskEvent, Tier1Decision};
use crate::{Error, Result};
use rusqlite::Connection;
use surge_persistence::intake::IntakeRepo;
use tracing::trace;

/// Tier-1 (computational) dedup pre-filter for incoming `TaskEvent`s.
///
/// Borrows a `&Connection`; the caller is responsible for locking around it.
/// This component does not own state; instantiate per call.
pub struct Tier1PreFilter<'a> {
    conn: &'a Connection,
}

impl<'a> Tier1PreFilter<'a> {
    /// Construct a pre-filter borrowing the given connection.
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    /// Decide whether the event represents a duplicate of an active run.
    pub fn check(&self, event: &TaskEvent) -> Result<Tier1Decision> {
        let repo = IntakeRepo::new(self.conn);
        let task_id = event.task_id.as_str();

        match repo.lookup_active_run(task_id) {
            Ok(Some(run_id)) => {
                trace!(?task_id, %run_id, "tier1 early-duplicate hit");
                Ok(Tier1Decision::EarlyDuplicate { run_id })
            },
            Ok(None) => {
                trace!(?task_id, "tier1 pass");
                Ok(Tier1Decision::Pass)
            },
            Err(e) => Err(Error::Storage(e.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{TaskEventKind, TaskId};
    use chrono::Utc;
    use rusqlite::Connection;
    use surge_persistence::intake::{IntakeRepo, IntakeRow, TicketState};

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE runs (id TEXT PRIMARY KEY);")
            .unwrap();
        let sql = include_str!(
            "../../surge-persistence/src/runs/migrations/registry/0002_ticket_index.sql"
        );
        conn.execute_batch(sql).unwrap();
        conn
    }

    fn sample_event(task_id: &str) -> TaskEvent {
        TaskEvent {
            source_id: "linear:wsp1".into(),
            task_id: TaskId::try_new(task_id).unwrap(),
            kind: TaskEventKind::NewTask,
            seen_at: Utc::now(),
            raw_payload: serde_json::json!({}),
        }
    }

    fn sample_row(task_id: &str, run_id: Option<&str>, state: TicketState) -> IntakeRow {
        IntakeRow {
            task_id: task_id.into(),
            source_id: "linear:wsp1".into(),
            provider: "linear".into(),
            run_id: run_id.map(String::from),
            triage_decision: None,
            duplicate_of: None,
            priority: None,
            state,
            first_seen: Utc::now(),
            last_seen: Utc::now(),
            snooze_until: None,
        }
    }

    #[test]
    fn pass_when_no_existing_row() {
        let conn = db();
        let f = Tier1PreFilter::new(&conn);
        let dec = f.check(&sample_event("linear:wsp1/A-1")).unwrap();
        assert_eq!(dec, Tier1Decision::Pass);
    }

    #[test]
    fn early_duplicate_when_active_run_exists() {
        let conn = db();
        conn.execute("INSERT INTO runs(id) VALUES ('run_x')", [])
            .unwrap();
        IntakeRepo::new(&conn)
            .insert(&sample_row(
                "linear:wsp1/A-2",
                Some("run_x"),
                TicketState::Active,
            ))
            .unwrap();
        let f = Tier1PreFilter::new(&conn);
        let dec = f.check(&sample_event("linear:wsp1/A-2")).unwrap();
        assert_eq!(
            dec,
            Tier1Decision::EarlyDuplicate {
                run_id: "run_x".into()
            }
        );
    }

    #[test]
    fn pass_when_existing_run_completed() {
        let conn = db();
        conn.execute("INSERT INTO runs(id) VALUES ('run_done')", [])
            .unwrap();
        IntakeRepo::new(&conn)
            .insert(&sample_row(
                "linear:wsp1/A-3",
                Some("run_done"),
                TicketState::Completed,
            ))
            .unwrap();
        let f = Tier1PreFilter::new(&conn);
        let dec = f.check(&sample_event("linear:wsp1/A-3")).unwrap();
        assert_eq!(dec, Tier1Decision::Pass);
    }
}
