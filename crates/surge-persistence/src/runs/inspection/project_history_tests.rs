use super::*;
use std::{collections::BTreeMap, num::NonZeroU32};
use surge_core::{ContentHash, EventPayload as E};

#[path = "project_history_snapshot.rs"]
mod snapshot;
#[cfg(windows)]
#[path = "windows_readonly_opening_tests.rs"]
mod windows_readonly_opening_tests;
use snapshot::{Snapshot, files};
fn assert_readonly(before: &Snapshot, after: &Snapshot) {
    snapshot::validate_readonly(before, after).unwrap();
}
fn id(value: u64) -> RunId {
    format!("run-{value:026}").parse().unwrap()
}
fn summary(value: u64, project: &str) -> RunSummary {
    RunSummary {
        id: id(value),
        project_path: project.into(),
        pipeline_template: None,
        status: surge_core::RunStatus::Running,
        started_at_ms: 100,
        ended_at_ms: None,
        daemon_pid: None,
        wake_at_ms: None,
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn readonly_pages_continue_past_foreign_runs_and_defer_new_upper_keys() {
    let home = crate::runtime_home_fixture::FixtureHome::new().unwrap();
    {
        let storage = Storage::open(home.path()).await.unwrap();
        for value in 1..=8 {
            super::super::registry::insert_run(
                &storage.registry_pool,
                &summary(value, if value == 1 { "/ours" } else { "/foreign" }),
            )
            .unwrap();
        }
        let before = files(home.path());
        let limit = NonZeroU32::new(2).unwrap();
        let first = Storage::inspect_existing_run_summary_page(home.path().into(), limit, None)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            first.entries.iter().map(|row| row.id).collect::<Vec<_>>(),
            [id(8), id(7)]
        );
        assert_readonly(&before, &files(home.path()));
        super::super::registry::insert_run(&storage.registry_pool, &summary(9, "/ours")).unwrap();
        let mut all = first.entries;
        let mut cursor = first.next_cursor;
        let mut pages = 1;
        while let Some(next) = cursor {
            let page =
                Storage::inspect_existing_run_summary_page(home.path().into(), limit, Some(next))
                    .await
                    .unwrap()
                    .unwrap();
            all.extend(page.entries);
            cursor = page.next_cursor;
            pages += 1;
            assert!(pages <= 4, "cursor must make bounded forward progress");
        }
        assert_eq!(pages, 4);
        assert_eq!(
            all.iter().map(|row| row.id).collect::<Vec<_>>(),
            [id(8), id(7), id(6), id(5), id(4), id(3), id(2), id(1)]
        );
        assert_eq!(all.last().unwrap().project_path, Path::new("/ours"));
        let refreshed = Storage::inspect_existing_run_summary_page(home.path().into(), limit, None)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(refreshed.entries[0].id, id(9));
    }
    home.close().unwrap();
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn readonly_pages_distinguish_missing_corrupt_and_foreign_home_cursor() {
    let home = crate::runtime_home_fixture::FixtureHome::new().unwrap();
    let other = crate::runtime_home_fixture::FixtureHome::new().unwrap();
    {
        let missing = home.path().join("missing");
        assert!(
            Storage::inspect_existing_run_summary_page(missing.clone(), NonZeroU32::MIN, None)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            Storage::inspect_existing_events_after(
                missing.clone(),
                id(1),
                EventSeq::ZERO,
                NonZeroU32::MIN
            )
            .await
            .unwrap()
            .is_none()
        );
        assert!(
            Storage::inspect_existing_bootstrap_operation_for_run(missing.clone(), id(1))
                .await
                .unwrap()
                .is_none()
        );
        assert!(!missing.exists());
        let a = Storage::open(home.path()).await.unwrap();
        let b = Storage::open(other.path()).await.unwrap();
        for value in 1..=2 {
            super::super::registry::insert_run(&a.registry_pool, &summary(value, "/ours")).unwrap();
        }
        let page =
            Storage::inspect_existing_run_summary_page(home.path().into(), NonZeroU32::MIN, None)
                .await
                .unwrap()
                .unwrap();
        assert!(
            Storage::inspect_existing_run_summary_page(
                other.path().into(),
                NonZeroU32::MIN,
                page.next_cursor
            )
            .await
            .is_err()
        );
        drop(b);
        drop(a);
        let invalid = other.path().join("db/registry.sqlite");
        std::fs::write(&invalid, b"corrupt registry fixture").unwrap();
        let before = files(other.path());
        assert!(
            Storage::inspect_existing_run_summary_page(other.path().into(), NonZeroU32::MIN, None)
                .await
                .is_err()
        );
        assert!(
            Storage::inspect_existing_bootstrap_operation_for_run(other.path().into(), id(1))
                .await
                .is_err()
        );
        assert_eq!(files(other.path()), before);
    }
    other.close().unwrap();
    home.close().unwrap();
}
type RegistryRows = BTreeMap<String, Vec<String>>;
fn registry_rows(connection: &rusqlite::Connection) -> RegistryRows {
    let names: Vec<String> = connection
        .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    names
        .into_iter()
        .map(|name| {
            let mut query = connection
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
            (name, rows)
        })
        .collect()
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn static_history_producer() {
    use std::io::Read;
    let Some(root) = std::env::var_os("SURGE_TEST_HISTORY_PRODUCER") else {
        return;
    };
    let home = std::path::PathBuf::from(root);
    let storage = Storage::open(&home).await.unwrap();
    // Pin a pre-creation read snapshot: original origin/control commits stay in live WAL.
    let mut connection = storage.acquire_registry_conn().unwrap();
    let pin = connection.transaction().unwrap();
    let _: i64 = pin
        .query_row("SELECT COUNT(*) FROM runs", [], |row| row.get(0))
        .unwrap();
    let writer = storage.create_run(id(1), "/ours", None).await.unwrap();
    for reason in ["first", "second", "third"] {
        writer
            .append_event(VersionedEventPayload::new(E::RunAborted {
                reason: reason.into(),
            }))
            .await
            .unwrap();
    }
    println!("SURGE_HISTORY_LIVE_READY");
    std::io::stdin().read_exact(&mut [0_u8]).unwrap();
    drop(pin);
    drop(connection);
    writer.close().await.unwrap();
    let expected = registry_rows(&storage.acquire_registry_conn().unwrap());
    println!(
        "SURGE_HISTORY_ROWS={}",
        serde_json::to_string(&expected).unwrap()
    );
    drop(storage);
}
struct HistoryProducer {
    child: std::process::Child,
    lines: std::sync::mpsc::Receiver<String>,
    reader: Option<std::thread::JoinHandle<()>>,
}
impl Drop for HistoryProducer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader) = self.reader.take() {
            reader.join().unwrap();
        }
    }
}
impl HistoryProducer {
    fn start(home: &Path) -> Self {
        use std::io::BufRead;
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "runs::inspection::project_history_tests::static_history_producer",
                "--nocapture",
            ])
            .env("SURGE_TEST_HISTORY_PRODUCER", home)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit())
            .spawn()
            .unwrap();
        let output = child.stdout.take().unwrap();
        let (send, lines) = std::sync::mpsc::channel();
        let reader = std::thread::spawn(move || {
            for line in std::io::BufReader::new(output).lines() {
                let line = line.unwrap();
                if let Some((_, marker)) = line.split_once("SURGE_HISTORY_") {
                    send.send(marker.to_owned()).unwrap();
                }
            }
        });
        let mut producer = Self {
            child,
            lines,
            reader: Some(reader),
        };
        assert_eq!(producer.line(), "LIVE_READY");
        producer
    }
    fn line(&mut self) -> String {
        match self.lines.recv_timeout(std::time::Duration::from_secs(10)) {
            Ok(line) => line,
            Err(error) => {
                let _ = self.child.kill();
                let _ = self.child.wait();
                panic!("producer marker missing: {error}");
            },
        }
    }
    fn finish(mut self) -> RegistryRows {
        use std::io::Write;
        self.child.stdin.as_mut().unwrap().write_all(b"c").unwrap();
        let rows = self.line();
        let rows = serde_json::from_str(rows.strip_prefix("ROWS=").unwrap()).unwrap();
        assert!(self.child.wait().unwrap().success());
        self.reader.take().unwrap().join().unwrap();
        rows
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn static_event_pages_read_latest_live_wal_and_keep_cold_files_unchanged() {
    let home = crate::runtime_home_fixture::FixtureHome::new().unwrap();
    let run = id(1);
    let producer = HistoryProducer::start(home.path());
    let before = files(home.path());
    let run_home = home.path().join("runs").join(run.to_string());
    let lock = run_home.join("events.sqlite.lock");
    assert_eq!(std::fs::metadata(&lock).unwrap().len(), 0);
    assert_eq!(
        before.entries.get(&lock).map(|fact| &fact.hash),
        Some(&Some(ContentHash::compute(b"")))
    );
    let wal = run_home.join("events.sqlite-wal");
    assert!(std::fs::metadata(&wal).unwrap().len() > 0);
    assert!(before.entries[&wal].hash.is_some());
    assert_eq!(
        before
            .entries
            .get(&run_home.join("events.sqlite-shm"))
            .map(|fact| &fact.hash),
        Some(&None)
    );
    assert!(before.entries[&home.path().join("db/registry.sqlite-wal")].length > 32);
    let page = Storage::inspect_existing_events_after(
        home.path().join("runs"),
        run,
        EventSeq(1),
        NonZeroU32::new(2).unwrap(),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(page.iter().map(|row| row.seq.0).collect::<Vec<_>>(), [2, 3]);
    assert!(matches!(&page[1].payload.payload,E::RunAborted{reason} if reason=="third"));
    assert_readonly(&before, &files(home.path()));
    let expected = producer.finish(); // Successful exec'd producer exit closes ALL pools/workers/SQLite.
    let before = files(home.path());
    for suffix in ["-wal", "-shm"] {
        assert!(
            !before
                .entries
                .contains_key(&home.path().join(format!("db/registry.sqlite{suffix}")))
        );
    }
    let page = Storage::inspect_existing_events_after(
        home.path().join("runs"),
        run,
        EventSeq::ZERO,
        NonZeroU32::MIN,
    )
    .await;
    let after = files(home.path()); // Must precede every SQL verification open.
    assert_readonly(&before, &after);
    for suffix in ["-wal", "-shm"] {
        assert!(
            after
                .entries
                .contains_key(&home.path().join(format!("db/registry.sqlite{suffix}")))
        );
    }
    assert_eq!(page.unwrap().unwrap()[0].seq.0, 1);
    let registry = crate::runs::connection::RetainedConnection::read_only(
        &home.path().join("db/registry.sqlite"),
    )
    .unwrap();
    assert_eq!(registry_rows(&registry), expected);
    drop(registry);
    home.close().unwrap();
}

fn operation_capture(
    root: &Path,
    planning: RunId,
    implementation: RunId,
) -> surge_core::bootstrap_operation::BootstrapCapture {
    use surge_core::bootstrap_operation::{BootstrapCaptureFields, BootstrapContentRef};
    BootstrapCaptureFields {
        version: 1,
        planning_run: planning,
        implementation_run: implementation,
        repository: root.into(),
        git_common_dir: root.join("project.git"),
        base_commit: "a".repeat(40),
        planning_worktree: root.join(planning.to_string()),
        implementation_worktree: root.join(implementation.to_string()),
        planning_branch: format!("surge/{planning}"),
        implementation_branch: format!("surge/{implementation}"),
        graph: BootstrapContentRef {
            name: "bootstrap".into(),
            digest: ContentHash::compute(b"bootstrap"),
        },
        project_context: None,
        bootstrap_edit_loop_cap: None,
        runtime_identity: ContentHash::compute(b"runtime"),
        configuration: vec![],
        credential_refs: vec![],
    }
    .try_into()
    .unwrap()
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn readonly_operation_inspection_preserves_every_older_continuation_reference() {
    use surge_core::{
        bootstrap_continuation::BootstrapContinuation,
        bootstrap_operation::{BootstrapContentRef, BootstrapIntent},
        run_state::ArtifactRef,
    };
    let home = crate::runtime_home_fixture::FixtureHome::new().unwrap();
    {
        let storage = Storage::open(home.path()).await.unwrap();
        let store = storage.bootstrap_operation_store();
        let mut originals = Vec::new();
        for index in 1..=3 {
            let capture = operation_capture(home.path(), id(20 + index), id(30 + index));
            let intent = BootstrapIntent::new(
                home.path().into(),
                format!("  Original operation {index}\\n"),
                surge_core::budget::BudgetGuard::default(),
            )
            .unwrap();
            let accepted = store
                .insert_if_absent(id(10 + index), &intent, &capture)
                .unwrap();
            let preparing = store
                .claim(accepted.status.operation_id, accepted.status.revision)
                .unwrap();
            let planning = store
                .mark_started(preparing.status.operation_id, preparing.status.revision)
                .unwrap();
            let references = ["description", "roadmap", "flow"].map(|name| ArtifactRef {
                hash: ContentHash::compute(format!("{index}:{name}").as_bytes()),
                path: home.path().join(format!("{index}-{name}")),
                name: name.into(),
                produced_by: "planner".parse().unwrap(),
                produced_at_seq: Some(
                    surge_core::execution_recovery::semantic_position::SemanticPosition::local(7)
                        .unwrap(),
                ),
            });
            let child = BootstrapContinuation::new(
                capture.fields().planning_run,
                BootstrapContentRef {
                    name: "materialized:flow".into(),
                    digest: ContentHash::compute(format!("graph-{index}").as_bytes()),
                },
                references.into(),
                surge_core::budget::BudgetGuard::default(),
            )
            .unwrap();
            originals.push(
                store
                    .continue_planning(
                        planning.status.operation_id,
                        planning.status.revision,
                        &child,
                    )
                    .unwrap(),
            );
        }
        let before = files(home.path());
        for original in originals {
            for run in [
                original.status.planning_run,
                original.status.implementation_run,
            ] {
                let observed =
                    Storage::inspect_existing_bootstrap_operation_for_run(home.path().into(), run)
                        .await
                        .unwrap()
                        .unwrap();
                assert_eq!(observed, original);
                assert_eq!(
                    observed
                        .child
                        .unwrap()
                        .artifacts()
                        .iter()
                        .map(|reference| reference.name.as_str())
                        .collect::<Vec<_>>(),
                    ["description", "roadmap", "flow"]
                );
            }
        }
        assert_readonly(&before, &files(home.path()));
    }
    home.close().unwrap();
}
