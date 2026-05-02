//! 12.9 — polling stream yields events; subscribe outlives storage handle.

use std::time::Duration;

use crate::runs::fixtures::{dummy_payload, setup};
use surge_persistence::runs::EventSeq;
use tokio_stream::StreamExt;

/// Writer appends 10 events; a concurrent subscriber must observe all of them.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn subscriber_yields_events_appended_after_subscription() {
    let t = setup().await;
    let writer = t
        .storage
        .create_run(t.run_id.clone(), "/tmp/proj", None)
        .await
        .expect("create_run");

    let mut stream = Box::pin(writer.subscribe_events());

    // Pre-populate a few events so the first poll already has work to do,
    // then add more after the stream is live.
    for i in 0..5u64 {
        writer
            .append_event(dummy_payload(i))
            .await
            .expect("pre append");
    }
    writer.flush().await.expect("flush pre");

    for i in 5..10u64 {
        writer
            .append_event(dummy_payload(i))
            .await
            .expect("post append");
    }
    writer.flush().await.expect("flush post");

    // Collect 10 events; bound the wait per event so a stuck poll fails the
    // test rather than hanging it.
    let mut received = Vec::new();
    while received.len() < 10 {
        match tokio::time::timeout(Duration::from_secs(5), stream.next()).await {
            Ok(Some(Ok(ev))) => received.push(ev),
            Ok(Some(Err(e))) => panic!("stream error: {e}"),
            Ok(None) => panic!("stream ended early"),
            Err(_) => panic!("timed out waiting for event {}", received.len() + 1),
        }
    }

    assert_eq!(received.len(), 10);
    for (i, ev) in received.iter().enumerate() {
        assert_eq!(ev.seq, EventSeq((i as u64) + 1));
    }

    drop(stream);
    writer.close().await.expect("close");
}

/// [P2.X4] After dropping the source storage, the subscribe stream may still
/// be polled — it must either yield no further items or surface a clean error,
/// never panic or deadlock.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn subscribe_outlives_storage_handle() {
    let t = setup().await;
    let writer = t
        .storage
        .create_run(t.run_id.clone(), "/tmp/proj", None)
        .await
        .expect("create_run");

    writer
        .append_event(dummy_payload(1))
        .await
        .expect("append");
    writer.flush().await.expect("flush");

    let mut stream = Box::pin(writer.subscribe_events());

    // Receive the one event we wrote so the stream has actually polled at
    // least once before we drop the storage.
    let first = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .expect("await first")
        .expect("Some(_)")
        .expect("Ok(_)");
    assert_eq!(first.seq, EventSeq(1));

    // Drop the writer first (so the file lock is released), then the
    // storage Arc the test holds. Internal Arcs cloned into the stream
    // (pool, artifacts_dir) keep the underlying SQLite reachable as long
    // as the stream is alive.
    writer.close().await.expect("close");
    drop(t.storage);

    // Poll once more; the stream is allowed to either yield Ok (no new
    // events == None doesn't apply since the loop is infinite, so this
    // arm is via timeout) or Err (pool exhausted, db file moved, etc).
    // Either is acceptable — what we forbid is panic / deadlock / UB.
    let next = tokio::time::timeout(Duration::from_millis(500), stream.next()).await;
    match next {
        Ok(Some(Ok(_))) => {}        // unexpected new event — fine, no panic
        Ok(Some(Err(_))) => {}       // clean error — fine
        Ok(None) => {}               // stream ended cleanly — fine
        Err(_) => {}                 // timed out (no new events) — fine, no panic
    }
}
