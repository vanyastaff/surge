//! Integration test: two MockTaskSource instances feed the same router.
//! Both events should be observed.

use chrono::Utc;
use rusqlite::Connection;
use std::sync::Arc;
use std::time::Duration;
use surge_intake::TaskSource;
use surge_intake::router::{RouterOutput, TaskRouter};
use surge_intake::testing::MockTaskSource;
use surge_intake::types::{TaskEvent, TaskEventKind, TaskId};
use tokio::sync::{Mutex, mpsc};

fn ev(source_id: &str, task_id: &str) -> TaskEvent {
    TaskEvent {
        source_id: source_id.into(),
        task_id: TaskId::try_new(task_id).unwrap(),
        kind: TaskEventKind::NewTask,
        seen_at: Utc::now(),
        raw_payload: serde_json::json!({}),
    }
}

fn db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE runs (id TEXT PRIMARY KEY);")
        .unwrap();
    let sql =
        include_str!("../../surge-persistence/src/runs/migrations/registry/0002_ticket_index.sql");
    conn.execute_batch(sql).unwrap();
    conn
}

#[tokio::test]
async fn two_sources_both_observed() {
    let conn = Arc::new(Mutex::new(db()));

    let src_a = Arc::new(MockTaskSource::new("mock:A", "mock"));
    let src_b = Arc::new(MockTaskSource::new("mock:B", "mock"));
    src_a.push_event(ev("mock:A", "mock:A#1")).await;
    src_b.push_event(ev("mock:B", "mock:B#1")).await;

    let (tx, mut rx) = mpsc::channel(8);
    let router = TaskRouter::new(
        vec![src_a as Arc<dyn TaskSource>, src_b as Arc<dyn TaskSource>],
        Arc::clone(&conn),
        tx,
    );
    let handle = tokio::spawn(router.run());

    let mut seen: Vec<String> = Vec::new();
    for _ in 0..2 {
        let item = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("router did not emit in time")
            .expect("channel closed");
        match item {
            RouterOutput::Triage { event } => seen.push(event.task_id.as_str().into()),
            other => panic!("unexpected: {other:?}"),
        }
    }

    seen.sort();
    assert_eq!(seen, vec!["mock:A#1".to_string(), "mock:B#1".to_string()]);

    drop(rx);
    let _ = handle.await;
}
