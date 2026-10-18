//! Actual producer processes stop at durable boundaries; cold consumers use their real inventory.
use super::Storage;
use crate::{
    runs::{
        error::{OpenError, StorageError},
        host_settlement::control,
    },
    runtime_home_fixture::FixtureHome,
};
use std::{
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
use surge_core::RunId;

struct OwnedChild(Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
impl OwnedChild {
    fn stop(&mut self) {
        if self.0.try_wait().unwrap().is_none() {
            self.0.kill().unwrap();
        }
        self.0.wait().unwrap();
    }
}
fn wait_closed(storage: &Storage, run: RunId) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match control::acquire(storage.home(), &storage.registry_pool, run, true) {
            Ok(owner) => {
                drop(owner);
                return;
            },
            Err(OpenError::RunInUse { .. }) if Instant::now() < deadline => {
                std::thread::yield_now();
            },
            Err(error) => panic!("actual creation owners did not close: {error:?}"),
        }
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deletion_worker_process() {
    let Some(home) = std::env::var_os("SURGE_TEST_DELETE_HOME") else {
        return;
    };
    let run: RunId = std::env::var("SURGE_TEST_DELETE_RUN")
        .unwrap()
        .parse()
        .unwrap();
    let storage = Storage::open(&home).await.unwrap();
    if std::env::var("SURGE_TEST_DELETE_PHASE").ok().as_deref() == Some("reader-held") {
        let reader = storage.open_run_reader(run).await.unwrap();
        let connection = reader.pool.get().unwrap();
        let _: i64 = connection
            .query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))
            .unwrap();
        let marker =
            std::path::PathBuf::from(std::env::var_os("SURGE_TEST_DELETE_MARKER").unwrap());
        let staging = marker.with_extension("publishing");
        std::fs::write(&staging, b"reader-held").unwrap();
        std::fs::rename(staging, marker).unwrap();
        std::io::Read::read_exact(&mut std::io::stdin(), &mut [0_u8]).unwrap();
        drop(connection);
        drop(reader);
        return;
    }
    let writer = storage.create_run(run, &home, None).await.unwrap();
    std::fs::write(
        storage.artifacts_dir(&run).join("source"),
        b"fixed original artifact",
    )
    .unwrap();
    writer.close().await.unwrap();
    wait_closed(&storage, run);
    storage.delete_run(&run).await.unwrap();
}
fn paused(home: &Path, run: RunId, phase: &str) -> (OwnedChild, bool) {
    let marker = home.join("deletion-boundary");
    let child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "runs::storage::deletion_process_tests::deletion_worker_process",
            "--nocapture",
        ])
        .env("SURGE_TEST_DELETE_HOME", home)
        .env("SURGE_TEST_DELETE_RUN", run.to_string())
        .env("SURGE_TEST_DELETE_PHASE", phase)
        .env("SURGE_TEST_DELETE_MARKER", &marker)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let mut child = OwnedChild(child);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match std::fs::read(&marker) {
            Ok(bytes) => return (child, bytes == phase.as_bytes()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
            Err(error) => panic!("cannot observe complete deletion marker: {error}"),
        }
        if child.0.try_wait().unwrap().is_some() || Instant::now() >= deadline {
            let complete = std::fs::read(&marker).is_ok_and(|bytes| bytes == phase.as_bytes());
            return (child, complete);
        }
        std::thread::yield_now();
    }
}
async fn cold_boundary(phase: &str) -> (bool, bool, bool, bool) {
    let home = FixtureHome::new().unwrap();
    let run = RunId::new();
    let (mut child, observed) = paused(home.path(), run, phase);
    child.stop();
    drop(child);
    let facts = {
        let storage = Storage::open(home.path()).await.unwrap();
        let pending:bool=storage.acquire_registry_conn().unwrap().query_row(
            "SELECT EXISTS(SELECT 1 FROM run_host_tombstones WHERE run_id=? AND state='DeletePending')",[run.to_string()],|r|r.get(0)).unwrap();
        let expected_absence = (phase == "root-removed") != storage.run_dir(&run).exists();
        let first = storage.delete_run(&run).await;
        let second = storage.delete_run(&run).await;
        let deleted:bool=storage.acquire_registry_conn().unwrap().query_row(
            "SELECT EXISTS(SELECT 1 FROM run_host_tombstones WHERE run_id=? AND state='Deleted')",[run.to_string()],|r|r.get(0)).unwrap();
        (
            pending && expected_absence,
            first.is_ok() && second.is_ok(),
            deleted,
        )
    };
    home.close().unwrap();
    (observed, facts.0, facts.1, facts.2)
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_producer_cold_delete_after_intent_publication() {
    assert_eq!(cold_boundary("published").await, (true, true, true, true));
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_producer_cold_delete_after_original_journal_lock_removal() {
    assert_eq!(
        cold_boundary("lock-removed").await,
        (true, true, true, true)
    );
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_producer_cold_delete_after_original_root_removal() {
    assert_eq!(
        cold_boundary("root-removed").await,
        (true, true, true, true)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_producer_inventory_preserves_replacement_then_accepts_original_cold_retry() {
    let home = FixtureHome::new().unwrap();
    let run = RunId::new();
    let (mut child, observed) = paused(home.path(), run, "published");
    child.stop();
    drop(child);
    let root = home.path().join("runs").join(run.to_string());
    let held = home.path().join("held-original");
    std::fs::rename(&root, &held).unwrap();
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("foreign"), b"preserved foreign root").unwrap();
    let original = std::fs::read(held.join("events.sqlite")).unwrap();
    let refusal = {
        let storage = Storage::open(home.path()).await.unwrap();
        matches!(storage.delete_run(&run).await,Err(StorageError::DeletionOwnership{run_id,..}) if run_id==run)
    };
    let preserved = std::fs::read(root.join("foreign")).unwrap() == b"preserved foreign root"
        && std::fs::read(held.join("events.sqlite")).unwrap() == original
        && std::fs::read(held.join("artifacts/source")).unwrap() == b"fixed original artifact";
    std::fs::remove_file(root.join("foreign")).unwrap();
    std::fs::remove_dir(&root).unwrap();
    std::fs::rename(held, &root).unwrap();
    let resumed = {
        let storage = Storage::open(home.path()).await.unwrap();
        storage.delete_run(&run).await.is_ok()
    };
    home.close().unwrap();
    assert_eq!(
        (observed, refusal, preserved, resumed),
        (true, true, true, true)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_independent_reader_process_excludes_delete_until_checked_out_connection_exits() {
    let home = FixtureHome::new().unwrap();
    let run = RunId::new();
    let facts = {
        let storage = Storage::open(home.path()).await.unwrap();
        storage
            .create_run(run, home.path(), None)
            .await
            .unwrap()
            .close()
            .await
            .unwrap();
        wait_closed(&storage, run);
        let (mut child, observed) = paused(home.path(), run, "reader-held");
        let refusal = matches!(storage.delete_run(&run).await,
            Err(StorageError::RunInUse { run_id }) if run_id == run);
        child.stop();
        drop(child);
        wait_closed(&storage, run);
        let deleted = storage.delete_run(&run).await.is_ok();
        (observed, refusal, deleted)
    };
    home.close().unwrap();
    assert_eq!(facts, (true, true, true));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_producer_cold_delete_after_original_content_removal() {
    assert_eq!(
        cold_boundary("contents-removed").await,
        (true, true, true, true)
    );
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_producer_cold_delete_after_original_artifacts_removal() {
    assert_eq!(
        cold_boundary("artifacts-removed").await,
        (true, true, true, true)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn simultaneous_cold_retries_share_original_deleted_root_intent_without_new_identity() {
    let home = FixtureHome::new().unwrap();
    let run = RunId::new();
    let (mut child, observed) = paused(home.path(), run, "root-removed");
    child.stop();
    drop(child);
    let facts = {
        let first = Storage::open(home.path()).await.unwrap();
        let second = Storage::open(home.path()).await.unwrap();
        let before: String = first
            .acquire_registry_conn()
            .unwrap()
            .query_row(
                "SELECT deletion_id FROM run_host_tombstones WHERE run_id=?",
                [run.to_string()],
                |r| r.get(0),
            )
            .unwrap();
        let (left, right) = tokio::join!(first.delete_run(&run), second.delete_run(&run));
        let acceptable = |result: &Result<(), StorageError>| {
            matches!(result, Ok(()))
                || matches!(result, Err(StorageError::RunInUse { run_id }) if *run_id == run)
        };
        let final_retry = first.delete_run(&run).await;
        let after: (String, String) = first
            .acquire_registry_conn()
            .unwrap()
            .query_row(
                "SELECT deletion_id,state FROM run_host_tombstones WHERE run_id=?",
                [run.to_string()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        observed
            && acceptable(&left)
            && acceptable(&right)
            && (left.is_ok() || right.is_ok())
            && final_retry.is_ok()
            && after == (before, "Deleted".into())
            && !first.run_dir(&run).exists()
    };
    home.close().unwrap();
    assert!(facts);
}
