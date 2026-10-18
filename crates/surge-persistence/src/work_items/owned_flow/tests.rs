use super::*;
use crate::runtime_home_fixture::FixtureHome;
use crate::work_items::{WorkItemError, WorkItemStore};
use std::path::Path;
use surge_core::{Graph, id::WorkItemOperationId, work_item::*};

fn begin(
    store: &WorkItemStore,
    operation: WorkItemOperationId,
    body: &[u8],
) -> OwnedFlowPreparation {
    let OwnedFlowPreparationResult::Preparing(preparation) = store
        .begin_owned_flow(
            operation,
            body,
            crate::work_items::OwnedFlowPreparationPurpose::Ordinary(
                crate::work_items::OrdinaryRequestPrivacy::Public,
            ),
            1,
        )
        .unwrap()
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
    accepted_with_servers(store, home, operation, &[])
}
pub(super) fn accepted_with_servers(
    store: &WorkItemStore,
    home: &Path,
    operation: WorkItemOperationId,
    servers: &[surge_core::mcp_config::McpServerRef],
) -> (OwnedFlowReceipt, crate::work_items::WorkItemLaunchClaim) {
    let mut preparation = begin(store, operation, b"original body");
    let source = source(home, &preparation);
    preparation.freeze_source(&source, None).unwrap();
    preparation
        .freeze_startup(
            r#"{"mcp_servers":[]}"#,
            if servers.is_empty() {
                OwnedFlowMcpSelection::Explicit
            } else {
                OwnedFlowMcpSelection::HostDefault
            },
            servers,
        )
        .unwrap();
    preparation.retain_launch_ownership().unwrap();
    preparation.finalize(2).unwrap().into_parts()
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn abandoned_first_body_ids_and_source_are_retained_without_an_accepted_item() {
    let home = FixtureHome::new().unwrap();
    {
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
            store.begin_owned_flow(
                operation,
                b"original",
                crate::work_items::OwnedFlowPreparationPurpose::Ordinary(
                    crate::work_items::OrdinaryRequestPrivacy::Public
                ),
                2
            ),
            Err(WorkItemError::Busy)
        ));
        drop(preparation);
        assert!(matches!(
            store.begin_owned_flow(
                operation,
                b"different",
                crate::work_items::OwnedFlowPreparationPurpose::Ordinary(
                    crate::work_items::OrdinaryRequestPrivacy::Public
                ),
                2
            ),
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
    home.close().expect("close runtime home");
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn atomic_acceptance_retains_launch_arc_and_receipt_replay_conveys_no_authority() {
    let home = FixtureHome::new().unwrap();
    {
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
                .replay_owned_flow(
                    operation,
                    b"original body",
                    crate::work_items::OwnedFlowPreparationPurpose::Ordinary(
                        crate::work_items::OrdinaryRequestPrivacy::Public
                    )
                )
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
    home.close().expect("close runtime home");
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn queued_stop_without_any_memory_queue_cancels_the_real_intent() {
    let home = FixtureHome::new().unwrap();
    {
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
    home.close().expect("close runtime home");
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
        let home = FixtureHome::new().unwrap();
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
        drop(claim);
        drop(store);
        drop(storage);
        home.close().expect("close runtime home");
    }
}

fn publish_launch_observation(home: &Path, value: &str) {
    assert!(matches!(value, "busy" | "held"));
    let observed = home.join("lease-child-observed");
    let pending = home.join("lease-child-observed.pending");
    assert!(!observed.exists() && !pending.exists());
    std::fs::write(&pending, value).unwrap();
    std::fs::rename(pending, observed).unwrap();
}
fn complete_launch_observation(observed: &Path) -> Option<String> {
    match std::fs::read_to_string(observed) {
        Ok(value) => {
            assert!(
                matches!(value.as_str(), "busy" | "held"),
                "unexpected complete child observation: {value:?}"
            );
            Some(value)
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => panic!("child observation read failed: {error}"),
    }
}
async fn wait_for_launch_observation(child: &mut std::process::Child, observed: &Path) {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            if complete_launch_observation(observed).is_some() {
                break;
            }
            if let Some(status) = child.try_wait().unwrap() {
                // Publication can complete between the absent read and child exit.
                assert!(
                    complete_launch_observation(observed).is_some(),
                    "child exited before a complete observation: {status}"
                );
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("complete child observation within original deadline");
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
            publish_launch_observation(Path::new(&home), "busy");
        },
        Ok(claim) => {
            publish_launch_observation(Path::new(&home), "held");
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
    let home = FixtureHome::new().unwrap();
    {
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
        wait_for_launch_observation(&mut child.0, &observed).await;
        assert_eq!(std::fs::read_to_string(&observed).unwrap(), "busy");
        assert!(child.0.wait().unwrap().success());
        let engine_owner = claim.clone();
        drop(claim);
        assert!(matches!(store.claim(receipt.run), Err(WorkItemError::Busy)));
        drop(engine_owner);
        std::fs::remove_file(&observed).unwrap();
        let mut child = spawn();
        wait_for_launch_observation(&mut child.0, &observed).await;
        assert_eq!(std::fs::read_to_string(&observed).unwrap(), "held");
        assert!(matches!(store.claim(receipt.run), Err(WorkItemError::Busy)));
        child.0.kill().unwrap();
        child.0.wait().unwrap();
        store
            .validate_owned_flow_pending(&store.claim(receipt.run).unwrap())
            .unwrap();
    }
    home.close().expect("close runtime home");
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
    let home = FixtureHome::new().unwrap();
    {
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
            store.begin_owned_flow(
                operation,
                b"original",
                crate::work_items::OwnedFlowPreparationPurpose::Ordinary(
                    crate::work_items::OrdinaryRequestPrivacy::Public
                ),
                2
            ),
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
    home.close().expect("close runtime home");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owned_flow_reconciliation_claim_preserves_new_manual_control() {
    let home = FixtureHome::new().unwrap();
    let storage = crate::runs::Storage::open(home.path()).await.unwrap();
    let store = storage.work_items();
    let (receipt, claim) = accepted(&store, home.path(), WorkItemOperationId::new());
    let manifest = store.owned_flow_manifest(receipt.run).unwrap().unwrap();
    // SQL authority oracle only; host integration verifies actual startup.
    store
        .acknowledge_owned_flow_startup(&claim, &manifest)
        .unwrap();
    let observed = store.for_run(receipt.run).unwrap().unwrap();
    let command = WorkItemCommand::Suspend {
        operation_id: WorkItemOperationId::new(),
        item: receipt.item,
        expected_version: store.show(receipt.item).unwrap().item.version,
    };
    store.mutate(&command, None, None, "operator", 3).unwrap();
    drop(claim);
    let before = crate::work_items::reconciliation_tests::snapshot(&store);
    assert!(
        store
            .claim_reconciliation(&observed, None)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        crate::work_items::reconciliation_tests::snapshot(&store),
        before
    );
    let current = store.execution_control(receipt.run).unwrap().unwrap();
    let accepted = store
        .claim_reconciliation(&observed, Some(&current))
        .unwrap()
        .unwrap();
    assert_eq!(
        store.validate_claim(&accepted).unwrap().binding,
        observed.binding
    );
    drop(accepted);
    drop(store);
    drop(storage);
    home.close().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn accepted_public_private_mcp_replay_refuses_unavailable_storage_without_changing_facts() {
    let home = FixtureHome::new().unwrap();
    let checks = {
        let storage = crate::runs::Storage::open(home.path()).await.unwrap();
        let store = storage.work_items();
        let operation = WorkItemOperationId::new();
        // HostDefault is a public request whose first host resolution is private.
        let mut preparation = begin(&store, operation, b"public HostDefault body");
        preparation
            .freeze_source(&source(home.path(), &preparation), None)
            .unwrap();
        let preparation_identity = || -> (String, String, String) {
            store.pool.get().unwrap().query_row(
                "SELECT request_identity,operation_lock_identity,token FROM owned_flow_operations WHERE operation=?",
                [operation.to_string()], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            ).unwrap()
        };
        let original_owner = preparation_identity();
        assert!(matches!(
            store.begin_owned_flow(
                operation,
                b"public HostDefault body",
                crate::work_items::OwnedFlowPreparationPurpose::Ordinary(
                    crate::work_items::OrdinaryRequestPrivacy::Public
                ),
                3
            ),
            Err(WorkItemError::Busy)
        ));
        let server = surge_core::mcp_config::McpServerRef::new(
            "private-host-default".into(),
            surge_core::mcp_config::McpTransportConfig::stdio(
                "private-provider.exe".into(),
                vec!["private-argument".into()],
                [("PRIVATE_TOKEN".into(), "private-value".into())]
                    .into_iter()
                    .collect(),
            ),
            Some(vec!["allowed".into()]),
            std::time::Duration::from_secs(7),
            false,
        );
        let manifest = preparation
            .freeze_startup(
                r#"{"mcp_servers":[]}"#,
                OwnedFlowMcpSelection::HostDefault,
                &[server],
            )
            .unwrap();
        assert!(matches!(manifest.mcp(), FrozenOwnedFlowMcp::Private { .. }));
        assert_eq!(
            preparation_identity(),
            original_owner,
            "private upgrade replaced the retained preparation owner"
        );
        assert!(matches!(
            store.begin_owned_flow(
                operation,
                b"public HostDefault body",
                crate::work_items::OwnedFlowPreparationPurpose::Ordinary(
                    crate::work_items::OrdinaryRequestPrivacy::Public
                ),
                4
            ),
            Err(WorkItemError::Busy)
        ));

        preparation.retain_launch_ownership().unwrap();
        let (_receipt, claim) = preparation.finalize(2).unwrap().into_parts();
        assert_eq!(
            store
                .hydrate_owned_flow_inputs(&claim)
                .unwrap()
                .unwrap()
                .servers()
                .len(),
            1
        );
        drop(claim);
        let audit = || -> (i64, i64, String, String, String) {
            store.pool.get().unwrap().query_row(
                "SELECT (SELECT COUNT(*) FROM owned_flow_operations), \
                 (SELECT COUNT(*) FROM work_item_attempts), o.receipt,o.inputs_manifest,a.claim_token \
                 FROM owned_flow_operations o JOIN work_item_attempts a ON a.run=o.run WHERE o.operation=?",
                [operation.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            ).unwrap()
        };
        let before = audit();
        let namespace = home.path().join("work-items/private-flow-inputs");
        let displaced = home.path().join("displaced-private-flow-inputs");
        assert!(namespace.join("host-key-v1").is_file());
        std::fs::rename(&namespace, &displaced).unwrap();
        std::fs::write(&namespace, b"private namespace unavailable").unwrap();
        // Any accidental authentication/object lookup now encounters a file,
        // rather than a directory, and cannot silently recreate the namespace.
        assert!(std::fs::read(namespace.join("host-key-v1")).is_err());
        let replay = store.replay_owned_flow(
            operation,
            b"public HostDefault body",
            crate::work_items::OwnedFlowPreparationPurpose::Ordinary(
                crate::work_items::OrdinaryRequestPrivacy::Public,
            ),
        );
        let replay_expected = matches!(
            &replay,
            Err(WorkItemError::PrivateInputsRecoveryRequired | WorkItemError::PrivateInputsUnsafe)
        );
        let replay_diagnostic = replay.as_ref().err().map(|error| format!("{error:?}"));
        drop(replay);
        let begin = store.begin_owned_flow(
            operation,
            b"public HostDefault body",
            crate::work_items::OwnedFlowPreparationPurpose::Ordinary(
                crate::work_items::OrdinaryRequestPrivacy::Public,
            ),
            99,
        );
        let begin_expected = matches!(
            &begin,
            Err(WorkItemError::PrivateInputsRecoveryRequired | WorkItemError::PrivateInputsUnsafe)
        );
        let begin_diagnostic = begin.as_ref().err().map(|error| format!("{error:?}"));
        drop(begin);
        let after = audit();
        let unavailable_bytes = std::fs::read(&namespace).unwrap();
        let original_key_preserved = displaced.join("host-key-v1").is_file();
        std::fs::remove_file(&namespace).unwrap();
        std::fs::rename(displaced, namespace).unwrap();
        #[cfg(windows)]
        let witness = storage.registry_pool_witness().unwrap();
        drop(store);
        drop(storage);
        #[cfg(windows)]
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while witness.is_retained() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        (
            replay_expected,
            begin_expected,
            replay_diagnostic,
            begin_diagnostic,
            before,
            after,
            unavailable_bytes,
            original_key_preserved,
        )
    };
    home.close().unwrap();
    let (
        replay_expected,
        begin_expected,
        replay_diagnostic,
        begin_diagnostic,
        before,
        after,
        unavailable_bytes,
        original_key_preserved,
    ) = checks;
    assert_eq!(
        after, before,
        "replay changed an accepted operation, attempt or claim token"
    );
    assert_eq!(unavailable_bytes, b"private namespace unavailable");
    assert!(original_key_preserved);
    assert!(
        replay_expected,
        "actual replay refusal: {replay_diagnostic:?}; actual begin refusal: {begin_diagnostic:?}"
    );
    assert!(begin_expected, "actual begin refusal: {begin_diagnostic:?}");
}

#[cfg(any(target_os = "linux", target_os = "macos", windows))]
fn private_namespace_bytes(home: &Path) -> Vec<(String, Vec<u8>)> {
    let mut files: Vec<_> = std::fs::read_dir(home.join("work-items/private-flow-inputs"))
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            (
                entry.file_name().into_string().unwrap(),
                std::fs::read(entry.path()).unwrap(),
            )
        })
        .collect();
    files.sort();
    files
}

#[cfg(any(target_os = "linux", target_os = "macos", windows))]
async fn accepted_private_replay_control(
    home: &Path,
    operation: WorkItemOperationId,
    private_request: bool,
) -> (OwnedFlowReceipt, String) {
    let storage = crate::runs::Storage::open(home).await.unwrap();
    let store = storage.work_items();
    let OwnedFlowPreparationResult::Preparing(mut preparation) = store
        .begin_owned_flow(
            operation,
            b"fixed private-backed body",
            crate::work_items::OwnedFlowPreparationPurpose::Ordinary(if private_request {
                crate::work_items::OrdinaryRequestPrivacy::Private
            } else {
                crate::work_items::OrdinaryRequestPrivacy::Public
            }),
            1,
        )
        .unwrap()
    else {
        panic!("new preparation required")
    };
    preparation
        .freeze_source(&source(home, &preparation), None)
        .unwrap();
    let server = surge_core::mcp_config::McpServerRef::new(
        "fixed-private-server".into(),
        surge_core::mcp_config::McpTransportConfig::stdio(
            "fixed-command".into(),
            vec![],
            Default::default(),
        ),
        None,
        std::time::Duration::from_secs(7),
        false,
    );
    preparation
        .freeze_startup(
            r#"{"mcp_servers":[]}"#,
            OwnedFlowMcpSelection::HostDefault,
            &[server],
        )
        .unwrap();
    preparation.retain_launch_ownership().unwrap();
    let (receipt, claim) = preparation.finalize(2).unwrap().into_parts();
    assert_eq!(
        store
            .hydrate_owned_flow_inputs(&claim)
            .unwrap()
            .unwrap()
            .servers()
            .len(),
        1
    );
    drop(claim);
    assert_eq!(
        store
            .replay_owned_flow(
                operation,
                b"fixed private-backed body",
                crate::work_items::OwnedFlowPreparationPurpose::Ordinary(if private_request {
                    crate::work_items::OrdinaryRequestPrivacy::Private
                } else {
                    crate::work_items::OrdinaryRequestPrivacy::Public
                })
            )
            .unwrap(),
        Some(receipt.clone())
    );
    let row: String = store.pool.get().unwrap().query_row(
        "SELECT json_object('identity',request_identity,'item',item,'run',run,'owner',workspace_owner,'source',source_snapshot,'config',config_snapshot,'inputs',inputs_manifest,'receipt',receipt,'state',state) FROM owned_flow_operations WHERE operation=?",
        [operation.to_string()], |r| r.get(0),
    ).unwrap();
    (receipt, row)
}

#[cfg(any(target_os = "linux", target_os = "macos", windows))]
async fn private_replay_replacement_case(private_request: bool) {
    let home = FixtureHome::new().unwrap();
    let operation = WorkItemOperationId::new();
    let (receipt, original_rows) =
        accepted_private_replay_control(home.path(), operation, private_request).await;
    let bytes = private_namespace_bytes(home.path());
    let root = home.path().join("work-items/private-flow-inputs");
    std::fs::rename(
        &root,
        home.path().join("retired-original-private-namespace"),
    )
    .unwrap();
    {
        let replacement = super::private_files::PrivateFiles::open(home.path(), true).unwrap();
        for (name, value) in &bytes {
            assert!(replacement.publish(name, value).unwrap());
        }
    }
    assert_eq!(
        private_namespace_bytes(home.path()),
        bytes,
        "replacement is fully valid byte-for-byte, not corrupt material"
    );
    let facts = {
        let storage = crate::runs::Storage::open(home.path()).await.unwrap();
        let store = storage.work_items();
        let replay = store.replay_owned_flow(
            operation,
            b"fixed private-backed body",
            crate::work_items::OwnedFlowPreparationPurpose::Ordinary(if private_request {
                crate::work_items::OrdinaryRequestPrivacy::Private
            } else {
                crate::work_items::OrdinaryRequestPrivacy::Public
            }),
        );
        let begin = store.begin_owned_flow(
            operation,
            b"fixed private-backed body",
            crate::work_items::OwnedFlowPreparationPurpose::Ordinary(if private_request {
                crate::work_items::OrdinaryRequestPrivacy::Private
            } else {
                crate::work_items::OrdinaryRequestPrivacy::Public
            }),
            99,
        );
        let refused = matches!(replay, Err(WorkItemError::PrivateInputsRecoveryRequired))
            && matches!(begin, Err(WorkItemError::PrivateInputsRecoveryRequired));
        let row: String = store.pool.get().unwrap().query_row(
            "SELECT json_object('identity',request_identity,'item',item,'run',run,'owner',workspace_owner,'source',source_snapshot,'config',config_snapshot,'inputs',inputs_manifest,'receipt',receipt,'state',state) FROM owned_flow_operations WHERE operation=?",
            [operation.to_string()], |r| r.get(0),
        ).unwrap();
        let count: i64 = store
            .pool
            .get()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM owned_flow_operations", [], |r| {
                r.get(0)
            })
            .unwrap();
        (
            refused,
            row == original_rows,
            count == 1,
            private_namespace_bytes(home.path()) == bytes,
            matches!(replay, Ok(Some(actual)) if actual == receipt),
        )
    };
    home.close().unwrap();
    assert!(
        facts.0 && facts.1 && facts.2 && facts.3,
        "cold native replacement accepted: refused={}, rows_unchanged={}, one_operation={}, bytes_unchanged={}, original_receipt_returned={}",
        facts.0,
        facts.1,
        facts.2,
        facts.3,
        facts.4
    );
}

#[cfg(any(target_os = "linux", target_os = "macos", windows))]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn established_private_request_cold_replay_refuses_valid_native_replacement() {
    private_replay_replacement_case(true).await;
}

#[cfg(any(target_os = "linux", target_os = "macos", windows))]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn established_public_request_private_mcp_cold_replay_refuses_valid_native_replacement() {
    private_replay_replacement_case(false).await;
}

#[cfg(any(target_os = "linux", target_os = "macos", windows))]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn established_healthy_original_private_namespace_cold_replay_control() {
    for private_request in [false, true] {
        let home = FixtureHome::new().unwrap();
        let operation = WorkItemOperationId::new();
        let (receipt, _) =
            accepted_private_replay_control(home.path(), operation, private_request).await;
        let facts = {
            let storage = crate::runs::Storage::open(home.path()).await.unwrap();
            let store = storage.work_items();
            let replay = store
                .replay_owned_flow(
                    operation,
                    b"fixed private-backed body",
                    crate::work_items::OwnedFlowPreparationPurpose::Ordinary(if private_request {
                        crate::work_items::OrdinaryRequestPrivacy::Private
                    } else {
                        crate::work_items::OrdinaryRequestPrivacy::Public
                    }),
                )
                .unwrap();
            let claim = store.claim(receipt.run).unwrap();
            let inputs = store.hydrate_owned_flow_inputs(&claim).unwrap().unwrap();
            (replay == Some(receipt), inputs.servers().len() == 1)
        };
        home.close().unwrap();
        assert!(
            facts.0 && facts.1,
            "healthy original cold owner/inputs changed"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn established_true_public_empty_mcp_cold_receipt_needs_no_private_namespace() {
    let home = FixtureHome::new().unwrap();
    let operation = WorkItemOperationId::new();
    let receipt = {
        let storage = crate::runs::Storage::open(home.path()).await.unwrap();
        let store = storage.work_items();
        let (receipt, claim) = accepted(&store, home.path(), operation);
        drop(claim);
        receipt
    };
    let facts = {
        let storage = crate::runs::Storage::open(home.path()).await.unwrap();
        let store = storage.work_items();
        let replay = store
            .replay_owned_flow(
                operation,
                b"original body",
                crate::work_items::OwnedFlowPreparationPurpose::Ordinary(
                    crate::work_items::OrdinaryRequestPrivacy::Public,
                ),
            )
            .unwrap();
        (
            replay == Some(receipt),
            !home.path().join("work-items/private-flow-inputs").exists(),
        )
    };
    home.close().unwrap();
    assert!(
        facts.0 && facts.1,
        "true public empty replay acquired a private dependency"
    );
}

pub(super) fn operation_snapshot(store: &WorkItemStore, operation: WorkItemOperationId) -> String {
    store.pool.get().unwrap().query_row("SELECT json_object('state',state,'identity',request_identity,'source',source_snapshot,'config',config_snapshot,'manifest',inputs_manifest,'receipt',receipt,'intent',intent_state) FROM owned_flow_operations WHERE operation=?", [operation.to_string()],|row|row.get(0)).unwrap()
}
