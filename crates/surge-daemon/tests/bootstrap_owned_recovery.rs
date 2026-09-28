//! Legacy recovery must never claim a durable bootstrap operation's reserved runs.
use std::{collections::HashSet, time::Duration};
use surge_core::{
    ContentHash, RunId,
    bootstrap_operation::{BootstrapCaptureFields, BootstrapContentRef, BootstrapIntent},
    budget::BudgetGuard,
};
use surge_daemon::recovery::{RecoveryOptions, plan_recovery};
use surge_persistence::runs::Storage;

#[tokio::test(flavor = "multi_thread")]
async fn legacy_recovery_excludes_both_reserved_ids_even_for_future_payloads() {
    let root = tempfile::tempdir().unwrap();
    let storage = Storage::open(root.path()).await.unwrap();
    let operation = RunId::new();
    let planning = RunId::new();
    let implementation = RunId::new();
    let legacy = RunId::new();
    let pins = BootstrapCaptureFields {
        version: 1,
        planning_run: planning,
        implementation_run: implementation,
        repository: root.path().to_path_buf(),
        git_common_dir: root.path().to_path_buf(),
        base_commit: "a".repeat(40),
        planning_worktree: root.path().join("plan"),
        planning_branch: "surge/plan".into(),
        implementation_worktree: root.path().join("child"),
        implementation_branch: "surge/child".into(),
        graph: BootstrapContentRef {
            name: "bootstrap@1".into(),
            digest: ContentHash::compute(b"graph"),
        },
        project_context: None,
        runtime_identity: ContentHash::compute(b"runtime"),
        configuration: vec![],
        credential_refs: vec![],
    }
    .try_into()
    .unwrap();
    storage
        .bootstrap_operation_store()
        .insert_if_absent(
            operation,
            &BootstrapIntent::new(
                root.path().to_path_buf(),
                "create app".into(),
                BudgetGuard::default(),
            )
            .unwrap(),
            &pins,
        )
        .unwrap();
    let worktrees = root.path().join("worktrees");
    let mut writers = Vec::new();
    for id in [planning, implementation, legacy] {
        writers.push(storage.create_run(id, "/project", None).await.unwrap());
        std::fs::create_dir_all(worktrees.join(id.to_string())).unwrap();
    }
    let options = RecoveryOptions {
        stuck_threshold: Duration::from_secs(3600),
        worktrees_root: worktrees,
        now_ms: 0,
    };
    for version in [1, 99] {
        storage
            .acquire_registry_conn()
            .unwrap()
            .execute(
                "UPDATE bootstrap_operations SET payload_version = ? WHERE operation_id = ?",
                rusqlite::params![version, operation.to_string()],
            )
            .unwrap();
        let report = plan_recovery(&storage, &options, &HashSet::new())
            .await
            .unwrap();
        let ids: Vec<_> = report
            .decisions
            .iter()
            .map(|decision| decision.run_id)
            .collect();
        assert_eq!(
            ids,
            [legacy],
            "legacy recovery claimed reserved bootstrap IDs for payload v{version}"
        );
    }
}
