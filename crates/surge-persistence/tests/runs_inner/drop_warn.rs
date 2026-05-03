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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drop_without_close_emits_tracing_warning() {
    let buf = Arc::new(Mutex::new(Vec::<u8>::new()));
    let writer_make = CaptureWriter { buf: buf.clone() };

    let subscriber = tracing_subscriber::fmt()
        .with_writer(writer_make)
        .with_max_level(tracing::Level::WARN)
        .with_ansi(false)
        .finish();

    // Install the subscriber for THIS thread only — the .await points may
    // hop threads, but the warn! we care about fires synchronously inside
    // Drop on whichever thread the writer happens to be dropped on.
    // Using set_default returns a guard that lives for the rest of the
    // function; we stay inside the with_default closure for the awaits to
    // make sure both the create_run and drop occur under the subscriber.
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
