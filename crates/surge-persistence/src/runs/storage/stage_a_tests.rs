//! Existing-API regressions for the durable opening boundary, before its implementation.

use super::Storage;
use crate::runs::OpenError;
use surge_core::RunId;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn existing_writer_refuses_missing_original_lock_without_recreation() {
    let home = crate::runtime_home_fixture::FixtureHome::new().unwrap();
    let storage = Storage::open(home.path()).await.unwrap();
    let run = RunId::new();
    storage
        .create_run(run, home.path(), None)
        .await
        .unwrap()
        .close()
        .await
        .unwrap();
    let lock = storage.lock_path(&run);
    std::fs::remove_file(&lock).unwrap();
    assert!(!lock.exists(), "fixture must remove the original lock");

    let outcome = storage.open_run_writer(run).await;
    let (refused_missing, observed, close) = match outcome {
        Ok(writer) => (
            false,
            "unexpected writer opened".to_owned(),
            writer.close().await,
        ),
        Err(error) => (
            matches!(&error, OpenError::Io(io) if io.kind() == std::io::ErrorKind::NotFound),
            error.to_string(),
            Ok(()),
        ),
    };
    let recreated = lock.exists();
    // On the RED baseline, settle the unexpected real writer before removing its new lock.
    let remove_recreated = if recreated {
        std::fs::remove_file(&lock)
    } else {
        Ok(())
    };
    drop(storage);
    let cleanup = home.close();
    println!(
        "missing-lock observation: {observed}; recreated={recreated}; writer_close={close:?}; recreated_lock_cleanup={remove_recreated:?}; fixture_cleanup={cleanup:?}"
    );
    close.unwrap();
    remove_recreated.unwrap();
    cleanup.unwrap();
    assert!(
        refused_missing && !recreated,
        "existing-only writer must refuse the missing original lock without recreation: {observed}; recreated={recreated}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn public_reader_pool_refuses_actual_sql_write() {
    let home = crate::runtime_home_fixture::FixtureHome::new().unwrap();
    let storage = Storage::open(home.path()).await.unwrap();
    let run = RunId::new();
    storage
        .create_run(run, home.path(), None)
        .await
        .unwrap()
        .close()
        .await
        .unwrap();
    let reader = storage.open_run_reader(run).await.unwrap();
    let observed = tokio::task::spawn_blocking(move || {
        let connection = reader.pool.get().unwrap();
        let available: i64 = connection.query_row("SELECT 1", [], |row| row.get(0)).unwrap();
        let write = connection.execute(
            "CREATE TABLE stage_a_reader_write_probe (value INTEGER NOT NULL)",
            [],
        );
        let created: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='stage_a_reader_write_probe'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        // Undo only the dedicated fixture table if the baseline allowed its creation.
        let repair = if created == 1 {
            connection.execute("DROP TABLE stage_a_reader_write_probe", []).map(|_| ())
        } else {
            Ok(())
        };
        drop(connection);
        drop(reader);
        (available, write, created, repair)
    })
    .await;
    drop(storage);
    let cleanup = home.close();
    println!("reader-write observation: {observed:?}; fixture_cleanup={cleanup:?}");
    cleanup.unwrap();
    let (available, write, created, repair) = observed.unwrap();
    repair.unwrap();
    assert_eq!(available, 1, "actual pooled SQL read control must succeed");
    assert!(
        matches!(&write, Err(rusqlite::Error::SqliteFailure(error, _)) if error.code == rusqlite::ErrorCode::ReadOnly)
            && created == 0,
        "public reader must reject actual SQL with SQLITE_READONLY and create no table: write={write:?}; created={created}"
    );
}
