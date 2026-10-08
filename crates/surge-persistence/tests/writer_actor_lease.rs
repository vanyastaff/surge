//! A journal actor retains real write exclusion after its caller disappears.
mod runtime_home_fixture {
    #[cfg(windows)]
    use surge_persistence::RuntimeHomeOwner;
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../scripts/test-support/runtime_home.rs"
    ));
}

use std::sync::{
    Arc, Condvar, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::Duration;
use surge_core::{EventPayload, RunId, SessionId, VersionedEventPayload};
use surge_persistence::runs::{Clock, OpenError, Storage};

struct ActorClock {
    armed: AtomicBool,
    entered: mpsc::Sender<()>,
    released: Mutex<bool>,
    signal: Condvar,
}
impl Clock for ActorClock {
    fn now_ms(&self) -> i64 {
        if self.armed.swap(false, Ordering::SeqCst) {
            self.entered.send(()).unwrap();
            let mut released = self.released.lock().unwrap();
            while !*released {
                released = self.signal.wait(released).unwrap();
            }
        }
        1_700_000_000_000
    }
}
impl ActorClock {
    fn release(&self) {
        *self.released.lock().unwrap() = true;
        self.signal.notify_all();
    }
}
struct ReleaseClock(Arc<ActorClock>);
impl Drop for ReleaseClock {
    fn drop(&mut self) {
        self.0.release();
    }
}
fn payload(index: u32) -> VersionedEventPayload {
    VersionedEventPayload::new(EventPayload::TokensConsumed {
        session: SessionId::new(),
        prompt_tokens: index,
        output_tokens: 1,
        cache_hits: 0,
        model: "actor-lease-oracle".into(),
        cost_usd: None,
    })
}

async fn actor_exclusion_after_caller_loss(cancel_close: bool, full_queue: bool) {
    let home = runtime_home_fixture::FixtureHome::new().unwrap();
    std::fs::write(
        home.path().join("config.toml"),
        "[storage]\nwriter_channel_capacity=2\n",
    )
    .unwrap();
    let (entered, entry) = mpsc::channel();
    let clock = Arc::new(ActorClock {
        armed: AtomicBool::new(false),
        entered,
        released: Mutex::new(false),
        signal: Condvar::new(),
    });
    let _release_on_failure = ReleaseClock(clock.clone());
    let storage = Storage::open_with(home.path(), clock.clone())
        .await
        .unwrap();
    let run = RunId::new();
    let writer = storage.create_run(run, home.path(), None).await.unwrap();
    let recorder = writer.event_recorder();
    clock.armed.store(true, Ordering::SeqCst);
    let first_recorder = recorder.clone();
    let first = tokio::spawn(async move { first_recorder.append_event(payload(11)).await });
    tokio::task::spawn_blocking(move || entry.recv_timeout(Duration::from_secs(5)))
        .await
        .unwrap()
        .unwrap();
    // This is the real writer actor inside its production AppendEvent clock call.
    let mut queued = Vec::new();
    if full_queue {
        for index in [21, 22] {
            let mut append = Box::pin(recorder.append_event(payload(index)));
            assert!(futures::poll!(&mut append).is_pending());
            queued.push(append);
        }
    }
    if cancel_close {
        let mut close = Box::pin(writer.close());
        // Poll the actual close: either Shutdown is queued or its send is blocked
        // by the actual full writer queue, while the old actor remains running.
        assert!(futures::poll!(&mut close).is_pending());
        drop(close);
    } else {
        drop(writer);
    }
    let attempted = storage.open_run_writer(run).await;
    let blocked =
        matches!(&attempted, Err(OpenError::WriterAlreadyHeld { run_id }) if *run_id == run);
    clock.release();
    let first_seq = first.await.unwrap().unwrap();
    for append in queued {
        append.await.unwrap();
    }
    drop(recorder);
    // On the baseline, the new writer already acquired BOTH exclusions while
    // the old actor was blocked; observe that old actor's real committed SQL.
    if let Ok(second) = attempted {
        let row = second.read_event(first_seq).await.unwrap().unwrap();
        assert!(matches!(
            row.payload.payload,
            EventPayload::TokensConsumed {
                prompt_tokens: 11,
                ..
            }
        ));
        second.close().await.unwrap();
    }
    let reopened = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match storage.open_run_writer(run).await {
                Ok(writer) => return writer,
                Err(OpenError::WriterAlreadyHeld { .. }) => {
                    tokio::time::sleep(Duration::from_millis(10)).await
                },
                Err(error) => panic!("unexpected reopen failure: {error}"),
            }
        }
    })
    .await
    .unwrap();
    assert!(reopened.read_event(first_seq).await.unwrap().is_some());
    reopened.close().await.unwrap();
    assert!(
        blocked,
        "caller loss released journal exclusion before actual actor exit"
    );
    drop(storage);
    home.close().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropped_writer_retains_exclusion_until_actual_actor_exit() {
    actor_exclusion_after_caller_loss(false, false).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_close_retains_exclusion_until_actual_actor_exit() {
    actor_exclusion_after_caller_loss(true, false).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_full_queue_close_retains_exclusion_until_actual_actor_exit() {
    actor_exclusion_after_caller_loss(true, true).await;
}
