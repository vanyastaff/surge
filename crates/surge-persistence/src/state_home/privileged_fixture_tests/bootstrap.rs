//! Synchronous, provisioner-bound entry barrier for the two privileged fixture actors.
#[path = "bootstrap/handles.rs"]
mod handles;
#[path = "bootstrap/paths.rs"]
mod paths;
#[path = "bootstrap/security.rs"]
mod security;

use serde::Deserialize;
use std::{
    fs::File,
    io::{Read, Write},
    path::PathBuf,
};

const LIMIT: u64 = 16 * 1024;
const PREFIX: &str = "state_home::privileged_fixture_tests::";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Output {
    handle: String,
    volume: String,
    file_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema_version: u32,
    candidate_hash: String,
    filter: String,
    nonce: String,
    actor_sid: String,
    standard_sid: String,
    foreign_sid: String,
    pid: u32,
    stdout: Output,
    stderr: Output,
    ready_handle: String,
    proceed_handle: String,
    cancel_handle: String,
}

fn env(name: &str) -> String {
    std::env::var(format!("SURGE_FIXTURE_{name}")).unwrap()
}

fn decimal(value: &str) -> u64 {
    let number: u64 = value.parse().unwrap();
    assert_eq!(number.to_string(), value, "noncanonical unsigned decimal");
    number
}

fn hex(value: &str, length: usize) {
    assert_eq!(value.len(), length);
    assert!(
        value
            .bytes()
            .all(|v| v.is_ascii_digit() || (b'a'..=b'f').contains(&v))
    );
}

impl Manifest {
    fn validate(&self, expected_filter: &str) {
        assert_eq!(self.schema_version, 1);
        assert_eq!(self.pid, std::process::id());
        assert_eq!(self.filter, expected_filter);
        assert!(
            [
                format!("{PREFIX}admin_default_owner_creation"),
                format!("{PREFIX}provision_foreign_owner_fixture")
            ]
            .contains(&self.filter)
        );
        for (key, value) in [
            ("HASH", &self.candidate_hash),
            ("FILTER", &self.filter),
            ("NONCE", &self.nonce),
            ("ACTOR_SID", &self.actor_sid),
            ("STANDARD_SID", &self.standard_sid),
            ("FOREIGN_SID", &self.foreign_sid),
        ] {
            assert_eq!(*value, env(key));
        }
        hex(&self.candidate_hash, 64);
        hex(&self.nonce, 32);
        assert_ne!(self.actor_sid, self.standard_sid);
        assert_ne!(self.actor_sid, self.foreign_sid);
        assert_ne!(self.standard_sid, self.foreign_sid);
        assert!(self.standard_sid.starts_with("S-1-5-21-"));
        assert!(self.foreign_sid.starts_with("S-1-5-21-"));
        super::test_security::token_principals_for_actor(&self.actor_sid);
        let args: Vec<_> = std::env::args_os().collect();
        assert_eq!(args.len(), 6);
        assert_eq!(PathBuf::from(&args[0]), std::env::current_exe().unwrap());
        for (actual, expected) in args[1..].iter().zip([
            "--ignored",
            "--exact",
            expected_filter,
            "--nocapture",
            "--test-threads=1",
        ]) {
            assert_eq!(actual, expected);
        }
    }
}

/// Keeps transferred descriptors and all verified path components alive through the actor.
pub(super) struct BootstrapGuard {
    _handles: handles::Transferred,
    _routes: paths::Routes,
    _manifest: File,
}

