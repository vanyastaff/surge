use super::*;
use crate::work_items::{WorkItemError, WorkItemStore};
use std::path::Path;
use surge_core::{Graph, id::WorkItemOperationId, work_item::*};

fn begin(
    store: &WorkItemStore,
    operation: WorkItemOperationId,
    body: &[u8],
) -> OwnedFlowPreparation {
    let OwnedFlowPreparationResult::Preparing(preparation) =
        store.begin_owned_flow(operation, body, false, 1).unwrap()
    else {
        panic!("new owning preparation")
    };
    *preparation
}
fn source(home: &Path, preparation: &OwnedFlowPreparation) -> OwnedFlowSourceSnapshot {
    let graph: Graph = toml::from_str(include_str!(
        "../../../../../examples/flow_terminal_only.toml"
    ))
    .unwrap();
    OwnedFlowSourceSnapshot {
        contract: AcceptedFlowContract::new(Box::new(graph), " \n\t ".into()).unwrap(),
        source_base: home.join("project"),
        workspace: WorkItemWorkspace {
            repository: home.join("project/.git"),
            checkout: home.join("project"),
            path: home.join("workspace"),
            ownership: preparation.workspace_owner().to_string(),
            branch: "codex/owned".into(),
            base_commit: "a".repeat(40),
        },
    }
}
pub(super) fn accepted(
    store: &WorkItemStore,
    home: &Path,
    operation: WorkItemOperationId,
) -> (OwnedFlowReceipt, crate::work_items::WorkItemLaunchClaim) {
    let mut preparation = begin(store, operation, b"original body");
    let source = source(home, &preparation);
    preparation.freeze_source(&source, None).unwrap();
    preparation
        .freeze_startup(
            r#"{"mcp_servers":[]}"#,
            OwnedFlowMcpSelection::Explicit,
            &[],
        )
        .unwrap();
    preparation.retain_launch_ownership().unwrap();
    preparation.finalize(2).unwrap().into_parts()
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn abandoned_first_body_ids_and_source_are_retained_without_an_accepted_item() {
    let home = tempfile::tempdir().unwrap();
    let storage = crate::runs::Storage::open(home.path()).await.unwrap();
    let store = storage.work_items();
    let operation = WorkItemOperationId::new();
    let preparation = begin(&store, operation, b"original");
    let item = preparation.item();
    let run = preparation.run();
    let owner = preparation.workspace_owner();
    let source = source(home.path(), &preparation);
    preparation.freeze_source(&source, None).unwrap();
    assert!(store.show(item).is_err());
    assert!(matches!(
        store.begin_owned_flow(operation, b"original", false, 2),
        Err(WorkItemError::Busy)
    ));
    drop(preparation);
    assert!(matches!(
        store.begin_owned_flow(operation, b"different", false, 2),
        Err(WorkItemError::Conflict(_))
    ));
    let preparation = begin(&store, operation, b"original");
    assert_eq!(
        (
            preparation.item(),
            preparation.run(),
            preparation.workspace_owner()
        ),
        (item, run, owner)
    );
    assert!(preparation.source_snapshot().unwrap().unwrap() == source);
    assert!(store.show(item).is_err());
    let conn = store.pool.get().unwrap();
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM work_item_attempts", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0);
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn atomic_acceptance_retains_launch_arc_and_receipt_replay_conveys_no_authority() {
    let home = tempfile::tempdir().unwrap();
    let storage = crate::runs::Storage::open(home.path()).await.unwrap();
    let store = storage.work_items();
    let operation = WorkItemOperationId::new();
    let (receipt, claim) = accepted(&store, home.path(), operation);
    assert!(matches!(store.claim(receipt.run), Err(WorkItemError::Busy)));
    store.validate_owned_flow_pending(&claim).unwrap();
    assert!(store.validate_owned_flow_effect(&claim).is_err());
    let inputs = store.hydrate_owned_flow_inputs(&claim).unwrap().unwrap();
    assert!(inputs.servers().is_empty());
    assert_eq!(
        store
            .show(receipt.item)
            .unwrap()
            .revision
            .origin
            .flow()
            .unwrap()
            .raw_prompt(),
        " \n\t "
    );
    assert_eq!(
        store
            .replay_owned_flow(operation, b"original body", false)
            .unwrap(),
        Some(receipt.clone())
    );
    let supervisor = claim.clone();
    drop(claim);
    assert!(matches!(store.claim(receipt.run), Err(WorkItemError::Busy)));
    drop(supervisor);
    let owner = store.claim(receipt.run).unwrap();
    store.validate_owned_flow_pending(&owner).unwrap();
    let token: String = store
        .pool
        .get()
        .unwrap()
        .query_row(
            "SELECT claim_token FROM work_item_attempts WHERE run=?",
            [receipt.run.to_string()],
            |r| r.get(0),
        )
        .unwrap();
    drop(owner);
    let name = format!("flow-launch-v1-{}-{}.lock", operation, receipt.run);
    let path = home.path().join("work-items/preparation-locks").join(name);
    std::fs::rename(&path, home.path().join("original-owned-lock")).unwrap();
    assert!(matches!(
        store.claim(receipt.run),
        Err(WorkItemError::Conflict(_))
    ));
    let unchanged: String = store
        .pool
        .get()
        .unwrap()
        .query_row(
            "SELECT claim_token FROM work_item_attempts WHERE run=?",
            [receipt.run.to_string()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(token, unchanged);
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn queued_stop_without_any_memory_queue_cancels_the_real_intent() {
    let home = tempfile::tempdir().unwrap();
    let storage = crate::runs::Storage::open(home.path()).await.unwrap();
    let store = storage.work_items();
    let (receipt, claim) = accepted(&store, home.path(), WorkItemOperationId::new());
    assert!(store.has_pending_owned_flow(receipt.run).unwrap());
    assert!(store.cancel_pending_owned_flow(receipt.run).unwrap());
    assert!(!store.has_pending_owned_flow(receipt.run).unwrap());
    assert!(store.validate_owned_flow_pending(&claim).is_err());
    assert!(store.hydrate_owned_flow_inputs(&claim).is_err());
    assert_eq!(
        store.for_run(receipt.run).unwrap().unwrap().state,
        WorkItemAttemptState::Aborted
    );
    assert!(store.show(receipt.item).unwrap().item.active_run.is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn started_effect_authority_follows_the_real_latest_control_and_assignment() {
    for mutation in [
        "suspend",
        "archive",
        "generation",
        "attention",
        "suspended",
        "intent",
    ] {
        let changed_state = match mutation {
            "attention" => WorkItemAttemptState::Attention,
            _ => WorkItemAttemptState::Suspended,
        };
        let home = tempfile::tempdir().unwrap();
        let storage = crate::runs::Storage::open(home.path()).await.unwrap();
        let store = storage.work_items();
        let (receipt, claim) = accepted(&store, home.path(), WorkItemOperationId::new());
        let manifest = store.owned_flow_manifest(receipt.run).unwrap().unwrap();
        // This tests the SQL predicate, not journal authorization; the actual
        // host separately verifies its unique startup before acknowledgment.
        store
            .acknowledge_owned_flow_startup(&claim, &manifest)
            .unwrap();
        store.validate_owned_flow_effect(&claim).unwrap();
        if mutation == "suspend" {
            let command = WorkItemCommand::Suspend {
                operation_id: WorkItemOperationId::new(),
                item: receipt.item,
                expected_version: store.show(receipt.item).unwrap().item.version,
            };
            store.mutate(&command, None, None, "operator", 3).unwrap();
        } else {
            let conn = store.pool.get().unwrap();
            match mutation {
                "archive" => {
                    conn.execute(
                        "UPDATE work_items SET archived_at_ms=3 WHERE id=?",
                        [receipt.item.to_string()],
                    )
                    .unwrap();
                },
                "generation" => {
                    conn.execute(
                        "UPDATE work_items SET generation=generation+1 WHERE id=?",
                        [receipt.item.to_string()],
                    )
                    .unwrap();
                },
                "intent" => {
                    conn.execute(
                        "UPDATE owned_flow_operations SET intent_state='canceled' WHERE run=?",
                        [receipt.run.to_string()],
                    )
                    .unwrap();
                },
                "attention" | "suspended" => {
                    let mut attempt = crate::work_items::attempt(&conn, receipt.run).unwrap();
                    attempt.state = changed_state;
                    conn.execute(
                        "UPDATE work_item_attempts SET state=?,payload=? WHERE run=?",
                        rusqlite::params![
                            mutation,
                            serde_json::to_string(&attempt).unwrap(),
                            receipt.run.to_string()
                        ],
                    )
                    .unwrap();
                },
                _ => panic!("unknown test mutation"),
            }
        }
        assert!(
            store.validate_owned_flow_effect(&claim).is_err(),
            "changed real authority admitted an effect"
        );
        assert!(
            matches!(store.claim(receipt.run), Err(WorkItemError::Busy)),
            "rejected effect released original launch ownership"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owned_launch_child() {
    let Some(home) = std::env::var_os("SURGE_OWNED_FLOW_LEASE_HOME") else {
        return;
    };
    let run: surge_core::RunId = std::env::var("SURGE_OWNED_FLOW_LEASE_RUN")
        .unwrap()
        .parse()
        .unwrap();
    let storage = crate::runs::Storage::open(&home).await.unwrap();
    let store = storage.work_items();
    match store.claim(run) {
        Err(WorkItemError::Busy) => {
            std::fs::write(Path::new(&home).join("lease-child-observed"), "busy").unwrap();
        },
        Ok(claim) => {
            std::fs::write(Path::new(&home).join("lease-child-observed"), "held").unwrap();
            let _claim = claim;
            std::future::pending::<()>().await;
        },
        Err(error) => panic!("child claim refusal {error}"),
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn launch_owner_handoff_excludes_a_real_process_and_death_reacquires_original_object() {
    struct Child(std::process::Child);
    impl Drop for Child {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let home = tempfile::tempdir().unwrap();
    let storage = crate::runs::Storage::open(home.path()).await.unwrap();
    let store = storage.work_items();
    let (receipt, claim) = accepted(&store, home.path(), WorkItemOperationId::new());
    let spawn = || {
        Child(
            std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "work_items::owned_flow::tests::owned_launch_child",
                    "--nocapture",
                ])
                .env("SURGE_OWNED_FLOW_LEASE_HOME", home.path())
                .env("SURGE_OWNED_FLOW_LEASE_RUN", receipt.run.to_string())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .unwrap(),
        )
    };
    let observed = home.path().join("lease-child-observed");
    let mut child = spawn();
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while !observed.exists() {
            assert!(child.0.try_wait().unwrap().is_none());
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(std::fs::read_to_string(&observed).unwrap(), "busy");
    assert!(child.0.wait().unwrap().success());
    let engine_owner = claim.clone();
    drop(claim);
    assert!(matches!(store.claim(receipt.run), Err(WorkItemError::Busy)));
    drop(engine_owner);
    std::fs::remove_file(&observed).unwrap();
    let mut child = spawn();
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while !observed.exists() {
            assert!(child.0.try_wait().unwrap().is_none());
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(std::fs::read_to_string(&observed).unwrap(), "held");
    assert!(matches!(store.claim(receipt.run), Err(WorkItemError::Busy)));
    child.0.kill().unwrap();
    child.0.wait().unwrap();
    store
        .validate_owned_flow_pending(&store.claim(receipt.run).unwrap())
        .unwrap();
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn canceled_caller_leaves_the_real_blocking_worker_as_preparation_owner() {
    struct WorkerOwner {
        preparation: Option<OwnedFlowPreparation>,
        done: Option<tokio::sync::oneshot::Sender<()>>,
    }
    impl Drop for WorkerOwner {
        fn drop(&mut self) {
            drop(self.preparation.take());
            if let Some(done) = self.done.take() {
                let _ = done.send(());
            }
        }
    }
    let home = tempfile::tempdir().unwrap();
    let storage = crate::runs::Storage::open(home.path()).await.unwrap();
    let store = storage.work_items();
    let operation = WorkItemOperationId::new();
    let worker_store = store.clone();
    let (ready, ready_rx) = tokio::sync::oneshot::channel();
    let (release, release_rx) = std::sync::mpsc::channel();
    let (done, done_rx) = tokio::sync::oneshot::channel();
    let caller = tokio::spawn(async move {
        tokio::task::spawn_blocking(move || {
            let mut preparation = begin(&worker_store, operation, b"original");
            preparation.retain_launch_ownership().unwrap();
            let item = preparation.item();
            let owner = WorkerOwner {
                preparation: Some(preparation),
                done: Some(done),
            };
            ready.send(item).unwrap();
            release_rx
                .recv_timeout(std::time::Duration::from_secs(10))
                .unwrap();
            owner
        })
        .await
        .unwrap()
    });
    let item = ready_rx.await.unwrap();
    caller.abort();
    assert!(matches!(caller.await,Err(error) if error.is_cancelled()));
    assert!(matches!(
        store.begin_owned_flow(operation, b"original", false, 2),
        Err(WorkItemError::Busy)
    ));
    assert!(store.show(item).is_err());
    release.send(()).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(10), done_rx)
        .await
        .unwrap()
        .unwrap();
    let preparation = begin(&store, operation, b"original");
    assert_eq!(preparation.item(), item);
    assert!(store.show(item).is_err());
}
