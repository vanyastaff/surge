//! 12.3 — append events of various kinds, query views, assert aggregates.

use std::str::FromStr;

use crate::runs::fixtures::setup;
use surge_core::run_event::{EventPayload, VersionedEventPayload};
use surge_core::{NodeKey, OutcomeKey, SessionId};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn views_aggregate_after_canonical_event_sequence() {
    let t = setup().await;
    let writer = t
        .storage
        .create_run(t.run_id, "/tmp/proj", None)
        .await
        .expect("create_run");

    let node = NodeKey::from_str("impl_1").expect("nodekey");
    let outcome = OutcomeKey::from_str("done").expect("outcomekey");

    // Stage entered → tokens consumed (× 2) → stage completed.
    writer
        .append_event(VersionedEventPayload::new(EventPayload::StageEntered {
            node: node.clone(),
            attempt: 1,
        }))
        .await
        .expect("append StageEntered");

    writer
        .append_event(VersionedEventPayload::new(EventPayload::TokensConsumed {
            session: SessionId::new(),
            prompt_tokens: 100,
            output_tokens: 50,
            cache_hits: 10,
            model: "claude".into(),
            cost_usd: Some(0.01),
        }))
        .await
        .expect("append TokensConsumed #1");

    writer
        .append_event(VersionedEventPayload::new(EventPayload::TokensConsumed {
            session: SessionId::new(),
            prompt_tokens: 200,
            output_tokens: 75,
            cache_hits: 5,
            model: "claude".into(),
            cost_usd: Some(0.02),
        }))
        .await
        .expect("append TokensConsumed #2");

    writer
        .append_event(VersionedEventPayload::new(EventPayload::StageCompleted {
            node: node.clone(),
            outcome: outcome.clone(),
        }))
        .await
        .expect("append StageCompleted");

    writer.flush().await.expect("flush");

    let stages = writer.stage_executions().await.expect("stage_executions");
    assert_eq!(stages.len(), 1);
    assert_eq!(stages[0].node_id, node);
    assert_eq!(stages[0].attempt, 1);
    assert_eq!(stages[0].outcome.as_deref(), Some("done"));
    assert!(stages[0].ended_seq.is_some(), "stage should be closed");

    let cost = writer.cost_summary().await.expect("cost_summary");
    assert_eq!(cost.tokens_in, 300);
    assert_eq!(cost.tokens_out, 125);
    assert_eq!(cost.cache_hits, 15);
    assert!(
        (cost.cost_usd - 0.03).abs() < 1e-9,
        "expected cost ~0.03, got {}",
        cost.cost_usd
    );

    writer.close().await.expect("close");
}
