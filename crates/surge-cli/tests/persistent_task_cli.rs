//! The compiled CLI talks to the real durable daemon through an isolated home.
use serde_json::Value;
use std::{path::Path, sync::Arc};
use surge_orchestrator::engine::{
    Engine, EngineConfig, facade::LocalEngineFacade, tools::worktree::WorktreeToolDispatcher,
};
use surge_persistence::runs::Storage;
async fn cli(home: &Path, project: &Path, args: Vec<String>) -> Value {
    let home = home.to_path_buf();
    let project = project.to_path_buf();
    let result = tokio::task::spawn_blocking(move || {
        std::process::Command::new(env!("CARGO_BIN_EXE_surge"))
            .args(args)
            .env("SURGE_HOME", home)
            .current_dir(project)
            .output()
            .unwrap()
    })
    .await
    .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    serde_json::from_slice(&result.stdout).unwrap()
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn compiled_task_commands_create_start_replay_and_show_the_same_persistent_item() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let repo = git2::Repository::init(project.path()).unwrap();
    let oid = repo.index().unwrap().write_tree().unwrap();
    let tree = repo.find_tree(oid).unwrap();
    let signature = git2::Signature::now("Fixture", "fixture@example.com").unwrap();
    repo.commit(Some("HEAD"), &signature, &signature, "base", &tree, &[])
        .unwrap();
    drop(tree);
    repo.remote("origin", "https://github.com/fixture/repo.git")
        .unwrap();
    drop(repo);
    let storage = Storage::open(home.path()).await.unwrap();
    let engine = Arc::new(Engine::new(
        Arc::new(surge_acp::bridge::acp_bridge::AcpBridge::with_defaults().unwrap()),
        storage.clone(),
        Arc::new(WorktreeToolDispatcher::new(project.path().into())),
        EngineConfig::default(),
    ));
    let shutdown = tokio_util::sync::CancellationToken::new();
    let socket = surge_daemon::pidfile::socket_path_in(home.path());
    std::fs::create_dir_all(socket.parent().unwrap()).unwrap();
    let host = tokio::spawn(surge_daemon::run_runs_only(
        surge_daemon::ServerConfig {
            socket_path: socket.clone(),
            max_active: 2,
            max_queue: 2,
        },
        Arc::new(LocalEngineFacade::new(engine.clone())),
        surge_daemon::tracked_run::TrackingContext::new(engine, storage.clone()),
        Arc::new(surge_daemon::broadcast::BroadcastRegistry::new()),
        Arc::new(surge_daemon::admission::AdmissionController::new(2, 2)),
        shutdown.clone(),
    ));
    for _ in 0..100 {
        if socket.exists() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    // The subprocess receives only this temp home; it cannot discover the user's daemon.
    std::fs::write(
        home.path().join("daemon/daemon.pid"),
        std::process::id().to_string(),
    )
    .unwrap();
    let requirements = home.path().join("requirements.json");
    std::fs::write(
        &requirements,
        r#"{"text":"CLI accepted task","criteria":["Wire goes through daemon"]}"#,
    )
    .unwrap();
    let operation = surge_core::id::WorkItemOperationId::new().to_string();
    let created = cli(
        home.path(),
        project.path(),
        vec![
            "task".into(),
            "create".into(),
            "--operation".into(),
            operation,
            "--project".into(),
            project.path().display().to_string(),
            "--title".into(),
            "Task".into(),
            "--requirements".into(),
            requirements.display().to_string(),
        ],
    )
    .await;
    let item: surge_core::id::WorkItemId =
        serde_json::from_value(created["value"]["item"]["id"].clone()).unwrap();
    let flow = home.path().join("flow.toml");
    std::fs::write(
        &flow,
        include_str!("../../../examples/flow_terminal_only.toml"),
    )
    .unwrap();
    let operation = surge_core::id::WorkItemOperationId::new().to_string();
    let start = vec![
        "task".into(),
        "start".into(),
        item.to_string(),
        "--operation".into(),
        operation,
        "--version".into(),
        "1".into(),
        flow.display().to_string(),
    ];
    let first = cli(home.path(), project.path(), start.clone()).await;
    let replay = cli(home.path(), project.path(), start).await;
    assert_eq!(first["value"]["run"], replay["value"]["run"]);
    let shown = cli(
        home.path(),
        project.path(),
        vec!["task".into(), "show".into(), item.to_string()],
    )
    .await;
    assert_eq!(
        shown["value"]["revision"]["requirements"]["text"],
        "CLI accepted task"
    );
    assert_eq!(shown["value"]["usage"]["runs"], 1);
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while storage
            .work_items()
            .show(item)
            .unwrap()
            .item
            .active_run
            .is_some()
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let workspace = storage.work_items().show(item).unwrap().item.workspace.path;
    std::fs::write(workspace.join("untracked-feedback.txt"), "Retain feedback").unwrap();
    let args = |action: &str, extra: Vec<String>| {
        let current = storage.work_items().show(item).unwrap();
        let mut args = vec![
            "task".into(),
            action.into(),
            item.to_string(),
            "--operation".into(),
            surge_core::id::WorkItemOperationId::new().to_string(),
            "--version".into(),
            current.item.version.to_string(),
        ];
        args.extend(extra);
        args
    };
    cli(
        home.path(),
        project.path(),
        args("attach-pr", vec!["fixture/repo".into(), "41".into()]),
    )
    .await;
    std::fs::write(
        &requirements,
        r#"{"text":"Proposed feedback","criteria":["Preserve PR"]}"#,
    )
    .unwrap();
    cli(
        home.path(),
        project.path(),
        args(
            "discuss",
            vec![
                "Review feedback".into(),
                "--proposal".into(),
                requirements.display().to_string(),
            ],
        ),
    )
    .await;
    assert_eq!(
        storage.work_items().show(item).unwrap().revision.revision,
        1
    );
    let discussion = cli(
        home.path(),
        project.path(),
        vec!["task".into(), "discussion".into(), item.to_string()],
    )
    .await;
    assert_eq!(discussion["value"]["entries"][0]["body"], "Review feedback");
    cli(
        home.path(),
        project.path(),
        args(
            "accept-proposal",
            vec!["--revision".into(), "1".into(), "1".into()],
        ),
    )
    .await;
    let accepted = storage.work_items().show(item).unwrap();
    assert_eq!(accepted.revision.accepted_proposal, Some(1));
    assert_eq!(
        accepted.revision.origin.requirements().unwrap().text(),
        "Proposed feedback"
    );
    std::fs::write(
        &requirements,
        r#"{"text":"Explicit final acceptance","criteria":["Preserve PR and files"]}"#,
    )
    .unwrap();
    cli(
        home.path(),
        project.path(),
        args(
            "edit",
            vec![
                "--revision".into(),
                "2".into(),
                "--requirements".into(),
                requirements.display().to_string(),
            ],
        ),
    )
    .await;
    let revisions = cli(
        home.path(),
        project.path(),
        vec!["task".into(), "revisions".into(), item.to_string()],
    )
    .await;
    assert_eq!(revisions["value"]["entries"].as_array().unwrap().len(), 3);
    cli(
        home.path(),
        project.path(),
        args("start", vec![flow.display().to_string()]),
    )
    .await;
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while storage
            .work_items()
            .show(item)
            .unwrap()
            .item
            .active_run
            .is_some()
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let attempts = cli(
        home.path(),
        project.path(),
        vec!["task".into(), "attempts".into(), item.to_string()],
    )
    .await;
    assert_eq!(attempts["value"]["entries"].as_array().unwrap().len(), 2);
    assert_eq!(
        attempts["value"]["entries"][0]["accepted_revision_relation"],
        "superseded"
    );
    assert_eq!(
        attempts["value"]["entries"][1]["accepted_revision_relation"],
        "matches_current"
    );
    cli(home.path(), project.path(), args("archive", vec![])).await;
    let archived = storage.work_items().show(item).unwrap();
    assert!(archived.item.archived_at_ms.is_some());
    assert_eq!(archived.pr.unwrap().number, 41);
    assert_eq!(
        std::fs::read_to_string(workspace.join("untracked-feedback.txt")).unwrap(),
        "Retain feedback"
    );
    let listed = cli(
        home.path(),
        project.path(),
        vec!["task".into(), "list".into()],
    )
    .await;
    assert_eq!(listed["value"]["entries"].as_array().unwrap().len(), 1);
    shutdown.cancel();
    host.await.unwrap().unwrap();
}
