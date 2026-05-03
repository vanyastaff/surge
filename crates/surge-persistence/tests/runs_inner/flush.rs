//! 12.12 [P1.X1] — issue 100 concurrent appends, flush, verify count.

use std::sync::Arc;

use crate::runs::fixtures::{dummy_payload, setup};
use futures::future::join_all;
use surge_persistence::runs::EventSeq;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn flush_drains_pending_appends() {
    let t = setup().await;
    let writer = t
        .storage
        .create_run(t.run_id, "/tmp/proj", None)
        .await
        .expect("create_run");
    let writer = Arc::new(writer);

    // Spawn 100 append futures all at once. Each future is independent of
    // the others — they race into the writer's mpsc channel concurrently.
    let mut futures = Vec::with_capacity(100);
    for i in 0..100u64 {
        let w = writer.clone();
        futures.push(tokio::spawn(async move {
            w.append_event(dummy_payload(i)).await
        }));
    }
    let results = join_all(futures).await;
    for r in results {
        let seq = r.expect("task join").expect("append_event");
        assert!(seq.as_u64() >= 1 && seq.as_u64() <= 100);
    }

    // Now issue Flush — once it returns, the strict-ordered mpsc guarantees
    // every previously enqueued append has been processed and committed.
    writer.flush().await.expect("flush");

    let seq = writer.current_seq().await.expect("current_seq");
    assert_eq!(
        seq,
        EventSeq(100),
        "all 100 concurrent appends must be committed after flush"
    );

    Arc::try_unwrap(writer)
        .map_err(|_| "writer Arc still has refs")
        .expect("unique writer")
        .close()
        .await
        .expect("close");
}
