use super::*;
use crate::{
    runs::{Clock, FeedLifecycleError, OpenError, Storage, WriterFailure},
    runtime_home_fixture::FixtureHome,
};
use std::sync::{
    Condvar, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;
use surge_core::{EventPayload, SessionId};

fn payload(index: u32) -> VersionedEventPayload {
    VersionedEventPayload::new(EventPayload::TokensConsumed {
        session: SessionId::new(),
        prompt_tokens: index,
        output_tokens: 1,
        cache_hits: 0,
        model: "finite-feed-oracle".into(),
        cost_usd: None,
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn empty_seal_is_zero_and_feed_holds_exclusion_until_drop() {
    let home = FixtureHome::new().unwrap();
    let storage = Storage::open(home.path()).await.unwrap();
    let run = RunId::new();
    let writer = storage.create_run(run, home.path(), None).await.unwrap();
    let feed = writer.event_feed();
    let prefix = writer.seal_for_read_drain().await.unwrap();
    assert_eq!(prefix, EventSeq::ZERO);
    assert_eq!(
        feed.current_lifecycle(),
        WriterLifecycle::Sealed(EventSeq::ZERO)
    );
    assert!(matches!(
        storage.open_run_writer(run).await,
        Err(OpenError::WriterAlreadyHeld { .. })
    ));
    let read = feed.read_lease();
    let batch = tokio::task::spawn_blocking(move || read.read_batch(EventSeq::ZERO, prefix))
        .await
        .unwrap()
        .unwrap();
    assert!(batch.is_empty());
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
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sealed_prefix_reads_fixed_three_hundred_events_in_bounded_batches() {
    let home = FixtureHome::new().unwrap();
    let storage = Storage::open(home.path()).await.unwrap();
    let run = RunId::new();
    let writer = storage.create_run(run, home.path(), None).await.unwrap();
    writer
        .append_events((1..=300).map(payload).collect())
        .await
        .unwrap();
    let feed = writer.event_feed();
    let prefix = writer.seal_for_read_drain().await.unwrap();
    assert_eq!(prefix, EventSeq(300));
    let read = feed.read_lease();
    let (first, second, invalid) = tokio::task::spawn_blocking(move || {
        (
            read.read_batch(EventSeq::ZERO, EventSeq(300)).unwrap(),
            read.read_batch(EventSeq(256), EventSeq(300)).unwrap(),
            read.read_batch(EventSeq::ZERO, EventSeq(u64::MAX)),
        )
    })
    .await
    .unwrap();
    assert_eq!(
        first.iter().map(|event| event.seq.0).collect::<Vec<_>>(),
        (1..=256).collect::<Vec<_>>()
    );
    assert_eq!(
        second.iter().map(|event| event.seq.0).collect::<Vec<_>>(),
        (257..=300).collect::<Vec<_>>()
    );
    assert!(matches!(invalid, Err(StorageError::InvalidEventSequence)));
    drop(feed);
    drop(storage);
    home.close().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_sql_worker_keeps_exclusion_after_feed_drop_until_join() {
    let home = FixtureHome::new().unwrap();
    let storage = Storage::open(home.path()).await.unwrap();
    let run = RunId::new();
    let writer = storage.create_run(run, home.path(), None).await.unwrap();
    writer.append_event(payload(1)).await.unwrap();
    let mut feed = writer.event_feed();
    let mut gate = feed.install_read_gate_for_test().unwrap();
    let read = feed.read_lease();
    let job = tokio::task::spawn_blocking(move || read.read_batch(EventSeq::ZERO, EventSeq(1)));
    let entered = tokio::time::timeout(Duration::from_secs(5), gate.entered()).await;
    let sealed = writer.seal_for_read_drain().await;
    drop(feed);
    let blocked = matches!(
        storage.open_run_writer(run).await,
        Err(OpenError::WriterAlreadyHeld { .. })
    );
    let pending = !job.is_finished();
    gate.release();
    let rows = job.await;
    // The real worker is joined before any collected assertion can fail.
    assert!(entered.unwrap().is_ok());
    assert_eq!(sealed.unwrap(), EventSeq(1));
    assert!(blocked && pending);
    assert_eq!(
        rows.unwrap()
            .unwrap()
            .iter()
            .map(|event| event.seq.0)
            .collect::<Vec<_>>(),
        [1]
    );
    storage
        .open_run_writer(run)
        .await
        .unwrap()
        .close()
        .await
        .unwrap();
    drop(storage);
    home.close().unwrap();
}

struct ActorClock {
    armed: AtomicBool,
    entered: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    released: Mutex<bool>,
    signal: Condvar,
}
impl Clock for ActorClock {
    fn now_ms(&self) -> i64 {
        if self.armed.swap(false, Ordering::SeqCst) {
            self.entered
                .lock()
                .unwrap()
                .take()
                .unwrap()
                .send(())
                .unwrap();
            let mut released = self.released.lock().unwrap();
            while !*released {
                released = self.signal.wait(released).unwrap();
            }
        }
        1_700_000_000_000
    }
}
struct ReleaseClock(Arc<ActorClock>);
impl Drop for ReleaseClock {
    fn drop(&mut self) {
        *self.0.released.lock().unwrap() = true;
        self.0.signal.notify_all();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn serialized_seal_includes_inflight_append_and_rejects_queued_follower() {
    let home = FixtureHome::new().unwrap();
    let (entered, entry) = oneshot::channel();
    let clock = Arc::new(ActorClock {
        armed: AtomicBool::new(false),
        entered: Mutex::new(Some(entered)),
        released: Mutex::new(false),
        signal: Condvar::new(),
    });
    let release = ReleaseClock(clock.clone());
    let storage = Storage::open_with(home.path(), clock.clone())
        .await
        .unwrap();
    let writer = storage
        .create_run(RunId::new(), home.path(), None)
        .await
        .unwrap();
    let feed = writer.event_feed();
    let recorder = writer.event_recorder();
    clock.armed.store(true, Ordering::SeqCst);
    let first_recorder = recorder.clone();
    let first = tokio::spawn(async move { first_recorder.append_event(payload(1)).await });
    let reached = tokio::time::timeout(Duration::from_secs(5), entry).await;
    let mut seal = Box::pin(writer.seal_for_read_drain());
    let seal_pending = futures::poll!(&mut seal).is_pending();
    let mut follower = Box::pin(recorder.append_event(payload(2)));
    let follower_pending = futures::poll!(&mut follower).is_pending();
    drop(release);
    let first = first.await;
    let prefix = seal.await;
    let follower = follower.await;
    assert!(reached.unwrap().is_ok() && seal_pending && follower_pending);
    assert_eq!(first.unwrap().unwrap(), EventSeq(1));
    assert_eq!(prefix.unwrap(), EventSeq(1));
    assert!(matches!(follower, Err(StorageError::WriterTaskDied)));
    let read = feed.read_lease();
    let rows = tokio::task::spawn_blocking(move || read.read_batch(EventSeq::ZERO, EventSeq(2)))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        rows.iter().map(|event| event.seq.0).collect::<Vec<_>>(),
        [1]
    );
    drop(recorder);
    drop(feed);
    drop(storage);
    home.close().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actor_loss_is_closed_running_and_seal_still_joins_failure() {
    let home = FixtureHome::new().unwrap();
    let storage = Storage::open(home.path()).await.unwrap();
    let writer = storage
        .create_run(RunId::new(), home.path(), None)
        .await
        .unwrap();
    writer.flush().await.unwrap();
    let mut feed = writer.event_feed();
    writer.writer_join.as_ref().unwrap().abort();
    let closed = writer.seal_for_read_drain().await;
    assert!(
        matches!(closed,Err(CloseError::ReceiptAndActor{actor,..}) if matches!(*actor,CloseError::JoinFailed(_)))
    );
    assert_eq!(feed.changed().await, Err(FeedLifecycleError::WriterLost));
    assert_eq!(feed.current_lifecycle(), WriterLifecycle::Running);
    drop(feed);
    drop(storage);
    home.close().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn seal_query_failure_is_failed_lifecycle_and_joined_actor_error() {
    let home = FixtureHome::new().unwrap();
    let storage = Storage::open(home.path()).await.unwrap();
    let run = RunId::new();
    let writer = storage.create_run(run, home.path(), None).await.unwrap();
    writer.flush().await.unwrap();
    let feed = writer.event_feed();
    // Real on-disk schema damage makes the actor's own MAX(seq) query fail.
    let connection = rusqlite::Connection::open(storage.events_db_path(&run)).unwrap();
    connection.execute_batch("DROP TABLE events").unwrap();
    connection.close().unwrap();
    let closed = writer.seal_for_read_drain().await;
    assert!(
        matches!(closed,Err(CloseError::ReceiptAndActor{actor,..}) if matches!(*actor,CloseError::Writer(WriterError::Sqlite(_))))
    );
    assert_eq!(
        feed.current_lifecycle(),
        WriterLifecycle::Failed(WriterFailure::ActorFailed)
    );
    drop(feed);
    drop(storage);
    home.close().unwrap();
}
