//! 12.6 — drop writer mid-write (no close), reopen, verify integrity.

use crate::runs::fixtures::{dummy_payload, setup};
use surge_persistence::runs::EventSeq;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropped_writer_can_be_reopened_with_intact_log() {
    let t = setup().await;

    {
        let writer = t
            .storage
            .create_run(t.run_id.clone(), "/tmp/proj", None)
            .await
            .expect("create_run");

        for i in 0..50u64 {
            writer
                .append_event(dummy_payload(i))
                .await
                .expect("append_event");
        }
        // flush ensures writer task processed all 50 commands before drop.
        writer.flush().await.expect("flush");
        // Intentionally drop without close() — simulates a crash. The Drop
        // impl emits tracing::warn but is best-effort; the file lock and
        // in-process token release at scope exit.
        drop(writer);
    }

    // Drop is fire-and-forget — the writer task may still be running and
    // holding the file-lock guard when this test resumes. Poll until the
    // slot frees (writer task finished processing Shutdown and dropped the
    // file lock).
    let mut writer2 = None;
    for _ in 0..200 {
        match t.storage.open_run_writer(t.run_id.clone()).await {
            Ok(w) => {
                writer2 = Some(w);
                break;
            }
            Err(_) => {
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            }
        }
    }
    let writer2 = writer2.expect("reopen after drop within timeout");

    let seq = writer2.current_seq().await.expect("current_seq");
    assert!(
        seq.as_u64() >= 50,
        "expected at least 50 events recovered, got {seq}"
    );

    // Verify we can still append on top.
    let next = writer2
        .append_event(dummy_payload(999))
        .await
        .expect("append after recovery");
    assert_eq!(next, EventSeq(seq.as_u64() + 1));

    writer2.close().await.expect("close");
}
