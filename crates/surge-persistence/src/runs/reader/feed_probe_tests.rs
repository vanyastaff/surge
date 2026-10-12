use super::*;
use crate::{
    runs::{FeedLifecycleError, Storage, WriterFailure, WriterLifecycle},
    runtime_home_fixture::FixtureHome,
};
use std::time::Duration;
use surge_core::EventPayload;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn feed_pool_probe_controls() {
    let home = FixtureHome::new().unwrap();
    std::fs::write(
        home.path().join("config.toml"),
        "[storage]\nreader_pool_size=2\n",
    )
    .unwrap();
    let storage = Storage::open(home.path()).await.unwrap();
    let run = RunId::new();
    let writer = storage.create_run(run, home.path(), None).await.unwrap();
    let mut feed = writer.event_feed();
    let (mut gate, mut probe) = feed.install_pool_probe_for_test().unwrap();
    let read = feed.read_lease();
    let worker = tokio::task::spawn_blocking(move || {
        read.read_batch(EventSeq::ZERO, EventSeq(i64::MAX as u64))
    });
    let entered = tokio::time::timeout(Duration::from_secs(5), gate.entered()).await;
    let appended = writer
        .append_event(VersionedEventPayload::new(EventPayload::RunAborted {
            reason: "pool control unit fixture".into(),
        }))
        .await;
    let sealed = writer.seal_for_read_drain().await;
    let observed = probe.wait_sealed().await;
    let controls = tokio::task::spawn_blocking(move || {
        let available = probe.check_available_zero_timeout();
        let slots = probe.exhaust_all_slots();
        let exhausted = probe.verify_exhausted_zero_timeout();
        (probe, available, slots, exhausted)
    })
    .await;
    gate.release();
    let read_result = worker.await;
    // No assertion propagates before the actual worker and controller job join.
    let (probe, available, slots, exhausted) = controls.unwrap();
    let slots = slots.unwrap();
    let count = slots.count();
    let capacity = slots.capacity();
    drop(slots);
    let restored = tokio::task::spawn_blocking(move || {
        let restored = probe.check_available_zero_timeout();
        drop(probe);
        restored
    })
    .await;
    drop(feed);
    storage
        .open_run_writer(run)
        .await
        .unwrap()
        .close()
        .await
        .unwrap();
    drop(storage);
    home.close().unwrap();
    assert!(entered.unwrap().is_ok());
    assert_eq!(appended.unwrap(), EventSeq(1));
    assert_eq!(sealed.unwrap(), EventSeq(1));
    assert_eq!(observed.unwrap(), EventSeq(1));
    assert!(available.is_ok() && exhausted.is_ok());
    assert_eq!((count, capacity), (2, 2));
    assert!(matches!(read_result.unwrap(), Err(StorageError::Pool(_))));
    assert!(restored.unwrap().is_ok());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pool_probe_rejects_failed_and_lost_running_lifecycle() {
    for abort_actor in [false, true] {
        let home = FixtureHome::new().unwrap();
        let storage = Storage::open(home.path()).await.unwrap();
        let writer = storage
            .create_run(RunId::new(), home.path(), None)
            .await
            .unwrap();
        writer.flush().await.unwrap();
        let mut writer = writer;
        let mut feed = writer.event_feed();
        let (gate, mut probe) = feed.install_pool_probe_for_test().unwrap();
        if abort_actor {
            writer.writer_join.as_ref().unwrap().abort();
        } else {
            writer.request_stop();
        }
        // A cancellation request alone does not establish actor loss: a seal
        // command could still win. Observe the intended lifecycle first.
        let stopped = tokio::time::timeout(Duration::from_secs(5), feed.changed())
            .await
            .unwrap();
        if abort_actor {
            assert_eq!(stopped, Err(FeedLifecycleError::WriterLost));
        } else {
            assert_eq!(
                stopped,
                Ok(WriterLifecycle::Failed(WriterFailure::Interrupted))
            );
        }
        let closed = writer.seal_for_read_drain().await;
        let observed = probe.wait_sealed().await;
        let lifecycle = feed.current_lifecycle();
        drop(gate);
        drop(probe);
        drop(feed);
        drop(storage);
        home.close().unwrap();
        assert!(closed.is_err());
        assert!(matches!(observed, Err(StorageError::WriterTaskDied)));
        if abort_actor {
            assert_eq!(lifecycle, WriterLifecycle::Running);
        } else {
            assert!(matches!(lifecycle, WriterLifecycle::Failed(_)));
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn all_feed_gate_installation_orders_reject_nesting() {
    let home = FixtureHome::new().unwrap();
    let storage = Storage::open(home.path()).await.unwrap();
    let writer = storage
        .create_run(RunId::new(), home.path(), None)
        .await
        .unwrap();
    for first in 0..3 {
        let mut feed = writer.event_feed();
        let guard = match first {
            0 => {
                let (gate, probe) = feed.install_pool_probe_for_test().unwrap();
                drop(probe);
                gate
            },
            1 => feed.install_read_gate_for_test().unwrap(),
            _ => feed.install_post_read_gate_for_test().unwrap(),
        };
        assert!(matches!(
            feed.install_pool_probe_for_test(),
            Err(StorageError::OperationRejected(_))
        ));
        assert!(matches!(
            feed.install_read_gate_for_test(),
            Err(StorageError::OperationRejected(_))
        ));
        assert!(matches!(
            feed.install_post_read_gate_for_test(),
            Err(StorageError::OperationRejected(_))
        ));
        drop(guard);
        drop(feed);
    }
    writer.close().await.unwrap();
    drop(storage);
    home.close().unwrap();
}
