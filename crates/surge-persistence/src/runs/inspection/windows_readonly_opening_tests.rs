//! Native Windows regression: read admission cannot repair missing artifacts.
//! These are prepared on macOS; only native Windows execution supplies RED/GREEN.
use super::*;
use crate::runs::{OpenError, StorageError, host_settlement::control};
use std::{sync::Arc, time::Duration};

async fn settle_run(storage: &Storage, run: RunId) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            match control::acquire(storage.home(), &storage.registry_pool, run, true) {
                Ok(owner) => {
                    drop(owner);
                    break;
                },
                Err(OpenError::RunInUse { .. }) => tokio::task::yield_now().await,
                Err(error) => panic!("actual run owner did not settle: {error:?}"),
            }
        }
    })
    .await
    .unwrap();
}
async fn settle_storage(storage: Arc<Storage>) {
    let witness = storage.registry_pool_witness().unwrap();
    drop(storage);
    tokio::time::timeout(Duration::from_secs(10), async {
        while witness.is_retained() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}
async fn actual_run(home: &Path, run: RunId) -> Arc<Storage> {
    let storage = Storage::open(home).await.unwrap();
    storage
        .create_run(run, "/fixed-readonly", None)
        .await
        .unwrap()
        .close()
        .await
        .unwrap();
    settle_run(&storage, run).await;
    storage
}
#[derive(Clone, Copy, Debug)]
enum Inspection {
    Static,
    Current,
    Folded,
}
async fn inspect(
    home: &Path,
    run: RunId,
    method: Inspection,
    storage: Option<&Arc<Storage>>,
) -> Result<(), StorageError> {
    match method {
        Inspection::Static => Storage::inspect_existing_run(home.into(), run)
            .await
            .map(|_| ()),
        Inspection::Current => storage.unwrap().inspect_run(run).await.map(|_| ()),
        Inspection::Folded => storage.unwrap().inspect_folded_run(run).await.map(|_| ()),
    }
}
fn permanent_native_refusal(result: &Result<(), StorageError>) -> bool {
    matches!(
        result,
        Err(StorageError::RunOpening(OpenError::StateHome { .. }))
    )
}
async fn missing_artifacts_matrix(latest: &str) {
    let mut observations = Vec::new();
    for method in [Inspection::Static, Inspection::Current, Inspection::Folded] {
        let home = crate::runtime_home_fixture::FixtureHome::new().unwrap();
        let run = RunId::new();
        let storage = actual_run(home.path(), run).await;
        {
            let connection = storage.acquire_registry_conn().unwrap();
            match latest {
                "bound" => {},
                "opening-null" => {
                    connection.execute("UPDATE run_host_openings SET state='Opening',database_identity=NULL WHERE run_id=?", [run.to_string()]).unwrap();
                },
                "malformed" => {
                    connection.execute("UPDATE run_host_openings SET state='Failed',database_identity=NULL WHERE run_id=?", [run.to_string()]).unwrap();
                },
                _ => unreachable!(),
            }
        }
        let expected = registry_rows(&storage.acquire_registry_conn().unwrap());
        settle_storage(storage).await;
        // Actual run/manager/registry close precedes this fixture mutation.
        let artifacts = home
            .path()
            .join("runs")
            .join(run.to_string())
            .join("artifacts");
        std::fs::remove_dir(&artifacts).unwrap();
        let current = if matches!(method, Inspection::Static) {
            None
        } else {
            Some(Storage::open(home.path()).await.unwrap())
        };
        let before = files(home.path());
        let result = inspect(home.path(), run, method, current.as_ref()).await;
        let after = files(home.path()); // BEFORE any verifying SQL.
        let still_absent = !artifacts.exists();
        let unchanged = snapshot::validate_readonly(&before, &after);
        let storage = match current {
            Some(storage) => storage,
            None => Storage::open(home.path()).await.unwrap(),
        };
        let actual = registry_rows(&storage.acquire_registry_conn().unwrap());
        settle_storage(storage).await;
        home.close().unwrap();
        observations.push((
            method,
            permanent_native_refusal(&result),
            still_absent,
            unchanged,
            expected == actual,
            format!("{result:?}"),
        ));
    }
    // Collect every public path and settle every owner before reporting a native RED.
    assert!(
        observations
            .iter()
            .all(|(_, typed, absent, readonly, rows, _)| *typed
                && *absent
                && readonly.is_ok()
                && *rows),
        "{latest}: {observations:#?}"
    );
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn windows_missing_artifacts_bound_readonly_admission() {
    missing_artifacts_matrix("bound").await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn windows_missing_artifacts_opening_null_readonly_admission() {
    missing_artifacts_matrix("opening-null").await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn windows_missing_artifacts_malformed_latest_readonly_admission() {
    missing_artifacts_matrix("malformed").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn windows_complete_route_writer_and_64_readers_still_work() {
    let home = crate::runtime_home_fixture::FixtureHome::new().unwrap();
    let run = RunId::new();
    let storage = Storage::open(home.path()).await.unwrap();
    let writer = storage
        .create_run(run, "/fixed-positive", None)
        .await
        .unwrap();
    let mut readers = Vec::new();
    let mut counts = Vec::new();
    for _ in 0..64 {
        let reader = storage.open_run_reader(run).await.unwrap();
        let count: i64 = reader
            .pool
            .get()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))
            .unwrap();
        counts.push(count);
        readers.push(reader);
    }
    let artifacts = storage.artifacts_dir(&run);
    let displaced = storage.run_dir(&run).join("live-displaced-artifacts");
    let before = files(home.path());
    let rename = std::fs::rename(&artifacts, &displaced);
    let after = files(home.path());
    let readonly = snapshot::validate_readonly(&before, &after);
    let reader_count = readers.len();
    drop(readers);
    writer.close().await.unwrap();
    settle_run(&storage, run).await;
    if rename.is_ok() {
        std::fs::rename(&displaced, &artifacts).unwrap();
    }
    settle_storage(storage).await;
    let cold = Storage::inspect_existing_run(home.path().into(), run).await;
    let storage = Storage::open(home.path()).await.unwrap();
    let current = storage.inspect_run(run).await;
    let folded = storage.inspect_folded_run(run).await;
    settle_storage(storage).await;
    home.close().unwrap();
    assert_eq!(reader_count, 64);
    assert_eq!(counts, vec![0; 64]);
    assert!(
        matches!(
            rename.as_ref().err().and_then(std::io::Error::raw_os_error),
            Some(5 | 32)
        ),
        "live native retention must deny replacement: {rename:?}"
    );
    assert!(readonly.is_ok(), "live protection: {readonly:?}");
    let cold = cold.unwrap();
    let current = current.unwrap();
    assert_eq!(cold.registry.unwrap().id, run);
    assert_eq!(current.registry.unwrap().id, run);
    assert!(
        matches!(cold.database, RunDatabaseInspection::Present { events } if events.is_empty())
    );
    assert!(
        matches!(current.database, RunDatabaseInspection::Present { events } if events.is_empty())
    );
    assert_eq!(folded.unwrap().database.unwrap().event_count, 0);
}

async fn foreign_identity_case(replace_run: bool) {
    let home = crate::runtime_home_fixture::FixtureHome::new().unwrap();
    let run = RunId::new();
    let foreign = RunId::new();
    let storage = actual_run(home.path(), run).await;
    storage
        .create_run(foreign, "/fixed-foreign", None)
        .await
        .unwrap()
        .close()
        .await
        .unwrap();
    settle_run(&storage, foreign).await;
    let expected = registry_rows(&storage.acquire_registry_conn().unwrap());
    settle_storage(storage).await;
    let original = home.path().join("runs").join(run.to_string());
    let foreign_root = home.path().join("runs").join(foreign.to_string());
    let displaced = if replace_run {
        let displaced = home.path().join("preserved-original-run");
        std::fs::rename(&original, &displaced).unwrap();
        std::fs::rename(&foreign_root, &original).unwrap();
        displaced
    } else {
        let displaced = original.join("preserved-original-database");
        std::fs::rename(original.join("events.sqlite"), &displaced).unwrap();
        std::fs::rename(
            foreign_root.join("events.sqlite"),
            original.join("events.sqlite"),
        )
        .unwrap();
        displaced
    };
    let before = files(home.path());
    let result = Storage::inspect_existing_run(home.path().into(), run).await;
    let after = files(home.path());
    let readonly = snapshot::validate_readonly(&before, &after);
    let both_preserved = displaced.exists() && original.exists();
    let storage = Storage::open(home.path()).await.unwrap();
    let actual = registry_rows(&storage.acquire_registry_conn().unwrap());
    settle_storage(storage).await;
    home.close().unwrap();
    assert!(
        matches!(
            result,
            Err(StorageError::RunOpening(OpenError::Authority(_)))
        ),
        "{result:?}"
    );
    assert!(
        both_preserved && readonly.is_ok(),
        "original+foreign: {readonly:?}"
    );
    assert_eq!(actual, expected);
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn windows_foreign_cold_run_namespace_is_not_adopted_by_inspection() {
    foreign_identity_case(true).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn windows_foreign_cold_database_is_not_adopted_by_inspection() {
    foreign_identity_case(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn windows_reparse_artifacts_are_not_followed_by_inspection() {
    let home = crate::runtime_home_fixture::FixtureHome::new().unwrap();
    let run = RunId::new();
    let storage = actual_run(home.path(), run).await;
    let foreign = RunId::new();
    storage
        .create_run(foreign, "/fixed-reparse-target", None)
        .await
        .unwrap()
        .close()
        .await
        .unwrap();
    settle_run(&storage, foreign).await;
    let target = storage.artifacts_dir(&foreign);
    let expected = registry_rows(&storage.acquire_registry_conn().unwrap());
    settle_storage(storage).await;
    let artifacts = home
        .path()
        .join("runs")
        .join(run.to_string())
        .join("artifacts");
    std::fs::write(target.join("sentinel"), b"independent foreign bytes").unwrap();
    std::fs::remove_dir(&artifacts).unwrap();
    // Native privilege/setup failure is an explicit test failure, never a skip or RED.
    std::os::windows::fs::symlink_dir(&target, &artifacts).unwrap();
    let before = files(&target);
    let result = Storage::inspect_existing_run(home.path().into(), run)
        .await
        .map(|_| ());
    let after = files(&target);
    let link_unchanged = std::fs::read_link(&artifacts).unwrap() == target;
    let storage = Storage::open(home.path()).await.unwrap();
    let actual = registry_rows(&storage.acquire_registry_conn().unwrap());
    settle_storage(storage).await;
    std::fs::remove_dir(&artifacts).unwrap();
    home.close().unwrap();
    assert!(permanent_native_refusal(&result), "{result:?}");
    assert_eq!(before, after, "foreign reparse target changed");
    assert!(link_unchanged);
    assert_eq!(actual, expected);
}
