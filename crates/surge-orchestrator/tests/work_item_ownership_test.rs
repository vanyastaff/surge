//! Reserved tasks must not be dispatched through generic engine entry points.
use std::sync::Arc;
use surge_core::{Graph, id::WorkItemOperationId, work_item::*};
use surge_orchestrator::engine::tools::worktree::WorktreeToolDispatcher;
use surge_orchestrator::engine::{Engine, EngineConfig, EngineRunConfig};
use surge_persistence::runs::Storage;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn generic_engine_start_cannot_bypass_persistent_reservation() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let storage = Storage::open(home.path()).await.unwrap();
    let store = storage.work_items();
    let command = WorkItemCommand::Create {
        operation_id: WorkItemOperationId::new(),
        project: project.path().into(),
        title: "Pinned task".into(),
        requirements: WorkItemRequirements::new(
            "Immutable spec".into(),
            vec!["Accepted criterion".into()],
        )
        .unwrap(),
    };
    let workspace = WorkItemWorkspace {
        repository: project.path().join(".git"),
        checkout: project.path().into(),
        path: project.path().into(),
        ownership: surge_core::RunId::new().to_string(),
        branch: "fixture".into(),
        base_commit: "a".repeat(40),
    };
    let WorkItemResult::Detail(created) = store
        .mutate(&command, Some(&workspace), None, "human", 1)
        .unwrap()
    else {
        panic!("created detail")
    };
    let graph: Graph =
        toml::from_str(include_str!("../../../examples/flow_terminal_only.toml")).unwrap();
    let command = WorkItemCommand::Start {
        operation_id: WorkItemOperationId::new(),
        item: created.item.id,
        expected_version: created.item.version,
        graph: Box::new(graph),
        quota_recovery: None,
    };
    let WorkItemResult::Attempt(attempt) = store
        .mutate(
            &command,
            None,
            Some(&serde_json::to_string(&EngineRunConfig::default()).unwrap()),
            "human",
            2,
        )
        .unwrap()
    else {
        panic!("reserved attempt")
    };
    let engine = Engine::new(
        Arc::new(surge_acp::bridge::acp_bridge::AcpBridge::with_defaults().unwrap()),
        storage.clone(),
        Arc::new(WorktreeToolDispatcher::new(project.path().into())),
        EngineConfig::default(),
    );
    let result = engine
        .start_run(
            attempt.run,
            *attempt.graph.clone(),
            project.path().into(),
            EngineRunConfig::default(),
        )
        .await;
    assert!(
        result.is_err(),
        "reserved task run must require host launch ownership, not generic start"
    );
    assert!(
        engine
            .resume_run(attempt.run, project.path().into())
            .await
            .is_err()
    );
    let child = surge_core::RunId::new();
    assert!(
        surge_orchestrator::engine::fork::fork(
            &storage,
            surge_orchestrator::engine::fork::ForkRequest::new(attempt.run, child, 1)
        )
        .await
        .is_err()
    );
    assert!(
        surge_orchestrator::engine::fork::fork(
            &storage,
            surge_orchestrator::engine::fork::ForkRequest::new(child, attempt.run, 1)
        )
        .await
        .is_err()
    );
    assert!(storage.get_run(&attempt.run).await.unwrap().is_none());
}
