//! Fixed pre-0036 schema fixtures are structural old-storage evidence, not an old binary.
use super::*;
use crate::runs::{
    clock::MockClock,
    migrations::{PER_RUN_MIGRATIONS, REGISTRY_MIGRATIONS, apply},
};
use crate::runtime_home_fixture::FixtureHome;
use rusqlite::{Connection, params};
use sha2::{Digest, Sha256};
use std::os::unix::{ffi::OsStrExt, fs::MetadataExt};

pub(crate) fn legacy_fixture(home: &Path, run: RunId) {
    let root = home.join("runs").join(run.to_string());
    std::fs::create_dir_all(root.join("artifacts")).unwrap();
    std::fs::create_dir_all(home.join("db")).unwrap();
    let mut registry = Connection::open(home.join("db/registry.sqlite")).unwrap();
    let last = REGISTRY_MIGRATIONS
        .iter()
        .position(|(id, _)| *id == "registry-0035-owned-flow-private-establishment")
        .unwrap();
    apply(
        &mut registry,
        &REGISTRY_MIGRATIONS[..=last],
        &MockClock::new(100),
    )
    .unwrap();
    registry.execute("INSERT INTO runs(id,project_path,status,started_at,ended_at) VALUES(?,'/fixed-legacy-project','completed',1,2)", [run.to_string()]).unwrap();
    let mut journal = Connection::open(root.join("events.sqlite")).unwrap();
    apply(&mut journal, PER_RUN_MIGRATIONS, &MockClock::new(100)).unwrap();
    let event = surge_core::VersionedEventPayload::new(surge_core::EventPayload::StageEntered {
        node: "legacy".parse().unwrap(),
        attempt: 1,
    });
    journal
        .execute(
            "INSERT INTO events(seq,timestamp,kind,payload,schema_version) VALUES(1,1,?,?,?)",
            params![
                event.payload().discriminant_str(),
                serde_json::to_vec(&event).unwrap(),
                event.schema_version()
            ],
        )
        .unwrap();
    drop(journal);
    drop(
        crate::runs::file_lock::FileLock::try_acquire(&root.join("events.sqlite.lock"), run)
            .unwrap(),
    );
    std::fs::write(
        root.join("artifacts/original-marker"),
        b"fixed legacy artifact",
    )
    .unwrap();
}
fn registry_facts(storage: &Storage) -> String {
    let conn = storage.registry_pool.get().unwrap();
    let names: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let mut facts = Vec::new();
    for name in names {
        let mut query = conn
            .prepare(&format!("SELECT * FROM \"{}\"", name.replace('"', "\"\"")))
            .unwrap();
        let count = query.column_count();
        let mut rows: Vec<String> = query
            .query_map([], |row| {
                let values: Vec<rusqlite::types::Value> = (0..count)
                    .map(|index| row.get(index))
                    .collect::<Result<_, _>>()?;
                Ok(format!("{values:?}"))
            })
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        rows.sort();
        facts.push((name, rows));
    }
    format!("{facts:?}")
}
fn namespace_facts(root: &Path) -> Vec<String> {
    let mut paths = vec![root.to_path_buf()];
    let mut facts = Vec::new();
    while let Some(path) = paths.pop() {
        let metadata = std::fs::symlink_metadata(&path).unwrap();
        assert!(!metadata.file_type().is_symlink());
        // Never open another database/WAL/SHM descriptor while SQLite owners live.
        let database = path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("events.sqlite");
        let hash = (metadata.is_file() && !database)
            .then(|| hex::encode(Sha256::digest(std::fs::read(&path).unwrap())));
        facts.push(format!(
            "{:?}:{}:{}:{}:{hash:?}",
            path.strip_prefix(root).unwrap().as_os_str().as_bytes(),
            metadata.dev(),
            metadata.ino(),
            metadata.len()
        ));
        if metadata.is_dir() {
            paths.extend(
                std::fs::read_dir(path)
                    .unwrap()
                    .map(|entry| entry.unwrap().path()),
            );
        }
    }
    facts.sort();
    facts
}
async fn legacy_delete_refusal(with_reader: bool) {
    let home = FixtureHome::new().unwrap();
    let run = RunId::new();
    legacy_fixture(home.path(), run);
    let facts = {
        let storage = Storage::open(home.path()).await.unwrap();
        let root = storage.run_dir(&run);
        let reader = if with_reader {
            Some(storage.open_run_reader(run).await.unwrap())
        } else {
            None
        };
        let before_rows = registry_facts(&storage);
        let before_files = namespace_facts(&root);
        let controls = home.path().join("run-controls");
        let before_controls = controls.exists().then(|| namespace_facts(&controls));
        let result = storage.delete_run(&run).await;
        let unchanged = root.exists()
            && namespace_facts(&root) == before_files
            && registry_facts(&storage) == before_rows
            && controls.exists().then(|| namespace_facts(&controls)) == before_controls;
        let history = if let Some(reader) = &reader {
            reader.current_seq().await.ok()
        } else {
            None
        };
        (
            matches!(&result,Err(crate::runs::error::StorageError::DeletionCoordinationUnavailable{run_id}) if *run_id==run),
            unchanged,
            !with_reader || history == Some(crate::runs::seq::EventSeq(1)),
            format!("{result:?}"),
        )
    };
    home.close().unwrap();
    assert!(
        facts.0 && facts.1 && facts.2,
        "legacy deletion must refuse without namespace/SQL mutation; reader={with_reader}; settled: {facts:?}"
    );
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn legacy_physical_delete_refuses_without_any_namespace_or_registry_mutation() {
    legacy_delete_refusal(false).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn legacy_live_reader_history_survives_refused_physical_delete() {
    legacy_delete_refusal(true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn new_creation_records_controlled_origin_and_original_control_before_namespace_use() {
    let home = FixtureHome::new().unwrap();
    let run = RunId::new();
    let facts = {
        let storage = Storage::open(home.path()).await.unwrap();
        match storage.create_run(run, home.path(), None).await {
            Ok(writer) => {
                writer.close().await.unwrap();
                let conn = storage.acquire_registry_conn().unwrap();
                let row:(String,String,Vec<u8>,Vec<u8>,Vec<u8>)=conn.query_row(
                    "SELECT o.kind,c.creation_nonce,c.home_identity,c.parent_identity,c.file_identity FROM run_coordination_origins o JOIN run_host_controls c USING(run_id) WHERE run_id=?",
                    [run.to_string()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).unwrap();
                let expected = [
                    home.path().to_path_buf(),
                    home.path().join("run-controls"),
                    home.path().join(format!("run-controls/{run}.lock")),
                ]
                .map(|path| {
                    let metadata = std::fs::symlink_metadata(path).unwrap();
                    [metadata.dev().to_le_bytes(), metadata.ino().to_le_bytes()].concat()
                });
                let linked:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM run_host_openings WHERE run_id=? AND coordination_origin=run_id)",[run.to_string()],|r|r.get(0)).unwrap();
                (
                    row.0 == "NewControlled"
                        && !row.1.is_empty()
                        && [row.2, row.3, row.4] == expected
                        && linked,
                    "created".to_owned(),
                )
            },
            Err(error) => (false, format!("{error:?}")),
        }
    };
    home.close().unwrap();
    assert!(
        facts.0,
        "actual new producer must retain durable native coordination: {facts:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn controlled_live_reader_pools_refuse_physical_delete_after_actual_writer_exit() {
    let home = FixtureHome::new().unwrap();
    let run = RunId::new();
    let facts = {
        let storage = Storage::open(home.path()).await.unwrap();
        let writer = storage.create_run(run, home.path(), None).await.unwrap();
        let mut readers = Vec::new();
        for _ in 0..64 {
            readers.push(storage.open_run_reader(run).await.unwrap());
        }
        writer.close().await.unwrap();
        let result = storage.delete_run(&run).await;
        let refusal = matches!(&result,Err(crate::runs::error::StorageError::RunInUse{run_id}) if *run_id==run);
        let diagnostic = format!("{result:?}");
        let exists = storage.run_dir(&run).exists();
        let registered: i64 = storage
            .acquire_registry_conn()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM runs WHERE id=?",
                [run.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        let count = readers.len();
        drop(readers);
        // The actual outside control, not a sleep, proves all connection/manager workers closed.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            match super::super::host_settlement::control::acquire(
                storage.home(),
                &storage.registry_pool,
                run,
                true,
            ) {
                Ok(owner) => {
                    drop(owner);
                    break;
                },
                Err(OpenError::RunInUse { .. }) if std::time::Instant::now() < deadline => {
                    std::thread::yield_now();
                },
                Err(error) => panic!("actual reader owners did not settle: {error:?}"),
            }
        }
        (refusal, exists, registered, count, diagnostic)
    };
    home.close().unwrap();
    assert_eq!(
        (facts.0, facts.1, facts.2, facts.3),
        (true, true, 1, 64),
        "all actual shared pools must exclude deletion after writer exit: {facts:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn controlled_delete_actual_producer_persists_inventory_and_cold_retry_converges() {
    let home = FixtureHome::new().unwrap();
    let run = RunId::new();
    let first = {
        let storage = Storage::open(home.path()).await.unwrap();
        storage
            .create_run(run, home.path(), None)
            .await
            .unwrap()
            .close()
            .await
            .unwrap();
        let control = home.path().join(format!("run-controls/{run}.lock"));
        let original = std::fs::symlink_metadata(&control).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            match super::super::host_settlement::control::acquire(
                storage.home(),
                &storage.registry_pool,
                run,
                true,
            ) {
                Ok(owner) => {
                    drop(owner);
                    break;
                },
                Err(OpenError::RunInUse { .. }) if std::time::Instant::now() < deadline => {
                    std::thread::yield_now();
                },
                Err(error) => panic!("actual producer owners did not close: {error:?}"),
            }
        }
        let result = storage.delete_run(&run).await;
        let current = std::fs::symlink_metadata(&control).unwrap();
        let conn = storage.acquire_registry_conn().unwrap();
        let tombstones:i64=conn.query_row("SELECT COUNT(*) FROM run_host_tombstones WHERE run_id=? AND state='Deleted' AND provenance='ExplicitStorageDelete'",[run.to_string()],|row|row.get(0)).unwrap();
        let entries:i64=conn.query_row("SELECT COUNT(*) FROM run_host_deletion_inventory WHERE deletion_id IN (SELECT deletion_id FROM run_host_tombstones WHERE run_id=?)",[run.to_string()],|row|row.get(0)).unwrap();
        (
            result.is_ok(),
            !storage.run_dir(&run).exists(),
            tombstones,
            entries,
            original.dev() == current.dev() && original.ino() == current.ino(),
            format!("{result:?}"),
        )
    };
    let second = {
        let storage = Storage::open(home.path()).await.unwrap();
        let result = storage.delete_run(&run).await;
        let recreation = storage.create_run(run, home.path(), None).await;
        let refused = recreation.is_err();
        drop(recreation);
        (result.is_ok(), refused)
    };
    home.close().unwrap();
    assert!(
        first.0 && first.1 && first.2 == 1 && first.3 >= 3 && first.4,
        "actual producer must persist its own successful deletion: {first:?}"
    );
    assert_eq!(
        second,
        (true, true),
        "cold authenticated retry is idempotent and resurrection refused"
    );
}

fn settle_control(storage: &Storage, run: RunId) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        match super::super::host_settlement::control::acquire(
            storage.home(),
            &storage.registry_pool,
            run,
            true,
        ) {
            Ok(owner) => {
                drop(owner);
                return;
            },
            Err(OpenError::RunInUse { .. }) if std::time::Instant::now() < deadline => {
                std::thread::yield_now();
            },
            Err(error) => panic!("actual owners did not settle: {error:?}"),
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_running_reader_worker_retains_checkout_and_manager_until_actual_exit() {
    let home = FixtureHome::new().unwrap();
    let run = RunId::new();
    let facts = {
        let storage = Storage::open(home.path()).await.unwrap();
        let writer = storage.create_run(run, home.path(), None).await.unwrap();
        let writer_refused = matches!(storage.delete_run(&run).await,
            Err(crate::runs::StorageError::WriterStillActive { run_id }) if run_id == run);
        let reader = storage.open_run_reader(run).await.unwrap();
        writer.close().await.unwrap();
        let pool = reader.pool.clone();
        drop(reader);
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let worker = tokio::task::spawn_blocking(move || {
            let connection = pool.get().unwrap();
            let value: i64 = connection
                .query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))
                .unwrap();
            ready_tx.send(value).unwrap();
            release_rx
                .recv_timeout(std::time::Duration::from_secs(10))
                .unwrap();
            drop(connection);
            drop(pool);
        });
        let empty_history = ready_rx.await.unwrap() == 0;
        worker.abort();
        let refused = matches!(storage.delete_run(&run).await,
            Err(crate::runs::StorageError::RunInUse { run_id }) if run_id == run);
        release_tx.send(()).unwrap();
        let joined = worker.await.is_ok();
        settle_control(&storage, run);
        let deleted = storage.delete_run(&run).await.is_ok();
        (writer_refused, empty_history, refused, joined, deleted)
    };
    home.close().unwrap();
    assert_eq!(
        facts,
        (true, true, true, true, true),
        "settled worker: {facts:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cold_control_loss_and_same_byte_replacement_never_downgrade_or_recreate() {
    let mut facts = Vec::new();
    for replace in [false, true] {
        let home = FixtureHome::new().unwrap();
        let run = RunId::new();
        {
            let storage = Storage::open(home.path()).await.unwrap();
            storage
                .create_run(run, home.path(), None)
                .await
                .unwrap()
                .close()
                .await
                .unwrap();
            settle_control(&storage, run);
        }
        let control = home.path().join(format!("run-controls/{run}.lock"));
        let original = control.with_extension("original");
        std::fs::rename(&control, &original).unwrap();
        if replace {
            std::fs::copy(&original, &control).unwrap();
            let old = std::fs::symlink_metadata(&original).unwrap();
            let new = std::fs::symlink_metadata(&control).unwrap();
            assert_ne!((old.dev(), old.ino()), (new.dev(), new.ino()));
        }
        let result = {
            let storage = Storage::open(home.path()).await.unwrap();
            let before = registry_facts(&storage);
            let files = namespace_facts(&storage.run_dir(&run));
            let controls = namespace_facts(&home.path().join("run-controls"));
            let reader = storage.open_run_reader(run).await;
            let writer = storage.open_run_writer(run).await;
            let deletion = storage.delete_run(&run).await;
            let refused = matches!(reader, Err(OpenError::Authority(_) | OpenError::Io(_)))
                && matches!(writer, Err(OpenError::Authority(_) | OpenError::Io(_)))
                && matches!(deletion, Err(crate::runs::StorageError::DeletionOwnership { run_id, .. }) if run_id == run);
            refused
                && before == registry_facts(&storage)
                && files == namespace_facts(&storage.run_dir(&run))
                && controls == namespace_facts(&home.path().join("run-controls"))
        };
        home.close().unwrap();
        facts.push((replace, result));
    }
    assert_eq!(facts, [(false, true), (true, true)]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn legacy_writer_and_history_reopen_without_new_coordination_origin_or_control() {
    let home = FixtureHome::new().unwrap();
    let run = RunId::new();
    legacy_fixture(home.path(), run);
    let facts = {
        let storage = Storage::open(home.path()).await.unwrap();
        let writer = storage.open_run_writer(run).await.unwrap();
        let reader = storage.open_run_reader(run).await.unwrap();
        let history = reader.current_seq().await.unwrap();
        writer.close().await.unwrap();
        let origin: String = storage
            .acquire_registry_conn()
            .unwrap()
            .query_row(
                "SELECT kind FROM run_coordination_origins WHERE run_id=?",
                [run.to_string()],
                |r| r.get(0),
            )
            .unwrap();
        (
            history.0 == 1,
            origin == "Legacy",
            !home
                .path()
                .join(format!("run-controls/{run}.lock"))
                .exists(),
        )
    };
    home.close().unwrap();
    assert_eq!(facts, (true, true, true));
}