/// Must be the first statement of an ordinary synchronous `#[test]` actor.
pub(super) fn enter(expected_filter: &str) -> BootstrapGuard {
    security::reject_impersonation();
    let actor = env("ACTOR_SID");
    super::test_security::token_principals_for_actor(&actor);
    let mut routes = paths::Routes::open(&PathBuf::from(env("BOOTSTRAP")), &actor);
    let mut manifest_file = routes.open_file("bootstrap.json", false);
    let mut bytes = Vec::new();
    (&mut manifest_file)
        .take(LIMIT + 1)
        .read_to_end(&mut bytes)
        .unwrap();
    assert!(u64::try_from(bytes.len()).unwrap() <= LIMIT);
    let manifest: Manifest = serde_json::from_slice(&bytes).unwrap();
    manifest.validate(expected_filter);
    routes.require_path(&PathBuf::from(env("READY_RECEIPT")), "ready.json");
    routes.require_path(&PathBuf::from(env("RECEIPT")), "complete.json");
    let mut handles = handles::Transferred::take(&manifest);
    handles.validate(&manifest, &mut routes);
    handles.install();
    let mut ready = routes.open_file("ready.json", true);
    let receipt = serde_json::json!({
        "schema_version": 1, "phase": "ready", "candidate_hash": manifest.candidate_hash,
        "filter": manifest.filter, "nonce": manifest.nonce, "actor_sid": manifest.actor_sid,
        "pid": manifest.pid,
    });
    ready
        .write_all(&serde_json::to_vec(&receipt).unwrap())
        .unwrap();
    ready.sync_all().unwrap();
    handles.signal_and_wait();
    BootstrapGuard {
        _handles: handles,
        _routes: routes,
        _manifest: manifest_file,
    }
}

#[test]
fn manifest_rejects_duplicate_fields_and_numeric_handles() {
    let output = r#"{"handle":"4","volume":"7","file_id":"00000000000000000000000000000001"}"#;
    assert!(serde_json::from_str::<Output>(output).is_ok());
    assert!(
        serde_json::from_str::<Output>(
            &output.replace("\"handle\":\"4\"", "\"handle\":\"4\",\"handle\":\"8\"")
        )
        .is_err()
    );
    assert!(
        serde_json::from_str::<Output>(&output.replace("\"handle\":\"4\"", "\"handle\":4"))
            .is_err()
    );
}

#[test]
fn manifest_rejects_outer_duplicates_unknown_fields_and_wrong_pid_type() {
    let literal = r#"{
        "schema_version":1,"candidate_hash":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "filter":"state_home::privileged_fixture_tests::admin_default_owner_creation",
        "nonce":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","actor_sid":"S-1-5-21-1-2-3-4",
        "standard_sid":"S-1-5-21-1-2-3-5","foreign_sid":"S-1-5-21-1-2-3-6","pid":42,
        "stdout":{"handle":"4","volume":"7","file_id":"00000000000000000000000000000001"},
        "stderr":{"handle":"8","volume":"7","file_id":"00000000000000000000000000000002"},
        "ready_handle":"12","proceed_handle":"16","cancel_handle":"20"
    }"#;
    assert!(serde_json::from_str::<Manifest>(literal).is_ok());
    for bad in [
        literal.replace("\"pid\":42", "\"pid\":42,\"pid\":43"),
        literal.replace("\"pid\":42", "\"pid\":\"42\""),
        literal.replace("\"pid\":42", "\"pid\":42.5"),
        literal.replace("\"pid\":42", "\"pid\":42,\"extra\":0"),
        literal.replace("\"volume\":\"7\"", "\"volume\":\"7\",\"extra\":0"),
    ] {
        assert!(serde_json::from_str::<Manifest>(&bad).is_err());
    }
}

#[test]
fn canonical_numbers_and_hex_reject_alternate_spellings() {
    assert_eq!(decimal("0"), 0);
    assert_eq!(decimal("18446744073709551615"), u64::MAX);
    hex("00112233445566778899aabbccddeeff", 32);
    for bad in ["00", "+1", "-1", " 1", "1.0", "18446744073709551616"] {
        assert!(std::panic::catch_unwind(|| decimal(bad)).is_err());
    }
    for bad in [
        "00112233445566778899AABBCCDDEEFF",
        "00112233445566778899aabbccddeef",
        "00112233445566778899aabbccddeefg",
    ] {
        assert!(std::panic::catch_unwind(|| hex(bad, 32)).is_err());
    }
}
