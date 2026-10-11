//! Isolated fixture actors; privileged setup is separate from standard runtime acceptance.
use super::{RuntimeHomeOwner, test_security};
use crate::{
    PersistenceError,
    runs::{OpenError, Storage},
};
use std::{
    fs::OpenOptions,
    io::Write,
    path::Path,
    time::{Duration, Instant},
};

#[path = "privileged_fixture_tests/bootstrap.rs"]
mod bootstrap;
#[path = "privileged_fixture_tests/cleanup.rs"]
mod cleanup;
#[path = "privileged_fixture_tests/context.rs"]
mod context;
#[path = "privileged_fixture_tests/objects.rs"]
mod objects;
#[path = "privileged_fixture_tests/privileges.rs"]
mod privileges;

use context::{Context, Role};
const ADMIN: &str = "S-1-5-32-544";
const OWNER_REFUSAL: &str = "native security validation refused: object owner is not trusted";

fn inventory(path: &Path) -> Vec<std::ffi::OsString> {
    let mut entries = std::fs::read_dir(path)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<Vec<_>>();
    entries.sort();
    entries
}

fn identity(path: &Path) -> serde_json::Value {
    let identity = test_security::path_identity(path);
    let id = identity
        .id
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    serde_json::json!({"volume":identity.volume.to_string(), "file_id":id})
}

fn assert_settled(path: &Path) {
    let moved = path.with_extension("settled");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match std::fs::rename(path, &moved) {
            Ok(()) => break,
            Err(error) => {
                assert!(
                    Instant::now() < deadline,
                    "actual owners did not settle: {error}"
                );
                std::thread::sleep(Duration::from_millis(10));
            },
        }
    }
    std::fs::rename(moved, path).unwrap();
}

struct CreatedHome {
    facts: serde_json::Value,
    cleanup: cleanup::Home,
}

fn create_and_settle(home: &Path, actor: &str) -> CreatedHome {
    let parent = cleanup::Parent::capture(home.parent().unwrap());
    assert!(!home.exists());
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let owner = RuntimeHomeOwner::prepare(home).unwrap();
    let storage = runtime.block_on(Storage::open(home)).unwrap();
    let database = storage.registry_db_path();
    test_security::assert_private_for_actor(home, actor, true);
    test_security::assert_private_for_actor(&database, actor, false);
    let connection = storage.registry_pool.get().unwrap();
    let value: i64 = connection
        .query_row("SELECT 7", [], |row| row.get(0))
        .unwrap();
    assert_eq!(value, 7);
    let home_id = test_security::path_identity(home);
    let database_id = test_security::path_identity(&database);
    let facts = serde_json::json!({
        "home":objects::snapshot(home).sddl, "database":objects::snapshot(&database).sddl,
        "home_identity":identity(home), "database_identity":identity(&database), "sql":value,
    });
    drop(connection);
    drop(storage);
    drop(owner);
    drop(runtime);
    assert_settled(&database);
    assert_settled(home);
    CreatedHome {
        facts,
        cleanup: cleanup::Home::capture(parent, home, actor, home_id, database_id),
    }
}

fn standard_parent_sddl(standard: &str, explicit_owner: bool) -> String {
    let owner = if explicit_owner { "O:BA" } else { "" };
    format!("{owner}D:P(A;OICI;FA;;;BA)(A;OICI;FA;;;SY)(A;;FA;;;{standard})")
}

