use super::*;
use std::{collections::BTreeMap, num::NonZeroU32};
use surge_core::{ContentHash, EventPayload as E};

fn files(root: &Path) -> BTreeMap<std::path::PathBuf, ContentHash> {
    let mut found = BTreeMap::new();
    if !root.exists() {
        return found;
    }
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else {
                found.insert(
                    path.clone(),
                    ContentHash::compute(&std::fs::read(path).unwrap()),
                );
            }
        }
    }
    found
}
// SQLite readers coordinate through existing ephemeral SHM read marks.
// Every path and every durable DB/WAL/content byte must remain unchanged.
fn assert_readonly(
    before: &BTreeMap<std::path::PathBuf, ContentHash>,
    after: &BTreeMap<std::path::PathBuf, ContentHash>,
) {
    assert_eq!(
        before.keys().collect::<Vec<_>>(),
        after.keys().collect::<Vec<_>>(),
        "inspection must not create or remove any path"
    );
    for (path, hash) in before {
        if !path.to_string_lossy().ends_with("-shm") {
            assert_eq!(
                Some(hash),
                after.get(path),
                "durable content changed: {}",
                path.display()
            );
        }
    }
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
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn static_event_pages_read_latest_live_wal_and_keep_cold_files_unchanged() {
    let home = crate::runtime_home_fixture::FixtureHome::new().unwrap();
    let run = id(1);
    {
        let storage = Storage::open(home.path()).await.unwrap();
        let writer = storage.create_run(run, "/ours", None).await.unwrap();
        for reason in ["first", "second", "third"] {
            writer
                .append_event(VersionedEventPayload::new(E::RunAborted {
                    reason: reason.into(),
                }))
                .await
                .unwrap();
        }
        let before = files(home.path());
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
        writer.close().await.unwrap();
    }
    let before = files(home.path());
    let page = Storage::inspect_existing_events_after(
        home.path().join("runs"),
        run,
        EventSeq::ZERO,
        NonZeroU32::MIN,
    )
    .await;
    assert_readonly(&before, &files(home.path()));
    assert_eq!(page.unwrap().unwrap()[0].seq.0, 1);
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
                produced_at_seq: 7,
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
