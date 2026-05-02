//! 12.2 — append 1000 events and verify monotonic seq + range reads.

use crate::runs::fixtures::{dummy_payload, setup};
use surge_persistence::runs::EventSeq;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn append_read_1000_events_correct_ordered_atomic() {
    let t = setup().await;
    let writer = t
        .storage
        .create_run(t.run_id.clone(), "/tmp/proj", None)
        .await
        .expect("create_run");

    for i in 0..1000u64 {
        let seq = writer
            .append_event(dummy_payload(i))
            .await
            .expect("append_event");
        assert_eq!(seq.as_u64(), i + 1, "seq must be monotonic, 1-based");
    }

    writer.flush().await.expect("flush");

    let current = writer.current_seq().await.expect("current_seq");
    assert_eq!(current, EventSeq(1000));

    let chunk = writer
        .read_events(EventSeq(1)..EventSeq(101))
        .await
        .expect("read_events");
    assert_eq!(chunk.len(), 100, "100 events in [1, 101)");
    assert_eq!(chunk.first().unwrap().seq, EventSeq(1));
    assert_eq!(chunk.last().unwrap().seq, EventSeq(100));

    // The full range must come back in order with no gaps.
    let all = writer
        .read_events(EventSeq(1)..EventSeq(1001))
        .await
        .expect("read full range");
    assert_eq!(all.len(), 1000);
    for (i, ev) in all.iter().enumerate() {
        assert_eq!(ev.seq.as_u64(), (i as u64) + 1);
    }

    writer.close().await.expect("close");
}
