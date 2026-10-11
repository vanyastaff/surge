use super::*;
use crate::history_bypass_fixture::{Fixture, close_registry};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn history_after_actual_settlement_failure_does_not_recover_success() {
    let fixture = Fixture::new().await;
    let recovered = durable_outcome(&fixture.storage, fixture.run).await;
    fixture.close();
    assert!(
        !matches!(recovered, Some(RunOutcome::Completed { .. })),
        "historical recovery authorized success after actual Engine SettlementFailed: {recovered:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn history_after_actual_settlement_failure_does_not_enqueue_terminal_comment() {
    let fixture = Fixture::new().await;
    let conn = fixture.registry("mock:test#unconfirmed-intake");
    let sources = Arc::new(HashMap::new());
    reconcile_pending(&fixture.storage, &sources, &conn).await;
    let (state, outbox_count) = {
        let guard = conn.lock().await;
        let row = IntakeRepo::new(&guard)
            .lookup_ticket_by_run_id(&fixture.run.to_string())
            .unwrap()
            .unwrap();
        let count: i64 = guard
            .query_row("SELECT count(*) FROM terminal_comment_outbox", [], |row| {
                row.get(0)
            })
            .unwrap();
        (row.state, count)
    };
    close_registry(conn);
    fixture.close();
    assert_eq!(
        (state, outbox_count),
        (TicketState::Active, 0),
        "historical reconciliation authorized a terminal state/outbox after actual Engine SettlementFailed"
    );
}
