//! Real public Bootstrap and preparation APIs; structural storage is not source eligibility.
use super::*;
use crate::work_items::WorkItemStore;
use crate::{runs::Storage, runtime_home_fixture::FixtureHome};
use std::path::Path;
use surge_core::bootstrap_operation::{
    BootstrapCapture, BootstrapCaptureFields, BootstrapContentRef, BootstrapIntent,
};
use surge_core::budget::BudgetGuard;
use surge_core::{ContentHash, RunId, id::WorkItemOperationId, work_item::*};

async fn registered_bootstrap(
    storage: &std::sync::Arc<Storage>,
    home: &Path,
) -> (RunId, RunId, RunId) {
    let operation = RunId::new();
    let planning = RunId::new();
    let implementation = RunId::new();
    for run in [planning, implementation] {
        storage
            .create_run(run, home, None)
            .await
            .unwrap()
            .close()
            .await
            .unwrap();
    }
    register_bootstrap_runs(storage, home, operation, planning, implementation);
    (operation, planning, implementation)
}
fn register_bootstrap_runs(
    storage: &std::sync::Arc<Storage>,
    home: &Path,
    operation: RunId,
    planning: RunId,
    implementation: RunId,
) {
    let capture: BootstrapCapture = BootstrapCaptureFields {
        version: 1,
        planning_run: planning,
        implementation_run: implementation,
        repository: home.to_path_buf(),
        git_common_dir: home.to_path_buf(),
        base_commit: "a".repeat(40),
        planning_worktree: home.join("planning"),
        implementation_worktree: home.join("implementation"),
        planning_branch: "surge/planning".into(),
        implementation_branch: "surge/implementation".into(),
        graph: BootstrapContentRef {
            name: "bootstrap@1".into(),
            digest: ContentHash::compute(b"fixed graph"),
        },
        project_context: None,
        bootstrap_edit_loop_cap: None,
        runtime_identity: ContentHash::compute(b"fixed runtime"),
        configuration: vec![],
        credential_refs: vec![],
    }
    .try_into()
    .unwrap();
    let intent = BootstrapIntent::new(
        home.to_path_buf(),
        "fixed source request".into(),
        BudgetGuard::default(),
    )
    .unwrap();
    storage
        .bootstrap_operation_store()
        .insert_if_absent(operation, &intent, &capture)
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_mcp_continuation_reserves_original_private_requirement_and_actual_source_pins() {
    let home = FixtureHome::new().unwrap();
    let facts = {
        let storage = Storage::open(home.path()).await.unwrap();
        let store: WorkItemStore = storage.work_items();
        let (bootstrap, planning, implementation) =
            registered_bootstrap(&storage, home.path()).await;
        let operation = WorkItemOperationId::new();
        let required = ContinuationInputRequirement::new(bootstrap, implementation).unwrap();
        let result = store.begin_owned_flow(
            operation,
            b"fixed continuation body",
            OwnedFlowPreparationPurpose::Continuation(required),
            10,
        );
        match result {
            Ok(OwnedFlowPreparationResult::Preparing(preparation)) => {
                let owner_matches = preparation.workspace_owner() == implementation;
                let conn = storage.acquire_registry_conn().unwrap();
                let mut query = conn.prepare("SELECT source_run FROM owned_flow_source_pins WHERE operation=? ORDER BY source_run").unwrap();
                let pinned: Vec<String> = query
                    .query_map([operation.to_string()], |row| row.get(0))
                    .unwrap()
                    .collect::<Result<_, _>>()
                    .unwrap();
                let mut expected = vec![planning.to_string(), implementation.to_string()];
                expected.sort();
                let correct = pinned == expected && !pinned.contains(&bootstrap.to_string());
                drop(query);
                drop(conn);
                drop(preparation);
                let source_deletions = [planning, implementation];
                let mut sources_retained = true;
                for source in source_deletions {
                    sources_retained &= matches!(storage.delete_run(&source).await,
                        Err(crate::runs::StorageError::RunReferenced { run_id }) if run_id == source);
                }
                let retained: i64 = storage
                    .acquire_registry_conn()
                    .unwrap()
                    .query_row(
                        "SELECT COUNT(*) FROM owned_flow_source_pins WHERE operation=?",
                        [operation.to_string()],
                        |row| row.get(0),
                    )
                    .unwrap();
                (
                    owner_matches,
                    correct && retained == 2 && sources_retained,
                    home.path()
                        .join("work-items/private-flow-inputs/host-key-v1")
                        .exists(),
                    "preparing".into(),
                )
            },
            Err(error) => (false, false, false, format!("{error:?}")),
            Ok(OwnedFlowPreparationResult::Replay(_)) => {
                (false, false, false, "unexpected replay".into())
            },
        }
    };
    home.close().unwrap();
    assert!(
        facts.0 && facts.1 && facts.2,
        "private reservation/source pins must be durable before exposing identities; settled: {facts:?}"
    );
}

fn source_snapshot(home: &Path, owner: RunId) -> OwnedFlowSourceSnapshot {
    let graph = toml::from_str(include_str!(
        "../../../../../examples/flow_terminal_only.toml"
    ))
    .unwrap();
    OwnedFlowSourceSnapshot {
        contract: AcceptedFlowContract::new(Box::new(graph), "fixed successor prompt".into())
            .unwrap(),
        source_base: home.to_path_buf(),
        workspace: WorkItemWorkspace {
            repository: home.join("project/.git"),
            checkout: home.join("project"),
            path: home.join("source-workspace"),
            ownership: owner.to_string(),
            branch: "codex/source".into(),
            base_commit: "a".repeat(40),
        },
    }
}
fn seed_binding(
    operation: WorkItemOperationId,
    preparation: &OwnedFlowPreparation,
    source: ContinuationInputRequirement,
    snapshot: &OwnedFlowSourceSnapshot,
) -> SeedStorageBinding {
    use surge_core::execution_recovery::semantic_position::SourceEventRef;
    let boundary = SourceEventRef::new(source.source_run(), 7).unwrap();
    SeedStorageBinding::new(SeedStorageBindingFields {
        operation,
        run: preparation.run(),
        binding: WorkItemBinding {
            item: preparation.item(),
            revision: 1,
            requirements_hash: snapshot.contract.hash().unwrap(),
            generation: 1,
        },
        workspace_owner: preparation.workspace_owner(),
        workspace_identity: ContentHash::compute(b"fixed workspace correlation"),
        source,
        boundary,
        prefix_hash: ContentHash::compute(b"fixed prefix"),
        terminal: boundary,
        terminal_hash: ContentHash::compute(b"fixed terminal"),
        failure_tail: None,
        source_checkpoint: ContentHash::compute(b"fixed checkpoint"),
        graph_hash: ContentHash::compute(b"fixed registered graph"),
    })
    .unwrap()
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn private_seed_seal_readback_and_empty_startup_preserve_both_mcp_selections() {
    let mut failures = Vec::new();
    for selection in [
        OwnedFlowMcpSelection::Explicit,
        OwnedFlowMcpSelection::HostDefault,
    ] {
        let home = FixtureHome::new().unwrap();
        let facts = {
            let storage = Storage::open(home.path()).await.unwrap();
            let store = storage.work_items();
            let (bootstrap, _, implementation) = registered_bootstrap(&storage, home.path()).await;
            let required = ContinuationInputRequirement::new(bootstrap, implementation).unwrap();
            let operation = WorkItemOperationId::new();
            let result = store
                .begin_owned_flow(
                    operation,
                    b"fixed continuation body",
                    OwnedFlowPreparationPurpose::Continuation(required.clone()),
                    10,
                )
                .unwrap();
            let OwnedFlowPreparationResult::Preparing(mut preparation) = result else {
                panic!("first preparation")
            };
            let snapshot = source_snapshot(home.path(), implementation);
            preparation.freeze_source(&snapshot, None).unwrap();
            let binding = seed_binding(operation, &preparation, required.clone(), &snapshot);
            let result = preparation.freeze_seed_storage(&binding, b"PRIVATE_SEED_TEST_SENTINEL");
            match result {
                Ok(seed) => {
                    assert_eq!(seed.fields().operation, binding.operation());
                    assert_eq!(seed.fields().run, binding.run());
                    assert_eq!(&seed.fields().binding, binding.binding());
                    assert_eq!(seed.fields().source, binding.boundary());
                    let inputs =
                        preparation.freeze_startup(r#"{"mcp_servers":[]}"#, selection, &[]);
                    match inputs {
                        Ok(inputs) => (
                            inputs.schema_version() == 2
                                && inputs.mcp() == &FrozenOwnedFlowMcp::empty(selection)
                                && matches!(
                                    inputs.request_identity(),
                                    OwnedFlowRequestIdentity::Hmac { .. }
                                )
                                && inputs.purpose()
                                    == &OwnedFlowInputPurpose::Continuation { source: required },
                            "sealed private empty startup".into(),
                        ),
                        Err(error) => (false, format!("startup:{error:?}")),
                    }
                },
                Err(error) => (false, format!("seal:{error:?}")),
            }
        };
        home.close().unwrap();
        if !facts.0 {
            failures.push((selection, facts.1));
        }
    }
    assert!(
        failures.is_empty(),
        "actual seed seal/readback/empty startup failed after owner settlement: {failures:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sealed_continuation_generic_finalize_refuses_without_acceptance() {
    let home = FixtureHome::new().unwrap();
    let facts = {
        let storage = Storage::open(home.path()).await.unwrap();
        let store = storage.work_items();
        let (bootstrap, _, source) = registered_bootstrap(&storage, home.path()).await;
        let required = ContinuationInputRequirement::new(bootstrap, source).unwrap();
        let operation = WorkItemOperationId::new();
        let OwnedFlowPreparationResult::Preparing(mut preparation) = store
            .begin_owned_flow(
                operation,
                b"fixed continuation body",
                OwnedFlowPreparationPurpose::Continuation(required.clone()),
                10,
            )
            .unwrap()
        else {
            panic!("first preparation")
        };
        let snapshot = source_snapshot(home.path(), source);
        preparation.freeze_source(&snapshot, None).unwrap();
        let binding = seed_binding(operation, &preparation, required, &snapshot);
        preparation
            .freeze_seed_storage(&binding, b"PRIVATE_SEED_TEST_SENTINEL")
            .unwrap();
        preparation
            .freeze_startup(
                r#"{"mcp_servers":[]}"#,
                OwnedFlowMcpSelection::Explicit,
                &[],
            )
            .unwrap();
        preparation.retain_launch_ownership().unwrap();
        let result = preparation.finalize(11);
        let refusal = matches!(
            &result,
            Err(crate::work_items::WorkItemError::ContinuationExecutionUnavailable)
        );
        let diagnostic = match result {
            Ok(accepted) => {
                drop(accepted);
                "accepted continuation".to_owned()
            },
            Err(error) => format!("{error:?}"),
        };
        let conn = storage.acquire_registry_conn().unwrap();
        let accepted:i64=conn.query_row("SELECT COUNT(*) FROM owned_flow_operations WHERE operation=? AND receipt IS NOT NULL",[operation.to_string()],|row|row.get(0)).unwrap();
        let attempts: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM work_item_attempts WHERE run=?",
                [binding.run().to_string()],
                |row| row.get(0),
            )
            .unwrap();
        (refusal, accepted, attempts, diagnostic)
    };
    home.close().unwrap();
    assert!(
        facts.0,
        "generic execution must refuse sealed continuation after actual owner settlement: {facts:?}"
    );
    assert_eq!(
        (facts.1, facts.2),
        (0, 0),
        "no accepted receipt or attempt may be published"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sealed_preparing_continuation_generic_inspection_never_erases_required_purpose() {
    let home = FixtureHome::new().unwrap();
    let facts = {
        let storage = Storage::open(home.path()).await.unwrap();
        let store = storage.work_items();
        let (bootstrap, _, source) = registered_bootstrap(&storage, home.path()).await;
        let required = ContinuationInputRequirement::new(bootstrap, source).unwrap();
        let operation = WorkItemOperationId::new();
        let OwnedFlowPreparationResult::Preparing(mut preparation) = store
            .begin_owned_flow(
                operation,
                b"fixed continuation body",
                OwnedFlowPreparationPurpose::Continuation(required.clone()),
                10,
            )
            .unwrap()
        else {
            panic!("first preparation")
        };
        let snapshot = source_snapshot(home.path(), source);
        preparation.freeze_source(&snapshot, None).unwrap();
        let binding = seed_binding(operation, &preparation, required, &snapshot);
        preparation
            .freeze_seed_storage(&binding, b"PRIVATE_SEED_TEST_SENTINEL")
            .unwrap();
        let run = preparation.run();
        let manifest = store.owned_flow_manifest(run);
        let pending = store.has_pending_owned_flow(run);
        let wake = store.claim_owned_flow_quota_wake(run, 11);
        let cases = [
            (
                "manifest",
                format!("{manifest:?}"),
                matches!(
                    manifest,
                    Err(crate::work_items::WorkItemError::ContinuationExecutionUnavailable)
                ),
            ),
            (
                "pending",
                format!("{pending:?}"),
                matches!(
                    pending,
                    Err(crate::work_items::WorkItemError::ContinuationExecutionUnavailable)
                ),
            ),
            (
                "wake",
                match &wake {
                    Err(error) => format!("{error:?}"),
                    Ok(_) => "non-refused admission".into(),
                },
                matches!(
                    wake,
                    Err(crate::work_items::WorkItemError::ContinuationExecutionUnavailable)
                ),
            ),
        ];
        drop(preparation);
        cases
    };
    home.close().unwrap();
    assert!(
        facts.iter().all(|case| case.2),
        "independent stored requirement must survive omitted startup metadata: {facts:?}"
    );
}

struct ColdSeed {
    operation: WorkItemOperationId,
    required: ContinuationInputRequirement,
    binding: SeedStorageBinding,
    manifest: ContinuationSeedManifest,
    reference: String,
    bytes: Vec<u8>,
}
async fn prepare_cold_seed(home: &Path) -> ColdSeed {
    let storage = Storage::open(home).await.unwrap();
    let store = storage.work_items();
    let (bootstrap, _, source) = registered_bootstrap(&storage, home).await;
    let required = ContinuationInputRequirement::new(bootstrap, source).unwrap();
    let operation = WorkItemOperationId::new();
    let OwnedFlowPreparationResult::Preparing(mut preparation) = store
        .begin_owned_flow(
            operation,
            b"fixed continuation body",
            OwnedFlowPreparationPurpose::Continuation(required.clone()),
            10,
        )
        .unwrap()
    else {
        panic!("first preparation")
    };
    let snapshot = source_snapshot(home, source);
    preparation.freeze_source(&snapshot, None).unwrap();
    let binding = seed_binding(operation, &preparation, required.clone(), &snapshot);
    let manifest = preparation
        .freeze_seed_storage(&binding, b"PRIVATE_SEED_TEST_SENTINEL")
        .unwrap();
    let reference: String = storage
        .acquire_registry_conn()
        .unwrap()
        .query_row(
            "SELECT pending_ref FROM owned_flow_continuation_requirements WHERE operation=?",
            [operation.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    let bytes = std::fs::read(home.join(format!(
        "work-items/private-flow-inputs/seed-{reference}.json"
    )))
    .unwrap();
    ColdSeed {
        operation,
        required,
        binding,
        manifest,
        reference,
        bytes,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn private_seed_cold_retry_retains_reference_and_loss_never_republishes() {
    let home = FixtureHome::new().unwrap();
    let ColdSeed {
        operation,
        required,
        binding,
        manifest,
        reference,
        bytes,
    } = prepare_cold_seed(home.path()).await;
    let retry_facts = {
        let storage = Storage::open(home.path()).await.unwrap();
        let store = storage.work_items();
        let OwnedFlowPreparationResult::Preparing(mut preparation) = store
            .begin_owned_flow(
                operation,
                b"fixed continuation body",
                OwnedFlowPreparationPurpose::Continuation(required.clone()),
                12,
            )
            .unwrap()
        else {
            panic!("cold preparing")
        };
        let retried = preparation
            .freeze_seed_storage(&binding, b"PRIVATE_SEED_TEST_SENTINEL")
            .unwrap();
        let changed = preparation.freeze_seed_storage(&binding, b"different seed");
        let stable = std::fs::read(home.path().join(format!(
            "work-items/private-flow-inputs/seed-{reference}.json"
        )))
        .unwrap()
            == bytes;
        (
            retried == manifest,
            matches!(changed, Err(crate::work_items::WorkItemError::Conflict(_))),
            stable,
        )
    };
    let object = home.path().join(format!(
        "work-items/private-flow-inputs/seed-{reference}.json"
    ));
    std::fs::remove_file(&object).unwrap();
    let loss_facts = {
        let storage = Storage::open(home.path()).await.unwrap();
        let store = storage.work_items();
        let result = store.begin_owned_flow(
            operation,
            b"fixed continuation body",
            OwnedFlowPreparationPurpose::Continuation(required),
            14,
        );
        let refusal = matches!(
            &result,
            Err(crate::work_items::WorkItemError::PrivateInputsRecoveryRequired)
        );
        drop(result);
        let conn = storage.acquire_registry_conn().unwrap();
        let retained:(String,i64)=conn.query_row("SELECT pending_ref,(SELECT COUNT(*) FROM owned_flow_private_object_pins WHERE operation=?) FROM owned_flow_continuation_requirements WHERE operation=?",
            rusqlite::params![operation.to_string(),operation.to_string()],|row|Ok((row.get(0)?,row.get(1)?))).unwrap();
        (refusal, !object.exists(), retained == (reference, 1))
    };
    home.close().unwrap();
    assert_eq!(
        retry_facts,
        (true, true, true),
        "cold immutable retry after all owners settled"
    );
    assert_eq!(
        loss_facts,
        (true, true, true),
        "lost bound object must refuse without new ref/object/pin"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_hydration_checks_independent_purpose_before_empty_or_replaced_manifest() {
    let home = FixtureHome::new().unwrap();
    let facts = {
        let storage = Storage::open(home.path()).await.unwrap();
        let store = storage.work_items();
        let (bootstrap, _, source) = registered_bootstrap(&storage, home.path()).await;
        let required = ContinuationInputRequirement::new(bootstrap, source).unwrap();
        let operation = WorkItemOperationId::new();
        let OwnedFlowPreparationResult::Preparing(mut preparation) = store
            .begin_owned_flow(
                operation,
                b"fixed continuation body",
                OwnedFlowPreparationPurpose::Continuation(required.clone()),
                10,
            )
            .unwrap()
        else {
            panic!("first preparation")
        };
        let snapshot = source_snapshot(home.path(), source);
        preparation.freeze_source(&snapshot, None).unwrap();
        let binding = seed_binding(operation, &preparation, required, &snapshot);
        preparation
            .freeze_seed_storage(&binding, b"PRIVATE_SEED_TEST_SENTINEL")
            .unwrap();
        let manifest = preparation
            .freeze_startup(
                r#"{"mcp_servers":[]}"#,
                OwnedFlowMcpSelection::Explicit,
                &[],
            )
            .unwrap();
        let replaced = OwnedFlowInputsManifest::new(
            operation,
            OwnedFlowRequestIdentity::PlainPublic {
                digest: ContentHash::compute(b"fixed continuation body"),
            },
            binding.binding().clone(),
            binding.run(),
            binding.workspace_owner(),
            snapshot.workspace.clone(),
            FrozenOwnedFlowMcp::empty(OwnedFlowMcpSelection::Explicit),
        )
        .unwrap();
        [manifest, replaced].map(|value| {
            let result = private_inputs::hydrate(&store, &value);
            let refusal = matches!(
                &result,
                Err(crate::work_items::WorkItemError::ContinuationExecutionUnavailable)
            );
            (
                refusal,
                match result {
                    Err(error) => format!("{error:?}"),
                    Ok(_) => "hydrated continuation".into(),
                },
            )
        })
    };
    home.close().unwrap();
    assert!(
        facts.iter().all(|case| case.0),
        "generic hydration may not erase independent required storage: {facts:?}"
    );
}

// Private SQL fixture only: storage acceptance is deliberately unavailable in production.
fn accepted_storage_fixture(
    store: &WorkItemStore,
    mut preparation: Box<OwnedFlowPreparation>,
    snapshot: &OwnedFlowSourceSnapshot,
    inputs: &OwnedFlowInputsManifest,
) -> crate::work_items::WorkItemLaunchClaim {
    use crate::work_items::start_preparation::secure_lock::{PreparationLock, PreparationLockKey};
    use std::sync::Arc;
    preparation.retain_launch_ownership().unwrap();
    let run = preparation.run();
    let operation = inputs.operation();
    let owner = private_establishment::open(store, None, None).unwrap();
    let mut conn = store.pool.get().unwrap();
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .unwrap();
    let project = surge_core::id::WorkItemProjectId::new();
    tx.execute(
        "INSERT INTO work_item_projects(id,repository,checkout) VALUES(?,?,?)",
        rusqlite::params![
            project.to_string(),
            snapshot.workspace.repository.to_string_lossy(),
            snapshot.workspace.checkout.to_string_lossy()
        ],
    )
    .unwrap();
    tx.execute("INSERT INTO work_items(id,project_id,title,accepted_revision,version,workspace,workspace_prepared) VALUES(?,?,?,1,1,?,1)",
        rusqlite::params![preparation.item().to_string(),project.to_string(),"fixed storage fixture",serde_json::to_string(&snapshot.workspace).unwrap()]).unwrap();
    let revision = WorkItemRevision {
        revision: 1,
        hash: inputs.binding().requirements_hash,
        origin: AcceptedWorkItemOrigin::Flow(snapshot.contract.clone()),
        actor: "host-fixture".into(),
        accepted_proposal: None,
        accepted_at_ms: 11,
    };
    tx.execute(
        "INSERT INTO work_item_revisions(item,revision,payload) VALUES(?,1,?)",
        rusqlite::params![
            preparation.item().to_string(),
            serde_json::to_string(&revision).unwrap()
        ],
    )
    .unwrap();
    let config: String = tx
        .query_row(
            "SELECT config_snapshot FROM owned_flow_operations WHERE operation=?",
            [operation.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    store
        .reserve_with_host_run(
            &tx,
            preparation.item(),
            1,
            snapshot.contract.graph(),
            &config,
            run,
        )
        .unwrap();
    let receipt = OwnedFlowReceipt {
        operation_id: operation,
        item: preparation.item(),
        run,
        binding: inputs.binding().clone(),
        workspace_owner: preparation.workspace_owner(),
        accepted_at_ms: 11,
    };
    let token = RunId::new().to_string();
    tx.execute(
        "UPDATE work_item_attempts SET claim_token=? WHERE run=?",
        rusqlite::params![token, run.to_string()],
    )
    .unwrap();
    tx.execute("UPDATE owned_flow_operations SET state='accepted',receipt=?,intent_state='pending' WHERE operation=?",
        rusqlite::params![serde_json::to_string(&receipt).unwrap(),operation.to_string()]).unwrap();
    tx.commit().unwrap();
    drop(conn);
    drop(preparation);
    let guard = Arc::new(
        PreparationLock::acquire_stable(
            &store.home,
            PreparationLockKey::FlowLaunch { operation, run },
        )
        .unwrap(),
    );
    let identity = guard.identity().unwrap();
    crate::work_items::WorkItemLaunchClaim {
        lock: crate::work_items::LaunchLock::OwnedFlow {
            guard,
            operation,
            identity,
            private_owner: Some(owner),
        },
        run,
        token,
        binding: inputs.binding().clone(),
    }
}
fn refusal_case<T>(
    label: &'static str,
    result: crate::work_items::Result<T>,
) -> (&'static str, bool, String) {
    let refused = matches!(
        &result,
        Err(crate::work_items::WorkItemError::ContinuationExecutionUnavailable)
    );
    (
        label,
        refused,
        match result {
            Err(error) => format!("{error:?}"),
            Ok(_) => "returned generic permission".into(),
        },
    )
}
fn generic_fixture_results(
    store: &WorkItemStore,
    claim: &crate::work_items::WorkItemLaunchClaim,
    inputs: &OwnedFlowInputsManifest,
) -> Vec<(&'static str, bool, String)> {
    let observed = crate::work_items::attempt(&store.pool.get().unwrap(), claim.run).unwrap();
    vec![
        refusal_case("claim", store.claim(claim.run)),
        refusal_case(
            "reconciliation",
            store.claim_reconciliation(&observed, None),
        ),
        refusal_case("wake", store.claim_owned_flow_quota_wake(claim.run, 12)),
        refusal_case("pending", store.validate_owned_flow_pending(claim)),
        refusal_case("effect", store.validate_owned_flow_effect(claim)),
        refusal_case("manifest", store.owned_flow_manifest(claim.run)),
        refusal_case("hydrate", store.hydrate_owned_flow_inputs(claim)),
        refusal_case("ack", store.acknowledge_owned_flow_startup(claim, inputs)),
        refusal_case("validate", store.validate_claim(claim)),
    ]
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn private_accepted_storage_fixture_refuses_every_generic_execution_even_without_marker() {
    let mut results = Vec::new();
    for shape in 0..3 {
        let home = FixtureHome::new().unwrap();
        let facts = accepted_fixture_shape(home.path(), shape, false).await;
        home.close().unwrap();
        results.push(facts);
    }
    assert!(
        results
            .iter()
            .all(|facts| facts.0.iter().all(|case| case.1)),
        "private storage fixture is not generic authority: {results:?}"
    );
    assert!(
        results.iter().all(|facts| facts.1),
        "all refused paths preserve fixture claimant token"
    );
}
async fn accepted_fixture_shape(
    home: &Path,
    shape: u8,
    bounded: bool,
) -> (Vec<(&'static str, bool, String)>, bool) {
    let storage = Storage::open(home).await.unwrap();
    let store = if bounded {
        private_inputs::bounded_private_store(&storage, home)
    } else {
        storage.work_items()
    };
    let (bootstrap, _, source) = registered_bootstrap(&storage, home).await;
    let required = ContinuationInputRequirement::new(bootstrap, source).unwrap();
    let operation = WorkItemOperationId::new();
    let OwnedFlowPreparationResult::Preparing(mut preparation) = store
        .begin_owned_flow(
            operation,
            b"fixed continuation body",
            OwnedFlowPreparationPurpose::Continuation(required.clone()),
            10,
        )
        .unwrap()
    else {
        panic!("first preparation")
    };
    let snapshot = source_snapshot(home, source);
    preparation.freeze_source(&snapshot, None).unwrap();
    let binding = seed_binding(operation, &preparation, required.clone(), &snapshot);
    preparation
        .freeze_seed_storage(&binding, b"PRIVATE_SEED_TEST_SENTINEL")
        .unwrap();
    let identity: String = store
        .pool
        .get()
        .unwrap()
        .query_row(
            "SELECT request_identity FROM owned_flow_operations WHERE operation=?",
            [operation.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    let OwnedFlowRequestIdentity::Hmac { tag } = serde_json::from_str(&identity).unwrap() else {
        panic!("required HMAC")
    };
    let inputs = OwnedFlowInputsManifest::new_continuation(ContinuationInputsFields {
        operation,
        request_auth_tag: tag,
        binding: binding.binding().clone(),
        run: binding.run(),
        workspace_owner: binding.workspace_owner(),
        workspace: snapshot.workspace.clone(),
        mcp: FrozenOwnedFlowMcp::empty(OwnedFlowMcpSelection::Explicit),
        source: required,
    })
    .unwrap();
    let ordinary = OwnedFlowInputsManifest::new(
        operation,
        OwnedFlowRequestIdentity::PlainPublic {
            digest: ContentHash::compute(b"fixed continuation body"),
        },
        binding.binding().clone(),
        binding.run(),
        binding.workspace_owner(),
        snapshot.workspace.clone(),
        FrozenOwnedFlowMcp::empty(OwnedFlowMcpSelection::Explicit),
    )
    .unwrap();
    let manifest = match shape {
        0 => Some(serde_json::to_string(&inputs).unwrap()),
        1 => Some(serde_json::to_string(&ordinary).unwrap()),
        _ => None,
    };
    // First snapshot is inserted by the private corruption fixture, not rewritten past immutable triggers.
    store.pool.get().unwrap().execute("UPDATE owned_flow_operations SET config_snapshot=?,inputs_manifest=? WHERE operation=?",
        rusqlite::params![r#"{"mcp_servers":[]}"#,manifest,operation.to_string()]).unwrap();
    let claim = accepted_storage_fixture(&store, preparation, &snapshot, &inputs);
    let held = bounded.then(|| store.pool.get().unwrap());
    let results = generic_fixture_results(&store, &claim, &inputs);
    drop(held);
    let token: String = store
        .pool
        .get()
        .unwrap()
        .query_row(
            "SELECT claim_token FROM work_item_attempts WHERE run=?",
            [claim.run.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    let unchanged = token == claim.token;
    drop(claim);
    (results, unchanged)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bounded_continuation_generic_claim_validation_does_not_reborrow_held_transaction_pool() {
    let home = FixtureHome::new().unwrap();
    let facts = accepted_fixture_shape(home.path(), 0, true).await;
    home.close().unwrap();
    assert!(
        facts.0.iter().all(|case| case.1),
        "max2/held1 public continuation refusals after owner settlement: {:?}",
        facts.0
    );
    assert!(facts.1);
}

pub(super) fn reservation_boundary(
    operation: WorkItemOperationId,
) -> crate::work_items::Result<()> {
    use std::io::Read;
    if std::env::var("SURGE_TEST_PIN_OPERATION").ok().as_deref()
        != Some(operation.to_string().as_str())
    {
        return Ok(());
    }
    let marker = std::path::PathBuf::from(std::env::var_os("SURGE_TEST_PIN_MARKER").unwrap());
    let staging = marker.with_extension("publishing");
    std::fs::write(&staging, b"pins-before-commit")?;
    std::fs::rename(staging, marker)?;
    std::io::stdin().read_exact(&mut [0_u8])?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pin_reservation_worker_process() {
    let Some(home) = std::env::var_os("SURGE_TEST_PIN_HOME") else {
        return;
    };
    let storage = Storage::open(&home).await.unwrap();
    let bootstrap = std::env::var("SURGE_TEST_PIN_BOOTSTRAP")
        .unwrap()
        .parse()
        .unwrap();
    let source = std::env::var("SURGE_TEST_PIN_SOURCE")
        .unwrap()
        .parse()
        .unwrap();
    let operation = std::env::var("SURGE_TEST_PIN_OPERATION")
        .unwrap()
        .parse()
        .unwrap();
    let result = storage.work_items().begin_owned_flow(
        operation,
        b"racing source pins",
        OwnedFlowPreparationPurpose::Continuation(
            ContinuationInputRequirement::new(bootstrap, source).unwrap(),
        ),
        10,
    );
    assert!(matches!(
        result,
        Ok(OwnedFlowPreparationResult::Preparing(_))
    ));
}

struct PinChild(std::process::Child);
impl Drop for PinChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn paused_pin(
    home: &Path,
    bootstrap: RunId,
    source: RunId,
    operation: WorkItemOperationId,
) -> PinChild {
    let marker = home.join("pin-boundary");
    let child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "work_items::owned_flow::continuation_tests::pin_reservation_worker_process",
            "--nocapture",
        ])
        .env("SURGE_TEST_PIN_HOME", home)
        .env("SURGE_TEST_PIN_BOOTSTRAP", bootstrap.to_string())
        .env("SURGE_TEST_PIN_SOURCE", source.to_string())
        .env("SURGE_TEST_PIN_OPERATION", operation.to_string())
        .env("SURGE_TEST_PIN_MARKER", &marker)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::inherit())
        .spawn()
        .unwrap();
    let mut child = PinChild(child);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if std::fs::read(&marker).is_ok_and(|bytes| bytes == b"pins-before-commit") {
            return child;
        }
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "pin child exited before complete publication"
        );
        assert!(
            std::time::Instant::now() < deadline,
            "pin transaction did not reach boundary"
        );
        std::thread::yield_now();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_pin_and_delete_transactions_serialize_commit_and_rollback_without_partial_pins() {
    use std::io::Write;
    let mut facts = Vec::new();
    for commit in [false, true] {
        let home = FixtureHome::new().unwrap();
        let result = {
            let storage = Storage::open(home.path()).await.unwrap();
            let (bootstrap, _, source) = registered_bootstrap(&storage, home.path()).await;
            let operation = WorkItemOperationId::new();
            let mut child = paused_pin(home.path(), bootstrap, source, operation);
            let independent =
                rusqlite::Connection::open(home.path().join("db/registry.sqlite")).unwrap();
            independent.busy_timeout(std::time::Duration::ZERO).unwrap();
            let blocked = matches!(independent.execute_batch("BEGIN IMMEDIATE"),
                Err(rusqlite::Error::SqliteFailure(error, _)) if error.code == rusqlite::ErrorCode::DatabaseBusy);
            drop(independent);
            let deleting = storage.clone();
            let delete = tokio::spawn(async move { deleting.delete_run(&source).await });
            if commit {
                child.0.stdin.as_mut().unwrap().write_all(b"c").unwrap();
            } else {
                child.0.kill().unwrap();
            }
            let status = child.0.wait().unwrap();
            drop(child);
            let deletion = delete.await.unwrap();
            let pins: i64 = storage
                .acquire_registry_conn()
                .unwrap()
                .query_row(
                    "SELECT COUNT(*) FROM owned_flow_source_pins WHERE operation=?",
                    [operation.to_string()],
                    |row| row.get(0),
                )
                .unwrap();
            let correct = if commit {
                status.success()
                    && pins == 2
                    && matches!(deletion,
                    Err(crate::runs::StorageError::RunReferenced { run_id }) if run_id == source)
            } else {
                pins == 0 && deletion.is_ok()
            };
            (blocked, correct)
        };
        home.close().unwrap();
        facts.push((commit, result));
    }
    assert_eq!(facts, [(false, (true, true)), (true, (true, true))]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn committed_source_tombstone_refuses_new_actual_preparation_without_partial_pins() {
    let home = FixtureHome::new().unwrap();
    let facts = {
        let storage = Storage::open(home.path()).await.unwrap();
        let (bootstrap, _, source) = registered_bootstrap(&storage, home.path()).await;
        storage.delete_run(&source).await.unwrap();
        let operation = WorkItemOperationId::new();
        let result = storage.work_items().begin_owned_flow(
            operation,
            b"deleted source",
            OwnedFlowPreparationPurpose::Continuation(
                ContinuationInputRequirement::new(bootstrap, source).unwrap(),
            ),
            10,
        );
        let refused = matches!(result, Err(crate::work_items::WorkItemError::NotFound));
        let conn = storage.acquire_registry_conn().unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM owned_flow_operations WHERE operation=?",
                [operation.to_string()],
                |r| r.get(0),
            )
            .unwrap();
        let pins: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM owned_flow_source_pins WHERE operation=?",
                [operation.to_string()],
                |r| r.get(0),
            )
            .unwrap();
        (refused, count, pins)
    };
    home.close().unwrap();
    assert_eq!(facts, (true, 0, 0));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cold_seed_faults_preserve_original_requirement_refs_and_source_pins() {
    let mut facts = Vec::new();
    for fault in [
        "namespace",
        "key",
        "marker",
        "missing",
        "payload",
        "mac",
        "noncanonical",
        "foreign",
        "oversize",
    ] {
        let home = FixtureHome::new().unwrap();
        let fixture = prepare_cold_seed(home.path()).await;
        let root = home.path().join("work-items/private-flow-inputs");
        let object = root.join(format!("seed-{}.json", fixture.reference));
        match fault {
            "namespace" => {
                std::fs::rename(&root, home.path().join("original-private-namespace")).unwrap()
            },
            "key" => std::fs::rename(root.join("host-key-v1"), root.join("original-key")).unwrap(),
            "marker" => std::fs::rename(
                root.join("host-key-established-v1"),
                root.join("original-marker"),
            )
            .unwrap(),
            "missing" => std::fs::remove_file(&object).unwrap(),
            "noncanonical" => {
                let mut bytes = fixture.bytes.clone();
                bytes.push(b'\n');
                std::fs::write(&object, bytes).unwrap();
            },
            "foreign" => {
                std::fs::write(&object, include_bytes!("seed_objects/fixed-envelope.json")).unwrap()
            },
            "oversize" => std::fs::write(&object, vec![b' '; 8 * 1024 * 1024 + 1]).unwrap(),
            "payload" | "mac" => {
                let mut value: serde_json::Value = serde_json::from_slice(&fixture.bytes).unwrap();
                if fault == "payload" {
                    value["payload"][0] = serde_json::json!(0);
                } else {
                    value["auth_tag"] =
                        serde_json::json!(format!("hmac-sha256:{}", "0".repeat(64)));
                }
                std::fs::write(&object, super::private_inputs::canonical(&value).unwrap()).unwrap();
            },
            _ => unreachable!(),
        }
        let remaining = object.exists().then(|| std::fs::read(&object).unwrap());
        let result = {
            let storage = Storage::open(home.path()).await.unwrap();
            let conn = storage.acquire_registry_conn().unwrap();
            let before: (String,String,i64,i64) = conn.query_row(
                "SELECT pending_ref,seed_manifest,(SELECT COUNT(*) FROM owned_flow_source_pins WHERE operation=?),(SELECT COUNT(*) FROM owned_flow_private_object_pins WHERE operation=?) FROM owned_flow_continuation_requirements WHERE operation=?",
                rusqlite::params![fixture.operation.to_string(),fixture.operation.to_string(),fixture.operation.to_string()], |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).unwrap();
            drop(conn);
            let replay = storage.work_items().begin_owned_flow(
                fixture.operation,
                b"fixed continuation body",
                OwnedFlowPreparationPurpose::Continuation(fixture.required),
                20,
            );
            let refused = match fault {
                "payload" | "mac" | "noncanonical" | "foreign" => matches!(
                    replay,
                    Err(crate::work_items::WorkItemError::PrivateInputsCorrupt)
                ),
                "oversize" => matches!(
                    replay,
                    Err(crate::work_items::WorkItemError::PrivateInputsUnsafe)
                ),
                _ => matches!(
                    replay,
                    Err(crate::work_items::WorkItemError::PrivateInputsRecoveryRequired)
                ),
            };
            let after: (String,String,i64,i64) = storage.acquire_registry_conn().unwrap().query_row(
                "SELECT pending_ref,seed_manifest,(SELECT COUNT(*) FROM owned_flow_source_pins WHERE operation=?),(SELECT COUNT(*) FROM owned_flow_private_object_pins WHERE operation=?) FROM owned_flow_continuation_requirements WHERE operation=?",
                rusqlite::params![fixture.operation.to_string(),fixture.operation.to_string(),fixture.operation.to_string()], |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).unwrap();
            (
                refused,
                before == after && after.2 == 2 && after.3 == 1,
                remaining == object.exists().then(|| std::fs::read(&object).unwrap()),
            )
        };
        home.close().unwrap();
        facts.push((fault, result));
    }
    assert!(
        facts
            .iter()
            .all(|(_, result)| *result == (true, true, true)),
        "settled cold faults: {facts:?}"
    );
}

pub(super) fn seed_boundary(
    operation: WorkItemOperationId,
    phase: &str,
) -> crate::work_items::Result<()> {
    use std::io::Read;
    if std::env::var("SURGE_TEST_SEED_OPERATION").ok().as_deref()
        != Some(operation.to_string().as_str())
        || std::env::var("SURGE_TEST_SEED_PHASE").ok().as_deref() != Some(phase)
    {
        return Ok(());
    }
    let marker = std::path::PathBuf::from(std::env::var_os("SURGE_TEST_SEED_MARKER").unwrap());
    let staging = marker.with_extension("publishing");
    std::fs::write(&staging, phase.as_bytes())?;
    std::fs::rename(staging, marker)?;
    std::io::stdin().read_exact(&mut [0_u8])?;
    Ok(())
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn seed_storage_worker_process() {
    let Some(home) = std::env::var_os("SURGE_TEST_SEED_HOME") else {
        return;
    };
    let storage = Storage::open(&home).await.unwrap();
    let bootstrap = std::env::var("SURGE_TEST_SEED_BOOTSTRAP")
        .unwrap()
        .parse()
        .unwrap();
    let source = std::env::var("SURGE_TEST_SEED_SOURCE")
        .unwrap()
        .parse()
        .unwrap();
    let operation = std::env::var("SURGE_TEST_SEED_OPERATION")
        .unwrap()
        .parse()
        .unwrap();
    let required = ContinuationInputRequirement::new(bootstrap, source).unwrap();
    let OwnedFlowPreparationResult::Preparing(mut preparation) = storage
        .work_items()
        .begin_owned_flow(
            operation,
            b"crash-bound seed",
            OwnedFlowPreparationPurpose::Continuation(required.clone()),
            10,
        )
        .unwrap()
    else {
        panic!("first preparing");
    };
    let snapshot = source_snapshot(Path::new(&home), source);
    preparation.freeze_source(&snapshot, None).unwrap();
    let binding = seed_binding(operation, &preparation, required, &snapshot);
    preparation
        .freeze_seed_storage(&binding, b"fixed crash-bound payload")
        .unwrap();
}
fn paused_seed(
    home: &Path,
    bootstrap: RunId,
    source: RunId,
    operation: WorkItemOperationId,
    phase: &str,
) -> PinChild {
    let marker = home.join("seed-boundary");
    let child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "work_items::owned_flow::continuation_tests::seed_storage_worker_process",
            "--nocapture",
        ])
        .env("SURGE_TEST_SEED_HOME", home)
        .env("SURGE_TEST_SEED_BOOTSTRAP", bootstrap.to_string())
        .env("SURGE_TEST_SEED_SOURCE", source.to_string())
        .env("SURGE_TEST_SEED_OPERATION", operation.to_string())
        .env("SURGE_TEST_SEED_PHASE", phase)
        .env("SURGE_TEST_SEED_MARKER", &marker)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::inherit())
        .spawn()
        .unwrap();
    let mut child = PinChild(child);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if std::fs::read(&marker).is_ok_and(|bytes| bytes == phase.as_bytes()) {
            return child;
        }
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "seed producer exited before complete marker"
        );
        assert!(
            std::time::Instant::now() < deadline,
            "seed producer did not reach boundary"
        );
        std::thread::yield_now();
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_seed_producer_death_preserves_pending_refs_and_only_completes_published_objects() {
    let mut facts = Vec::new();
    for phase in ["before-pin", "pending", "published"] {
        let home = FixtureHome::new().unwrap();
        let operation = WorkItemOperationId::new();
        let (bootstrap, _, source) = {
            let storage = Storage::open(home.path()).await.unwrap();
            registered_bootstrap(&storage, home.path()).await
        };
        let mut child = paused_seed(home.path(), bootstrap, source, operation, phase);
        child.0.kill().unwrap();
        child.0.wait().unwrap();
        drop(child);
        let result = {
            let storage = Storage::open(home.path()).await.unwrap();
            let before: Option<String> = storage.acquire_registry_conn().unwrap().query_row(
                "SELECT pending_ref FROM owned_flow_continuation_requirements WHERE operation=?", [operation.to_string()], |r|r.get(0)).unwrap();
            let required = ContinuationInputRequirement::new(bootstrap, source).unwrap();
            let resumed = storage.work_items().begin_owned_flow(
                operation,
                b"crash-bound seed",
                OwnedFlowPreparationPurpose::Continuation(required.clone()),
                20,
            );
            let correct = if phase == "pending" {
                matches!(
                    resumed,
                    Err(crate::work_items::WorkItemError::PrivateInputsRecoveryRequired)
                )
            } else if let Ok(OwnedFlowPreparationResult::Preparing(mut preparation)) = resumed {
                let snapshot = source_snapshot(home.path(), source);
                let binding = seed_binding(operation, &preparation, required, &snapshot);
                preparation
                    .freeze_seed_storage(&binding, b"fixed crash-bound payload")
                    .is_ok()
            } else {
                false
            };
            let after: (Option<String>,Option<String>,i64) = storage.acquire_registry_conn().unwrap().query_row(
                "SELECT pending_ref,seed_manifest,(SELECT COUNT(*) FROM owned_flow_private_object_pins WHERE operation=?) FROM owned_flow_continuation_requirements WHERE operation=?",
                rusqlite::params![operation.to_string(), operation.to_string()], |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
            correct
                && after.2 == 1
                && (phase == "before-pin" || before == after.0)
                && (phase == "pending") == after.1.is_none()
        };
        home.close().unwrap();
        facts.push((phase, result));
    }
    assert_eq!(
        facts,
        [("before-pin", true), ("pending", true), ("published", true)]
    );
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_legacy_source_pins_precede_unavailable_deletion_coordination_and_preserve_history()
{
    let home = FixtureHome::new().unwrap();
    let planning = RunId::new();
    let source = RunId::new();
    for run in [planning, source] {
        crate::runs::storage::legacy_fixture(home.path(), run);
    }
    let facts = {
        let storage = Storage::open(home.path()).await.unwrap();
        let bootstrap = RunId::new();
        register_bootstrap_runs(&storage, home.path(), bootstrap, planning, source);
        let operation = WorkItemOperationId::new();
        let preparation = storage
            .work_items()
            .begin_owned_flow(
                operation,
                b"legacy source continuation",
                OwnedFlowPreparationPurpose::Continuation(
                    ContinuationInputRequirement::new(bootstrap, source).unwrap(),
                ),
                10,
            )
            .unwrap();
        drop(preparation);
        let mut outcomes = Vec::new();
        for run in [planning, source] {
            let deletion = storage.delete_run(&run).await;
            let exact = matches!(deletion, Err(crate::runs::StorageError::RunReferenced { run_id }) if run_id == run);
            let history = storage
                .open_run_reader(run)
                .await
                .unwrap()
                .current_seq()
                .await
                .unwrap()
                .0;
            let origin: String = storage
                .acquire_registry_conn()
                .unwrap()
                .query_row(
                    "SELECT kind FROM run_coordination_origins WHERE run_id=?",
                    [run.to_string()],
                    |row| row.get(0),
                )
                .unwrap();
            outcomes.push((
                exact,
                history,
                origin,
                !home
                    .path()
                    .join(format!("run-controls/{run}.lock"))
                    .exists(),
            ));
        }
        outcomes
    };
    home.close().unwrap();
    assert_eq!(
        facts,
        vec![
            (true, 1, "Legacy".into(), true),
            (true, 1, "Legacy".into(), true)
        ]
    );
}
