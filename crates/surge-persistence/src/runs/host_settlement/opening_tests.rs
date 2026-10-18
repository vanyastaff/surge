//! Real producer pause at the unpublished database identity boundary.
use super::*;
use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
};
use tokio::sync::Notify;

#[derive(Default)]
struct Gate {
    entered: Notify,
    release: Notify,
}
static GATES: OnceLock<Mutex<HashMap<RunId, Arc<Gate>>>> = OnceLock::new();
pub(super) async fn pause(run: RunId) {
    let gate = GATES
        .get_or_init(Mutex::default)
        .lock()
        .unwrap()
        .remove(&run);
    if let Some(gate) = gate {
        gate.entered.notify_one();
        gate.release.notified().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unpublished_real_producer_is_a_distinct_opening_refusal() {
    let home = crate::runtime_home_fixture::FixtureHome::new().unwrap();
    let storage = Storage::open(home.path()).await.unwrap();
    let run = RunId::new();
    let gate = Arc::new(Gate::default());
    GATES
        .get_or_init(Mutex::default)
        .lock()
        .unwrap()
        .insert(run, gate.clone());
    let producer = storage.clone();
    let task = tokio::spawn(async move { producer.create_run(run, "/fixed-opening", None).await });
    tokio::time::timeout(std::time::Duration::from_secs(10), gate.entered.notified())
        .await
        .unwrap();
    let initial = storage.inspect_run(run).await;
    let folded = storage.inspect_folded_run(run).await;
    let row: (String, Option<Vec<u8>>) = storage.registry_pool.get().unwrap().query_row(
        "SELECT state,database_identity FROM run_host_openings WHERE run_id=? ORDER BY epoch DESC LIMIT 1",
        [run.to_string()], |row| Ok((row.get(0)?, row.get(1)?))).unwrap();
    gate.release.notify_one();
    let writer = task.await.unwrap().unwrap();
    let healthy = storage.inspect_run(run).await;
    writer.close().await.unwrap();
    drop(storage);
    home.close().unwrap();
    assert_eq!(row, ("Opening".into(), None));
    assert!(healthy.is_ok(), "bound control: {healthy:?}");
    assert!(
        matches!(initial, Err(StorageError::RunOpening(OpenError::OpeningNotReady { run_id })) if run_id == run),
        "initial: {initial:?}"
    );
    assert!(
        matches!(folded, Err(StorageError::RunOpening(OpenError::OpeningNotReady { run_id })) if run_id == run),
        "folded: {folded:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn malformed_latest_opening_never_uses_older_bound_identity() {
    for case in [
        "route",
        "version",
        "non-opening-null",
        "missing",
        "control-loss",
    ] {
        let home = crate::runtime_home_fixture::FixtureHome::new().unwrap();
        let storage = Storage::open(home.path()).await.unwrap();
        let run = RunId::new();
        let writer = storage.create_run(run, "/fixed-bound", None).await.unwrap();
        writer.close().await.unwrap();
        let old_identity: Vec<u8> = {
            let conn = storage.registry_pool.get().unwrap();
            let identity = conn.query_row("SELECT database_identity FROM run_host_openings WHERE run_id=? ORDER BY epoch DESC LIMIT 1", [run.to_string()], |row| row.get(0)).unwrap();
            // Corruption fixtures deliberately bypass CHECKs, never production admission.
            conn.execute_batch("PRAGMA ignore_check_constraints=ON")
                .unwrap();
            if case == "missing" {
                conn.execute(
                    "DELETE FROM run_host_settlements WHERE run_id=?",
                    [run.to_string()],
                )
                .unwrap();
                conn.execute(
                    "DELETE FROM run_host_openings WHERE run_id=?",
                    [run.to_string()],
                )
                .unwrap();
            } else if case != "control-loss" {
                conn.execute("INSERT INTO run_host_openings(opening_id,run_id,owner_mode,state,identity_version,route_identity,database_identity,coordination_origin) SELECT ?,run_id,owner_mode,'Opening',identity_version,route_identity,NULL,coordination_origin FROM run_host_openings WHERE run_id=? ORDER BY epoch DESC LIMIT 1", params![format!("fixed-corrupt-{case}"),run.to_string()]).unwrap();
                let update = match case {
                    "route" => {
                        "UPDATE run_host_openings SET route_identity=zeroblob(length(route_identity)) WHERE opening_id=?"
                    },
                    "version" => {
                        "UPDATE run_host_openings SET identity_version=2 WHERE opening_id=?"
                    },
                    "non-opening-null" => {
                        "UPDATE run_host_openings SET state='Failed' WHERE opening_id=?"
                    },
                    _ => unreachable!(),
                };
                conn.execute(update, [format!("fixed-corrupt-{case}")])
                    .unwrap();
            }
            conn.execute_batch("PRAGMA ignore_check_constraints=OFF")
                .unwrap();
            identity
        };
        if case == "control-loss" {
            std::fs::remove_file(home.path().join("run-controls").join(format!("{run}.lock")))
                .unwrap();
        }
        let initial = storage.inspect_run(run).await;
        let folded = storage.inspect_folded_run(run).await;
        drop(storage);
        home.close().unwrap();
        assert!(matches!(old_identity.len(), 16 | 24));
        assert!(
            matches!(initial, Err(StorageError::RunOpening(_))),
            "{case}: {initial:?}"
        );
        assert!(
            !matches!(
                initial,
                Err(StorageError::RunOpening(OpenError::OpeningNotReady { .. }))
            ),
            "{case}: {initial:?}"
        );
        assert!(
            matches!(folded, Err(StorageError::RunOpening(_))),
            "{case}: {folded:?}"
        );
        assert!(
            !matches!(
                folded,
                Err(StorageError::RunOpening(OpenError::OpeningNotReady { .. }))
            ),
            "{case}: {folded:?}"
        );
    }
}

#[test]
fn storage_opening_conversion_preserves_only_real_sqlite_errors() {
    let run = RunId::new();
    let sqlite = StorageError::from(OpenError::Sqlite(rusqlite::Error::QueryReturnedNoRows));
    assert!(matches!(
        sqlite,
        StorageError::Sqlite(rusqlite::Error::QueryReturnedNoRows)
    ));
    let opening = StorageError::from(OpenError::OpeningNotReady { run_id: run });
    assert!(
        matches!(opening, StorageError::RunOpening(OpenError::OpeningNotReady { run_id }) if run_id == run)
    );
    let authority = StorageError::from(OpenError::Authority("fixed-permanent".into()));
    assert!(
        matches!(authority, StorageError::RunOpening(OpenError::Authority(message)) if message == "fixed-permanent")
    );
}
