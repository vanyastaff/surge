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
    let m2 =
        include_str!("../../surge-persistence/src/runs/migrations/registry/0002_ticket_index.sql");
    conn.execute_batch(m2).unwrap();
    let m4 = include_str!(
        "../../surge-persistence/src/runs/migrations/registry/0004_inbox_callback_columns.sql"
    );
    conn.execute_batch(m4).unwrap();
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
        callback_token: None,
        tg_chat_id: None,
        tg_message_id: None,
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

/// Polling helper for `RunOutcome::Parked` (Task 12 M3): unlike every other
/// outcome, a parked run does **not** transition the ticket FSM to a
/// terminal state (see `format_completion`'s doc — no `TicketState`
/// represents "paused, resumes on its own", so the ticket is left
/// untouched). Waits up to 2 seconds for the tracker comment to arrive
/// instead of polling for a state change that will never happen.
async fn wait_for_comment(src: &Arc<MockTaskSource>) -> Vec<(surge_intake::types::TaskId, String)> {
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while std::time::Instant::now() < deadline {
        let comments = src.posted_comments().await;
        if !comments.is_empty() {
            return comments;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    Vec::new()
}

/// Polling helper modelled on the deadline+loop pattern in
/// `daemon_queue_full.rs` / `daemon_queue_drain.rs`. Waits up to 2
/// seconds for `lookup_ticket_by_run_id` to return a row whose state
/// is one of the terminal states the consumer transitions through.
/// Returns the row on success, or `None` on timeout.
async fn wait_for_terminal_state(
    conn: &Arc<TokioMutex<Connection>>,
    run_id_str: &str,
) -> Option<IntakeRow> {
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while std::time::Instant::now() < deadline {
        let row_opt = {
            let guard = conn.lock().await;
            IntakeRepo::new(&guard)
                .lookup_ticket_by_run_id(run_id_str)
                .ok()
                .flatten()
        };
        if let Some(row) = row_opt
            && matches!(
                row.state,
                TicketState::Completed | TicketState::Failed | TicketState::Aborted
            )
        {
            return Some(row);
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    None
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

    let row = wait_for_terminal_state(&conn, &run_id_str)
        .await
        .expect("consumer did not transition ticket within deadline");
    assert_eq!(row.state, TicketState::Completed);

    let comments = src.posted_comments().await;
    assert_eq!(comments.len(), 1, "expected exactly one comment");
    assert!(comments[0].1.starts_with("✅"), "got: {}", comments[0].1);
    assert!(
        comments[0].1.contains("end"),
        "expected terminal node in body, got: {}",
        comments[0].1
    );
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

    let row = wait_for_terminal_state(&conn, &run_id_str)
        .await
        .expect("consumer did not transition ticket within deadline");
    assert_eq!(row.state, TicketState::Failed);

    let comments = src.posted_comments().await;
    assert_eq!(comments.len(), 1);
    assert!(
        comments[0].1.starts_with("❌ Run failed:"),
        "got: {}",
        comments[0].1
    );
    assert!(comments[0].1.contains("graph validation error"));
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

    let row = wait_for_terminal_state(&conn, &run_id_str)
        .await
        .expect("consumer did not transition ticket within deadline");
    assert_eq!(row.state, TicketState::Aborted);

    let comments = src.posted_comments().await;
    assert_eq!(comments.len(), 1);
    assert!(comments[0].1.starts_with("Run aborted:"));
    assert!(comments[0].1.contains("user pressed Stop"));
}

/// Task 12 M3: a parked run must post an accurate "paused" comment and
/// leave the ticket's FSM state exactly where it was (`Active`, from
/// `seed_ticket`) — not `Failed`/`Aborted`. Before this milestone's fix,
/// `format_completion`'s catch-all folded every non-Completed/Failed/
/// Aborted `RunOutcome` onto `Aborted`, which would have told the human
/// the run stopped when it is actually parked, working as designed, and
/// will resume on its own.
#[tokio::test]
async fn run_parked_posts_pause_comment_and_leaves_ticket_state_untouched() {
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
        seed_ticket(&guard, "mock:test#4", &run_id_str);
    }

    let _handle = intake_completion::spawn(rx, map, Arc::clone(&conn));

    let wake_at = chrono::DateTime::parse_from_rfc3339("2026-01-01T00:05:00Z")
        .unwrap()
        .with_timezone(&Utc);
    tx.send(GlobalDaemonEvent::RunFinished {
        run_id,
        outcome: RunOutcome::Parked { wake_at },
    })
    .unwrap();

    let comments = wait_for_comment(&src).await;
    assert_eq!(
        comments.len(),
        1,
        "consumer did not post a comment within deadline"
    );
    assert!(
        comments[0].1.contains("paused") && comments[0].1.contains("2026-01-01"),
        "expected a pause comment naming the wake time, got: {}",
        comments[0].1
    );
    let lower = comments[0].1.to_lowercase();
    assert!(
        !lower.contains("fail") && !lower.contains("abort"),
        "comment must not read as a failure, got: {}",
        comments[0].1
    );

    // Give the (already-completed) state-update branch a moment to run if
    // it were ever going to — then assert the ticket is still `Active`.
    tokio::time::sleep(Duration::from_millis(50)).await;
    let row = {
        let guard = conn.lock().await;
        IntakeRepo::new(&guard)
            .lookup_ticket_by_run_id(&run_id_str)
            .unwrap()
            .expect("ticket row must still exist")
    };
    assert_eq!(
        row.state,
        TicketState::Active,
        "a parked run must not transition the ticket FSM — it is not finished"
    );
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
    // Seed only a sentinel ticket. The first event below uses a different
    // run_id with no row in `ticket_index`, so the consumer must skip it.
    // The second event targets the sentinel, giving us a deterministic
    // signal that the consumer has drained past the no-match event.
    let sentinel_run_id = RunId::new();
    let sentinel_run_id_str = sentinel_run_id.to_string();
    {
        let guard = conn.lock().await;
        seed_ticket(&guard, "mock:test#sentinel", &sentinel_run_id_str);
    }

    let _handle = intake_completion::spawn(rx, map, Arc::clone(&conn));

    let unmatched_run_id = RunId::new();
    tx.send(GlobalDaemonEvent::RunFinished {
        run_id: unmatched_run_id,
        outcome: RunOutcome::Completed {
            terminal: NodeKey::try_new("end").unwrap(),
        },
    })
    .unwrap();
    tx.send(GlobalDaemonEvent::RunFinished {
        run_id: sentinel_run_id,
        outcome: RunOutcome::Completed {
            terminal: NodeKey::try_new("end").unwrap(),
        },
    })
    .unwrap();

    // Once the sentinel reaches terminal state, the consumer has
    // necessarily already processed (and skipped) the unmatched event
    // ahead of it in the broadcast queue.
    let row = wait_for_terminal_state(&conn, &sentinel_run_id_str)
        .await
        .expect("consumer did not process sentinel event within deadline");
    assert_eq!(row.state, TicketState::Completed);

    let comments = src.posted_comments().await;
    assert_eq!(
        comments.len(),
        1,
        "exactly one comment expected: only the sentinel matched"
    );
    assert_eq!(comments[0].0.as_str(), "mock:test#sentinel");
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

    // FSM still transitioned: tracker-side cosmetic post is best-effort,
    // but the on-disk ticket state is authoritative.
    let row = wait_for_terminal_state(&conn, &run_id_str)
        .await
        .expect("consumer did not transition ticket within deadline");
    assert_eq!(row.state, TicketState::Completed);

    // post_comment was armed to fail → MockTaskSource records nothing.
    assert!(src.posted_comments().await.is_empty());
}

/// A row whose stored `task_id` string fails `TaskId::try_new` (e.g., a
/// future migration or manual edit that loosened DB validation) must not
/// block the FSM transition. The cosmetic comment post is skipped, but
/// the on-disk ticket state still moves to terminal.
#[tokio::test]
async fn invalid_task_id_string_skips_comment_but_transitions_state() {
    let Setup {
        src,
        map,
        conn,
        tx,
        rx,
    } = make_setup();
    let run_id = RunId::new();
    let run_id_str = run_id.to_string();

    // Seed a row with a `task_id` that fails `TaskId::try_new` (no `:`).
    {
        let guard = conn.lock().await;
        let bad_task_id = "no-provider-prefix";
        guard
            .execute("INSERT INTO runs(id) VALUES (?1)", [&run_id_str])
            .unwrap();
        IntakeRepo::new(&guard)
            .insert(&IntakeRow {
                task_id: bad_task_id.into(),
                source_id: "mock:test".into(),
                provider: "mock".into(),
                run_id: Some(run_id_str.clone()),
                triage_decision: Some("enqueued".into()),
                duplicate_of: None,
                priority: Some("medium".into()),
                state: TicketState::Active,
                first_seen: Utc::now(),
                last_seen: Utc::now(),
                snooze_until: None,
                callback_token: None,
                tg_chat_id: None,
                tg_message_id: None,
            })
            .unwrap();
    }

    let _handle = intake_completion::spawn(rx, map, Arc::clone(&conn));

    tx.send(GlobalDaemonEvent::RunFinished {
        run_id,
        outcome: RunOutcome::Completed {
            terminal: NodeKey::try_new("end").unwrap(),
        },
    })
    .unwrap();

    let row = wait_for_terminal_state(&conn, &run_id_str)
        .await
        .expect("consumer did not transition ticket within deadline");
    assert_eq!(row.state, TicketState::Completed);
    assert_eq!(row.task_id, "no-provider-prefix");

    // No comment posted: the bad task_id couldn't be parsed for the
    // TaskSource API, so the cosmetic note was skipped.
    assert!(src.posted_comments().await.is_empty());
}
