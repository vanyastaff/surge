//! Integration test for RFC-0010 acceptance criterion #5.
//!
//! Verifies that the daemon's run-completion → tracker-comment hook
//! (`surge_daemon::intake_completion`) reacts to a `RunFinished` global
//! event by posting a status comment to the originating ticket and
//! transitioning the ticket FSM to the matching terminal state.
//!
//! The test drives the consumer directly: it constructs the same
//! components the daemon's `main.rs` wires up (a `MockTaskSource`
//! registry, an in-memory SQLite registry DB, a fresh broadcast
//! channel) and publishes synthetic `RunFinished` events.
//! No daemon binary is spawned.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use rusqlite::Connection;
use surge_core::id::RunId;
use surge_core::keys::NodeKey;
use surge_daemon::intake_completion;
use surge_intake::TaskSource;
use surge_intake::testing::MockTaskSource;
use surge_orchestrator::engine::handle::RunOutcome;
use surge_orchestrator::engine::ipc::GlobalDaemonEvent;
use surge_persistence::intake::{IntakeRepo, IntakeRow, TicketState};
use tokio::sync::{Mutex as TokioMutex, broadcast};

fn db_with_schema() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE runs (id TEXT PRIMARY KEY);")
        .unwrap();
    let sql =
        include_str!("../../surge-persistence/src/runs/migrations/registry/0002_ticket_index.sql");
    conn.execute_batch(sql).unwrap();
    conn
}

