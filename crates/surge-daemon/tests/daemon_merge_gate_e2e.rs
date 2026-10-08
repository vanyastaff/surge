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

#[path = "support/runtime_home.rs"]
mod runtime_home_fixture;
use runtime_home_fixture::FixtureHome;

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
    // Run event-log store for the optional Run Report attachment (spec
    // §10/R31). Most fixtures in this file never create a real run in it,
    // so `run_report_attachment` degrades to `None` (`RunNotFound`) for
    // them — this test file is mostly about the merge decision, not the
    // report itself (see `run_report`'s own tests for that coverage).
    // `l3_merged_comment_attaches_the_run_report_and_flags_it_unverified`
    // and its `publish_run_report=false` counterpart below are the
    // exceptions: they write a real, minimal run so the attachment path
    // itself — and the config flag gating it — gets one end-to-end check.
    // Kept alive by `_home` for the lifetime of the `Storage` handle.
    runs: Arc<surge_persistence::runs::Storage>,
    _home: FixtureHome,
}

impl Setup {
    async fn finish(self, handle: tokio::task::JoinHandle<()>) {
        let Self {
            tx, runs, _home, ..
        } = self;
        drop(tx);
        tokio::time::timeout(Duration::from_secs(10), handle)
            .await
            .unwrap()
            .unwrap();
        drop(runs);
        _home.close().unwrap();
    }

    fn close(self) {
        let Self { runs, _home, .. } = self;
        drop(runs);
        _home.close().unwrap();
    }
}

async fn make_setup() -> Setup {
    let src = Arc::new(MockTaskSource::new("mock:test", "mock"));
    let mut map: HashMap<String, Arc<dyn TaskSource>> = HashMap::new();
    map.insert("mock:test".into(), Arc::clone(&src) as Arc<dyn TaskSource>);
    let map = Arc::new(map);
    let conn = Arc::new(TokioMutex::new(db_with_schema()));
    let notifier = Arc::new(RecordingNotifier::new());
    let (tx, _rx0) = broadcast::channel(8);
    let home = FixtureHome::new().unwrap();
    let runs = surge_persistence::runs::Storage::open(home.path())
        .await
        .unwrap();
    Setup {
        src,
        map,
        conn,
        notifier,
        tx,
        runs,
        _home: home,
    }
}

