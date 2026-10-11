use super::*;
use surge_core::{NodeKey, RunId, VersionedEventPayload, run_event::EventPayload};
use surge_persistence::runs::Storage;

mod runtime_home {
    #[cfg(windows)]
    use surge_persistence::RuntimeHomeOwner;
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../scripts/test-support/runtime_home.rs"
    ));
}

fn event() -> VersionedEventPayload {
    VersionedEventPayload::new(EventPayload::RunCompleted {
        terminal_node: NodeKey::try_new("end").unwrap(),
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sealed_prefix_drains_more_than_one_batch_and_releases_writer() {
    let home = runtime_home::FixtureHome::new().unwrap();
    let storage = Storage::open(home.path()).await.unwrap();
    let run = RunId::new();
    let writer = storage.create_run(run, home.path(), None).await.unwrap();
    writer.append_events(vec![event(); 600]).await.unwrap();
    let (tap, mut received) = broadcast::channel(1024);
    let forwarder = spawn(writer.event_feed(), tap);
    let prefix = writer.seal_for_read_drain().await.unwrap();
    assert_eq!(prefix, EventSeq(600));
    assert_eq!(forwarder.await.unwrap().unwrap(), EventSeq(600));
    let mut actual = Vec::new();
    while let Ok(event) = received.try_recv() {
        assert_eq!(event.run_id, run);
        actual.push(event.event.seq.0);
    }
    assert_eq!(actual, (1..=600).collect::<Vec<_>>());
    storage
        .open_run_writer(run)
        .await
        .unwrap()
        .close()
        .await
        .unwrap();
    drop(received);
    drop(storage);
    home.close().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sealed_empty_journal_finishes_without_an_event() {
    let home = runtime_home::FixtureHome::new().unwrap();
    let storage = Storage::open(home.path()).await.unwrap();
    let writer = storage
        .create_run(RunId::new(), home.path(), None)
        .await
        .unwrap();
    let (tap, mut received) = broadcast::channel(1);
    let forwarder = spawn(writer.event_feed(), tap);
    assert_eq!(writer.seal_for_read_drain().await.unwrap(), EventSeq::ZERO);
    assert_eq!(forwarder.await.unwrap().unwrap(), EventSeq::ZERO);
    assert!(received.try_recv().is_err());
    drop(received);
    drop(storage);
    home.close().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gapped_real_journal_reports_sequence_failure_and_settles() {
    let home = runtime_home::FixtureHome::new().unwrap();
    let storage = Storage::open(home.path()).await.unwrap();
    let run = RunId::new();
    let writer = storage.create_run(run, home.path(), None).await.unwrap();
    writer.append_events(vec![event(); 3]).await.unwrap();
    // Independent corruption oracle: actual SQLite rows, not a mocked batch.
    let connection = rusqlite::Connection::open(
        home.path()
            .join("runs")
            .join(run.to_string())
            .join("events.sqlite"),
    )
    .unwrap();
    // Deliberately bypass the append-only fixture trigger before corrupting rows.
    connection
        .execute("DROP TRIGGER trg_events_no_delete", [])
        .unwrap();
    connection
        .execute("DELETE FROM events WHERE seq = 2", [])
        .unwrap();
    drop(connection);
    let (tap, received) = broadcast::channel(8);
    let forwarder = spawn(writer.event_feed(), tap);
    writer.seal_for_read_drain().await.unwrap();
    assert_eq!(
        forwarder.await.unwrap(),
        Err(RunSettlementFailure::ForwarderSequence)
    );
    storage
        .open_run_writer(run)
        .await
        .unwrap()
        .close()
        .await
        .unwrap();
    drop(received);
    drop(storage);
    home.close().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_blocking_read_keeps_join_pending_and_original_writer_lease() {
    use std::task::Poll;
    let home = runtime_home::FixtureHome::new().unwrap();
    let storage = Storage::open(home.path()).await.unwrap();
    let run = RunId::new();
    let writer = storage.create_run(run, home.path(), None).await.unwrap();
    writer.append_event(event()).await.unwrap();
    let mut feed = writer.event_feed();
    let mut gate = feed.install_read_gate_for_test().unwrap();
    let (tap, received) = broadcast::channel(8);
    let mut forwarder = spawn(feed, tap);
    gate.entered().await.unwrap();
    let prefix = writer.seal_for_read_drain().await.unwrap();
    let before_release = futures::poll!(&mut forwarder);
    let busy = match storage.open_run_writer(run).await {
        Err(surge_persistence::runs::OpenError::WriterAlreadyHeld { .. }) => true,
        Ok(unexpected) => {
            unexpected.close().await.unwrap();
            false
        },
        Err(_) => false,
    };
    // Release and join before assertions, including an unexpected early completion.
    gate.release();
    let (was_pending, result) = match before_release {
        Poll::Pending => (true, forwarder.await),
        Poll::Ready(result) => (false, result),
    };
    let result = result.unwrap();
    let reopened = storage.open_run_writer(run).await.unwrap();
    reopened.close().await.unwrap();
    drop(received);
    drop(storage);
    home.close().unwrap();
    assert!(
        was_pending,
        "actual blocked SQL must prevent joined completion"
    );
    assert!(
        busy,
        "same original writer lease must exclude replacement during drain"
    );
    assert_eq!(result.unwrap(), prefix);
}

#[test]
fn aborting_forwarder_keeps_actual_sql_lease_until_worker_exits() {
    let home = runtime_home::FixtureHome::new().unwrap();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let (cancelled, busy) = runtime.block_on(abort_during_read(home.path()));
    // Await the actual blocking worker even though its async observer was aborted.
    // This is containment, not a successful completion claim for the aborted run.
    drop(runtime);
    home.close().unwrap();
    assert!(
        cancelled,
        "aborted join must remain UNKNOWN, never normal completion"
    );
    assert!(
        busy,
        "cancelled async observer cannot release the real SQL worker's lease"
    );
}

async fn abort_during_read(home: &std::path::Path) -> (bool, bool) {
    let storage = Storage::open(home).await.unwrap();
    let run = RunId::new();
    let writer = storage.create_run(run, home, None).await.unwrap();
    writer.append_event(event()).await.unwrap();
    let mut feed = writer.event_feed();
    let mut gate = feed.install_read_gate_for_test().unwrap();
    let (tap, received) = broadcast::channel(8);
    let forwarder = spawn(feed, tap);
    gate.entered().await.unwrap();
    writer.seal_for_read_drain().await.unwrap();
    forwarder.abort();
    let cancelled = forwarder.await.unwrap_err().is_cancelled();
    let busy = match storage.open_run_writer(run).await {
        Err(surge_persistence::runs::OpenError::WriterAlreadyHeld { .. }) => true,
        Ok(unexpected) => {
            unexpected.close().await.unwrap();
            false
        },
        Err(_) => false,
    };
    // Drop/release is unconditional before the containing runtime joins workers.
    gate.release();
    drop(received);
    drop(storage);
    (cancelled, busy)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn empty_running_read_before_append_and_seal_requeries_final_prefix() {
    let home = runtime_home::FixtureHome::new().unwrap();
    let storage = Storage::open(home.path()).await.unwrap();
    let writer = storage
        .create_run(RunId::new(), home.path(), None)
        .await
        .unwrap();
    let mut feed = writer.event_feed();
    let mut gate = feed.install_post_read_gate_for_test().unwrap();
    let (tap, mut received) = broadcast::channel(8);
    let forwarder = spawn(feed, tap);
    // Actual SQL has finished reading an empty journal while lifecycle was Running.
    // Its captured result is held before the forwarder can recheck actor state.
    gate.entered().await.unwrap();
    let appended = writer.append_event(event()).await;
    let sealed = writer.seal_for_read_drain().await;
    gate.release();
    let result = forwarder.await;
    let delivered = received.try_recv();
    drop(received);
    drop(storage);
    let cleaned = home.close();
    // All real workers have joined before any oracle assertion can unwind.
    assert_eq!(appended.unwrap(), EventSeq(1));
    assert_eq!(sealed.unwrap(), EventSeq(1));
    assert_eq!(
        result.unwrap(),
        Ok(EventSeq(1)),
        "a pre-seal empty SQL snapshot cannot prove a gap in the later sealed prefix"
    );
    assert_eq!(delivered.unwrap().event.seq, EventSeq(1));
    cleaned.unwrap();
}
