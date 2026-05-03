//! 12.5 — single-writer enforcement (in-process and cross-process).

use crate::runs::fixtures::setup;
use surge_persistence::runs::OpenError;

/// In-process: opening a second writer for the same run while the first is
/// alive must fail with `WriterAlreadyHeld`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn second_in_process_writer_fails_with_writer_already_held() {
    let t = setup().await;
    let writer1 = t
        .storage
        .create_run(t.run_id, "/tmp/proj", None)
        .await
        .expect("create_run");

    let result = t.storage.open_run_writer(t.run_id).await;
    match result {
        Ok(_) => panic!("second writer must fail while first is alive"),
        Err(OpenError::WriterAlreadyHeld { ref run_id }) => {
            assert_eq!(run_id, &t.run_id);
        },
        Err(other) => panic!("expected WriterAlreadyHeld, got {other:?}"),
    }

    writer1.close().await.expect("close");

    // After the first writer closes, a new one can be opened.
    let writer2 = t
        .storage
        .open_run_writer(t.run_id)
        .await
        .expect("re-open after close");
    writer2.close().await.expect("close 2");
}

/// Cross-process exclusion via fd-lock. Stubbed under #[ignore] on Windows
/// because spawning a child process that takes the same fd-lock is fragile
/// in MSVC test environments (cargo test owns the process tree).
/// Tracked under [P3.X7] — Windows cross-process test reliability strategy.
#[cfg(not(target_os = "windows"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "P3.X7 cross-process — needs child harness, deferred"]
async fn cross_process_writer_lock_blocks_second_process() {
    // Stub: we'd spawn the test binary with a marker arg that opens the same
    // run dir's lock, then assert the second process fails. Defer until P3.X7.
}

#[cfg(target_os = "windows")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "P3.X7 cross-process — Windows test harness not yet built"]
async fn cross_process_writer_lock_blocks_second_process() {
    // Same stub on Windows; gated independently so the deferral note is OS-
    // specific and the future Unix implementation can land without touching
    // this branch.
}
