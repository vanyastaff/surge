//! 12.7 — 5 parallel RunWriters × 1000 events each, verify no deadlock.

use crate::runs::fixtures::{dummy_payload, setup};
use surge_core::RunId;
use surge_persistence::runs::EventSeq;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn five_parallel_run_writers_complete_independently() {
    let t = setup().await;
    let storage = t.storage.clone();

    let mut handles = Vec::new();
    for _ in 0..5 {
        let storage = storage.clone();
        let run_id = RunId::new();
        handles.push(tokio::spawn(async move {
            let writer = storage
                .create_run(run_id, "/tmp/proj", None)
                .await
                .expect("create_run");
            for i in 0..1000u64 {
                writer
                    .append_event(dummy_payload(i))
                    .await
                    .expect("append_event");
            }
            writer.flush().await.expect("flush");
            let seq = writer.current_seq().await.expect("current_seq");
            writer.close().await.expect("close");
            seq
        }));
    }

    // join_all-style; if any task panics or deadlocks, .await fails (test
    // timeout via cargo's default 60s if it deadlocks).
    for h in handles {
        let seq = h.await.expect("task join");
        assert_eq!(seq, EventSeq(1000));
    }
}
