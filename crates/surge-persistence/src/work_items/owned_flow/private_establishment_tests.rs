//! Structural integrity tests use facts from actual native establishment.
//! Isolated in-memory rows never issue a runtime/private ownership capability.
use super::*;
use crate::runtime_home_fixture::FixtureHome;

async fn established_record(home: &std::path::Path) -> Record {
    let storage = crate::runs::Storage::open(home).await.unwrap();
    let store = storage.work_items();
    let (_identity, owner) = super::super::private_inputs::request_identity(
        &store,
        b"fixed native marker schema fixture",
        true,
    )
    .unwrap();
    let record = read(&store.pool.get().unwrap()).unwrap().unwrap();
    drop(owner);
    drop(store);
    drop(storage);
    record
}
fn isolated_schema() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(include_str!(
        "../../runs/migrations/registry/0035_owned_flow_private_establishment.sql"
    ))
    .unwrap();
    conn
}
fn insert(
    conn: &Connection,
    record: &Record,
    state: &str,
    marker: Option<&[u8]>,
) -> rusqlite::Result<usize> {
    conn.execute("INSERT INTO owned_flow_private_establishment(singleton,schema_version,nonce,state,lock_identity,namespace_identity,key_identity,key_digest,marker_identity) VALUES(1,1,?,?,?,?,?,?,?)",
        params![record.nonce,state,record.lock,record.namespace,record.key,record.digest,marker])
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn typed_decode_refuses_incomplete_or_foreign_platform_marker_identity() {
    let home = FixtureHome::new().unwrap();
    let record = established_record(home.path()).await;
    let width = record.marker.as_ref().unwrap().len();
    let other_platform = vec![0; if width == 16 { 24 } else { 16 }];
    let mut refusals = Vec::new();
    for marker in [None, Some(other_platform.as_slice())] {
        let conn = isolated_schema();
        // Deliberately bypass SQL CHECKs only in this isolated database to prove
        // the typed reader does not trust constraints as runtime authority.
        conn.execute_batch("PRAGMA ignore_check_constraints=ON")
            .unwrap();
        insert(&conn, &record, "Established", marker).unwrap();
        refusals.push(matches!(
            read(&conn),
            Err(WorkItemError::PrivateInputsCorrupt)
        ));
    }
    let healthy = {
        let conn = isolated_schema();
        insert(&conn, &record, "Established", record.marker.as_deref()).unwrap();
        read(&conn).unwrap() == Some(record)
    };
    home.close().unwrap();
    assert_eq!(refusals, [true, true]);
    assert!(healthy);
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn schema_requires_complete_marker_and_forbids_rebinding_or_deletion() {
    let home = FixtureHome::new().unwrap();
    let record = established_record(home.path()).await;
    let facts = {
        let incomplete = isolated_schema();
        let missing = insert(&incomplete, &record, "Established", None);
        let conn = isolated_schema();
        insert(&conn, &record, "Establishing", record.marker.as_deref()).unwrap();
        let replacement = conn.execute(
            "UPDATE owned_flow_private_establishment SET marker_identity=key_identity",
            [],
        );
        let removal = conn.execute(
            "UPDATE owned_flow_private_establishment SET marker_identity=NULL",
            [],
        );
        let deletion = conn.execute("DELETE FROM owned_flow_private_establishment", []);
        let unchanged = read(&conn).unwrap().unwrap();
        let constraint = |result: rusqlite::Result<usize>| {
            matches!(result,
            Err(rusqlite::Error::SqliteFailure(error, _)) if error.code == rusqlite::ErrorCode::ConstraintViolation)
        };
        (
            constraint(missing),
            constraint(replacement),
            constraint(removal),
            constraint(deletion),
            unchanged.marker == record.marker && unchanged.state == "Establishing",
        )
    };
    home.close().unwrap();
    assert_eq!(facts, (true, true, true, true, true));
}
