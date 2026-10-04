//! Ordinary Flow submission is a durable host-owned operation.
#[path = "../../surge-orchestrator/tests/fixtures/mock_bridge.rs"]
mod mock_bridge;
use interprocess::local_socket::tokio::prelude::*;
use serde_json::{Value, json};
use std::{path::Path, sync::Arc, time::Duration};
use surge_orchestrator::engine::{Engine, EngineConfig, facade::LocalEngineFacade};
use surge_persistence::runs::Storage;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

async fn request(socket: &Path, body: &Value) -> Value {
    let stream = LocalSocketStream::connect(
        surge_orchestrator::engine::ipc::local_socket_name_from_path(socket).unwrap(),
    )
    .await
    .unwrap();
    let (read, mut write) = stream.split();
    let mut bytes = serde_json::to_vec(body).unwrap();
    bytes.push(b'\n');
    write.write_all(&bytes).await.unwrap();
    let mut reply = String::new();
    tokio::time::timeout(
        Duration::from_secs(10),
        BufReader::new(read).read_line(&mut reply),
    )
    .await
    .unwrap()
    .unwrap();
    if reply.is_empty() {
        return Value::Null;
    }
    serde_json::from_str(&reply).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn locator_replay_survives_missing_source_and_changed_configuration() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let repo = git2::Repository::init(project.path()).unwrap();
    let graph = include_str!("../../../examples/flow_terminal_only.toml");
    std::fs::write(project.path().join("flow.toml"), graph).unwrap();
    let mut index = repo.index().unwrap();
    index.add_path(Path::new("flow.toml")).unwrap();
    index.write().unwrap();
    let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
    let signature = git2::Signature::now("test", "test@example.com").unwrap();
    repo.commit(Some("HEAD"), &signature, &signature, "base", &tree, &[])
        .unwrap();
    let storage = Storage::open(home.path()).await.unwrap();
    let engine = Arc::new(Engine::new(
        Arc::new(mock_bridge::MockBridge::new()),
        storage.clone(),
        Arc::new(
            surge_orchestrator::engine::tools::worktree::WorktreeToolDispatcher::new(
                project.path().into(),
            ),
        ),
        EngineConfig::default(),
    ));
    let cancel = tokio_util::sync::CancellationToken::new();
    let socket = home.path().join("flow.sock");
    let server = tokio::spawn(surge_daemon::run_runs_only(
        surge_daemon::ServerConfig {
            socket_path: socket.clone(),
            max_active: 1,
            max_queue: 2,
        },
        Arc::new(LocalEngineFacade::new(engine.clone())),
        surge_daemon::tracked_run::TrackingContext::new(engine, storage.clone()),
        Arc::new(surge_daemon::broadcast::BroadcastRegistry::new()),
        Arc::new(surge_daemon::admission::AdmissionController::new(1, 2)),
        cancel.clone(),
    ));
    tokio::time::timeout(Duration::from_secs(3), async {
        while !socket.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let body = json!({"method":"owned_flow_start", "request_id":42,
        "request": {"operation_id":surge_core::id::WorkItemOperationId::new(),
            "source_project":project.path(), "input":{"kind":"project_file", "locator":"flow.toml"},
            "config":{}, "workspace":{"kind":"managed"}}});
    let first = request(&socket, &body).await;
    let mut replay = None;
    let mut changed = None;
    let mut startup = None;
    if first["method"] == "owned_flow_started" {
        let receipt: surge_core::work_item::OwnedFlowReceipt =
            serde_json::from_value(first["receipt"].clone()).unwrap();
        startup = Some(wait_terminal_history(&storage, receipt.run).await.startup);
        std::fs::remove_file(project.path().join("flow.toml")).unwrap();
        std::fs::write(
            project.path().join("surge.toml"),
            "invalid config after acceptance [",
        )
        .unwrap();
        replay = Some(request(&socket, &body).await);
        let mut changed_body = body.clone();
        changed_body["request"]["config"]["initial_prompt"] = json!("changed first body");
        changed = Some(request(&socket, &changed_body).await);
    }
    cancel.cancel();
    server.await.unwrap().unwrap();
    assert_eq!(
        first["method"], "owned_flow_started",
        "first owner submission: {first}"
    );
    assert_eq!(replay.unwrap()["receipt"], first["receipt"]);
    assert_eq!(changed.unwrap()["code"], "work_item_conflict");
    let startup = startup.unwrap();
    assert_eq!(
        startup
            .iter()
            .filter(|event| matches!(
                event.payload.payload,
                surge_core::EventPayload::OwnedFlowInputsBound { .. }
            ))
            .count(),
        1
    );
    let receipt: surge_core::work_item::OwnedFlowReceipt =
        serde_json::from_value(first["receipt"].clone()).unwrap();
    let detail = storage.work_items().show(receipt.item).unwrap();
    assert_eq!(detail.revision.origin.flow().unwrap().raw_prompt(), "");
    assert_eq!(
        detail.revision.origin.flow().unwrap().graph().metadata.name,
        "flow_terminal_only"
    );
    let surge_core::work_item::WorkItemResult::Attempts(attempts) = storage
        .work_items()
        .query(&surge_core::work_item::WorkItemCommand::Attempts {
            item: receipt.item,
            after: None,
            limit: 10,
        })
        .unwrap()
    else {
        panic!("attempt history")
    };
    assert_eq!(attempts.entries.len(), 1);
}

async fn wait_terminal_history(
    storage: &Arc<Storage>,
    run: surge_core::RunId,
) -> surge_persistence::runs::inspection::FoldedRunEvidence {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let inspected = storage.inspect_folded_run(run).await.unwrap();
            if let Some(history) = inspected.database
                && matches!(history.state, surge_core::RunState::Terminal { .. })
            {
                return history;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}
