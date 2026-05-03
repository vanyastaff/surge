//! 12.13 [P1.X2] — drop without close, verify tracing::warn emitted.

use std::io;
use std::sync::{Arc, Mutex};

use crate::runs::fixtures::setup;
use tracing_subscriber::fmt::MakeWriter;

/// MakeWriter that funnels every log line into a shared Mutex<Vec<u8>>.
///
/// Used to capture tracing output during the test so we can assert on its
/// contents without depending on test-runner stderr layout.
#[derive(Clone)]
struct CaptureWriter {
    buf: Arc<Mutex<Vec<u8>>>,
}

impl io::Write for CaptureWriter {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        let mut g = self.buf.lock().expect("lock");
        g.extend_from_slice(data);
        Ok(data.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for CaptureWriter {
    type Writer = CaptureWriter;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

// `Storage::open` rejects single-threaded tokio runtimes, so we must use
// `multi_thread`. But `tracing::subscriber::set_default` is per-thread,
// and `multi_thread`'s scheduler can hop the test future across worker
// threads at .await points — when `drop(writer)` runs, we may be on a
// thread without the subscriber installed, and `tracing::warn!` from the
// Drop impl silently falls through to the global default. Linux tokio
// hops more aggressively than macOS, which is why the original test
// passed on macOS but flaked on Linux.
//
// Fix: pin the test body to the calling thread via `LocalSet::run_until`.
// Storage internals (its own writer task) still spawn onto worker threads
// freely, but the Drop of `writer` we care about happens synchronously
// on whichever thread is executing the test future — and inside
// `run_until` that's the test thread, where `set_default` was applied.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drop_without_close_emits_tracing_warning() {
    let buf = Arc::new(Mutex::new(Vec::<u8>::new()));
    let writer_make = CaptureWriter { buf: buf.clone() };

    let subscriber = tracing_subscriber::fmt()
        .with_writer(writer_make)
        .with_max_level(tracing::Level::WARN)
        .with_ansi(false)
        .finish();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let _guard = tracing::subscriber::set_default(subscriber);

            let t = setup().await;
            let writer = t
                .storage
                .create_run(t.run_id, "/tmp/proj", None)
                .await
                .expect("create_run");

            // Intentionally drop without close() — the Drop impl emits warn.
            drop(writer);

            // Wait briefly to give any background task time to emit teardown
            // traces, then capture and assert.
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        })
        .await;

    let captured = buf.lock().expect("lock");
    let log = String::from_utf8_lossy(&captured);
    assert!(
        log.contains("dropped without close"),
        "expected drop warning in captured tracing output. Got: {log}"
    );
    assert!(
        log.contains("WARN"),
        "expected WARN level in tracing output. Got: {log}"
    );
}
