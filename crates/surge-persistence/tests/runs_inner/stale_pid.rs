//! 12.10 — manually insert a row with a bogus pid; list_runs returns Crashed.

use crate::runs::fixtures::setup;
use surge_core::RunStatus;
use surge_persistence::runs::RunFilter;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_runs_marks_dead_pid_as_crashed() {
    let t = setup().await;
    let writer = t
        .storage
        .create_run(t.run_id.clone(), "/tmp/proj", None)
        .await
        .expect("create_run");
    writer.close().await.expect("close");

    // Manually fake a "still running" registry row pointing at a pid that
    // can't possibly be alive (i32::MAX is well above any real pid).
    {
        let registry_path = t.home.join("db").join("registry.sqlite");
        let conn = rusqlite::Connection::open(&registry_path).expect("open registry");
        conn.execute(
            "UPDATE runs SET status = 'running', daemon_pid = ?, ended_at = NULL WHERE id = ?",
            rusqlite::params![i32::MAX, t.run_id.to_string()],
        )
        .expect("update");
    }

    let runs = t
        .storage
        .list_runs(RunFilter::default())
        .await
        .expect("list_runs");
    let row = runs
        .iter()
        .find(|r| r.id == t.run_id)
        .expect("our run is listed");
    assert_eq!(
        row.status,
        RunStatus::Crashed,
        "stale pid must promote status to Crashed"
    );
    assert!(
        row.ended_at_ms.is_some(),
        "stale-pid detection must populate ended_at_ms"
    );

    // get_run must apply the same probe.
    let single = t
        .storage
        .get_run(&t.run_id)
        .await
        .expect("get_run")
        .expect("Some");
    assert_eq!(single.status, RunStatus::Crashed);
}
