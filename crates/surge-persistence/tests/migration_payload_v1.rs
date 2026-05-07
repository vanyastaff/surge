//! Integration test: write events via `RunWriter`, read them back, and assert
//! the read path traverses `surge_core::migrate_payload`. We verify two
//! invariants:
//!
//! 1. Round-trip equality — what we wrote is what we read.
//! 2. The reader rejects rows whose `schema_version` column is outside the
//!    supported range, surfacing the typed `SchemaTooNew` error from the
//!    migration chain. This proves the persistence layer is calling
//!    `migrate_payload` rather than deserializing the blob directly.

use std::path::PathBuf;
use std::str::FromStr;

use rusqlite::params;
use surge_core::approvals::ApprovalPolicy;
use surge_core::id::RunId;
use surge_core::keys::{NodeKey, OutcomeKey};
use surge_core::run_event::{EventPayload, RunConfig, VersionedEventPayload};
use surge_core::sandbox::SandboxMode;
use surge_persistence::runs::seq::EventSeq;
use surge_persistence::runs::Storage;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_path_round_trips_v1_events() {
    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::open(dir.path()).await.unwrap();
    let run_id = RunId::new();
    let writer = storage.create_run(run_id, dir.path(), None).await.unwrap();

    let payloads = vec![
        EventPayload::RunStarted {
            pipeline_template: None,
            project_path: PathBuf::from("/migration-test"),
            initial_prompt: "v1 round trip".into(),
            config: RunConfig {
                sandbox_default: SandboxMode::WorkspaceWrite,
                approval_default: ApprovalPolicy::OnRequest,
                auto_pr: false,
                mcp_servers: Vec::new(),
            },
        },
        EventPayload::OutcomeReported {
            node: NodeKey::try_from("impl_1").unwrap(),
            outcome: OutcomeKey::from_str("done").unwrap(),
            summary: "ok".into(),
        },
        EventPayload::RunCompleted {
            terminal_node: NodeKey::try_from("end").unwrap(),
        },
    ];

    for p in &payloads {
        writer
            .append_event(VersionedEventPayload::new(p.clone()))
            .await
            .unwrap();
    }

    writer.flush().await.unwrap();
    drop(writer);

    let reader = storage.open_run_reader(run_id).await.unwrap();
    let events = reader
        .read_events(EventSeq(1)..EventSeq(4))
        .await
        .expect("read_events");

    assert_eq!(events.len(), 3);
    for (i, ev) in events.iter().enumerate() {
        assert_eq!(ev.payload.schema_version, 1);
        assert_eq!(ev.payload.payload, payloads[i]);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_path_rejects_unsupported_schema_version() {
    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::open(dir.path()).await.unwrap();
    let run_id = RunId::new();
    let writer = storage.create_run(run_id, dir.path(), None).await.unwrap();

    // Write one valid v1 event so the run exists.
    writer
        .append_event(VersionedEventPayload::new(EventPayload::RunFailed {
            error: "placeholder".into(),
        }))
        .await
        .unwrap();
    writer.close().await.expect("close writer");

    // Bypass the writer to insert a row with a future schema version. The
    // reader must surface the migration error via `migrate_payload` —
    // proving the migration chain is on the read path.
    let events_db = dir
        .path()
        .join("runs")
        .join(run_id.to_string())
        .join("events.sqlite");
    let path_for_blocking = events_db.clone();
    tokio::task::spawn_blocking(move || -> rusqlite::Result<()> {
        let conn = rusqlite::Connection::open(&path_for_blocking)?;
        conn.execute(
            "INSERT INTO events (timestamp, kind, payload, schema_version)
             VALUES (?, ?, ?, ?)",
            params![2, "Unknown", b"{}".to_vec(), 999],
        )?;
        Ok(())
    })
    .await
    .unwrap()
    .expect("forced insert succeeds");

    let reader = storage.open_run_reader(run_id).await.unwrap();
    let result = reader.read_events(EventSeq(1)..EventSeq(3)).await;
    let err = result.expect_err("v999 row must error");
    let msg = err.to_string();
    assert!(
        msg.contains("schema migration failed") && msg.contains("999"),
        "unexpected error: {msg}"
    );
}
