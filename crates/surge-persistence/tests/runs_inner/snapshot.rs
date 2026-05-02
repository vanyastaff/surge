//! 14.1 — handcrafted linear flow snapshot test.
//!
//! Uses MockClock + insta to lock the materialized view shape after a 3-event
//! handcrafted flow. Guards against silent view-schema drift.

use std::str::FromStr;

use crate::runs::fixtures::setup;
use insta::assert_yaml_snapshot;
use surge_core::run_event::{EventPayload, VersionedEventPayload};
use surge_core::{NodeKey, OutcomeKey, SessionId};

fn vp(p: EventPayload) -> VersionedEventPayload {
    VersionedEventPayload::new(p)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn handcrafted_linear_flow_view_snapshot() {
    let t = setup().await;
    let writer = t
        .storage
        .create_run(t.run_id.clone(), "/tmp/proj", None)
        .await
        .expect("create_run");

    let n = NodeKey::from_str("spec_1").expect("nodekey");
    t.clock.set(1_700_000_000_000);
    writer
        .append_event(vp(EventPayload::StageEntered {
            node: n.clone(),
            attempt: 1,
        }))
        .await
        .expect("append StageEntered");

    t.clock.advance(100);
    writer
        .append_event(vp(EventPayload::TokensConsumed {
            session: SessionId::new(),
            prompt_tokens: 100,
            output_tokens: 50,
            cache_hits: 5,
            model: "claude".into(),
            cost_usd: Some(0.01),
        }))
        .await
        .expect("append TokensConsumed");

    t.clock.advance(100);
    writer
        .append_event(vp(EventPayload::StageCompleted {
            node: n.clone(),
            outcome: OutcomeKey::from_str("done").expect("outcomekey"),
        }))
        .await
        .expect("append StageCompleted");
    writer.flush().await.expect("flush");

    let stages = writer.stage_executions().await.expect("stage_executions");
    let cost = writer.cost_summary().await.expect("cost_summary");

    assert_yaml_snapshot!("linear_flow_stages", stages, {
        "[].started_at_ms" => "[ts]",
        "[].ended_at_ms"   => "[ts]",
    });
    assert_yaml_snapshot!("linear_flow_cost", cost);

    writer.close().await.expect("close");
}
