use super::*;
use crate::history_bypass_fixture::{Fixture, close_registry};
use surge_core::notify_config::NotifyChannel;
use surge_intake::testing::MockTaskSource;
use surge_intake::types::TaskDetails;
use surge_notify::{NotifyDeliveryContext, NotifyError, RenderedNotification};

struct LocalNotifier;

#[async_trait::async_trait]
impl NotifyDeliverer for LocalNotifier {
    async fn deliver(
        &self,
        _context: &NotifyDeliveryContext<'_>,
        _channel: &NotifyChannel,
        _rendered: &RenderedNotification,
    ) -> Result<(), NotifyError> {
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn history_after_actual_settlement_failure_does_not_dispatch_merge() {
    let fixture = Fixture::new().await;
    let task = "mock:test#unconfirmed-merge";
    let conn = fixture.registry(task);
    let source = Arc::new(MockTaskSource::new("mock:test", "mock"));
    source
        .put_task(TaskDetails {
            task_id: TaskId::try_new(task).unwrap(),
            source_id: "mock:test".into(),
            title: "Unconfirmed engine run".into(),
            description: "real failed settlement".into(),
            status: "open".into(),
            labels: vec!["surge:auto".into()],
            url: format!("https://example.com/{task}"),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            assignee: None,
            raw_payload: serde_json::Value::Null,
        })
        .await;
    source
        .arm_merge_readiness(MergeReadiness::Ready { head_ref: None })
        .await;
    source
        .arm_merge_outcome(surge_intake::MergeOutcome::Merged)
        .await;
    let sources = Arc::new(HashMap::from([(
        "mock:test".to_owned(),
        source.clone() as Arc<dyn TaskSource>,
    )]));
    let notifier: Arc<dyn NotifyDeliverer> = Arc::new(LocalNotifier);
    reconcile_page(
        &sources,
        &conn,
        &notifier,
        &fixture.storage,
        false,
        &mut String::new(),
    )
    .await;
    let calls = source.merge_calls().await.len();
    let attempted: i64 = conn
        .lock()
        .await
        .query_row(
            "SELECT count(*) FROM intake_emit_log WHERE event_kind='merge_attempted'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    close_registry(conn);
    fixture.close();
    assert_eq!(
        (calls, attempted),
        (0, 0),
        "historical reconciliation authorized merge after actual Engine SettlementFailed"
    );
}
