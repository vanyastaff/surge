//! Structural pre-0035 fixtures use the original v1 private object domains.
//! They are not an old-binary execution or source/workspace eligibility proof.
use super::*;
use crate::runtime_home_fixture::FixtureHome;
use crate::work_items::start_preparation::secure_lock::{PreparationLock, PreparationLockKey};
use rusqlite::params;
use surge_core::{ContentHash, Graph, id::WorkItemId};

const BODY: &[u8] = b"fixed original legacy operation body";
#[derive(Clone, Copy)]
enum Kind {
    Hmac,
    PublicMcp,
}
#[derive(Clone, Copy, Debug)]
enum Mismatch {
    None,
    Operation,
    Item,
    Run,
    WorkspaceOwner,
    Workspace,
    Requirements,
    Request,
}
struct LegacyFixture {
    operation: WorkItemOperationId,
    item: WorkItemId,
    run: RunId,
    key_bytes: Vec<u8>,
    manifest: Option<OwnedFlowInputsManifest>,
}
fn fixed_key(files: &PrivateFiles) -> Vec<u8> {
    let mut bytes = KEY_MAGIC.to_vec();
    bytes.extend([7; 32]);
    bytes.extend(Sha256::digest(&bytes));
    assert!(files.publish(KEY_NAME, &bytes).unwrap());
    assert!(files.publish(KEY_MARKER, b"established-v1").unwrap());
    bytes
}
fn source(home: &Path, owner: RunId) -> super::super::OwnedFlowSourceSnapshot {
    let graph: Graph = toml::from_str(include_str!(
        "../../../../../examples/flow_terminal_only.toml"
    ))
    .unwrap();
    super::super::OwnedFlowSourceSnapshot {
        contract: AcceptedFlowContract::new(Box::new(graph), "fixed legacy prompt".into()).unwrap(),
        source_base: home.join("project"),
        workspace: WorkItemWorkspace {
            repository: home.join("project/.git"),
            checkout: home.join("project"),
            path: home.join("workspace"),
            ownership: owner.to_string(),
            branch: "codex/legacy".into(),
            base_commit: "a".repeat(40),
        },
    }
}
fn legacy_manifest(
    files: &PrivateFiles,
    mut binding: InputBinding,
    mismatch: Mismatch,
) -> OwnedFlowInputsManifest {
    match mismatch {
        Mismatch::None => {},
        Mismatch::Operation => {
            binding.operation = "work-item-op-00000000000000000000000009".parse().unwrap()
        },
        Mismatch::Item => {
            binding.binding.item = "work-item-00000000000000000000000009".parse().unwrap()
        },
        Mismatch::Run => binding.run = "run-00000000000000000000000009".parse().unwrap(),
        Mismatch::WorkspaceOwner => {
            binding.workspace_owner = "run-00000000000000000000000009".parse().unwrap()
        },
        Mismatch::Workspace => binding.workspace.path = binding.workspace.path.join("foreign"),
        Mismatch::Requirements => {
            binding.binding.requirements_hash = ContentHash::compute(b"foreign accepted contract")
        },
        Mismatch::Request => {
            binding.request_identity = OwnedFlowRequestIdentity::PlainPublic {
                digest: ContentHash::compute(b"foreign request"),
            }
        },
    }
    let reference = OwnedFlowObjectRef::new("1".repeat(64)).unwrap();
    let server = McpServerRef::new(
        "fixed-server".into(),
        surge_core::mcp_config::McpTransportConfig::stdio(
            "fixed-legacy-command".into(),
            vec!["fixed-argument".into()],
            Default::default(),
        ),
        None,
        std::time::Duration::from_secs(7),
        false,
    );
    let object = ServerObject {
        schema_version: 1,
        binding: binding.clone(),
        index: 0,
        object_ref: reference.clone(),
        server: server.clone(),
    };
    let bytes = canonical(&object).unwrap();
    let entry = OwnedFlowMcpManifestEntry::new(
        0,
        &server,
        reference.clone(),
        authentication(&[7; 32], SERVER_DOMAIN, &bytes).unwrap(),
    )
    .unwrap();
    assert!(files.publish(&object_name(&reference), &bytes).unwrap());
    let envelope_ref = OwnedFlowObjectRef::new("2".repeat(64)).unwrap();
    let envelope = EnvelopeObject {
        schema_version: 1,
        binding: binding.clone(),
        selection: OwnedFlowMcpSelection::HostDefault,
        entries: vec![entry.clone()],
        envelope_ref: envelope_ref.clone(),
    };
    let bytes = canonical(&envelope).unwrap();
    assert!(files.publish(&object_name(&envelope_ref), &bytes).unwrap());
    binding
        .manifest(
            FrozenOwnedFlowMcp::private(
                OwnedFlowMcpSelection::HostDefault,
                vec![entry],
                envelope_ref,
                authentication(&[7; 32], ENVELOPE_DOMAIN, &bytes).unwrap(),
            )
            .unwrap(),
        )
        .unwrap()
}
fn legacy_fixture(
    store: &WorkItemStore,
    home: &Path,
    kind: Kind,
    mismatch: Mismatch,
) -> LegacyFixture {
    let operation = "work-item-op-00000000000000000000000001".parse().unwrap();
    let item = "work-item-00000000000000000000000002".parse().unwrap();
    let run = "run-00000000000000000000000003".parse().unwrap();
    let workspace_owner = "run-00000000000000000000000004".parse().unwrap();
    let lock = PreparationLock::acquire_private(home, PreparationLockKey::FlowOperation(operation))
        .unwrap();
    let files = PrivateFiles::open(home, true).unwrap();
    let key_bytes = fixed_key(&files);
    let identity = match kind {
        Kind::Hmac => OwnedFlowRequestIdentity::Hmac {
            tag: authentication(&[7; 32], REQUEST_DOMAIN, BODY).unwrap(),
        },
        Kind::PublicMcp => OwnedFlowRequestIdentity::PlainPublic {
            digest: ContentHash::compute(BODY),
        },
    };
    let source = source(home, workspace_owner);
    let context = InputBinding::new(
        operation,
        identity.clone(),
        WorkItemBinding {
            item,
            revision: 1,
            requirements_hash: source.contract.hash().unwrap(),
            generation: 1,
        },
        run,
        workspace_owner,
        source.workspace.clone(),
    );
    let manifest = match kind {
        Kind::Hmac => None,
        Kind::PublicMcp => Some(legacy_manifest(&files, context, mismatch)),
    };
    store.pool.get().unwrap().execute(
        "INSERT INTO owned_flow_operations(operation,request_identity,item,run,workspace_owner,operation_lock_identity,token,state,source_snapshot,config_snapshot,inputs_manifest,created_at_ms) VALUES(?,?,?,?,?,?,?,'abandoned',?,?,?,1)",
        params![operation.to_string(),serde_json::to_string(&identity).unwrap(),item.to_string(),run.to_string(),workspace_owner.to_string(),lock.identity().unwrap(),"run-00000000000000000000000005",serde_json::to_string(&source).unwrap(),manifest.as_ref().map(|_|r#"{"mcp_servers":[]}"#),manifest.as_ref().map(|value|serde_json::to_string(value).unwrap())],
    ).unwrap();
    LegacyFixture {
        operation,
        item,
        run,
        key_bytes,
        manifest,
    }
}
fn establishment_count(store: &WorkItemStore) -> i64 {
    store
        .pool
        .get()
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM owned_flow_private_establishment",
            [],
            |row| row.get(0),
        )
        .unwrap()
}
fn audit(store: &WorkItemStore, operation: WorkItemOperationId) -> String {
    store.pool.get().unwrap().query_row(
        "SELECT json_object('identity',request_identity,'item',item,'run',run,'owner',workspace_owner,'lock',operation_lock_identity,'token',token,'state',state,'source',source_snapshot,'inputs',inputs_manifest,'receipt',receipt) FROM owned_flow_operations WHERE operation=?",
        [operation.to_string()],|row|row.get(0),
    ).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn legacy_existing_hmac_and_public_private_envelope_pin_only_original_key() {
    for kind in [Kind::Hmac, Kind::PublicMcp] {
        let home = FixtureHome::new().unwrap();
        let facts = {
            let storage = crate::runs::Storage::open(home.path()).await.unwrap();
            let store = storage.work_items();
            let fixture = legacy_fixture(&store, home.path(), kind, Mismatch::None);
            let before = audit(&store, fixture.operation);
            let replay = store
                .replay_owned_flow(fixture.operation, BODY, matches!(kind, Kind::Hmac))
                .unwrap();
            let after = audit(&store, fixture.operation);
            let prepared = store
                .begin_owned_flow(fixture.operation, BODY, matches!(kind, Kind::Hmac), 2)
                .unwrap();
            let super::super::OwnedFlowPreparationResult::Preparing(prepared) = prepared else {
                panic!("provisional legacy owner")
            };
            let ids = (prepared.item(), prepared.run());
            let key = std::fs::read(
                home.path()
                    .join("work-items/private-flow-inputs/host-key-v1"),
            )
            .unwrap();
            let state: String = store
                .pool
                .get()
                .unwrap()
                .query_row(
                    "SELECT state FROM owned_flow_private_establishment",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            let manifest_same = prepared
                .startup_snapshot()
                .unwrap()
                .map(|(_, manifest)| manifest)
                == fixture.manifest;
            (
                replay.is_none(),
                before == after,
                ids == (fixture.item, fixture.run),
                key == fixture.key_bytes,
                state == "Established",
                manifest_same,
            )
        };
        home.close().unwrap();
        assert_eq!(facts, (true, true, true, true, true, true));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn legacy_authenticated_foreign_envelope_never_pins_or_changes_original_operation() {
    let mut wrong = Vec::new();
    for mismatch in [
        Mismatch::Operation,
        Mismatch::Item,
        Mismatch::Run,
        Mismatch::WorkspaceOwner,
        Mismatch::Workspace,
        Mismatch::Requirements,
        Mismatch::Request,
    ] {
        let home = FixtureHome::new().unwrap();
        let correct = {
            let storage = crate::runs::Storage::open(home.path()).await.unwrap();
            let store = storage.work_items();
            let fixture = legacy_fixture(&store, home.path(), Kind::PublicMcp, mismatch);
            let before = audit(&store, fixture.operation);
            let result = store.replay_owned_flow(fixture.operation, BODY, false);
            matches!(result, Err(WorkItemError::PrivateInputsCorrupt))
                && establishment_count(&store) == 0
                && audit(&store, fixture.operation) == before
                && std::fs::read(
                    home.path()
                        .join("work-items/private-flow-inputs/host-key-v1"),
                )
                .unwrap()
                    == fixture.key_bytes
        };
        home.close().unwrap();
        if !correct {
            wrong.push(mismatch)
        }
    }
    assert!(
        wrong.is_empty(),
        "valid MAC hid foreign independently stored bindings: {wrong:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn legacy_wrong_hmac_body_refuses_without_first_pinning() {
    let home = FixtureHome::new().unwrap();
    let facts = {
        let storage = crate::runs::Storage::open(home.path()).await.unwrap();
        let store = storage.work_items();
        let fixture = legacy_fixture(&store, home.path(), Kind::Hmac, Mismatch::None);
        let before = audit(&store, fixture.operation);
        let result = store.replay_owned_flow(fixture.operation, b"changed body", true);
        (
            matches!(result, Err(WorkItemError::Conflict(_))),
            establishment_count(&store) == 0,
            audit(&store, fixture.operation) == before,
        )
    };
    home.close().unwrap();
    assert_eq!(facts, (true, true, true));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn new_private_request_cannot_first_pin_another_legacy_operation() {
    let home = FixtureHome::new().unwrap();
    let facts = {
        let storage = crate::runs::Storage::open(home.path()).await.unwrap();
        let store = storage.work_items();
        let fixture = legacy_fixture(&store, home.path(), Kind::Hmac, Mismatch::None);
        let before = audit(&store, fixture.operation);
        let result =
            store.begin_owned_flow(WorkItemOperationId::new(), b"new unrelated body", true, 3);
        (
            matches!(result, Err(WorkItemError::PrivateInputsRecoveryRequired)),
            establishment_count(&store) == 0,
            audit(&store, fixture.operation) == before,
            std::fs::read(
                home.path()
                    .join("work-items/private-flow-inputs/host-key-v1"),
            )
            .unwrap()
                == fixture.key_bytes,
        )
    };
    home.close().unwrap();
    assert_eq!(facts, (true, true, true, true));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retained_private_owners_refuse_marker_loss_at_actual_use_boundaries() {
    let home = FixtureHome::new().unwrap();
    let facts = {
        let storage = crate::runs::Storage::open(home.path()).await.unwrap();
        let store = storage.work_items();
        let fixture = legacy_fixture(&store, home.path(), Kind::PublicMcp, Mismatch::None);
        let super::super::OwnedFlowPreparationResult::Preparing(mut prepared) = store
            .begin_owned_flow(fixture.operation, BODY, false, 2)
            .unwrap()
        else {
            panic!("original provisional operation")
        };
        prepared.retain_launch_ownership().unwrap();
        let (receipt, claim) = prepared.finalize(3).unwrap().into_parts();
        let inputs = store.hydrate_owned_flow_inputs(&claim).unwrap().unwrap();
        let workspace = inputs.manifest().workspace().path.clone();
        store.validate_owned_flow_pending(&claim).unwrap();
        inputs
            .validate_for(receipt.run, &receipt.binding, &workspace)
            .unwrap();
        let before = audit(&store, fixture.operation);
        let loss = std::fs::remove_file(
            home.path()
                .join("work-items/private-flow-inputs")
                .join(KEY_MARKER),
        );
        let pending = store.validate_owned_flow_pending(&claim);
        let runtime_use = inputs.validate_for(receipt.run, &receipt.binding, &workspace);
        let (pending_ok, inputs_ok) = match loss {
            Ok(()) => (
                matches!(pending, Err(WorkItemError::PrivateInputsRecoveryRequired)),
                matches!(
                    runtime_use,
                    Err(WorkItemError::PrivateInputsRecoveryRequired)
                ),
            ),
            Err(error) => {
                let protected = cfg!(windows) && matches!(error.raw_os_error(), Some(5 | 32));
                (
                    protected && pending.is_ok(),
                    protected && runtime_use.is_ok(),
                )
            },
        };
        (
            pending_ok,
            inputs_ok,
            audit(&store, fixture.operation) == before,
            std::fs::read(
                home.path()
                    .join("work-items/private-flow-inputs")
                    .join(KEY_NAME),
            )
            .unwrap()
                == fixture.key_bytes,
        )
    };
    home.close().unwrap();
    assert_eq!(facts, (true, true, true, true));
}

fn private_bytes(home: &Path) -> Option<Vec<(String, Vec<u8>)>> {
    let directory = home.join("work-items/private-flow-inputs");
    if !directory.exists() {
        return None;
    }
    let mut values: Vec<_> = std::fs::read_dir(directory)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            (
                entry.file_name().to_str().unwrap().to_owned(),
                std::fs::read(entry.path()).unwrap(),
            )
        })
        .collect();
    values.sort();
    Some(values)
}
fn establishment_audit(store: &WorkItemStore) -> Option<String> {
    use rusqlite::OptionalExtension;
    store.pool.get().unwrap().query_row(
        "SELECT json_object('nonce',nonce,'state',state,'lock',lock_identity,'namespace',namespace_identity,'key',hex(key_identity),'digest',hex(key_digest),'marker',hex(marker_identity)) FROM owned_flow_private_establishment",
        [], |row| row.get(0),
    ).optional().unwrap()
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn interrupted_external_publication_never_rekeys_after_committed_intent() {
    let mut failures = Vec::new();
    for (name, trigger) in [
        (
            "namespace",
            "CREATE TRIGGER fixture_abort BEFORE UPDATE OF namespace_identity ON owned_flow_private_establishment BEGIN SELECT RAISE(ABORT,'fixture namespace publication'); END",
        ),
        (
            "established",
            "CREATE TRIGGER fixture_abort BEFORE UPDATE OF state ON owned_flow_private_establishment WHEN NEW.state='Established' BEGIN SELECT RAISE(ABORT,'fixture established publication'); END",
        ),
    ] {
        let home = FixtureHome::new().unwrap();
        let (row, bytes, initial_failed) = {
            let storage = crate::runs::Storage::open(home.path()).await.unwrap();
            let store = storage.work_items();
            store.pool.get().unwrap().execute_batch(trigger).unwrap();
            let failed = matches!(
                request_identity(&store, BODY, true),
                Err(WorkItemError::Sql(_))
            );
            store
                .pool
                .get()
                .unwrap()
                .execute_batch("DROP TRIGGER fixture_abort")
                .unwrap();
            (
                establishment_audit(&store),
                private_bytes(home.path()),
                failed,
            )
        };
        let correct = {
            let storage = crate::runs::Storage::open(home.path()).await.unwrap();
            let store = storage.work_items();
            let retry = request_identity(&store, b"new request after interruption", true);
            initial_failed
                && row.as_ref().unwrap().contains("Establishing")
                && matches!(retry, Err(WorkItemError::PrivateInputsRecoveryRequired))
                && establishment_audit(&store) == row
                && private_bytes(home.path()) == bytes
                && store
                    .pool
                    .get()
                    .unwrap()
                    .query_row("SELECT COUNT(*) FROM owned_flow_operations", [], |row| {
                        row.get::<_, i64>(0)
                    })
                    .unwrap()
                    == 0
        };
        home.close().unwrap();
        if !correct {
            failures.push(name);
        }
    }
    assert!(
        failures.is_empty(),
        "ambiguous publication changed identity: {failures:?}"
    );
}
#[derive(Clone, Copy, Debug)]
enum Damage {
    MissingKey,
    CorruptKey,
    MissingMarker,
    CorruptMarker,
    MissingObject,
}
fn damage_private(home: &Path, damage: Damage) {
    let root = home.join("work-items/private-flow-inputs");
    match damage {
        Damage::MissingKey => std::fs::remove_file(root.join(KEY_NAME)).unwrap(),
        Damage::CorruptKey => {
            std::fs::write(root.join(KEY_NAME), b"corrupt established key").unwrap()
        },
        Damage::MissingMarker => std::fs::remove_file(root.join(KEY_MARKER)).unwrap(),
        Damage::CorruptMarker => std::fs::write(root.join(KEY_MARKER), b"foreign marker").unwrap(),
        Damage::MissingObject => std::fs::remove_file(root.join(object_name(
            &OwnedFlowObjectRef::new("1".repeat(64)).unwrap(),
        )))
        .unwrap(),
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn legacy_and_established_missing_or_corrupt_private_material_never_repairs() {
    let mut failures = Vec::new();
    for (established, damage) in [
        (false, Damage::MissingKey),
        (false, Damage::CorruptKey),
        (false, Damage::MissingMarker),
        (false, Damage::CorruptMarker),
        (false, Damage::MissingObject),
        (true, Damage::MissingKey),
        (true, Damage::CorruptKey),
        (true, Damage::MissingMarker),
        (true, Damage::CorruptMarker),
        (true, Damage::MissingObject),
    ] {
        let home = FixtureHome::new().unwrap();
        let (operation, before, count) = {
            let storage = crate::runs::Storage::open(home.path()).await.unwrap();
            let store = storage.work_items();
            let fixture = legacy_fixture(&store, home.path(), Kind::PublicMcp, Mismatch::None);
            if established {
                assert!(
                    store
                        .replay_owned_flow(fixture.operation, BODY, false)
                        .unwrap()
                        .is_none()
                );
            }
            (
                fixture.operation,
                audit(&store, fixture.operation),
                establishment_count(&store),
            )
        };
        damage_private(home.path(), damage);
        let damaged = private_bytes(home.path());
        let correct = {
            let storage = crate::runs::Storage::open(home.path()).await.unwrap();
            let store = storage.work_items();
            let result = store.replay_owned_flow(operation, BODY, false);
            matches!(result, Err(WorkItemError::PrivateInputsRecoveryRequired))
                && audit(&store, operation) == before
                && establishment_count(&store) == count
                && private_bytes(home.path()) == damaged
        };
        home.close().unwrap();
        if !correct {
            failures.push((established, damage));
        }
    }
    assert!(
        failures.is_empty(),
        "private loss/corruption was repaired or misclassified: {failures:?}"
    );
}

fn private_preparation(
    store: &WorkItemStore,
    operation: WorkItemOperationId,
    now: i64,
) -> Box<super::super::OwnedFlowPreparation> {
    let super::super::OwnedFlowPreparationResult::Preparing(preparation) =
        store.begin_owned_flow(operation, BODY, false, now).unwrap()
    else {
        panic!("original private preparation")
    };
    preparation
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn canceled_caller_retains_private_preparation_until_actual_worker_exit() {
    let home = FixtureHome::new().unwrap();
    let facts = {
        let storage = crate::runs::Storage::open(home.path()).await.unwrap();
        let store = storage.work_items();
        let fixture = legacy_fixture(&store, home.path(), Kind::PublicMcp, Mismatch::None);
        let operation = fixture.operation;
        let worker_store = store.clone();
        let (ready, ready_rx) = tokio::sync::oneshot::channel();
        let (release, release_rx) = std::sync::mpsc::channel();
        let (done, done_rx) = tokio::sync::oneshot::channel();
        let caller = tokio::spawn(async move {
            tokio::task::spawn_blocking(move || {
                let mut preparation = private_preparation(&worker_store, operation, 2);
                preparation.retain_launch_ownership().unwrap();
                ready.send((preparation.item(), preparation.run())).unwrap();
                release_rx
                    .recv_timeout(std::time::Duration::from_secs(10))
                    .unwrap();
                drop(preparation);
                drop(worker_store);
                done.send(()).unwrap();
            })
            .await
            .unwrap();
        });
        let ids = ready_rx.await.unwrap();
        let established = establishment_audit(&store);
        let bytes = private_bytes(home.path());
        caller.abort();
        let canceled = matches!(caller.await, Err(error) if error.is_cancelled());
        let busy = matches!(
            store.begin_owned_flow(operation, BODY, false, 3),
            Err(WorkItemError::Busy)
        );
        #[cfg(windows)]
        let native_protected = std::fs::rename(
            home.path().join("work-items/private-flow-inputs"),
            home.path().join("retired-live-private"),
        )
        .is_err();
        #[cfg(not(windows))]
        let native_protected = true; // Unix cooperative exclusion is the retained operation lock.
        let retained =
            establishment_audit(&store) == established && private_bytes(home.path()) == bytes;
        release.send(()).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(10), done_rx)
            .await
            .unwrap()
            .unwrap();
        let super::super::OwnedFlowPreparationResult::Preparing(preparation) =
            store.begin_owned_flow(operation, BODY, false, 4).unwrap()
        else {
            panic!("original abandoned preparation")
        };
        (
            canceled,
            busy,
            native_protected,
            retained,
            (preparation.item(), preparation.run()) == ids && ids == (fixture.item, fixture.run),
            establishment_audit(&store) == established && private_bytes(home.path()) == bytes,
        )
    };
    home.close().unwrap();
    assert_eq!(facts, (true, true, true, true, true, true));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dependency_scan_rejects_malformed_rows_even_after_finding_valid_private_dependency() {
    let mut failures = Vec::new();
    for malformed_manifest in [false, true] {
        let home = FixtureHome::new().unwrap();
        let correct = {
            let storage = crate::runs::Storage::open(home.path()).await.unwrap();
            let store = storage.work_items();
            let fixture = legacy_fixture(&store, home.path(), Kind::Hmac, Mismatch::None);
            let identity = if malformed_manifest {
                serde_json::to_string(&OwnedFlowRequestIdentity::PlainPublic {
                    digest: ContentHash::compute(b"unrelated public body"),
                })
                .unwrap()
            } else {
                "{}".into()
            };
            let manifest = malformed_manifest.then_some("{}");
            store.pool.get().unwrap().execute(
                "INSERT INTO owned_flow_operations(operation,request_identity,item,run,workspace_owner,operation_lock_identity,token,state,source_snapshot,inputs_manifest,created_at_ms) SELECT 'work-item-op-00000000000000000000000009',?,'work-item-00000000000000000000000009','run-00000000000000000000000009','run-00000000000000000000000008',operation_lock_identity,token,'abandoned',source_snapshot,?,1 FROM owned_flow_operations WHERE operation=?",
                params![identity,manifest,fixture.operation.to_string()],
            ).unwrap();
            let before = audit(&store, fixture.operation);
            let bytes = private_bytes(home.path());
            let result = request_identity(&store, b"new private body", true);
            let public = request_identity(&store, b"unrelated public body", false);
            matches!(result, Err(WorkItemError::PrivateInputsCorrupt))
                && matches!(
                    public,
                    Ok((OwnedFlowRequestIdentity::PlainPublic { .. }, None))
                )
                && establishment_count(&store) == 0
                && audit(&store, fixture.operation) == before
                && private_bytes(home.path()) == bytes
        };
        home.close().unwrap();
        if !correct {
            failures.push(malformed_manifest);
        }
    }
    assert!(
        failures.is_empty(),
        "malformed dependencies disappeared: {failures:?}"
    );
}

#[derive(Clone, Copy, Debug)]
enum AcceptedHmacCase {
    Provisional,
    HealthyPrivate,
    MissingManifest,
    ForeignReceipt,
}
fn accepted_hmac_fixture(
    store: &WorkItemStore,
    home: &Path,
    case: AcceptedHmacCase,
) -> (LegacyFixture, Option<OwnedFlowReceipt>) {
    let fixture = legacy_fixture(store, home, Kind::Hmac, Mismatch::None);
    if matches!(case, AcceptedHmacCase::Provisional) {
        return (fixture, None);
    }
    let owner: RunId = "run-00000000000000000000000004".parse().unwrap();
    let source = source(home, owner);
    let identity = OwnedFlowRequestIdentity::Hmac {
        tag: authentication(&[7; 32], REQUEST_DOMAIN, BODY).unwrap(),
    };
    let binding = WorkItemBinding {
        item: fixture.item,
        revision: 1,
        generation: 1,
        requirements_hash: source.contract.hash().unwrap(),
    };
    let manifest = legacy_manifest(
        &PrivateFiles::open(home, false).unwrap(),
        InputBinding::new(
            fixture.operation,
            identity,
            binding.clone(),
            fixture.run,
            owner,
            source.workspace,
        ),
        Mismatch::None,
    );
    let mut receipt = OwnedFlowReceipt {
        operation_id: fixture.operation,
        item: fixture.item,
        run: fixture.run,
        binding,
        workspace_owner: owner,
        accepted_at_ms: 2,
    };
    if matches!(case, AcceptedHmacCase::ForeignReceipt) {
        receipt.run = "run-00000000000000000000000009".parse().unwrap();
    }
    let inputs = (!matches!(case, AcceptedHmacCase::MissingManifest))
        .then(|| serde_json::to_string(&manifest).unwrap());
    store.pool.get().unwrap().execute(
        "UPDATE owned_flow_operations SET state='accepted',receipt=?,intent_state='pending',config_snapshot=?,inputs_manifest=? WHERE operation=?",
        params![serde_json::to_string(&receipt).unwrap(),r#"{"mcp_servers":[]}"#,inputs,fixture.operation.to_string()],
    ).unwrap();
    (fixture, Some(receipt))
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn legacy_hmac_acceptance_validates_receipt_even_without_private_mcp() {
    let mut failures = Vec::new();
    for case in [
        AcceptedHmacCase::Provisional,
        AcceptedHmacCase::HealthyPrivate,
        AcceptedHmacCase::MissingManifest,
        AcceptedHmacCase::ForeignReceipt,
    ] {
        let home = FixtureHome::new().unwrap();
        let correct = {
            let storage = crate::runs::Storage::open(home.path()).await.unwrap();
            let store = storage.work_items();
            let (fixture, expected_receipt) = accepted_hmac_fixture(&store, home.path(), case);
            let before = audit(&store, fixture.operation);
            let bytes = private_bytes(home.path());
            let result = store.replay_owned_flow(fixture.operation, BODY, true);
            let (matches_result, count) = match case {
                AcceptedHmacCase::Provisional | AcceptedHmacCase::HealthyPrivate => (
                    matches!(result, Ok(receipt) if receipt == expected_receipt),
                    1,
                ),
                AcceptedHmacCase::MissingManifest | AcceptedHmacCase::ForeignReceipt => (
                    matches!(result, Err(WorkItemError::PrivateInputsCorrupt)),
                    0,
                ),
            };
            matches_result
                && establishment_count(&store) == count
                && audit(&store, fixture.operation) == before
                && private_bytes(home.path()) == bytes
        };
        home.close().unwrap();
        if !correct {
            failures.push(case);
        }
    }
    assert!(
        failures.is_empty(),
        "HMAC accepted association disappeared: {failures:?}"
    );
}

fn bounded_private_store(storage: &crate::runs::Storage, home: &Path) -> WorkItemStore {
    let database = home.join("db/registry.sqlite");
    #[cfg(windows)]
    let manager = crate::SqliteConnectionManager::file(
        &database,
        crate::state_home::StateHomeOwner::open(home)
            .unwrap()
            .registry()
            .unwrap(),
    );
    #[cfg(not(windows))]
    let manager = crate::SqliteConnectionManager::file(&database);
    let manager = manager.with_init(|connection| {
        crate::runs::pragmas::apply(connection, crate::runs::pragmas::REGISTRY_PRAGMAS)?;
        crate::runs::busy::install(connection)
    });
    let executor = scheduled_thread_pool::ScheduledThreadPool::builder()
        .num_threads(3)
        .thread_name_pattern("bounded-private-fixture-{}")
        .on_drop_behavior(scheduled_thread_pool::OnPoolDropBehavior::DiscardPendingScheduled)
        .build();
    let mut store = storage.work_items();
    store.pool = r2d2::Pool::builder()
        .thread_pool(std::sync::Arc::new(executor))
        .max_size(2)
        .min_idle(Some(2))
        .connection_timeout(std::time::Duration::from_secs(1))
        .build(manager)
        .unwrap();
    store
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn private_preparation_and_claim_reuse_the_available_transaction_connection() {
    let home = FixtureHome::new().unwrap();
    let facts = {
        let storage = crate::runs::Storage::open(home.path()).await.unwrap();
        let store = bounded_private_store(&storage, home.path());
        let fixture = legacy_fixture(&store, home.path(), Kind::PublicMcp, Mismatch::None);
        let mut preparation = private_preparation(&store, fixture.operation, 2);
        let original_source = source(
            home.path(),
            "run-00000000000000000000000004".parse().unwrap(),
        );
        preparation.freeze_source(&original_source, None).unwrap();
        let before = audit(&store, fixture.operation);
        let bytes = private_bytes(home.path());
        let held = store.pool.get().unwrap();
        let preparation_result = preparation.freeze_source(&original_source, None);
        drop(held);
        let preparation_unchanged = audit(&store, fixture.operation) == before;
        preparation.retain_launch_ownership().unwrap();
        let (receipt, claim) = preparation.finalize(3).unwrap().into_parts();
        let inputs = store.hydrate_owned_flow_inputs(&claim).unwrap().unwrap();
        let workspace = inputs.manifest().workspace().path.clone();
        store.validate_owned_flow_pending(&claim).unwrap();
        let accepted = audit(&store, fixture.operation);
        let held = store.pool.get().unwrap();
        let claim_result = store.validate_owned_flow_pending(&claim);
        let generic_claim = store.validate_claim(&claim);
        let accepted_unchanged = audit(&store, fixture.operation) == accepted;
        let startup = store.acknowledge_owned_flow_startup(&claim, inputs.manifest());
        let effect = store.validate_owned_flow_effect(&claim);
        let intent: String = held
            .query_row(
                "SELECT intent_state FROM owned_flow_operations WHERE operation=?",
                [fixture.operation.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        let independent_owner = inputs.validate_for(receipt.run, &receipt.binding, &workspace);
        drop(held);
        (
            (
                preparation_result.is_ok(),
                claim_result.is_ok(),
                independent_owner.is_ok(),
                preparation_unchanged
                    && accepted_unchanged
                    && audit(&store, fixture.operation) == accepted,
                private_bytes(home.path()) == bytes,
                generic_claim.is_ok(),
                startup.is_ok(),
                effect.is_ok(),
                intent == "started",
            ),
            (
                matches!(preparation_result, Err(WorkItemError::Pool(_))),
                matches!(claim_result, Err(WorkItemError::Pool(_))),
            ),
        )
    };
    home.close().unwrap();
    assert_eq!(
        facts.0,
        (true, true, true, true, true, true, true, true, true),
        "one real pool slot remains available; observed preparation/claim pool timeouts: {:?}",
        facts.1
    );
}

struct AcceptedPrivateFixture {
    fixture: LegacyFixture,
    receipt: OwnedFlowReceipt,
    claim: crate::work_items::WorkItemLaunchClaim,
    inputs: super::super::AuthenticatedOwnedFlowInputs,
}
fn accepted_private_fixture(store: &WorkItemStore, home: &Path) -> AcceptedPrivateFixture {
    let fixture = legacy_fixture(store, home, Kind::PublicMcp, Mismatch::None);
    let mut preparation = private_preparation(store, fixture.operation, 2);
    preparation.retain_launch_ownership().unwrap();
    let (receipt, claim) = preparation.finalize(3).unwrap().into_parts();
    let inputs = store.hydrate_owned_flow_inputs(&claim).unwrap().unwrap();
    AcceptedPrivateFixture {
        fixture,
        receipt,
        claim,
        inputs,
    }
}
fn all_establishment_facts(store: &WorkItemStore) -> Vec<rusqlite::types::Value> {
    let conn = store.pool.get().unwrap();
    let mut query = conn
        .prepare("SELECT * FROM owned_flow_private_establishment")
        .unwrap();
    let count = query.column_count();
    query
        .query_row([], |row| (0..count).map(|index| row.get(index)).collect())
        .unwrap()
}
#[cfg(unix)]
fn marker_native_identity(home: &Path) -> Vec<u8> {
    use std::os::unix::fs::MetadataExt;
    let metadata =
        std::fs::metadata(home.join("work-items/private-flow-inputs").join(KEY_MARKER)).unwrap();
    [metadata.dev().to_le_bytes(), metadata.ino().to_le_bytes()].concat()
}
#[cfg(windows)]
fn marker_native_identity(home: &Path) -> Vec<u8> {
    let files = PrivateFiles::open(home, false).unwrap();
    let marker = files.retain(KEY_MARKER).unwrap();
    files.retained_identity(&marker).unwrap()
}
fn replace_marker_preserving_original(home: &Path, retired: &str) -> std::io::Result<()> {
    let root = home.join("work-items/private-flow-inputs");
    std::fs::rename(root.join(KEY_MARKER), home.join(retired))?;
    assert_eq!(
        std::fs::read(home.join(retired)).unwrap(),
        b"established-v1"
    );
    let files = PrivateFiles::open(home, false).unwrap();
    assert!(files.publish(KEY_MARKER, b"established-v1").unwrap());
    Ok(())
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retained_claim_and_inputs_refuse_same_byte_marker_replacement() {
    let home = FixtureHome::new().unwrap();
    let facts = {
        let storage = crate::runs::Storage::open(home.path()).await.unwrap();
        let store = storage.work_items();
        let accepted = accepted_private_fixture(&store, home.path());
        let workspace = accepted.inputs.manifest().workspace().path.clone();
        store.validate_owned_flow_pending(&accepted.claim).unwrap();
        accepted
            .inputs
            .validate_for(accepted.receipt.run, &accepted.receipt.binding, &workspace)
            .unwrap();
        let row = audit(&store, accepted.fixture.operation);
        let establishment = all_establishment_facts(&store);
        let bytes = private_bytes(home.path());
        let original_native = marker_native_identity(home.path());
        let replacement = replace_marker_preserving_original(home.path(), "original-live-marker");
        let current_native = marker_native_identity(home.path());
        let native_challenge = if replacement.is_ok() {
            current_native != original_native
        } else {
            current_native == original_native
        };
        let pending = store.validate_owned_flow_pending(&accepted.claim);
        let inputs = accepted.inputs.validate_for(
            accepted.receipt.run,
            &accepted.receipt.binding,
            &workspace,
        );
        let (pending_ok, inputs_ok) = match replacement {
            Ok(()) => (
                matches!(pending, Err(WorkItemError::PrivateInputsRecoveryRequired)),
                matches!(inputs, Err(WorkItemError::PrivateInputsRecoveryRequired)),
            ),
            Err(error) => {
                let protected = cfg!(windows) && matches!(error.raw_os_error(), Some(5 | 32));
                (protected && pending.is_ok(), protected && inputs.is_ok())
            },
        };
        (
            pending_ok,
            inputs_ok,
            audit(&store, accepted.fixture.operation) == row,
            all_establishment_facts(&store) == establishment,
            private_bytes(home.path()) == bytes,
            native_challenge,
        )
    };
    home.close().unwrap();
    assert_eq!(
        facts,
        (true, true, true, true, true, true),
        "valid copied marker must not renew original native ownership"
    );
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cold_replay_and_begin_refuse_same_byte_marker_replacement() {
    let home = FixtureHome::new().unwrap();
    let (operation, row, establishment, bytes) = {
        let storage = crate::runs::Storage::open(home.path()).await.unwrap();
        let store = storage.work_items();
        let accepted = accepted_private_fixture(&store, home.path());
        assert_eq!(
            store
                .replay_owned_flow(accepted.fixture.operation, BODY, false)
                .unwrap(),
            Some(accepted.receipt)
        );
        (
            accepted.fixture.operation,
            audit(&store, accepted.fixture.operation),
            all_establishment_facts(&store),
            private_bytes(home.path()),
        )
    };
    let original_native = marker_native_identity(home.path());
    replace_marker_preserving_original(home.path(), "original-cold-marker").unwrap();
    let replacement_native = marker_native_identity(home.path());
    let facts = {
        let storage = crate::runs::Storage::open(home.path()).await.unwrap();
        let store = storage.work_items();
        let replay = store.replay_owned_flow(operation, BODY, false);
        let begin = store.begin_owned_flow(operation, BODY, false, 4);
        (
            matches!(replay, Err(WorkItemError::PrivateInputsRecoveryRequired)),
            matches!(begin, Err(WorkItemError::PrivateInputsRecoveryRequired)),
            audit(&store, operation) == row,
            all_establishment_facts(&store) == establishment,
            private_bytes(home.path()) == bytes,
            original_native != replacement_native,
        )
    };
    home.close().unwrap();
    assert_eq!(
        facts,
        (true, true, true, true, true, true),
        "cold byte equality does not prove original marker identity"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn private_recovery_and_reconciliation_reuse_one_available_pool_connection() {
    use crate::work_items::ReconciliationSettlement;
    let home = FixtureHome::new().unwrap();
    let facts = {
        let storage = crate::runs::Storage::open(home.path()).await.unwrap();
        let store = bounded_private_store(&storage, home.path());
        let accepted = accepted_private_fixture(&store, home.path());
        let observed = store.for_run(accepted.receipt.run).unwrap().unwrap();
        let invocation = "00000000000000000000000001";
        let healthy_cycle =
            store.begin_recovery_cycle(&accepted.claim, "00000000000000000000000002", 0);
        let held = store.pool.get().unwrap();
        let worker = std::thread::scope(|scope| {
            scope
                .spawn(|| {
                    let cycle = store.begin_recovery_cycle(&accepted.claim, invocation, 0);
                    let settlement = store.settle_reconciliation_error(
                        &accepted.claim,
                        &observed,
                        None,
                        "fixed recovery diagnostic",
                    );
                    (cycle, settlement)
                })
                .join()
                .unwrap()
        });
        drop(held);
        let after = store.for_run(accepted.receipt.run).unwrap().unwrap();
        let control_absent = store
            .execution_control(accepted.receipt.run)
            .unwrap()
            .is_none();
        let superseded = store.settle_reconciliation_error(
            &accepted.claim,
            &observed,
            None,
            "must not overwrite newer state",
        );
        (
            worker
                .0
                .as_ref()
                .is_ok_and(|cycle| cycle.generation == 1 && cycle.run == observed.run),
            matches!(worker.1, Ok(ReconciliationSettlement::Settled)),
            after.state == WorkItemAttemptState::Attention
                && after.diagnostic.as_deref() == Some("fixed recovery diagnostic"),
            control_absent,
            matches!(superseded, Ok(ReconciliationSettlement::Superseded)),
            healthy_cycle.is_ok(),
            format!("cycle={:?}, settlement={:?}", worker.0, worker.1),
        )
    };
    home.close().unwrap();
    assert!(
        facts.0 && facts.1 && facts.2 && facts.3 && facts.4 && facts.5,
        "bounded real recovery/reconciliation failed: {facts:?}"
    );
}

fn fresh_private_attempt(
    store: &WorkItemStore,
    home: &Path,
) -> (OwnedFlowReceipt, crate::work_items::WorkItemLaunchClaim) {
    let server = McpServerRef::new(
        "fresh-private-fixture".into(),
        surge_core::mcp_config::McpTransportConfig::stdio(
            "unlaunched-private-provider".into(),
            Vec::new(),
            Default::default(),
        ),
        None,
        std::time::Duration::from_secs(7),
        false,
    );
    super::super::tests::accepted_with_servers(store, home, WorkItemOperationId::new(), &[server])
}
fn unavailable_private_attempt(store: &WorkItemStore, home: &Path) -> OwnedFlowReceipt {
    let (receipt, claim) = fresh_private_attempt(store, home);
    assert!(matches!(store.claim(receipt.run), Err(WorkItemError::Busy)));
    drop(claim);
    std::fs::remove_file(home.join("work-items/private-flow-inputs").join(KEY_NAME)).unwrap();
    receipt
}
fn claim_token(store: &WorkItemStore, run: RunId) -> String {
    store
        .pool
        .get()
        .unwrap()
        .query_row(
            "SELECT claim_token FROM work_item_attempts WHERE run=?",
            [run.to_string()],
            |row| row.get(0),
        )
        .unwrap()
}
fn refusal_rows(store: &WorkItemStore) -> (u64, u64) {
    let conn = store.pool.get().unwrap();
    (
        conn.query_row("SELECT COUNT(*) FROM owned_flow_wake_refusals", [], |row| {
            row.get(0)
        })
        .unwrap(),
        conn.query_row(
            "SELECT COUNT(*) FROM owned_flow_wake_refusal_outbox",
            [],
            |row| row.get(0),
        )
        .unwrap(),
    )
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn automatic_private_unavailability_sets_attention_without_claim_or_control_identity() {
    let home = FixtureHome::new().unwrap();
    let facts = {
        let storage = crate::runs::Storage::open(home.path()).await.unwrap();
        let store = storage.work_items();
        let receipt = unavailable_private_attempt(&store, home.path());
        let observed = store.for_run(receipt.run).unwrap().unwrap();
        let version = store.show(receipt.item).unwrap().item.version;
        let token = claim_token(&store, receipt.run);
        let immutable = audit(&store, receipt.operation_id);
        let bytes = private_bytes(home.path());
        let direct = store.claim(receipt.run);
        let direct_unchanged = serde_json::to_value(store.for_run(receipt.run).unwrap().unwrap())
            .unwrap()
            == serde_json::to_value(&observed).unwrap()
            && store.show(receipt.item).unwrap().item.version == version;
        let result = store.claim_reconciliation(&observed, None);
        let after = store.for_run(receipt.run).unwrap().unwrap();
        let released = store.claim_reconciliation(&observed, None);
        (
            matches!(direct, Err(WorkItemError::PrivateInputsRecoveryRequired)),
            direct_unchanged,
            matches!(result, Err(WorkItemError::PrivateInputsRecoveryRequired)),
            after.state == WorkItemAttemptState::Attention
                && after.diagnostic.as_deref() == Some("owned_flow_private_inputs_unavailable"),
            store.show(receipt.item).unwrap().item.version == version + 1,
            store.execution_control(receipt.run).unwrap().is_none(),
            claim_token(&store, receipt.run) == token
                && audit(&store, receipt.operation_id) == immutable,
            private_bytes(home.path()) == bytes && refusal_rows(&store) == (0, 0),
            matches!(released, Ok(None)),
        )
    };
    home.close().unwrap();
    assert_eq!(
        facts,
        (true, true, true, true, true, true, true, true, true)
    );
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn automatic_private_failure_preserves_newer_operator_control_at_actual_ready_gate() {
    let home = FixtureHome::new().unwrap();
    let facts = {
        let storage = crate::runs::Storage::open(home.path()).await.unwrap();
        let store = storage.work_items();
        let receipt = unavailable_private_attempt(&store, home.path());
        let observed = store.for_run(receipt.run).unwrap().unwrap();
        let token = claim_token(&store, receipt.run);
        let gate = super::super::refusal_owner::ready_gate(receipt.run);
        let worker_store = store.clone();
        let worker = std::thread::spawn(move || worker_store.claim_reconciliation(&observed, None));
        gate.wait_entered();
        let current = store.show(receipt.item).unwrap();
        let command = WorkItemCommand::Suspend {
            operation_id: WorkItemOperationId::new(),
            item: receipt.item,
            expected_version: current.item.version,
        };
        store
            .mutate(&command, None, None, "private-unavailable-race", 100)
            .unwrap();
        let control = store.execution_control(receipt.run).unwrap().unwrap();
        let attempt = store.for_run(receipt.run).unwrap().unwrap();
        let version = store.show(receipt.item).unwrap().item.version;
        gate.release();
        let result = worker.join().unwrap();
        (
            matches!(result, Ok(None)),
            store.execution_control(receipt.run).unwrap() == Some(control),
            serde_json::to_value(store.for_run(receipt.run).unwrap()).unwrap()
                == serde_json::to_value(Some(attempt)).unwrap(),
            store.show(receipt.item).unwrap().item.version == version,
            claim_token(&store, receipt.run) == token,
            refusal_rows(&store) == (0, 0),
        )
    };
    home.close().unwrap();
    assert_eq!(facts, (true, true, true, true, true, true));
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn private_attention_sql_abort_preserves_facts_and_releases_actual_original_owner() {
    let home = FixtureHome::new().unwrap();
    let facts = {
        let storage = crate::runs::Storage::open(home.path()).await.unwrap();
        let store = storage.work_items();
        let receipt = unavailable_private_attempt(&store, home.path());
        let observed = store.for_run(receipt.run).unwrap().unwrap();
        let version = store.show(receipt.item).unwrap().item.version;
        let token = claim_token(&store, receipt.run);
        store.pool.get().unwrap().execute_batch("CREATE TRIGGER block_private_attention BEFORE UPDATE OF state ON work_item_attempts WHEN NEW.state='attention' BEGIN SELECT RAISE(ABORT,'fixed private attention abort'); END;").unwrap();
        let first = store.claim_reconciliation(&observed, None);
        let unchanged = serde_json::to_value(store.for_run(receipt.run).unwrap()).unwrap()
            == serde_json::to_value(Some(observed.clone())).unwrap()
            && store.show(receipt.item).unwrap().item.version == version
            && claim_token(&store, receipt.run) == token
            && refusal_rows(&store) == (0, 0);
        store
            .pool
            .get()
            .unwrap()
            .execute_batch("DROP TRIGGER block_private_attention")
            .unwrap();
        let second = store.claim_reconciliation(&observed, None);
        (
            matches!(first,Err(WorkItemError::Sql(rusqlite::Error::SqliteFailure(_,Some(ref message)))) if message=="fixed private attention abort"),
            unchanged,
            matches!(second, Err(WorkItemError::PrivateInputsRecoveryRequired)),
            store.for_run(receipt.run).unwrap().unwrap().state == WorkItemAttemptState::Attention,
        )
    };
    home.close().unwrap();
    assert_eq!(facts, (true, true, true, true));
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn automatic_reconciliation_late_private_loss_commits_attention_without_claim() {
    let home = FixtureHome::new().unwrap();
    let facts = {
        let storage = crate::runs::Storage::open(home.path()).await.unwrap();
        let store = storage.work_items();
        let (receipt, claim) = fresh_private_attempt(&store, home.path());
        drop(claim);
        let observed = store.for_run(receipt.run).unwrap().unwrap();
        let token = claim_token(&store, receipt.run);
        let version = store.show(receipt.item).unwrap().item.version;
        let gate = super::super::launch::claim_gate(receipt.run);
        let worker_store = store.clone();
        let worker = std::thread::spawn(move || worker_store.claim_reconciliation(&observed, None));
        gate.wait_entered();
        std::fs::remove_file(
            home.path()
                .join("work-items/private-flow-inputs")
                .join(KEY_NAME),
        )
        .unwrap();
        gate.release();
        let result = worker.join().unwrap();
        (
            matches!(result, Err(WorkItemError::PrivateInputsRecoveryRequired)),
            store.for_run(receipt.run).unwrap().unwrap().state == WorkItemAttemptState::Attention,
            store.show(receipt.item).unwrap().item.version == version + 1,
            claim_token(&store, receipt.run) == token,
            store.execution_control(receipt.run).unwrap().is_none(),
            refusal_rows(&store) == (0, 0),
        )
    };
    home.close().unwrap();
    assert_eq!(facts, (true, true, true, true, true, true));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn private_attention_preserves_normalized_usage_distinct_from_raw_attempt_payload() {
    let home = FixtureHome::new().unwrap();
    let facts = {
        let storage = crate::runs::Storage::open(home.path()).await.unwrap();
        let store = storage.work_items();
        let receipt = unavailable_private_attempt(&store, home.path());
        let conn = store.pool.get().unwrap();
        conn.execute("UPDATE work_item_attempts SET usage_seq=7,input_tokens=11,output_tokens=13,known_cost_usd=17.5,usage_unknown=0 WHERE run=?",[receipt.run.to_string()]).unwrap();
        let raw: String = conn
            .query_row(
                "SELECT payload FROM work_item_attempts WHERE run=?",
                [receipt.run.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        let original: WorkItemAttempt = serde_json::from_str(&raw).unwrap();
        drop(conn);
        let observed = store.for_run(receipt.run).unwrap().unwrap();
        let independent_projection = original.input_tokens == 0
            && original.usage_seq == 0
            && observed.usage_seq == 7
            && observed.input_tokens == 11
            && observed.output_tokens == 13
            && observed.known_cost_usd == 17.5
            && !observed.usage_unknown;
        let token = claim_token(&store, receipt.run);
        let result = store.claim_reconciliation(&observed, None);
        let after = store.for_run(receipt.run).unwrap().unwrap();
        (
            independent_projection,
            matches!(result, Err(WorkItemError::PrivateInputsRecoveryRequired)),
            after.state == WorkItemAttemptState::Attention,
            after.usage_seq == 7
                && after.input_tokens == 11
                && after.output_tokens == 13
                && after.known_cost_usd == 17.5
                && !after.usage_unknown,
            claim_token(&store, receipt.run) == token && refusal_rows(&store) == (0, 0),
            format!("result={:?}", result.err()),
        )
    };
    home.close().unwrap();
    assert!(
        facts.0 && facts.1 && facts.2 && facts.3 && facts.4,
        "{facts:?}"
    );
}

#[derive(Clone, Copy, Debug)]
enum EstablishedEntry {
    Replay,
    ColdClaim,
    Hydrate,
}
async fn bounded_established_entry(entry: EstablishedEntry) {
    let home = FixtureHome::new().unwrap();
    let facts = {
        let storage = crate::runs::Storage::open(home.path()).await.unwrap();
        let store = bounded_private_store(&storage, home.path());
        // Establish through the real public producer; do not prehydrate inputs.
        let (receipt, claim) = fresh_private_attempt(&store, home.path());
        let before = audit(&store, receipt.operation_id);
        let established = all_establishment_facts(&store);
        let bytes = private_bytes(home.path());
        let manifest = store.owned_flow_manifest(receipt.run).unwrap().unwrap();
        let mut claim = Some(claim);
        if matches!(entry, EstablishedEntry::ColdClaim) {
            drop(claim.take());
        }
        // The competitor is retained before the actual public entry, not after open.
        let held = store.pool.get().unwrap();
        let outcome = std::thread::scope(|scope| {
            scope
                .spawn(|| match entry {
                    EstablishedEntry::Replay => store
                        .replay_owned_flow(receipt.operation_id, b"original body", false)
                        .map(|value| value == Some(receipt.clone())),
                    EstablishedEntry::ColdClaim => store
                        .claim(receipt.run)
                        .map(|value| value.run() == receipt.run),
                    EstablishedEntry::Hydrate => store
                        .hydrate_owned_flow_inputs(claim.as_ref().unwrap())
                        .map(|value| {
                            value.is_some_and(|inputs| {
                                inputs.manifest() == &manifest && inputs.servers().len() == 1
                            })
                        }),
                })
                .join()
                .unwrap()
        });
        drop(held);
        let immutable = before == audit(&store, receipt.operation_id)
            && established == all_establishment_facts(&store)
            && bytes == private_bytes(home.path());
        let facts = (
            matches!(outcome, Ok(true)),
            immutable,
            format!("{outcome:?}"),
        );
        drop(claim);
        facts
    };
    home.close().unwrap();
    assert!(
        facts.0 && facts.1,
        "healthy established public entry {entry:?}; worker joined and fixture settled: {facts:?}"
    );
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bounded_established_public_replay_releases_lookup_connection() {
    bounded_established_entry(EstablishedEntry::Replay).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bounded_established_cold_claim_releases_lookup_connection() {
    bounded_established_entry(EstablishedEntry::ColdClaim).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bounded_established_input_hydration_releases_lookup_connection() {
    bounded_established_entry(EstablishedEntry::Hydrate).await;
}