#[test]
#[ignore = "exact protected provisioner helper under a verified administrative-default-owner token"]
fn admin_default_owner_creation() {
    let _bootstrap = bootstrap::enter(Role::AdministrativeCreation.filter());
    let context = Context::read(Role::AdministrativeCreation);
    let (actor, default_owner) = test_security::token_principals_for_actor(&context.actor);
    assert_eq!(default_owner, ADMIN);
    assert_ne!(actor, default_owner);
    let elevated = context.data.join("admin-production");
    // No owner in the initial descriptor: the actual duplicate token supplies it.
    objects::create_directory(
        &elevated,
        &format!("D:P(A;OICI;FA;;;{actor})(A;OICI;FA;;;BA)(A;OICI;FA;;;SY)"),
    );
    assert_eq!(objects::snapshot(&elevated).owner, ADMIN);
    let created = create_and_settle(&elevated.join("state"), &actor);
    created.cleanup.remove();
    let standard = context.standard_data.join("standard-production");
    objects::create_directory(&standard, &standard_parent_sddl(&context.standard, false));
    let ancestor = objects::snapshot(&standard);
    assert_eq!(ancestor.owner, ADMIN);
    context.complete("stage1 actual administrative TokenOwner production creation=PASS",
        serde_json::json!({"token_user":actor,"token_owner":default_owner,
            "objects":created.facts,"cleanup":{"runtime_home_removed":true,"runtime_home_identity":created.facts["home_identity"]},"standard_ancestor":ancestor.sddl,"standard_identity":identity(&standard)}));
}

#[test]
#[ignore = "exact protected provisioner helper with SeRestorePrivilege on a separate duplicate token"]
fn provision_foreign_owner_fixture() {
    let _bootstrap = bootstrap::enter(Role::ForeignSetup.filter());
    let context = Context::read(Role::ForeignSetup);
    privileges::assert_restore_privilege();
    let parent = context.data.join("foreign-production");
    objects::create_directory(&parent, &standard_parent_sddl(&context.standard, true));
    let ancestor = objects::snapshot(&parent);
    assert_eq!(ancestor.owner, ADMIN);
    let home = parent.join("state");
    // The setup's retained READ_CONTROL observation closes before standard runtime tests.
    let observed =
        objects::create_foreign_home(&home, &context.actor, &context.standard, &context.foreign);
    context.complete(
        "stage1 foreign-owner fixture exact user ACL setup=PASS",
        serde_json::json!({"home":observed.sddl,"foreign_sid":context.foreign,
            "privilege":"SeRestorePrivilege","parent":ancestor.sddl}),
    );
}

#[test]
#[ignore = "mandatory standard-user creation beneath an actual administrative ancestor"]
fn standard_user_below_admin_ancestor() {
    let context = Context::read(Role::StandardCreation);
    let parent = context.data.join("standard-production");
    let parent_fence = cleanup::Parent::capture(&parent);
    let before = objects::snapshot(&parent);
    assert_eq!(before.owner, ADMIN);
    let parent_identity = test_security::path_identity(&parent);
    // Standard actor proves real child-write access with its own pre-production sentinel.
    let sentinel = parent.join("sentinel");
    objects::create_sentinel(
        &sentinel,
        &context.actor,
        b"administrative ancestor unchanged",
    );
    let sentinel_object = cleanup::Object::capture(&sentinel, false);
    let entries = inventory(&parent);
    let created = create_and_settle(&parent.join("state"), &context.actor);
    assert_eq!(objects::snapshot(&parent), before);
    assert_eq!(test_security::path_identity(&parent), parent_identity);
    assert_eq!(
        std::fs::read(&sentinel).unwrap(),
        b"administrative ancestor unchanged"
    );
    let mut expected = entries;
    expected.push("state".into());
    expected.sort();
    assert_eq!(inventory(&parent), expected);
    created.cleanup.remove();
    sentinel_object.remove_exact();
    parent_fence.verify();
    assert!(inventory(&parent).is_empty());
    context.complete(
        "stage1 standard-user creation below administrative ancestor=PASS",
        serde_json::json!({"objects":created.facts,"cleanup":{"runtime_home_removed":true,"runtime_home_identity":created.facts["home_identity"],"sentinel_removed":true}}),
    );
}