/// Spawn the gate with the recording notifier from `setup`.
fn spawn_gate(
    setup: &Setup,
    rx: broadcast::Receiver<GlobalDaemonEvent>,
    publish_run_report: bool,
) -> tokio::task::JoinHandle<()> {
    automation_merge_gate::spawn(
        rx,
        Arc::clone(&setup.map),
        Arc::clone(&setup.conn),
        Arc::clone(&setup.notifier) as Arc<dyn NotifyDeliverer>,
        Arc::clone(&setup.runs),
        publish_run_report,
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

/// Wait up to 2 seconds for the gate to have invoked `fetch_task` at least
/// `expected` times. A positive signal that the gate processed the event and
/// resolved the policy (it does so before any merge decision), so a no-op
/// assertion does not rely on a bare timeout. Panics on timeout.
async fn wait_for_fetch_task(src: &Arc<MockTaskSource>, expected: u32) {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if src.fetch_task_calls().await >= expected {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "gate did not call fetch_task {expected}x within 2s (got {})",
            src.fetch_task_calls().await
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn l3_ready_merges_and_posts_merged_comment_and_label() {
    let setup = make_setup().await;
    let task_id_str = "mock:test#42";
    seed_l3_task(&setup.src, task_id_str).await;
    setup
        .src
        .arm_merge_readiness(MergeReadiness::Ready { head_ref: None })
        .await;
    setup.src.arm_merge_outcome(MergeOutcome::Merged).await;

    let run_id = RunId::new();
    {
        let guard = setup.conn.lock().await;
        seed_ticket(&guard, task_id_str, &run_id.to_string());
    }

    let rx = setup.tx.subscribe();
    let _handle = spawn_gate(&setup, rx, false);
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
    setup.finish(_handle).await;
}

/// Spec §10/R31: the merge gate's success comment optionally carries the
/// run's own Run Report, through the *existing* `post_comment` call —
/// no new tracker integration. This run never ran a verifier, so the
/// attachment must lead with the same "UNVERIFIED SUCCESS" signal Run
/// Report, `surge inbox`, and `surge ledger` all show for the identical
/// scenario (spec §10/R30) — one predicate, one answer, now visible on the
/// PR too.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn l3_merged_comment_attaches_the_run_report_and_flags_it_unverified() {
    let setup = make_setup().await;
    let task_id_str = "mock:test#report";
    seed_l3_task(&setup.src, task_id_str).await;
    setup
        .src
        .arm_merge_readiness(MergeReadiness::Ready { head_ref: None })
        .await;
    setup.src.arm_merge_outcome(MergeOutcome::Merged).await;

    let run_id = RunId::new();
    let project = std::env::temp_dir().join(format!("surge-merge-gate-report-{run_id}"));
    let writer = setup.runs.create_run(run_id, &project, None).await.unwrap();
    writer
        .append_event(surge_core::run_event::VersionedEventPayload::new(
            surge_core::run_event::EventPayload::RunCompleted {
                terminal_node: NodeKey::try_new("end").unwrap(),
            },
        ))
        .await
        .unwrap();
    writer.flush().await.unwrap();
    writer.close().await.unwrap();

    {
        let guard = setup.conn.lock().await;
        seed_ticket(&guard, task_id_str, &run_id.to_string());
    }

    let rx = setup.tx.subscribe();
    let _handle = spawn_gate(&setup, rx, true);
    setup.tx.send(completed_event(run_id)).unwrap();

    let comments = wait_for_comments(&setup.src, 1).await;
    assert_eq!(comments.len(), 1);
    let body = &comments[0].1;
    assert!(
        body.contains("<details>") && body.contains("Surge Run Report"),
        "merged comment must carry the Run Report attachment, got: {body}"
    );
    assert!(
        body.contains("UNVERIFIED SUCCESS"),
        "a run with no verifier verdict must be flagged, got: {body}"
    );
    setup.finish(_handle).await;
}

/// Spec §10/R31 says the attachment is *optional* — off by default
/// (`MergeGateConfig::publish_run_report`). Consent to L3 auto-merge is
/// consent to merge the PR, not to publish the run's transcript-derived
/// report to the tracker, so the merged comment must stay exactly what it
/// was before this attachment existed unless an operator opts in. Same
/// fixture as the test above, only the flag differs.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn l3_merged_comment_omits_the_run_report_when_publishing_is_disabled() {
    let setup = make_setup().await;
    let task_id_str = "mock:test#report-off";
    seed_l3_task(&setup.src, task_id_str).await;
    setup
        .src
        .arm_merge_readiness(MergeReadiness::Ready { head_ref: None })
        .await;
    setup.src.arm_merge_outcome(MergeOutcome::Merged).await;

    let run_id = RunId::new();
    let project = std::env::temp_dir().join(format!("surge-merge-gate-report-off-{run_id}"));
    let writer = setup.runs.create_run(run_id, &project, None).await.unwrap();
    writer
        .append_event(surge_core::run_event::VersionedEventPayload::new(
            surge_core::run_event::EventPayload::RunCompleted {
                terminal_node: NodeKey::try_new("end").unwrap(),
            },
        ))
        .await
        .unwrap();
    writer.flush().await.unwrap();
    writer.close().await.unwrap();

    {
        let guard = setup.conn.lock().await;
        seed_ticket(&guard, task_id_str, &run_id.to_string());
    }

    let rx = setup.tx.subscribe();
    let _handle = spawn_gate(&setup, rx, false);
    setup.tx.send(completed_event(run_id)).unwrap();

    let comments = wait_for_comments(&setup.src, 1).await;
    assert_eq!(comments.len(), 1);
    let body = &comments[0].1;
    assert_eq!(
        body, "Surge L3 auto-merge: PR merged ✓ (checks green + review approved).",
        "publish_run_report=false must post exactly the plain success comment, no attachment"
    );
    setup.finish(_handle).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn l3_merge_is_pinned_to_readiness_head() {
    // The readiness verdict carries the head SHA it validated; the gate must
    // pass that exact SHA to merge_pr so GitHub rejects a moved head rather
    // than merging unreviewed code.
    let setup = make_setup().await;
    let task_id_str = "mock:test#49";
    seed_l3_task(&setup.src, task_id_str).await;
    setup
        .src
        .arm_merge_readiness(MergeReadiness::Ready {
            head_ref: Some("approved-sha-abc123".into()),
        })
        .await;
    setup.src.arm_merge_outcome(MergeOutcome::Merged).await;

    let run_id = RunId::new();
    {
        let guard = setup.conn.lock().await;
        seed_ticket(&guard, task_id_str, &run_id.to_string());
    }

    let rx = setup.tx.subscribe();
    let _handle = spawn_gate(&setup, rx, false);
    setup.tx.send(completed_event(run_id)).unwrap();

    let _ = wait_for_comments(&setup.src, 1).await;
    let calls = setup.src.merge_calls().await;
    assert_eq!(calls.len(), 1, "merge_pr must run once");
    assert_eq!(
        calls[0].1,
        Some("approved-sha-abc123".to_string()),
        "merge must be pinned to the readiness-approved head SHA"
    );
    setup.finish(_handle).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn l3_blocked_posts_merge_blocked_comment_and_escalates() {
    let setup = make_setup().await;
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
    let _handle = spawn_gate(&setup, rx, false);
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
    setup.finish(_handle).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn l3_merge_conflict_escalates() {
    let setup = make_setup().await;
    let task_id_str = "mock:test#48";
    seed_l3_task(&setup.src, task_id_str).await;
    // Readiness says go, but the merge call itself hits a conflict (head
    // moved between the check and the merge).
    setup
        .src
        .arm_merge_readiness(MergeReadiness::Ready { head_ref: None })
        .await;
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
    let _handle = spawn_gate(&setup, rx, false);
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
    setup.finish(_handle).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn l3_already_merged_is_success() {
    // A manual merge raced the gate; merge_pr reports AlreadyMerged. The gate
    // treats it as a success terminal — surge:merged + success escalation,
    // never merge-blocked — instead of a false conflict alarm.
    let setup = make_setup().await;
    let task_id_str = "mock:test#50";
    seed_l3_task(&setup.src, task_id_str).await;
    setup
        .src
        .arm_merge_readiness(MergeReadiness::Ready { head_ref: None })
        .await;
    setup
        .src
        .arm_merge_outcome(MergeOutcome::AlreadyMerged)
        .await;

    let run_id = RunId::new();
    {
        let guard = setup.conn.lock().await;
        seed_ticket(&guard, task_id_str, &run_id.to_string());
    }

    let rx = setup.tx.subscribe();
    let _handle = spawn_gate(&setup, rx, false);
    setup.tx.send(completed_event(run_id)).unwrap();

    let _ = wait_for_comments(&setup.src, 1).await;
    let labels = setup.src.recorded_labels().await;
    assert!(
        labels
            .iter()
            .any(|(_, l, p)| l == automation_merge_gate::labels::MERGED && *p),
        "AlreadyMerged must apply merged label, got: {labels:?}"
    );
    assert!(
        labels
            .iter()
            .all(|(_, l, _)| l != automation_merge_gate::labels::MERGE_BLOCKED),
        "AlreadyMerged must not apply merge-blocked, got: {labels:?}"
    );
    let escalations = wait_for_escalations(&setup.notifier, 1).await;
    assert!(
        escalations
            .iter()
            .any(|(sev, _, _)| matches!(sev, NotifySeverity::Success)),
        "AlreadyMerged must escalate success, got: {escalations:?}"
    );
    setup.finish(_handle).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn l3_default_readiness_blocks_when_provider_does_not_implement() {
    // No `arm_merge_readiness` — MockTaskSource falls back to the same
    // Blocked reason a PR-less provider (Linear) surfaces.
    let setup = make_setup().await;
    let task_id_str = "mock:test#44";
    seed_l3_task(&setup.src, task_id_str).await;

    let run_id = RunId::new();
    {
        let guard = setup.conn.lock().await;
        seed_ticket(&guard, task_id_str, &run_id.to_string());
    }

    let rx = setup.tx.subscribe();
    let _handle = spawn_gate(&setup, rx, false);
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
    setup.finish(_handle).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn non_l3_task_is_a_no_op() {
    let setup = make_setup().await;
    let task_id_str = "mock:test#45";
    seed_l1_task(&setup.src, task_id_str).await;
    setup
        .src
        .arm_merge_readiness(MergeReadiness::Ready { head_ref: None })
        .await;
    setup.src.arm_merge_outcome(MergeOutcome::Merged).await;

    let run_id = RunId::new();
    {
        let guard = setup.conn.lock().await;
        seed_ticket(&guard, task_id_str, &run_id.to_string());
    }

    let rx = setup.tx.subscribe();
    let _handle = spawn_gate(&setup, rx, false);
    setup.tx.send(completed_event(run_id)).unwrap();

    // Positive anchor: the gate resolves the policy via fetch_task before it
    // can decide non-L3, so waiting for that call proves the gate processed
    // the event — then the no-op assertions are meaningful, not just a race.
    wait_for_fetch_task(&setup.src, 1).await;
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
    setup.finish(_handle).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn idempotent_double_merge() {
    let setup = make_setup().await;
    let task_id_str = "mock:test#46";
    seed_l3_task(&setup.src, task_id_str).await;
    setup
        .src
        .arm_merge_readiness(MergeReadiness::Ready { head_ref: None })
        .await;
    setup.src.arm_merge_outcome(MergeOutcome::Merged).await;

    let run_id = RunId::new();
    {
        let guard = setup.conn.lock().await;
        seed_ticket(&guard, task_id_str, &run_id.to_string());
    }

    let rx = setup.tx.subscribe();
    let _handle = spawn_gate(&setup, rx, false);

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
    setup.finish(_handle).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_run_outcome_does_not_trigger_gate() {
    let setup = make_setup().await;
    let task_id_str = "mock:test#47";
    seed_l3_task(&setup.src, task_id_str).await;
    setup
        .src
        .arm_merge_readiness(MergeReadiness::Ready { head_ref: None })
        .await;
    setup.src.arm_merge_outcome(MergeOutcome::Merged).await;

    let run_id = RunId::new();
    {
        let guard = setup.conn.lock().await;
        seed_ticket(&guard, task_id_str, &run_id.to_string());
    }

    let rx = setup.tx.subscribe();
    let _handle = spawn_gate(&setup, rx, false);
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
    setup.finish(_handle).await;
}

/// Durable completion survives a subscriber starting after broadcast delivery,
/// including tickets already settled by the separate completion consumer.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn startup_recovers_completed_terminal_ticket_without_broadcast() {
    let setup = make_setup().await;
    let task = "mock:test#restart";
    seed_l3_task(&setup.src, task).await;
    setup
        .src
        .arm_merge_readiness(MergeReadiness::Ready { head_ref: None })
        .await;
    setup.src.arm_merge_outcome(MergeOutcome::Merged).await;
    let run_id = RunId::new();
    seed_completed_journal(&setup, run_id).await;
    {
        let guard = setup.conn.lock().await;
        seed_ticket(&guard, task, &run_id.to_string());
        IntakeRepo::new(&guard)
            .update_state(task, TicketState::Completed)
            .unwrap();
    }
    let handle = spawn_gate(&setup, setup.tx.subscribe(), false);
    let comments = wait_for_comments(&setup.src, 1).await;
    assert_eq!(
        comments.len(),
        1,
        "startup must recover without any RunFinished event"
    );
    assert_eq!(setup.src.merge_calls().await.len(), 1);
    handle.abort();
    assert!(handle.await.unwrap_err().is_cancelled());
    let restarted = spawn_gate(&setup, setup.tx.subscribe(), false);
    assert_no_new_comments_for(&setup.src, Duration::from_millis(100)).await;
    assert_eq!(
        setup.src.merge_calls().await.len(),
        1,
        "durable merged receipt suppresses restart"
    );
    restarted.abort();
    assert!(restarted.await.unwrap_err().is_cancelled());
    setup.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn merge_receipt_storage_failure_never_calls_provider() {
    let setup = make_setup().await;
    let task = "mock:test#receipt-failure";
    seed_l3_task(&setup.src, task).await;
    setup
        .src
        .arm_merge_readiness(MergeReadiness::Ready { head_ref: None })
        .await;
    let run_id = RunId::new();
    {
        let guard = setup.conn.lock().await;
        seed_ticket(&guard, task, &run_id.to_string());
        guard.execute_batch("DROP TABLE intake_emit_log").unwrap();
    }
    let (tx, rx) = broadcast::channel(1);
    let handle = spawn_gate(&setup, rx, false);
    tx.send(completed_event(run_id)).unwrap();
    drop(tx);
    // Channel closure waits behind the dispatched event, proving it was consumed.
    tokio::time::timeout(Duration::from_secs(2), handle)
        .await
        .unwrap()
        .unwrap();
    assert!(
        setup.src.merge_calls().await.is_empty(),
        "missing durable receipt must fail closed"
    );
    setup.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn interrupted_merge_attempt_escalates_without_retrying_provider() {
    let setup = make_setup().await;
    let task = "mock:test#uncertain";
    seed_l3_task(&setup.src, task).await;
    setup
        .src
        .arm_merge_readiness(MergeReadiness::Ready { head_ref: None })
        .await;
    let run_id = RunId::new();
    {
        let guard = setup.conn.lock().await;
        seed_ticket(&guard, task, &run_id.to_string());
        guard.execute(
            "INSERT INTO intake_emit_log(source_id,task_id,event_kind,run_id,recorded_at) VALUES (?1,?2,'merge_attempted',?3,?4)",
            rusqlite::params!["mock:test", task, run_id.to_string(), Utc::now().timestamp_millis()],
        ).unwrap();
    }
    // Removing L3 after publication cannot erase the uncertain external effect.
    seed_l1_task(&setup.src, task).await;
    let handle = spawn_gate(&setup, setup.tx.subscribe(), false);
    setup.tx.send(completed_event(run_id)).unwrap();
    let notices = wait_for_escalations(&setup.notifier, 1).await;
    assert_eq!(
        notices.len(),
        1,
        "uncertain external result requires operator visibility"
    );
    assert!(notices[0].2.contains("manual"));
    assert!(
        setup.src.merge_calls().await.is_empty(),
        "uncertain published attempt cannot be replayed"
    );
    handle.abort();
    assert!(handle.await.unwrap_err().is_cancelled());
    let (tx, rx) = broadcast::channel(1);
    let restarted = spawn_gate(&setup, rx, false);
    tx.send(completed_event(run_id)).unwrap();
    drop(tx);
    tokio::time::timeout(Duration::from_secs(2), restarted)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        setup.notifier.snapshot().await.len(),
        1,
        "durable uncertainty classification suppresses repeated warnings"
    );
    assert!(setup.src.merge_calls().await.is_empty());
    setup.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejected_merge_reservation_never_calls_provider() {
    let setup = make_setup().await;
    let task = "mock:test#reservation-failure";
    seed_l3_task(&setup.src, task).await;
    setup
        .src
        .arm_merge_readiness(MergeReadiness::Ready { head_ref: None })
        .await;
    let run_id = RunId::new();
    {
        let guard = setup.conn.lock().await;
        seed_ticket(&guard, task, &run_id.to_string());
        guard.execute_batch("CREATE TRIGGER reject_merge_attempt BEFORE INSERT ON intake_emit_log WHEN NEW.event_kind = 'merge_attempted' BEGIN SELECT RAISE(ABORT, 'receipt unavailable'); END").unwrap();
    }
    let (tx, rx) = broadcast::channel(1);
    let handle = spawn_gate(&setup, rx, false);
    tx.send(completed_event(run_id)).unwrap();
    drop(tx);
    tokio::time::timeout(Duration::from_secs(2), handle)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        setup.src.fetch_task_calls().await,
        1,
        "normal admission reached provider lookup"
    );
    assert!(
        setup.src.merge_calls().await.is_empty(),
        "RPC requires a successfully inserted reservation"
    );
    assert!(setup.src.posted_comments().await.is_empty());
    setup.close();
}

async fn seed_completed_journal(setup: &Setup, run_id: RunId) {
    let writer = setup
        .runs
        .create_run(run_id, setup._home.path(), None)
        .await
        .unwrap();
    writer
        .append_event(surge_core::run_event::VersionedEventPayload::new(
            surge_core::run_event::EventPayload::RunStarted {
                pipeline_template: None,
                project_path: setup._home.path().into(),
                initial_prompt: "recovery fixture".into(),
                config: surge_core::run_event::RunConfig {
                    bootstrap_edit_loop_cap: None,
                    sandbox_default: surge_core::sandbox::SandboxMode::WorkspaceWrite,
                    approval_default: surge_core::approvals::ApprovalPolicy::OnRequest,
                    auto_pr: false,
                    mcp_servers: Vec::new(),
                    budget: surge_core::budget::BudgetGuard::default(),
                },
            },
        ))
        .await
        .unwrap();
    writer
        .append_event(surge_core::run_event::VersionedEventPayload::new(
            surge_core::run_event::EventPayload::RunCompleted {
                terminal_node: NodeKey::try_new("end").unwrap(),
            },
        ))
        .await
        .unwrap();
    writer.flush().await.unwrap();
    writer.close().await.unwrap();
}

/// Storage is opened on its required multi-thread runtime. Only the receiving
/// gate runs on a dedicated current-thread runtime so the burst cannot interleave.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lag_recovers_dropped_completion_from_journal() {
    let setup = make_setup().await;
    let warm = RunId::new();
    seed_l1_task(&setup.src, "mock:test#warm").await;
    {
        let guard = setup.conn.lock().await;
        seed_ticket(&guard, "mock:test#warm", &warm.to_string());
    }
    let run_id = RunId::new();
    seed_l3_task(&setup.src, "mock:test#lagged").await;
    seed_completed_journal(&setup, run_id).await;
    setup
        .src
        .arm_merge_readiness(MergeReadiness::Ready { head_ref: None })
        .await;
    setup.src.arm_merge_outcome(MergeOutcome::Merged).await;
    tokio::task::spawn_blocking(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async move {
                let (tx, rx) = broadcast::channel(1);
                let handle = spawn_gate(&setup, rx, false);
                tx.send(completed_event(warm)).unwrap();
                wait_for_fetch_task(&setup.src, 1).await;
                // Insert correlation only after the startup sweep has passed.
                {
                    let guard = setup.conn.lock().await;
                    seed_ticket(&guard, "mock:test#lagged", &run_id.to_string());
                }
                tx.send(completed_event(run_id)).unwrap();
                for _ in 0..8 {
                    tx.send(GlobalDaemonEvent::DaemonShuttingDown).unwrap();
                }
                let comments = wait_for_comments(&setup.src, 1).await;
                assert_eq!(comments.len(), 1, "lag must recover the durable completion");
                assert_eq!(setup.src.merge_calls().await.len(), 1);
                handle.abort();
                assert!(handle.await.unwrap_err().is_cancelled());
                setup.close();
            });
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn published_merge_error_is_durable_uncertainty_and_never_replayed() {
    let setup = make_setup().await;
    let task = "mock:test#lost-merge-response";
    seed_l3_task(&setup.src, task).await;
    setup
        .src
        .arm_merge_readiness(MergeReadiness::Ready { head_ref: None })
        .await;
    // Unarmed mock publishes/counts the merge call, then returns an error.
    // A transport error cannot prove the provider did not apply the request.
    let run_id = RunId::new();
    {
        let guard = setup.conn.lock().await;
        seed_ticket(&guard, task, &run_id.to_string());
    }
    let (tx, rx) = broadcast::channel(1);
    let handle = spawn_gate(&setup, rx, false);
    tx.send(completed_event(run_id)).unwrap();
    drop(tx);
    tokio::time::timeout(Duration::from_secs(2), handle)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(setup.src.merge_calls().await.len(), 1);
    {
        let guard = setup.conn.lock().await;
        assert!(
            surge_persistence::intake_emit_log::has(
                &guard,
                surge_persistence::intake_emit_log::EmitKey {
                    source_id: "mock:test",
                    task_id: task,
                    event_kind: surge_persistence::intake_emit_log::EmitEventKind::MergeUncertain,
                    run_id: &run_id.to_string(),
                }
            )
            .unwrap(),
            "error after publication must persist unknown external outcome"
        );
    }
    let notices = setup.notifier.snapshot().await;
    assert_eq!(notices.len(), 1);
    assert!(notices[0].2.contains("unknown external outcome"));
    assert!(notices[0].2.contains("manual"));
    let (tx, rx) = broadcast::channel(1);
    let restarted = spawn_gate(&setup, rx, false);
    tx.send(completed_event(run_id)).unwrap();
    drop(tx);
    tokio::time::timeout(Duration::from_secs(2), restarted)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        setup.src.merge_calls().await.len(),
        1,
        "uncertain merge cannot be replayed after restart"
    );
    assert_eq!(setup.notifier.snapshot().await.len(), 1);
    setup.close();
}
