//! Integration test for the L3 (`surge:auto`) auto-merge gate.
//!
//! Drives `surge_daemon::automation_merge_gate` directly: assembles the
//! same components `main.rs` wires up (a `MockTaskSource` registry, an
//! in-memory SQLite registry DB, a fresh broadcast channel, a recording
//! notifier) and publishes synthetic `RunFinished` events. No daemon binary
//! is spawned.
//!
//! The mock's `arm_merge_readiness` pins the readiness verdict and
//! `arm_merge_outcome` pins what the real `merge_pr` call returns — both
//! deterministic, no real HTTP.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use chrono::Utc;
use rusqlite::Connection;
use surge_core::id::RunId;
use surge_core::keys::NodeKey;
use surge_core::notify_config::{NotifyChannel, NotifySeverity};
use surge_daemon::automation_merge_gate;
use surge_intake::testing::MockTaskSource;
use surge_intake::types::{TaskDetails, TaskId};
use surge_intake::{MergeOutcome, MergeReadiness, TaskSource};
use surge_notify::{NotifyDeliverer, NotifyDeliveryContext, NotifyError, RenderedNotification};
use surge_orchestrator::engine::handle::RunOutcome;
use surge_orchestrator::engine::ipc::GlobalDaemonEvent;
use surge_persistence::intake::{IntakeRepo, IntakeRow, TicketState};
use tokio::sync::{Mutex as TokioMutex, broadcast};

/// In-memory `NotifyDeliverer` that records every escalation the gate
/// delivers, so tests can assert "never a silent stall".
struct RecordingNotifier {
    calls: TokioMutex<Vec<(NotifySeverity, String, String)>>,
}

impl RecordingNotifier {
    fn new() -> Self {
        Self {
            calls: TokioMutex::new(Vec::new()),
        }
    }

    async fn snapshot(&self) -> Vec<(NotifySeverity, String, String)> {
        self.calls.lock().await.clone()
    }
}

#[async_trait]
impl NotifyDeliverer for RecordingNotifier {
    async fn deliver(
        &self,
        _ctx: &NotifyDeliveryContext<'_>,
        _ch: &NotifyChannel,
        rendered: &RenderedNotification,
    ) -> Result<(), NotifyError> {
        self.calls.lock().await.push((
            rendered.severity,
            rendered.title.clone(),
            rendered.body.clone(),
        ));
        Ok(())
    }
}

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
    notifier: Arc<RecordingNotifier>,
    tx: broadcast::Sender<GlobalDaemonEvent>,
}

fn make_setup() -> Setup {
    let src = Arc::new(MockTaskSource::new("mock:test", "mock"));
    let mut map: HashMap<String, Arc<dyn TaskSource>> = HashMap::new();
    map.insert("mock:test".into(), Arc::clone(&src) as Arc<dyn TaskSource>);
    let map = Arc::new(map);
    let conn = Arc::new(TokioMutex::new(db_with_schema()));
    let notifier = Arc::new(RecordingNotifier::new());
    let (tx, _rx0) = broadcast::channel(8);
    Setup {
        src,
        map,
        conn,
        notifier,
        tx,
    }
}

/// Spawn the gate with the recording notifier from `setup`.
fn spawn_gate(
    setup: &Setup,
    rx: broadcast::Receiver<GlobalDaemonEvent>,
) -> tokio::task::JoinHandle<()> {
    automation_merge_gate::spawn(
        rx,
        Arc::clone(&setup.map),
        Arc::clone(&setup.conn),
        Arc::clone(&setup.notifier) as Arc<dyn NotifyDeliverer>,
    )
}

fn completed_event(run_id: RunId) -> GlobalDaemonEvent {
    GlobalDaemonEvent::RunFinished {
        run_id,
        outcome: RunOutcome::Completed {
            terminal: NodeKey::try_new("end").unwrap(),
        },
    }
}