#[test]
#[ignore = "mandatory standard-user exact foreign-owner unchanged refusal with same-route positive control"]
fn foreign_owner_refuses_before_initialization() {
    let context = Context::read(Role::ForeignRefusal);
    let parent = context.data.join("foreign-production");
    let _parent = test_security::open_observer(&parent);
    let parent_before = objects::snapshot(&parent);
    assert_eq!(parent_before.owner, ADMIN);
    let parent_identity = test_security::path_identity(&parent);
    let control = parent.join("control-state");
    let positive = create_and_settle(&control, &context.actor);
    let control_before = objects::snapshot(&control);
    control_before.exact_user_directory(&context.actor, &context.standard);
    let control_identity = test_security::path_identity(&control);
    let home = parent.join("state");
    let sentinel = home.join("sentinel");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&sentinel)
        .unwrap();
    file.write_all(b"foreign-owner existing sentinel").unwrap();
    file.sync_all().unwrap();
    drop(file);
    let home_identity = test_security::path_identity(&home);
    let sentinel_identity = objects::regular_sentinel_identity(&sentinel);
    let home_object = cleanup::Object::capture(&home, true);
    let sentinel_object = cleanup::Object::capture(&sentinel, false);
    let before = objects::snapshot(&home);
    before.exact_user_directory(&context.foreign, &context.standard);
    assert_eq!(
        before.dacl(),
        control_before.dacl(),
        "only owner policy differs"
    );
    let entries = inventory(&home);
    let siblings = inventory(&parent);
    assert!(!home.join("db").exists() && !home.join("runs").exists());
    assert!(matches!(RuntimeHomeOwner::prepare(&home),
        Err(PersistenceError::StateHome { category:"security refusal", message }) if message == OWNER_REFUSAL));
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    assert!(matches!(runtime.block_on(Storage::open(&home)),
        Err(OpenError::StateHome { category:"security refusal", message }) if message == OWNER_REFUSAL));
    drop(runtime);
    assert_eq!(objects::snapshot(&home), before);
    assert_eq!(inventory(&home), entries);
    assert_eq!(inventory(&parent), siblings);
    assert_eq!(objects::snapshot(&control), control_before);
    assert_eq!(test_security::path_identity(&control), control_identity);
    assert_eq!(objects::snapshot(&parent), parent_before);
    assert_eq!(test_security::path_identity(&parent), parent_identity);
    assert_eq!(
        std::fs::read(&sentinel).unwrap(),
        b"foreign-owner existing sentinel"
    );
    assert!(!home.join("db").exists() && !home.join("runs").exists());
    let facts = serde_json::json!({"home":before.sddl,"foreign_sid":context.foreign,
        "positive_control":positive.facts, "foreign_home_identity":identity(&home), "control_identity":identity(&control), "same_parent":true});
    cleanup_foreign_home(
        &home,
        &context,
        &before,
        home_identity,
        sentinel_identity,
        home_object,
        sentinel_object,
    );
    assert_eq!(test_security::path_identity(&control), control_identity);
    positive.cleanup.remove();
    assert!(inventory(&parent).is_empty());
    context.complete(
        "stage1 foreign-owner exact user ACL unchanged security refusal=PASS",
        serde_json::json!({"refusal":facts,"cleanup":{"sentinel_removed":true,"foreign_home_removed":true,"foreign_home_identity":facts["foreign_home_identity"],"control_home_removed":true,"control_home_identity":facts["positive_control"]["home_identity"]}}),
    );
}

fn cleanup_foreign_home(
    home: &Path,
    context: &Context,
    original: &objects::Snapshot,
    identity: test_security::FileIdentity,
    sentinel_identity: test_security::FileIdentity,
    home_object: cleanup::Object,
    sentinel_object: cleanup::Object,
) {
    assert_eq!(test_security::path_identity(home), identity);
    let observed = objects::snapshot(home);
    observed.exact_user_directory(&context.foreign, &context.standard);
    assert_eq!(&observed, original);
    assert_eq!(inventory(home), vec![std::ffi::OsString::from("sentinel")]);
    let sentinel = home.join("sentinel");
    assert_eq!(
        objects::regular_sentinel_identity(&sentinel),
        sentinel_identity
    );
    // Revalidate again on the actual DELETE handles; never delete by pathname.
    home_object.require_identity(identity);
    sentinel_object.require_identity(sentinel_identity);
    cleanup::remove_singleton(home_object, sentinel_object);
    assert!(!home.try_exists().unwrap());
}
