//! Integration test for the L3 (`surge:auto`) auto-merge gate.
//!
//! Drives `surge_daemon::automation_merge_gate` directly: assembles the
//! same components `main.rs` wires up (a `MockTaskSource` registry, an
//! in-memory SQLite registry DB, a fresh broadcast channel) and publishes
//! synthetic `RunFinished` events. No daemon binary is spawned.
//!
//! The mock's `arm_merge_readiness` lets each test pin the gate to a
//! deterministic Ready/Blocked verdict without making real HTTP calls.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use rusqlite::Connection;
use surge_core::id::RunId;
use surge_core::keys::NodeKey;
use surge_daemon::automation_merge_gate;
use surge_intake::testing::MockTaskSource;
use surge_intake::types::{TaskDetails, TaskId};
use surge_intake::{MergeReadiness, TaskSource};
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
    let m13 = include_str!(
        "../../surge-persistence/src/runs/migrations/registry/0013_intake_emit_log.sql"
    );
    conn.execute_batch(m13).unwrap();
    conn
}

fn seed_ticket(conn: &Connection, task_id: &str, run_id: &str) {
    conn.execute("INSERT INTO runs(id) VALUES (?1)", [run_id])
        .unwrap();
    IntakeRepo::new(conn)
        .insert(&IntakeRow {
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

async fn seed_l3_task(src: &Arc<MockTaskSource>, task_id_str: &str) -> TaskId {
    let id = TaskId::try_new(task_id_str).unwrap();
    src.put_task(TaskDetails {
        task_id: id.clone(),
        source_id: "mock:test".into(),
        title: "L3 fixture".into(),
        description: "auto-merge fixture".into(),
        status: "open".into(),
        labels: vec!["surge:auto".into()],
        url: format!("https://example.com/{task_id_str}"),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        assignee: None,
        raw_payload: serde_json::Value::Null,
    })
    .await;
    id
}

async fn seed_l1_task(src: &Arc<MockTaskSource>, task_id_str: &str) -> TaskId {
    let id = TaskId::try_new(task_id_str).unwrap();
    src.put_task(TaskDetails {
        task_id: id.clone(),
        source_id: "mock:test".into(),
        title: "L1 fixture".into(),
        description: "standard tier".into(),
        status: "open".into(),
        labels: vec!["surge:enabled".into()],
        url: format!("https://example.com/{task_id_str}"),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        assignee: None,
        raw_payload: serde_json::Value::Null,
    })
    .await;
    id
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

/// Wait up to 2 seconds for the mock to record `expected_count` comments.
/// Returns the snapshot on success, or whatever was recorded on timeout
/// (so failed assertions show what actually happened).
async fn wait_for_comments(
    src: &Arc<MockTaskSource>,
    expected_count: usize,
) -> Vec<(TaskId, String)> {
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        let comments = src.posted_comments().await;
        if comments.len() >= expected_count {
            return comments;
        }
        if std::time::Instant::now() >= deadline {
            return comments;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Assert that no new comments arrive over `window`, polling for the
/// full duration. Stronger than a fixed sleep because a delayed
/// side-effect lands as a count change anywhere in the window and
/// fails the test immediately, instead of slipping past a one-shot
/// check at the end.
async fn assert_no_new_comments_for(src: &Arc<MockTaskSource>, window: Duration) {
    let baseline = src.posted_comments().await.len();
    let deadline = std::time::Instant::now() + window;
    loop {
        let current = src.posted_comments().await.len();
        assert_eq!(
            current, baseline,
            "unexpected merge-gate comment observed; baseline={baseline}, now={current}"
        );
        if std::time::Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn l3_ready_posts_merge_proposed_comment_and_label() {
    let Setup {
        src,
        map,
        conn,
        tx,
        rx,
    } = make_setup();
    let task_id_str = "mock:test#42";
    seed_l3_task(&src, task_id_str).await;
    src.arm_merge_readiness(MergeReadiness::Ready).await;

    let run_id = RunId::new();
    {
        let guard = conn.lock().await;
        seed_ticket(&guard, task_id_str, &run_id.to_string());
    }

    let _handle = automation_merge_gate::spawn(rx, map, Arc::clone(&conn));

    tx.send(GlobalDaemonEvent::RunFinished {
        run_id,
        outcome: RunOutcome::Completed {
            terminal: NodeKey::try_new("end").unwrap(),
        },
    })
    .unwrap();

    let comments = wait_for_comments(&src, 1).await;
    assert_eq!(comments.len(), 1, "expected one merge-proposed comment");
    assert!(
        comments[0].1.contains("ready"),
        "body should mention ready, got: {}",
        comments[0].1
    );

    let labels = src.recorded_labels().await;
    assert!(
        labels.iter().any(|(_, label, present)| label
            == automation_merge_gate::labels::MERGE_PROPOSED
            && *present),
        "expected merge-proposed label to be applied, recorded: {labels:?}"
    );
}

#[tokio::test]
async fn l3_blocked_posts_merge_blocked_comment_and_label() {
    let Setup {
        src,
        map,
        conn,
        tx,
        rx,
    } = make_setup();
    let task_id_str = "mock:test#43";
    seed_l3_task(&src, task_id_str).await;
    src.arm_merge_readiness(MergeReadiness::Blocked("PR has merge conflicts".into()))
        .await;

    let run_id = RunId::new();
    {
        let guard = conn.lock().await;
        seed_ticket(&guard, task_id_str, &run_id.to_string());
    }

    let _handle = automation_merge_gate::spawn(rx, map, Arc::clone(&conn));

    tx.send(GlobalDaemonEvent::RunFinished {
        run_id,
        outcome: RunOutcome::Completed {
            terminal: NodeKey::try_new("end").unwrap(),
        },
    })
    .unwrap();

    let comments = wait_for_comments(&src, 1).await;
    assert_eq!(comments.len(), 1);
    assert!(
        comments[0].1.contains("merge conflicts"),
        "blocked reason must be in the body, got: {}",
        comments[0].1
    );

    let labels = src.recorded_labels().await;
    assert!(
        labels.iter().any(|(_, label, present)| label
            == automation_merge_gate::labels::MERGE_BLOCKED
            && *present),
        "expected merge-blocked label, recorded: {labels:?}"
    );
}

#[tokio::test]
async fn l3_default_readiness_blocks_when_provider_does_not_implement() {
    // No `arm_merge_readiness` call — MockTaskSource falls back to the
    // "no readiness override armed" Blocked variant, simulating a provider
    // that does not override the trait default.
    let Setup {
        src,
        map,
        conn,
        tx,
        rx,
    } = make_setup();
    let task_id_str = "mock:test#44";
    seed_l3_task(&src, task_id_str).await;

    let run_id = RunId::new();
    {
        let guard = conn.lock().await;
        seed_ticket(&guard, task_id_str, &run_id.to_string());
    }

    let _handle = automation_merge_gate::spawn(rx, map, Arc::clone(&conn));

    tx.send(GlobalDaemonEvent::RunFinished {
        run_id,
        outcome: RunOutcome::Completed {
            terminal: NodeKey::try_new("end").unwrap(),
        },
    })
    .unwrap();

    let comments = wait_for_comments(&src, 1).await;
    assert_eq!(comments.len(), 1);
    let labels = src.recorded_labels().await;
    assert!(
        labels
            .iter()
            .any(|(_, label, _)| label == automation_merge_gate::labels::MERGE_BLOCKED),
        "default-Blocked path must apply merge-blocked, got: {labels:?}"
    );
}

#[tokio::test]
async fn non_l3_task_is_a_no_op() {
    let Setup {
        src,
        map,
        conn,
        tx,
        rx,
    } = make_setup();
    let task_id_str = "mock:test#45";
    seed_l1_task(&src, task_id_str).await;
    src.arm_merge_readiness(MergeReadiness::Ready).await;

    let run_id = RunId::new();
    {
        let guard = conn.lock().await;
        seed_ticket(&guard, task_id_str, &run_id.to_string());
    }

    let _handle = automation_merge_gate::spawn(rx, map, Arc::clone(&conn));

    tx.send(GlobalDaemonEvent::RunFinished {
        run_id,
        outcome: RunOutcome::Completed {
            terminal: NodeKey::try_new("end").unwrap(),
        },
    })
    .unwrap();

    assert_no_new_comments_for(&src, Duration::from_millis(400)).await;
    let labels = src.recorded_labels().await;
    assert!(
        labels.iter().all(
            |(_, label, _)| label != automation_merge_gate::labels::MERGE_PROPOSED
                && label != automation_merge_gate::labels::MERGE_BLOCKED
        ),
        "non-L3 must not apply merge gate labels, got: {labels:?}"
    );
}

#[tokio::test]
async fn repeated_run_finished_is_idempotent() {
    let Setup {
        src,
        map,
        conn,
        tx,
        rx,
    } = make_setup();
    let task_id_str = "mock:test#46";
    seed_l3_task(&src, task_id_str).await;
    src.arm_merge_readiness(MergeReadiness::Ready).await;

    let run_id = RunId::new();
    {
        let guard = conn.lock().await;
        seed_ticket(&guard, task_id_str, &run_id.to_string());
    }

    let _handle = automation_merge_gate::spawn(rx, map, Arc::clone(&conn));

    let event = GlobalDaemonEvent::RunFinished {
        run_id,
        outcome: RunOutcome::Completed {
            terminal: NodeKey::try_new("end").unwrap(),
        },
    };
    tx.send(event.clone()).unwrap();
    let _ = wait_for_comments(&src, 1).await;
    tx.send(event).unwrap();
    // baseline is 1 from the first event; the helper fails if the
    // re-fired event slips past intake_emit_log dedup and adds another.
    assert_no_new_comments_for(&src, Duration::from_millis(400)).await;
}

#[tokio::test]
async fn failed_run_outcome_does_not_trigger_gate() {
    let Setup {
        src,
        map,
        conn,
        tx,
        rx,
    } = make_setup();
    let task_id_str = "mock:test#47";
    seed_l3_task(&src, task_id_str).await;
    src.arm_merge_readiness(MergeReadiness::Ready).await;

    let run_id = RunId::new();
    {
        let guard = conn.lock().await;
        seed_ticket(&guard, task_id_str, &run_id.to_string());
    }

    let _handle = automation_merge_gate::spawn(rx, map, Arc::clone(&conn));

    tx.send(GlobalDaemonEvent::RunFinished {
        run_id,
        outcome: RunOutcome::Failed {
            error: "graph error".into(),
        },
    })
    .unwrap();

    assert_no_new_comments_for(&src, Duration::from_millis(400)).await;
}