/// Wait up to 2 seconds for the mock to record `expected_count` comments.
async fn wait_for_comments(
    src: &Arc<MockTaskSource>,
    expected_count: usize,
) -> Vec<(TaskId, String)> {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let comments = src.posted_comments().await;
        if comments.len() >= expected_count {
            return comments;
        }
        if Instant::now() >= deadline {
            return comments;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Wait up to 2 seconds for the notifier to record `expected` escalations.
async fn wait_for_escalations(
    notifier: &Arc<RecordingNotifier>,
    expected: usize,
) -> Vec<(NotifySeverity, String, String)> {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let calls = notifier.snapshot().await;
        if calls.len() >= expected {
            return calls;
        }
        if Instant::now() >= deadline {
            return calls;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Assert no new comments arrive over `window`, polling the full duration.
async fn assert_no_new_comments_for(src: &Arc<MockTaskSource>, window: Duration) {
    let baseline = src.posted_comments().await.len();
    let deadline = Instant::now() + window;
    loop {
        let current = src.posted_comments().await.len();
        assert_eq!(
            current, baseline,
            "unexpected merge-gate comment observed; baseline={baseline}, now={current}"
        );
        if Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn l3_ready_merges_and_posts_merged_comment_and_label() {
    let setup = make_setup();
    let task_id_str = "mock:test#42";
    seed_l3_task(&setup.src, task_id_str).await;
    setup.src.arm_merge_readiness(MergeReadiness::Ready).await;
    setup.src.arm_merge_outcome(MergeOutcome::Merged).await;

    let run_id = RunId::new();
    {
        let guard = setup.conn.lock().await;
        seed_ticket(&guard, task_id_str, &run_id.to_string());
    }

    let rx = setup.tx.subscribe();
    let _handle = spawn_gate(&setup, rx);
    setup.tx.send(completed_event(run_id)).unwrap();

    let comments = wait_for_comments(&setup.src, 1).await;
    assert_eq!(comments.len(), 1, "expected one merged comment");
    assert!(
        comments[0].1.contains("merged"),
        "body should report the merge, got: {}",
        comments[0].1
    );

    // The gate executed the real merge exactly once.
    assert_eq!(
        setup.src.merge_calls().await.len(),
        1,
        "merge_pr must run once"
    );

    let labels = setup.src.recorded_labels().await;
    assert!(
        labels
            .iter()
            .any(|(_, label, present)| label == automation_merge_gate::labels::MERGED && *present),
        "expected merged label, recorded: {labels:?}"
    );

    // Operator gets a success escalation (never a silent merge).
    let escalations = wait_for_escalations(&setup.notifier, 1).await;
    assert!(
        escalations
            .iter()
            .any(|(sev, _, _)| matches!(sev, NotifySeverity::Success)),
        "expected a success escalation, got: {escalations:?}"
    );
}

#[tokio::test]
async fn l3_blocked_posts_merge_blocked_comment_and_escalates() {
    let setup = make_setup();
    let task_id_str = "mock:test#43";
    seed_l3_task(&setup.src, task_id_str).await;
    setup
        .src
        .arm_merge_readiness(MergeReadiness::Blocked("PR has merge conflicts".into()))
        .await;

    let run_id = RunId::new();
    {
        let guard = setup.conn.lock().await;
        seed_ticket(&guard, task_id_str, &run_id.to_string());
    }

    let rx = setup.tx.subscribe();
    let _handle = spawn_gate(&setup, rx);
    setup.tx.send(completed_event(run_id)).unwrap();

    let comments = wait_for_comments(&setup.src, 1).await;
    assert_eq!(comments.len(), 1);
    assert!(
        comments[0].1.contains("merge conflicts"),
        "blocked reason must be in the body, got: {}",
        comments[0].1
    );

    // Readiness blocked → the gate must NOT attempt a merge.
    assert!(
        setup.src.merge_calls().await.is_empty(),
        "blocked readiness must not call merge_pr"
    );

    let labels = setup.src.recorded_labels().await;
    assert!(
        labels.iter().any(|(_, label, present)| label
            == automation_merge_gate::labels::MERGE_BLOCKED
            && *present),
        "expected merge-blocked label, recorded: {labels:?}"
    );

    let escalations = wait_for_escalations(&setup.notifier, 1).await;
    assert!(
        escalations
            .iter()
            .any(|(sev, title, _)| matches!(sev, NotifySeverity::Warn) && title.contains("blocked")),
        "expected a warn escalation for the block, got: {escalations:?}"
    );
}

#[tokio::test]
async fn l3_merge_conflict_escalates() {
    let setup = make_setup();
    let task_id_str = "mock:test#48";
    seed_l3_task(&setup.src, task_id_str).await;
    // Readiness says go, but the merge call itself hits a conflict (head
    // moved between the check and the merge).
    setup.src.arm_merge_readiness(MergeReadiness::Ready).await;
    setup
        .src
        .arm_merge_outcome(MergeOutcome::Conflict("base branch moved".into()))
        .await;

    let run_id = RunId::new();
    {
        let guard = setup.conn.lock().await;
        seed_ticket(&guard, task_id_str, &run_id.to_string());
    }

    let rx = setup.tx.subscribe();
    let _handle = spawn_gate(&setup, rx);
    setup.tx.send(completed_event(run_id)).unwrap();

    let comments = wait_for_comments(&setup.src, 1).await;
    assert_eq!(comments.len(), 1);
    assert!(
        comments[0].1.contains("base branch moved"),
        "conflict reason must be surfaced, got: {}",
        comments[0].1
    );

    // The merge was attempted (and failed) — exactly once.
    assert_eq!(setup.src.merge_calls().await.len(), 1);

    let labels = setup.src.recorded_labels().await;
    assert!(
        labels
            .iter()
            .any(|(_, label, _)| label == automation_merge_gate::labels::MERGE_BLOCKED),
        "merge conflict must apply merge-blocked, got: {labels:?}"
    );

    let escalations = wait_for_escalations(&setup.notifier, 1).await;
    assert!(
        escalations
            .iter()
            .any(|(sev, _, _)| matches!(sev, NotifySeverity::Warn)),
        "merge conflict must escalate, got: {escalations:?}"
    );
}

#[tokio::test]
async fn l3_default_readiness_blocks_when_provider_does_not_implement() {
    // No `arm_merge_readiness` — MockTaskSource falls back to the same
    // Blocked reason a PR-less provider (Linear) surfaces.
    let setup = make_setup();
    let task_id_str = "mock:test#44";
    seed_l3_task(&setup.src, task_id_str).await;

    let run_id = RunId::new();
    {
        let guard = setup.conn.lock().await;
        seed_ticket(&guard, task_id_str, &run_id.to_string());
    }

    let rx = setup.tx.subscribe();
    let _handle = spawn_gate(&setup, rx);
    setup.tx.send(completed_event(run_id)).unwrap();

    let comments = wait_for_comments(&setup.src, 1).await;
    assert_eq!(comments.len(), 1);
    assert!(
        setup.src.merge_calls().await.is_empty(),
        "PR-less provider must not merge"
    );
    let labels = setup.src.recorded_labels().await;
    assert!(
        labels
            .iter()
            .any(|(_, label, _)| label == automation_merge_gate::labels::MERGE_BLOCKED),
        "default-Blocked path must apply merge-blocked, got: {labels:?}"
    );
}

#[tokio::test]
async fn non_l3_task_is_a_no_op() {
    let setup = make_setup();
    let task_id_str = "mock:test#45";
    seed_l1_task(&setup.src, task_id_str).await;
    setup.src.arm_merge_readiness(MergeReadiness::Ready).await;
    setup.src.arm_merge_outcome(MergeOutcome::Merged).await;

    let run_id = RunId::new();
    {
        let guard = setup.conn.lock().await;
        seed_ticket(&guard, task_id_str, &run_id.to_string());
    }

    let rx = setup.tx.subscribe();
    let _handle = spawn_gate(&setup, rx);
    setup.tx.send(completed_event(run_id)).unwrap();

    assert_no_new_comments_for(&setup.src, Duration::from_millis(400)).await;
    assert!(
        setup.src.merge_calls().await.is_empty(),
        "non-L3 must not merge"
    );
    let labels = setup.src.recorded_labels().await;
    assert!(
        labels.iter().all(
            |(_, label, _)| label != automation_merge_gate::labels::MERGED
                && label != automation_merge_gate::labels::MERGE_BLOCKED
        ),
        "non-L3 must not apply merge gate labels, got: {labels:?}"
    );
}

#[tokio::test]
async fn idempotent_double_merge() {
    let setup = make_setup();
    let task_id_str = "mock:test#46";
    seed_l3_task(&setup.src, task_id_str).await;
    setup.src.arm_merge_readiness(MergeReadiness::Ready).await;
    setup.src.arm_merge_outcome(MergeOutcome::Merged).await;

    let run_id = RunId::new();
    {
        let guard = setup.conn.lock().await;
        seed_ticket(&guard, task_id_str, &run_id.to_string());
    }

    let rx = setup.tx.subscribe();
    let _handle = spawn_gate(&setup, rx);

    let event = completed_event(run_id);
    setup.tx.send(event.clone()).unwrap();
    let _ = wait_for_comments(&setup.src, 1).await;
    // Re-fire the same completion (as a recovery re-emit would).
    setup.tx.send(event).unwrap();
    assert_no_new_comments_for(&setup.src, Duration::from_millis(400)).await;

    // The critical guarantee: the irreversible merge ran exactly once.
    assert_eq!(
        setup.src.merge_calls().await.len(),
        1,
        "re-fired completion must not double-merge"
    );
}

#[tokio::test]
async fn failed_run_outcome_does_not_trigger_gate() {
    let setup = make_setup();
    let task_id_str = "mock:test#47";
    seed_l3_task(&setup.src, task_id_str).await;
    setup.src.arm_merge_readiness(MergeReadiness::Ready).await;
    setup.src.arm_merge_outcome(MergeOutcome::Merged).await;

    let run_id = RunId::new();
    {
        let guard = setup.conn.lock().await;
        seed_ticket(&guard, task_id_str, &run_id.to_string());
    }

    let rx = setup.tx.subscribe();
    let _handle = spawn_gate(&setup, rx);
    setup
        .tx
        .send(GlobalDaemonEvent::RunFinished {
            run_id,
            outcome: RunOutcome::Failed {
                error: "graph error".into(),
            },
        })
        .unwrap();

    assert_no_new_comments_for(&setup.src, Duration::from_millis(400)).await;
    assert!(
        setup.src.merge_calls().await.is_empty(),
        "failed run must not merge"
    );
}
