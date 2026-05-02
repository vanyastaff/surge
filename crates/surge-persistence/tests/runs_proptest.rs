//! Property-based tests for the runs module.
//!
//! Separate test binary so proptest's long-running shrink cycle doesn't slow
//! down the regular integration suite.

use std::sync::Arc;

use proptest::prelude::*;
use surge_core::run_event::{EventPayload, VersionedEventPayload};
use surge_core::{RunId, SessionId};
use surge_persistence::runs::{EventSeq, MockClock, Storage};
use tempfile::TempDir;

fn payload_strategy() -> impl Strategy<Value = VersionedEventPayload> {
    (0u32..1000, 0u32..1000, 0u32..100).prop_map(|(p, o, c)| {
        VersionedEventPayload::new(EventPayload::TokensConsumed {
            session: SessionId::new(),
            prompt_tokens: p,
            output_tokens: o,
            cache_hits: c,
            model: "test-model".into(),
            cost_usd: None,
        })
    })
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 32,
        ..ProptestConfig::default()
    })]

    #[test]
    fn append_then_read_roundtrip(payloads in proptest::collection::vec(payload_strategy(), 1..50)) {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(2)
            .build()
            .unwrap();

        runtime.block_on(async {
            let tmp = TempDir::new().unwrap();
            let clock = MockClock::new(1_700_000_000_000);
            let storage = Storage::open_with(tmp.path(), Arc::new(clock)).await.unwrap();
            let run_id = RunId::new();
            let writer = storage.create_run(run_id.clone(), "/tmp", None).await.unwrap();

            let mut expected_seqs = Vec::new();
            for p in &payloads {
                let s = writer.append_event(p.clone()).await.unwrap();
                expected_seqs.push(s);
            }
            writer.flush().await.unwrap();

            let read = writer
                .read_events(EventSeq(1)..EventSeq(payloads.len() as u64 + 1))
                .await
                .unwrap();
            prop_assert_eq!(read.len(), payloads.len());
            for (i, ev) in read.iter().enumerate() {
                prop_assert_eq!(ev.seq, expected_seqs[i]);
            }
            writer.close().await.unwrap();
            Ok(())
        }).unwrap();
    }
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 16,
        ..ProptestConfig::default()
    })]

    #[test]
    fn view_maintenance_matches_rebuild(
        payloads in proptest::collection::vec(payload_strategy(), 1..30)
    ) {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(2)
            .build()
            .unwrap();

        runtime.block_on(async {
            let tmp = TempDir::new().unwrap();
            let clock = MockClock::new(1_700_000_000_000);
            let storage = Storage::open_with(tmp.path(), Arc::new(clock)).await.unwrap();
            let run_id = RunId::new();
            let writer = storage.create_run(run_id.clone(), "/tmp", None).await.unwrap();

            for p in &payloads { writer.append_event(p.clone()).await.unwrap(); }
            writer.flush().await.unwrap();

            let before = writer.cost_summary().await.unwrap();
            writer.rebuild_views().await.unwrap();
            let after = writer.cost_summary().await.unwrap();

            prop_assert_eq!(before.tokens_in, after.tokens_in);
            prop_assert_eq!(before.tokens_out, after.tokens_out);
            prop_assert_eq!(before.cache_hits, after.cache_hits);

            writer.close().await.unwrap();
            Ok(())
        }).unwrap();
    }
}
