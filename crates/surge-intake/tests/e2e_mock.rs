//! End-to-end mock pipeline test.
//!
//! Exercises the intake side of RFC-0010:
//!   MockTaskSource → TaskRouter → Tier-1 dedup
//!
//! First wave: a fresh ticket flows through as `RouterOutput::Triage`.
//! Then we simulate that a Surge run was created for it (insert into
//! `ticket_index` with `state=Active`).
//! Second wave: the same ticket appears again, and the router emits
//! `RouterOutput::EarlyDuplicate` (Tier-1 detected the active run).
//!
//! Daemon-side wiring (Triage Author + InboxCard delivery) is exercised
//! separately and is not part of this test.

use chrono::Utc;
use rusqlite::Connection;
use std::sync::Arc;
use std::time::Duration;
use surge_intake::router::{RouterOutput, TaskRouter};
use surge_intake::testing::MockTaskSource;
use surge_intake::types::{TaskEvent, TaskEventKind, TaskId};
use surge_intake::TaskSource;
use surge_persistence::intake::{IntakeRepo, IntakeRow, TicketState};
use tokio::sync::{mpsc, Mutex};

/// Initialise an in-memory database with the schema needed by the router.
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

fn ev(task_id: &str) -> TaskEvent {
    TaskEvent {
        source_id: "mock:t".into(),
        task_id: TaskId::try_new(task_id).unwrap(),
        kind: TaskEventKind::NewTask,
        seen_at: Utc::now(),
        raw_payload: serde_json::json!({}),
    }
}

#[tokio::test]
async fn e2e_new_task_then_dup() {
    let conn = Arc::new(Mutex::new(db()));

    // ----- Wave 1: fresh ticket. Should pass through as Triage. -----
    {
        let src = Arc::new(MockTaskSource::new("mock:t", "mock"));
        src.push_event(ev("mock:t#1")).await;
        let (tx, mut rx) = mpsc::channel(8);
        let router = TaskRouter::new(vec![src as Arc<dyn TaskSource>], Arc::clone(&conn), tx);
        let handle = tokio::spawn(router.run());

        let out = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("router did not emit in time")
            .expect("channel closed");
        match out {
            RouterOutput::Triage { event } => {
                assert_eq!(event.task_id.as_str(), "mock:t#1");
            }
            other => panic!("expected Triage, got {other:?}"),
        }
        // Drop receiver so router exits cleanly.
        drop(rx);
        let _ = handle.await;
    }

    // ----- Simulate that a Surge run got created for the ticket. -----
    {
        let c = conn.lock().await;
        c.execute("INSERT INTO runs(id) VALUES ('run_xyz')", [])
            .unwrap();
        IntakeRepo::new(&c)
            .insert(&IntakeRow {
                task_id: "mock:t#1".into(),
                source_id: "mock:t".into(),
                provider: "mock".into(),
                run_id: Some("run_xyz".into()),
                triage_decision: Some("enqueued".into()),
                duplicate_of: None,
                priority: Some("medium".into()),
                state: TicketState::Active,
                first_seen: Utc::now(),
                last_seen: Utc::now(),
                snooze_until: None,
            })
            .unwrap();
    }

    // ----- Wave 2: same ticket appears again. Should be EarlyDuplicate. -----
    {
        let src = Arc::new(MockTaskSource::new("mock:t", "mock"));
        src.push_event(ev("mock:t#1")).await;
        let (tx, mut rx) = mpsc::channel(8);
        let router = TaskRouter::new(vec![src as Arc<dyn TaskSource>], Arc::clone(&conn), tx);
        let handle = tokio::spawn(router.run());

        let out = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("router did not emit")
            .expect("channel closed");
        match out {
            RouterOutput::EarlyDuplicate { run_id, event } => {
                assert_eq!(run_id, "run_xyz");
                assert_eq!(event.task_id.as_str(), "mock:t#1");
            }
            other => panic!("expected EarlyDuplicate, got {other:?}"),
        }
        drop(rx);
        let _ = handle.await;
    }
}