fn seed_ticket(conn: &Connection, task_id: &str, run_id: &str) {
    conn.execute("INSERT INTO runs(id) VALUES (?1)", [run_id])
        .unwrap();
    let repo = IntakeRepo::new(conn);
    repo.insert(&IntakeRow {
        task_id: task_id.into(),
        source_id: "mock:test".into(),
        provider: "mock".into(),
        run_id: Some(run_id.into()),
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

struct Setup {
    src: Arc<MockTaskSource>,
    map: Arc<HashMap<String, Arc<dyn TaskSource>>>,
    conn: Arc<TokioMutex<Connection>>,
    tx: broadcast::Sender<GlobalDaemonEvent>,
    rx: broadcast::Receiver<GlobalDaemonEvent>,
}

fn make_setup() -> Setup {
    let src = Arc::new(MockTaskSource::new("mock:test", "mock"));
    let mut map: HashMap<String, Arc<dyn TaskSource>> = HashMap::new();
    map.insert("mock:test".into(), Arc::clone(&src) as Arc<dyn TaskSource>);
    let map = Arc::new(map);
    let conn = Arc::new(TokioMutex::new(db_with_schema()));
    let (tx, rx) = broadcast::channel(8);
    Setup {
        src,
        map,
        conn,
        tx,
        rx,
    }
}

#[tokio::test]
async fn run_completed_posts_success_comment_and_transitions_state() {
    let Setup {
        src,
        map,
        conn,
        tx,
        rx,
    } = make_setup();
    let run_id = RunId::new();
    let run_id_str = run_id.to_string();
    {
        let guard = conn.lock().await;
        seed_ticket(&guard, "mock:test#1", &run_id_str);
    }

    let _handle = intake_completion::spawn(rx, map, Arc::clone(&conn));

    tx.send(GlobalDaemonEvent::RunFinished {
        run_id,
        outcome: RunOutcome::Completed {
            terminal: NodeKey::try_new("end").unwrap(),
        },
    })
    .unwrap();

    tokio::time::sleep(Duration::from_millis(150)).await;

    let comments = src.posted_comments().await;
    assert_eq!(comments.len(), 1, "expected exactly one comment");
    assert!(comments[0].1.starts_with("✅"), "got: {}", comments[0].1);
    assert!(
        comments[0].1.contains("end"),
        "expected terminal node in body, got: {}",
        comments[0].1
    );

    let guard = conn.lock().await;
    let row = IntakeRepo::new(&guard)
        .lookup_ticket_by_run_id(&run_id_str)
        .unwrap()
        .unwrap();
    assert_eq!(row.state, TicketState::Completed);
}

#[tokio::test]
async fn run_failed_posts_failure_comment_and_transitions_state() {
    let Setup {
        src,
        map,
        conn,
        tx,
        rx,
    } = make_setup();
    let run_id = RunId::new();
    let run_id_str = run_id.to_string();
    {
        let guard = conn.lock().await;
        seed_ticket(&guard, "mock:test#2", &run_id_str);
    }

    let _handle = intake_completion::spawn(rx, map, Arc::clone(&conn));

    tx.send(GlobalDaemonEvent::RunFinished {
        run_id,
        outcome: RunOutcome::Failed {
            error: "graph validation error".into(),
        },
    })
    .unwrap();

    tokio::time::sleep(Duration::from_millis(150)).await;

    let comments = src.posted_comments().await;
    assert_eq!(comments.len(), 1);
    assert!(
        comments[0].1.starts_with("❌ Run failed:"),
        "got: {}",
        comments[0].1
    );
    assert!(comments[0].1.contains("graph validation error"));

    let guard = conn.lock().await;
    let row = IntakeRepo::new(&guard)
        .lookup_ticket_by_run_id(&run_id_str)
        .unwrap()
        .unwrap();
    assert_eq!(row.state, TicketState::Failed);
}

#[tokio::test]
async fn run_aborted_posts_abort_comment_and_transitions_state() {
    let Setup {
        src,
        map,
        conn,
        tx,
        rx,
    } = make_setup();
    let run_id = RunId::new();
    let run_id_str = run_id.to_string();
    {
        let guard = conn.lock().await;
        seed_ticket(&guard, "mock:test#3", &run_id_str);
    }

    let _handle = intake_completion::spawn(rx, map, Arc::clone(&conn));

    tx.send(GlobalDaemonEvent::RunFinished {
        run_id,
        outcome: RunOutcome::Aborted {
            reason: "user pressed Stop".into(),
        },
    })
    .unwrap();

    tokio::time::sleep(Duration::from_millis(150)).await;

    let comments = src.posted_comments().await;
    assert_eq!(comments.len(), 1);
    assert!(comments[0].1.starts_with("Run aborted:"));
    assert!(comments[0].1.contains("user pressed Stop"));

    let guard = conn.lock().await;
    let row = IntakeRepo::new(&guard)
        .lookup_ticket_by_run_id(&run_id_str)
        .unwrap()
        .unwrap();
    assert_eq!(row.state, TicketState::Aborted);
}

#[tokio::test]
async fn run_finished_with_no_matching_ticket_is_a_no_op() {
    let Setup {
        src,
        map,
        conn,
        tx,
        rx,
    } = make_setup();
    // Note: no `seed_ticket` — DB is empty, so the run_id we publish
    // does not match any ticket row.
    let _handle = intake_completion::spawn(rx, map, Arc::clone(&conn));

    let run_id = RunId::new();
    tx.send(GlobalDaemonEvent::RunFinished {
        run_id,
        outcome: RunOutcome::Completed {
            terminal: NodeKey::try_new("end").unwrap(),
        },
    })
    .unwrap();

    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(
        src.posted_comments().await.is_empty(),
        "no comment should be posted when run_id has no matching ticket"
    );
}

#[tokio::test]
async fn post_comment_failure_still_transitions_state() {
    let Setup {
        src,
        map,
        conn,
        tx,
        rx,
    } = make_setup();
    src.arm_post_comment_failure().await;
    let run_id = RunId::new();
    let run_id_str = run_id.to_string();
    {
        let guard = conn.lock().await;
        seed_ticket(&guard, "mock:test#5", &run_id_str);
    }

    let _handle = intake_completion::spawn(rx, map, Arc::clone(&conn));

    tx.send(GlobalDaemonEvent::RunFinished {
        run_id,
        outcome: RunOutcome::Completed {
            terminal: NodeKey::try_new("end").unwrap(),
        },
    })
    .unwrap();

    tokio::time::sleep(Duration::from_millis(150)).await;

    // post_comment was armed to fail → MockTaskSource records nothing.
    assert!(src.posted_comments().await.is_empty());

    // FSM still transitioned: tracker-side cosmetic post is best-effort,
    // but the on-disk ticket state is authoritative.
    let guard = conn.lock().await;
    let row = IntakeRepo::new(&guard)
        .lookup_ticket_by_run_id(&run_id_str)
        .unwrap()
        .unwrap();
    assert_eq!(row.state, TicketState::Completed);
}
