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
}
