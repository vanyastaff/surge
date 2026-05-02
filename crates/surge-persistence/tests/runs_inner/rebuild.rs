//! 12.4 — append events, snapshot views, rebuild_views(), assert identical.

use std::str::FromStr;

use crate::runs::fixtures::setup;
use surge_core::run_event::{EventPayload, VersionedEventPayload};
use surge_core::{NodeKey, OutcomeKey, SessionId};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rebuild_views_is_identity_over_event_log() {
    let t = setup().await;
    let writer = t
        .storage
        .create_run(t.run_id.clone(), "/tmp/proj", None)
        .await
        .expect("create_run");

    let node = NodeKey::from_str("impl_1").expect("nodekey");
    let outcome = OutcomeKey::from_str("done").expect("outcomekey");

    writer
        .append_event(VersionedEventPayload::new(EventPayload::StageEntered {
            node: node.clone(),
            attempt: 1,
        }))
        .await
        .expect("append StageEntered");

    for (p, o) in [(50u32, 25u32), (75u32, 30u32), (100u32, 40u32)] {
        writer
            .append_event(VersionedEventPayload::new(
                EventPayload::TokensConsumed {
                    session: SessionId::new(),
                    prompt_tokens: p,
                    output_tokens: o,
                    cache_hits: 0,
                    model: "claude".into(),
                    cost_usd: Some(0.005),
                },
            ))
            .await
            .expect("append TokensConsumed");
    }

    writer
        .append_event(VersionedEventPayload::new(EventPayload::StageCompleted {
            node: node.clone(),
            outcome: outcome.clone(),
        }))
        .await
        .expect("append StageCompleted");

    writer.flush().await.expect("flush");

    let pre_cost = writer.cost_summary().await.expect("pre cost");
    let pre_stages = writer.stage_executions().await.expect("pre stages");

    writer.rebuild_views().await.expect("rebuild_views");

    let post_cost = writer.cost_summary().await.expect("post cost");
    let post_stages = writer.stage_executions().await.expect("post stages");

    assert_eq!(pre_cost.tokens_in, post_cost.tokens_in);
    assert_eq!(pre_cost.tokens_out, post_cost.tokens_out);
    assert_eq!(pre_cost.cache_hits, post_cost.cache_hits);
    assert!((pre_cost.cost_usd - post_cost.cost_usd).abs() < 1e-9);
    assert_eq!(pre_stages.len(), post_stages.len());
    assert_eq!(pre_stages[0].node_id, post_stages[0].node_id);
    assert_eq!(pre_stages[0].outcome, post_stages[0].outcome);
    assert_eq!(pre_stages[0].started_seq, post_stages[0].started_seq);
    assert_eq!(pre_stages[0].ended_seq, post_stages[0].ended_seq);

    writer.close().await.expect("close");
}
