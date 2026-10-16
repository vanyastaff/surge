use super::*;
use surge_core::{EventPayload as E, VersionedEventPayload};

async fn decision_history(
    resolved: bool,
) -> (tempfile::TempDir, Arc<Storage>, ProjectHistory, RunId) {
    let home = tempfile::tempdir().unwrap();
    let storage = Storage::open(home.path()).await.unwrap();
    let graph: surge_core::Graph =
        toml::from_str(include_str!("../../testdata/nested_loops_flow.toml")).unwrap();
    let node = graph.start.clone();
    let run = RunId::new();
    let writer = storage.create_run(run, home.path(), None).await.unwrap();
    let mut events = vec![
        E::RunStarted {
            pipeline_template: None,
            project_path: home.path().into(),
            initial_prompt: "Decision history".into(),
            config: surge_core::RunConfig {
                budget: Default::default(),
                sandbox_default: surge_core::sandbox::SandboxMode::WorkspaceWrite,
                approval_default: surge_core::approvals::ApprovalPolicy::OnRequest,
                auto_pr: false,
                mcp_servers: Vec::new(),
                bootstrap_edit_loop_cap: None,
            },
        },
        E::PipelineMaterialized {
            graph_hash: graph.identity().unwrap(),
            graph: Box::new(graph),
        },
        E::StageEntered {
            node: node.clone(),
            attempt: 1,
        },
        E::HumanInputRequested {
            node: node.clone(),
            session: None,
            call_id: Some("original-request".into()),
            prompt: "Approve original scope?".into(),
            schema: None,
        },
    ];
    if resolved {
        events.extend((5..=1024).map(|seq| E::TaskDiscovered {
            task_id: format!("record-{seq}").into(),
            discovered_from: "unassigned".into(),
            title: "Recorded history".into(),
        }));
        events.push(E::HumanInputResolved {
            node,
            call_id: Some("original-request".into()),
            response: serde_json::json!({"approved":true}),
        });
    }
    writer
        .append_events(events.into_iter().map(VersionedEventPayload::new).collect())
        .await
        .unwrap();
    writer.close().await.unwrap();
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
        scope: Some(scope),
        summaries: batch.summaries,
        runs: batch.runs,
        ..Default::default()
    };
    (home, storage, history, run)
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn partial_decision_prefix_does_not_publish_request_resolved_in_unloaded_tail() {
    let (_home, _storage, mut history, run) = decision_history(true).await;
    assert_eq!(history.runs[&run].events.len(), 1024);
    assert_eq!(history.runs[&run].source.coverage, Coverage::Partial);
    assert_eq!(
        history.runs[&run].decisions.len(),
        1,
        "recorded prefix request remains inspectable"
    );
    let projection = HomeProjection::compile(&crate::work_items::TaskCache::default(), &history);
    assert!(
        projection.decisions.is_empty(),
        "partial prefix cannot establish a pending decision"
    );
    let complete = loading::load_run(
        history.scope.clone().unwrap(),
        run,
        RunOwnership::RecordedProject,
        history.runs[&run].events.clone(),
        None,
        Arc::default(),
    )
    .await
    .unwrap();
    assert_eq!(complete.events.last().unwrap().seq, 1025);
    assert!(matches!(
        complete.events.last().unwrap().payload,
        surge_core::EventPayload::HumanInputResolved { .. }
    ));
    assert!(complete.decisions.is_empty());
    history.runs.insert(run, Arc::new(complete));
    assert!(
        HomeProjection::compile(&crate::work_items::TaskCache::default(), &history)
            .decisions
            .is_empty()
    );
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unavailable_retained_decision_history_does_not_republish_an_action() {
    let (home, storage, mut history, run) = decision_history(false).await;
    assert_eq!(
        HomeProjection::compile(&crate::work_items::TaskCache::default(), &history).decisions[0]
            .id
            .request_seq,
        4
    );
    drop(storage);
    std::fs::write(
        home.path()
            .join("runs")
            .join(run.to_string())
            .join("events.sqlite"),
        b"corrupt journal",
    )
    .unwrap();
    let batch = loading::load_registry(history.scope.clone().unwrap(), None, history.runs)
        .await
        .unwrap();
    history.runs = batch.runs;
    assert!(matches!(
        history.runs[&run].source.availability,
        Availability::Unavailable(_)
    ));
    assert_eq!(history.runs[&run].decisions.len(), 1);
    assert!(
        HomeProjection::compile(&crate::work_items::TaskCache::default(), &history)
            .decisions
            .is_empty(),
        "unavailable retained request cannot establish an actionable decision"
    );
}

fn expected_active_run(
    root: &std::path::Path,
    run: RunId,
) -> (crate::work_items::TaskCache, ProjectHistory) {
    let requirements = surge_core::work_item::WorkItemRequirements::new(
        "Accepted task".into(),
        vec!["Retain exact owner".into()],
    )
    .unwrap();
    let mut detail = modern_detail(
        root,
        surge_core::work_item::AcceptedWorkItemOrigin::Requirements(requirements),
    );
    detail.item.active_run = Some(run);
    detail.item.generation = 1;
    let mut tasks = crate::work_items::TaskCache::default();
    tasks.records.push(detail.item.clone());
    tasks.details.insert(detail.item.id, Arc::new(detail));
    let history = ProjectHistory {
        scope: Some(HistoryScope {
            generation: 1,
            root: root.into(),
            repository: Some(root.join(".git")),
            home: root.into(),
        }),
        source: SourceObservation {
            availability: Availability::Available,
            freshness: Freshness::Observed,
            coverage: Coverage::Complete,
            through: None,
        },
        ..Default::default()
    };
    (tasks, history)
}
#[test]
fn missing_or_mismatched_expected_run_cannot_establish_no_pending_decisions() {
    let home = tempfile::tempdir().unwrap();
    let run = RunId::new();
    let (tasks, mut history) = expected_active_run(home.path(), run);
    let missing = HomeProjection::compile(&tasks, &history);
    assert!(missing.owners.contains_key(&run));
    assert!(
        missing.decisions_uncertain,
        "expected run has not been hydrated"
    );
    history.runs.insert(
        run,
        Arc::new(ProjectRun {
            run,
            ownership: RunOwnership::RecordedProject,
            requested: RunOwnership::RecordedProject,
            source: history.source.clone(),
            events: Arc::default(),
            execution: None,
            mission: None,
            decisions: Arc::default(),
        }),
    );
    assert!(
        HomeProjection::compile(&tasks, &history).decisions_uncertain,
        "hydrated ownership conflicts with expected task owner"
    );
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn first_corrupt_read_of_expected_run_keeps_decision_coverage_unknown() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().canonicalize().unwrap();
    let storage = Storage::open(home.path()).await.unwrap();
    let run = RunId::new();
    let writer = storage.create_run(run, &root, None).await.unwrap();
    writer.close().await.unwrap();
    drop(storage);
    std::fs::write(
        home.path()
            .join("runs")
            .join(run.to_string())
            .join("events.sqlite"),
        b"corrupt first read",
    )
    .unwrap();
    let (tasks, mut history) = expected_active_run(&root, run);
    let batch = loading::load_registry(history.scope.clone().unwrap(), None, HashMap::new())
        .await
        .unwrap();
    history.source = batch.source;
    history.summaries = batch.summaries;
    history.runs = batch.runs;
    assert_eq!(history.runs[&run].ownership, RunOwnership::Unknown);
    assert!(matches!(
        history.runs[&run].source.availability,
        Availability::Unavailable(_)
    ));
    assert!(
        HomeProjection::compile(&tasks, &history).decisions_uncertain,
        "first failed read cannot prove absence of pending decisions"
    );
}
#[test]
fn unrelated_unknown_registry_row_does_not_pollute_project_decision_coverage() {
    let home = tempfile::tempdir().unwrap();
    let (_, mut history) = expected_active_run(home.path(), RunId::new());
    let foreign = RunId::new();
    history.runs.insert(
        foreign,
        Arc::new(ProjectRun {
            run: foreign,
            ownership: RunOwnership::Unknown,
            requested: RunOwnership::Unknown,
            source: SourceObservation {
                availability: Availability::Unavailable("Foreign corrupt journal".into()),
                ..Default::default()
            },
            events: Arc::default(),
            execution: None,
            mission: None,
            decisions: Arc::default(),
        }),
    );
    let projection = HomeProjection::compile(&crate::work_items::TaskCache::default(), &history);
    assert!(projection.owners.is_empty());
    assert!(
        !projection.decisions_uncertain,
        "unrelated global unknown row is not project coverage"
    );
}
