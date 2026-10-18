//! 12.8 — store + dedup + read_artifact roundtrip.

use crate::runs::fixtures::setup;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn store_artifact_dedups_and_roundtrips() {
    let t = setup().await;
    let writer = t
        .storage
        .create_run(t.run_id, "/tmp/proj", None)
        .await
        .expect("create_run");

    let body = b"hello world";

    let r1 = writer
        .store_artifact("greeting.txt", body)
        .await
        .expect("store first");
    assert_eq!(r1.name, "greeting.txt");
    assert_eq!(r1.size_bytes, body.len() as u64);

    // Storing the same content (even under a different name) must dedup
    // by content hash and return the existing record.
    let r2 = writer
        .store_artifact("greeting-dup.txt", body)
        .await
        .expect("store second (dedup)");
    assert_eq!(
        r1.id, r2.id,
        "same content must produce same content-addressed id"
    );
    assert_eq!(
        r1.path, r2.path,
        "dedup must surface the original on-disk path"
    );

    // Round-trip the bytes through read_artifact.
    let bytes = writer.read_artifact(&r1.id).await.expect("read_artifact");
    assert_eq!(bytes, body);

    // The artifacts view must list a single row regardless of the dup
    // store call (dedup keeps the table 1-row, not 2-row).
    let rows = writer.artifacts().await.expect("artifacts view");
    assert_eq!(rows.len(), 1, "dedup must not double-row the view");
    assert_eq!(rows[0].id, r1.id);

    writer.close().await.expect("close");
    t.close().expect("close runtime home");
}

/// Post-edit compatibility coverage: SQL materialization and call-time association
/// are independent, and neither turns source seq87 into a local row index.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dedup_preserves_current_call_index_and_separate_stored_origin() {
    use surge_core::execution_recovery::semantic_position::{
        ContinuationSeedId, InheritedProvenance, SourceEventRef,
    };
    let t = setup().await;
    let writer = t
        .storage
        .create_run(t.run_id, t.home.join("project"), None)
        .await
        .unwrap();
    let first = writer
        .store_artifact("original.txt", b"same bytes")
        .await
        .unwrap();
    assert_eq!(first.produced_at_seq.as_u64(), 0);
    assert_eq!(first.inherited_provenance, None);
    let first_json = serde_json::to_value(&first).unwrap();
    assert_eq!(first_json["produced_at_seq"], 0);
    assert!(
        first_json.get("inherited_provenance").is_none(),
        "ordinary legacy JSON shape stays unchanged"
    );
    writer
        .append_event(surge_core::VersionedEventPayload::new(
            surge_core::EventPayload::RunStarted {
                pipeline_template: None,
                project_path: t.home.join("project"),
                initial_prompt: "byte association test".into(),
                config: surge_core::RunConfig {
                    bootstrap_edit_loop_cap: None,
                    budget: Default::default(),
                    sandbox_default: surge_core::sandbox::SandboxMode::WorkspaceWrite,
                    approval_default: surge_core::approvals::ApprovalPolicy::OnRequest,
                    auto_pr: false,
                    mcp_servers: vec![],
                },
            },
        ))
        .await
        .unwrap();
    writer.close().await.unwrap();
    let origin = InheritedProvenance::new(
        ContinuationSeedId::new(surge_core::ContentHash::compute(b"dedup-seed")),
        SourceEventRef::new(surge_core::RunId::new(), 87).unwrap(),
    );
    {
        let conn = rusqlite::Connection::open(
            t.home
                .join("runs")
                .join(t.run_id.to_string())
                .join("events.sqlite"),
        )
        .unwrap();
        conn.execute(
            "UPDATE artifacts SET inherited_provenance_json=? WHERE id=?",
            rusqlite::params![
                serde_json::to_string(&origin).unwrap(),
                first.id.to_string()
            ],
        )
        .unwrap();
    }
    let writer = t.storage.open_run_writer(t.run_id).await.unwrap();
    let duplicate = writer
        .store_artifact("duplicate.txt", b"same bytes")
        .await
        .unwrap();
    assert_eq!(
        duplicate.produced_at_seq.as_u64(),
        1,
        "returned current_seq remains the documented call-time association"
    );
    assert_eq!(duplicate.inherited_provenance, Some(origin));
    assert_eq!(
        writer.read_artifact(&duplicate.id).await.unwrap(),
        b"same bytes"
    );
    let rows = writer.artifacts().await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].produced_at_seq.as_u64(),
        0,
        "dedup must not rewrite the persisted row index"
    );
    assert_eq!(rows[0].inherited_provenance, Some(origin));
    writer.close().await.unwrap();
    t.close().unwrap();
}
