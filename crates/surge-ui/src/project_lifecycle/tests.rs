use super::*;
use surge_core::{
    ContentHash,
    bootstrap_operation::{BootstrapCaptureFields, BootstrapContentRef, BootstrapIntent},
};
use surge_persistence::runs::Storage;

fn capture(root: &std::path::Path) -> surge_core::bootstrap_operation::BootstrapCapture {
    BootstrapCaptureFields {
        version: 1,
        planning_run: RunId::new(),
        implementation_run: RunId::new(),
        repository: root.into(),
        git_common_dir: root.join(".git"),
        base_commit: "a".repeat(40),
        planning_worktree: root.join("planning"),
        implementation_worktree: root.join("implementation"),
        planning_branch: "surge/planning".into(),
        implementation_branch: "surge/implementation".into(),
        graph: BootstrapContentRef {
            name: "bootstrap".into(),
            digest: ContentHash::compute(b"graph"),
        },
        project_context: None,
        bootstrap_edit_loop_cap: None,
        runtime_identity: ContentHash::compute(b"runtime"),
        configuration: Vec::new(),
        credential_refs: Vec::new(),
    }
    .try_into()
    .unwrap()
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_corrupt_operation_does_not_hide_a_healthy_operation_in_the_same_page() {
    let home = tempfile::tempdir().unwrap();
    let scope = HistoryScope {
        generation: 1,
        root: home.path().into(),
        repository: Some(home.path().join(".git")),
        home: home.path().into(),
    };
    let storage = Storage::open(home.path()).await.unwrap();
    let store = storage.bootstrap_operation_store();
    let mut good = None;
    for label in ["Healthy older operation", "Corrupt newer operation"] {
        let pins = capture(home.path());
        let record = store
            .insert_if_absent(
                RunId::new(),
                &BootstrapIntent::new(home.path().into(), label.into(), Default::default())
                    .unwrap(),
                &pins,
            )
            .unwrap();
        let writer = storage
            .create_run(
                record.status.planning_run,
                &pins.fields().planning_worktree,
                None,
            )
            .await
            .unwrap();
        writer.close().await.unwrap();
        if label.starts_with("Healthy") {
            good = Some(record);
        } else {
            let pool = surge_persistence::runs::registry::open_registry_pool(
                home.path(),
                &surge_persistence::runs::MockClock::new(1),
            )
            .unwrap();
            pool.get().unwrap().execute("UPDATE bootstrap_operations SET intent_json='broken JSON' WHERE operation_id=?1",[record.status.operation_id.to_string()]).unwrap();
        }
    }
    let good = good.unwrap();
    let batch = loading::load_registry(scope, None, HashMap::new())
        .await
        .expect("one invalid operation must not abort healthy rows");
    assert_eq!(batch.summaries.len(), 2);
    assert_eq!(
        batch
            .operations
            .get(&good.status.operation_id)
            .unwrap()
            .record,
        good
    );
    assert!(batch.runs.values().any(|run| matches!(
        run.source.availability,
        Availability::Unavailable(_)
    ) && run.ownership == RunOwnership::Unknown));
}

#[tokio::test]
async fn proposed_file_status_never_creates_accepted_execution_or_verification() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("roadmap.toml");
    std::fs::write(
        &path,
        r#"schema_version=2
[[milestones]]
id="draft"
title="Authored proposal"
[[milestones.tasks]]
id="same-id"
title="Draft says completed"
status="completed"
verified=true
"#,
    )
    .unwrap();
    let inputs = load_planning_inputs(root.path().into()).await;
    assert_eq!(inputs.len(), 2);
    assert_eq!(inputs[0].path, path);
    assert_eq!(inputs[0].source.availability, Availability::Available);
    assert_eq!(inputs[1].source.availability, Availability::Absent);
    let history = ProjectHistory {
        scope: Some(HistoryScope {
            generation: 1,
            root: root.path().into(),
            repository: None,
            home: root.path().join("missing-home"),
        }),
        planning_inputs: Arc::new(inputs),
        ..Default::default()
    };
    let model = HomeProjection::compile(&crate::work_items::TaskCache::default(), &history);
    assert_eq!(
        (model.operations.len(), model.recorded_tasks, model.verified),
        (0, 0, 0)
    );
    assert!(!root.path().join("missing-home").exists());
    std::fs::write(&path, "corrupt TOML[").unwrap();
    let inputs = load_planning_inputs(root.path().into()).await;
    assert!(matches!(
        inputs[0].source.availability,
        Availability::Unavailable(_)
    ));
    assert!(inputs[0].roadmap.is_none());
}
#[test]
fn retained_history_snapshots_share_large_event_and_mission_allocations() {
    let run = RunId::new();
    let events = Arc::new(
        (1..=4096)
            .map(|seq| surge_core::RunEvent {
                run_id: run,
                seq,
                timestamp: chrono::DateTime::UNIX_EPOCH,
                payload: surge_core::EventPayload::TaskDiscovered {
                    task_id: format!("child-{seq}").into(),
                    discovered_from: "absent".into(),
                    title: "Retained discovery".into(),
                },
            })
            .collect::<Vec<_>>(),
    );
    let observed = Arc::new(ProjectRun {
        run,
        ownership: RunOwnership::Unknown,
        requested: RunOwnership::Unknown,
        source: SourceObservation::default(),
        events: events.clone(),
        execution: None,
        mission: None,
        decisions: Arc::default(),
    });
    let mut history = ProjectHistory::default();
    history.runs.insert(run, observed.clone());
    for _ in 0..1000 {
        let snapshot = history.clone();
        assert!(Arc::ptr_eq(&snapshot.runs[&run], &observed));
        assert!(Arc::ptr_eq(&snapshot.runs[&run].events, &events));
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn registry_refresh_continues_a_retained_prefix_beyond_1024_events() {
    let home = tempfile::tempdir().unwrap();
    let scope = HistoryScope {
        generation: 1,
        root: home.path().into(),
        repository: Some(home.path().join(".git")),
        home: home.path().into(),
    };
    let storage = Storage::open(home.path()).await.unwrap();
    let pins = capture(home.path());
    let record = storage
        .bootstrap_operation_store()
        .insert_if_absent(
            RunId::new(),
            &BootstrapIntent::new(
                home.path().into(),
                "Retain every recorded event".into(),
                Default::default(),
            )
            .unwrap(),
            &pins,
        )
        .unwrap();
    let run = record.status.planning_run;
    let writer = storage
        .create_run(run, &pins.fields().planning_worktree, None)
        .await
        .unwrap();
    writer
        .append_events(
            (1..=1200)
                .map(|index| {
                    surge_core::VersionedEventPayload::new(
                        surge_core::EventPayload::TaskDiscovered {
                            task_id: format!("child-{index}").into(),
                            discovered_from: "unassigned".into(),
                            title: format!("Literal child {index}"),
                        },
                    )
                })
                .collect(),
        )
        .await
        .unwrap();
    writer.close().await.unwrap();
    let first = loading::load_registry(scope.clone(), None, HashMap::new())
        .await
        .unwrap();
    assert_eq!(first.runs[&run].events.len(), 1024);
    assert_eq!(first.runs[&run].source.coverage, Coverage::Partial);
    let refreshed = loading::load_registry(scope.clone(), None, first.runs)
        .await
        .unwrap();
    let observed = &refreshed.runs[&run];
    assert_eq!(
        observed.events.len(),
        1200,
        "refresh must continue the retained prefix"
    );
    assert_eq!(observed.source.through, Some(1200));
    assert_eq!(observed.source.coverage, Coverage::Complete);
    assert_eq!(observed.events.first().unwrap().seq, 1);
    assert_eq!(observed.events.last().unwrap().seq, 1200);
    assert_eq!(observed.execution.as_ref().unwrap().tasks.len(), 1200);
    drop(storage);
    std::fs::write(
        home.path()
            .join("runs")
            .join(run.to_string())
            .join("events.sqlite"),
        b"corrupt journal",
    )
    .unwrap();
    let unavailable = loading::load_registry(scope, None, refreshed.runs)
        .await
        .unwrap();
    assert_eq!(
        unavailable.runs[&run].events.len(),
        1200,
        "failed refresh must retain the already observed prefix"
    );
    assert!(matches!(
        unavailable.runs[&run].source.availability,
        Availability::Unavailable(_)
    ));
    assert_eq!(unavailable.runs[&run].source.freshness, Freshness::Retained);
}
fn modern_detail(
    root: &std::path::Path,
    origin: surge_core::work_item::AcceptedWorkItemOrigin,
) -> surge_core::work_item::WorkItemDetail {
    use surge_core::work_item::*;
    let item = WorkItemRecord {
        id: surge_core::id::WorkItemId::nil(),
        project: surge_core::id::WorkItemProjectId::new(),
        title: "Same title".into(),
        accepted_revision: 2,
        version: 4,
        archived_at_ms: None,
        active_run: None,
        generation: 0,
        workspace: WorkItemWorkspace {
            repository: root.join(".git"),
            checkout: root.into(),
            path: root.join("task"),
            ownership: "fixture".into(),
            branch: "task/fixture".into(),
            base_commit: "a".repeat(40),
        },
    };
    let revision = WorkItemRevision {
        revision: 2,
        hash: origin.hash().unwrap(),
        origin,
        actor: "human".into(),
        accepted_proposal: None,
        accepted_at_ms: 1,
    };
    WorkItemDetail {
        item,
        revision,
        pr: None,
        usage: Default::default(),
        control: None,
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ordinary_flow_and_legacy_operation_with_colliding_ids_and_titles_remain_distinct() {
    use surge_core::work_item::{AcceptedFlowContract, AcceptedWorkItemOrigin};
    let home = tempfile::tempdir().unwrap();
    let graph = toml::from_str(include_str!("../../testdata/nested_loops_flow.toml")).unwrap();
    let flow = AcceptedFlowContract::new(Box::new(graph), "  Preserve exact Flow prompt\n ".into())
        .unwrap();
    let detail = modern_detail(home.path(), AcceptedWorkItemOrigin::Flow(flow));
    let mut tasks = crate::work_items::TaskCache::default();
    tasks.records.push(detail.item.clone());
    tasks
        .details
        .insert(detail.item.id, Arc::new(detail.clone()));
    let storage = Storage::open(home.path()).await.unwrap();
    let legacy = storage
        .bootstrap_operation_store()
        .insert_if_absent(
            RunId::nil(),
            &BootstrapIntent::new(home.path().into(), "Same title".into(), Default::default())
                .unwrap(),
            &capture(home.path()),
        )
        .unwrap();
    let mut history = ProjectHistory {
        scope: Some(HistoryScope {
            generation: 1,
            root: home.path().into(),
            repository: Some(home.path().join(".git")),
            home: home.path().into(),
        }),
        ..Default::default()
    };
    history.operations.insert(
        legacy.status.operation_id,
        Arc::new(LegacyOperation {
            record: legacy.clone(),
            roadmap: None,
            roadmap_source: Default::default(),
        }),
    );
    let model = HomeProjection::compile(&tasks, &history);
    assert_eq!(model.operations.len(), 2);
    assert_eq!(
        model.operations[0].key,
        OperationKey::Task {
            project: detail.item.project,
            item: detail.item.id
        }
    );
    assert_eq!(
        model.operations[0].asked.as_deref(),
        Some("  Preserve exact Flow prompt\n ")
    );
    assert_eq!(
        model.operations[1].key,
        OperationKey::Bootstrap(RunId::nil())
    );
    assert_eq!(
        model.operations[1].runs,
        vec![legacy.status.planning_run, legacy.status.implementation_run]
    );
    assert_eq!((model.recorded_tasks, model.verified), (0, 0));
    tasks.records[0].accepted_revision = 3;
    tasks.records[0].version = 5;
    let changed = HomeProjection::compile(&tasks, &history);
    assert!(
        changed.operations[0].accepted.is_none(),
        "stale accepted detail cannot become current scope"
    );
    assert_eq!(
        changed.operations.len(),
        2,
        "stale modern detail cannot hide legacy history"
    );
    history.begin(
        home.path().join("other"),
        Some(home.path().join("other/.git")),
        home.path().into(),
        false,
    );
    let switched = HomeProjection::compile(&tasks, &history);
    assert!(
        switched.operations.is_empty(),
        "project switch must quarantine old records"
    );
    drop(storage);
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn older_owned_operation_is_found_after_multiple_foreign_registry_pages() {
    let home = tempfile::tempdir().unwrap();
    let storage = Storage::open(home.path()).await.unwrap();
    let pins = capture(home.path());
    let record = storage
        .bootstrap_operation_store()
        .insert_if_absent(
            RunId::new(),
            &BootstrapIntent::new(
                home.path().into(),
                "Older owned request".into(),
                Default::default(),
            )
            .unwrap(),
            &pins,
        )
        .unwrap();
    let writer = storage
        .create_run(
            record.status.planning_run,
            &pins.fields().planning_worktree,
            None,
        )
        .await
        .unwrap();
    writer.close().await.unwrap();
    for _ in 0..100 {
        let writer = storage
            .create_run(RunId::new(), home.path().join("foreign-checkout"), None)
            .await
            .unwrap();
        writer.close().await.unwrap();
    }
    let scope = HistoryScope {
        generation: 1,
        root: home.path().into(),
        repository: Some(home.path().join(".git")),
        home: home.path().into(),
    };
    let batch = loading::load_registry(scope, None, HashMap::new())
        .await
        .unwrap();
    assert_eq!(
        batch.summaries.len(),
        101,
        "foreign-only pages must not terminate project discovery"
    );
    assert!(batch.operations.contains_key(&record.status.operation_id));
    assert!(batch.cursor.is_none());
    assert_eq!(batch.source.coverage, Coverage::Complete);
    drop(storage);
}
#[test]
fn fresh_live_stage_progress_is_removed_on_terminal_offline_or_partial_observation() {
    use surge_core::work_item::{AcceptedWorkItemOrigin, WorkItemBinding, WorkItemRequirements};
    let root = std::path::Path::new("/fixture/live-home");
    let run = RunId::new();
    let mut detail = modern_detail(
        root,
        AcceptedWorkItemOrigin::Requirements(
            WorkItemRequirements::new(
                "Accepted live scope".into(),
                vec!["Observe exact owner".into()],
            )
            .unwrap(),
        ),
    );
    detail.item.active_run = Some(run);
    detail.item.generation = 1;
    let binding = WorkItemBinding {
        item: detail.item.id,
        revision: detail.item.accepted_revision,
        requirements_hash: detail.revision.hash,
        generation: 1,
    };
    let mut tasks = crate::work_items::TaskCache::default();
    tasks.records.push(detail.item.clone());
    tasks.details.insert(detail.item.id, Arc::new(detail));
    let owner = RunOwnership::Modern {
        project: tasks.records[0].project,
        binding,
        graph: None,
    };
    let event = |seq, node: &str| surge_core::RunEvent {
        run_id: run,
        seq,
        timestamp: chrono::DateTime::UNIX_EPOCH,
        payload: surge_core::EventPayload::StageEntered {
            node: node.parse().unwrap(),
            attempt: 1,
        },
    };
    let mut events = vec![event(1, "spec")];
    let execution = TaskExecutionView::compile(run, &events).unwrap();
    let observed = ProjectRun {
        run,
        ownership: owner.clone(),
        requested: owner,
        source: SourceObservation {
            availability: Availability::Available,
            freshness: Freshness::Observed,
            coverage: Coverage::Complete,
            through: Some(1),
        },
        events: Arc::new(events.clone()),
        execution: Some(Arc::new(execution)),
        mission: None,
        decisions: Arc::default(),
    };
    let mut history = ProjectHistory {
        scope: Some(HistoryScope {
            generation: 1,
            root: root.into(),
            repository: Some(root.join(".git")),
            home: root.join("runtime"),
        }),
        ..Default::default()
    };
    history.runs.insert(run, Arc::new(observed));
    history.live_observed.insert(run, 1);
    assert_eq!(
        HomeProjection::compile(&tasks, &history).current[0].entered,
        1
    );
    events.push(event(2, "implement"));
    let observed = Arc::make_mut(history.runs.get_mut(&run).unwrap());
    observed.execution = Some(Arc::new(TaskExecutionView::compile(run, &events).unwrap()));
    observed.source.through = Some(2);
    history.live_observed.insert(run, 2);
    assert_eq!(
        HomeProjection::compile(&tasks, &history).current[0].entered,
        2
    );
    history.live_observed.clear();
    assert!(
        HomeProjection::compile(&tasks, &history).current.is_empty(),
        "offline cached stage is history, not current work"
    );
    history.live_observed.insert(run, 2);
    Arc::make_mut(history.runs.get_mut(&run).unwrap())
        .source
        .coverage = Coverage::Partial;
    assert!(
        HomeProjection::compile(&tasks, &history).current.is_empty(),
        "partial prefix cannot identify current stage"
    );
    let observed = Arc::make_mut(history.runs.get_mut(&run).unwrap());
    observed.source.coverage = Coverage::Complete;
    events.push(surge_core::RunEvent {
        run_id: run,
        seq: 3,
        timestamp: chrono::DateTime::UNIX_EPOCH,
        payload: surge_core::EventPayload::RunAborted {
            reason: "stopped".into(),
        },
    });
    observed.execution = Some(Arc::new(TaskExecutionView::compile(run, &events).unwrap()));
    observed.source.through = Some(3);
    history.live_observed.insert(run, 3);
    assert!(HomeProjection::compile(&tasks, &history).current.is_empty());
    assert_eq!(
        history.runs[&run].execution.as_ref().unwrap().stages.len(),
        2,
        "terminal history remains retained"
    );
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_owned_standalone_run_in_a_page_remains_visible_and_loadable() {
    let home = tempfile::tempdir().unwrap();
    let storage = Storage::open(home.path()).await.unwrap();
    let graph: surge_core::graph::Graph =
        toml::from_str(include_str!("../../testdata/nested_loops_flow.toml")).unwrap();
    let mut expected = Vec::new();
    for label in [
        "First standalone",
        "Second standalone",
        "Third standalone",
        "Fourth standalone",
    ] {
        let run = RunId::new();
        expected.push(run);
        let writer = storage.create_run(run, home.path(), None).await.unwrap();
        writer
            .append_events(vec![
                surge_core::VersionedEventPayload::new(surge_core::EventPayload::RunStarted {
                    pipeline_template: None,
                    project_path: home.path().into(),
                    initial_prompt: label.into(),
                    config: surge_core::RunConfig {
                        budget: Default::default(),
                        sandbox_default: surge_core::sandbox::SandboxMode::WorkspaceWrite,
                        approval_default: surge_core::approvals::ApprovalPolicy::OnRequest,
                        auto_pr: false,
                        mcp_servers: Vec::new(),
                        bootstrap_edit_loop_cap: None,
                    },
                }),
                surge_core::VersionedEventPayload::new(
                    surge_core::EventPayload::PipelineMaterialized {
                        graph: Box::new(graph.clone()),
                        graph_hash: graph.identity().unwrap(),
                    },
                ),
            ])
            .await
            .unwrap();
        writer.close().await.unwrap();
    }
    let scope = HistoryScope {
        generation: 1,
        root: home.path().canonicalize().unwrap(),
        repository: None,
        home: home.path().into(),
    };
    let batch = loading::load_registry(scope.clone(), None, HashMap::new())
        .await
        .unwrap();
    let history = ProjectHistory {
        scope: Some(scope.clone()),
        summaries: batch.summaries,
        runs: batch.runs,
        ..Default::default()
    };
    let model = HomeProjection::compile(&crate::work_items::TaskCache::default(), &history);
    let visible: std::collections::HashSet<_> = model
        .operations
        .iter()
        .flat_map(|operation| operation.runs.iter().copied())
        .collect();
    assert_eq!(
        visible,
        expected.iter().copied().collect(),
        "heavy-read cap cannot remove owned candidates"
    );
    assert_eq!(model.owners.len(), 4);
    for run in expected {
        let observed = loading::load_run(
            scope.clone(),
            run,
            model.owners[&run].clone(),
            Arc::default(),
            None,
            Arc::default(),
        )
        .await
        .unwrap();
        assert_eq!(observed.ownership, RunOwnership::RecordedProject);
    }
    drop(storage);
}

#[path = "decision_tests.rs"]
mod decision_tests;

#[path = "artifact_read_tests.rs"]
mod artifact_read_tests;
