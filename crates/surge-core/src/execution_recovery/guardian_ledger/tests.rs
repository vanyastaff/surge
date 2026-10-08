use super::*;
use serde_json::Value;

fn vectors() -> Vec<Value> {
    serde_json::from_str(include_str!("literal_vectors.json")).unwrap()
}

#[test]
fn frozen_independent_vectors_match_wire_preimage_and_digest() {
    for fixture in vectors() {
        let wire = fixture["record"].clone();
        let record: WindowsGuardianRecord = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(serde_json::to_value(&record).unwrap(), wire);
        let literal = hex::decode(fixture["preimage"].as_str().unwrap()).unwrap();
        assert_eq!(record.canonical_bytes(), literal, "{}", fixture["name"]);
        assert_eq!(record.hash().to_hex(), fixture["sha256"].as_str().unwrap());
    }
}

#[test]
fn mandatory_nullable_fields_cannot_disappear() {
    let genesis = vectors()
        .into_iter()
        .find(|f| f["name"] == "genesis")
        .unwrap();
    assert!(serde_json::from_value::<WindowsGuardianRecord>(genesis["record"].clone()).is_ok());
    for key in ["predecessor", "lease"] {
        let mut wire = genesis["record"].clone();
        wire.as_object_mut().unwrap().remove(key);
        assert!(
            serde_json::from_value::<WindowsGuardianRecord>(wire).is_err(),
            "missing {key}"
        );
    }
}

fn fixture(name: &str) -> Value {
    vectors()
        .into_iter()
        .find(|value| value["name"] == name)
        .unwrap()["record"]
        .clone()
}
fn reject(name: &str, pointer: &str, invalid: Value) {
    let mut wire = fixture(name);
    assert!(serde_json::from_value::<WindowsGuardianRecord>(wire.clone()).is_ok());
    *wire.pointer_mut(pointer).unwrap() = invalid;
    assert!(
        serde_json::from_value::<WindowsGuardianRecord>(wire).is_err(),
        "{name} {pointer}"
    );
}
fn maps(value: &Value, path: &str, output: &mut Vec<String>) {
    if let Some(map) = value.as_object() {
        output.push(path.to_owned());
        for (key, nested) in map {
            maps(nested, &format!("{path}/{key}"), output);
        }
    }
}
#[test]
fn every_map_rejects_extra_and_missing_fields() {
    for vector in vectors() {
        let wire = &vector["record"];
        let mut paths = Vec::new();
        maps(wire, "", &mut paths);
        for path in paths {
            let mut extra = wire.clone();
            extra
                .pointer_mut(&path)
                .unwrap()
                .as_object_mut()
                .unwrap()
                .insert("unexpected".into(), Value::Bool(true));
            assert!(
                serde_json::from_value::<WindowsGuardianRecord>(extra).is_err(),
                "extra {} {path}",
                vector["name"]
            );
            for key in wire.pointer(&path).unwrap().as_object().unwrap().keys() {
                let mut missing = wire.clone();
                missing
                    .pointer_mut(&path)
                    .unwrap()
                    .as_object_mut()
                    .unwrap()
                    .remove(key);
                assert!(
                    serde_json::from_value::<WindowsGuardianRecord>(missing).is_err(),
                    "missing {} {path}/{key}",
                    vector["name"]
                );
            }
        }
    }
}

#[test]
fn invalid_local_bindings_and_phase_shapes_refuse() {
    use serde_json::json;
    for (name, pointer, bad) in [
        ("genesis", "/lease", json!(1)),
        (
            "genesis",
            "/predecessor",
            fixture("bound")["predecessor"].clone(),
        ),
        ("cancelled", "/lease", json!(1)),
        ("bound", "/lease", json!(2)),
        ("child", "/lease", Value::Null),
        ("bound", "/body/host/pid", json!(7)),
        ("takeover", "/lease", json!(3)),
        ("takeover", "/body/registry_generation", json!(6)),
        (
            "takeover",
            "/body/host",
            fixture("takeover")["body"]["previous_host"].clone(),
        ),
        ("takeover", "/body/authorization/host/pid", json!(44)),
        ("takeover", "/body/authorization/lease", json!(2)),
        ("takeover", "/body/previous_lease", json!(u64::MAX)),
        (
            "bound",
            "/body/binding/occurrence",
            json!("00000000000000000000000004"),
        ),
        (
            "child",
            "/context/occurrence",
            json!("00000000000000000000000004"),
        ),
        (
            "child_job_empty",
            "/context/authority/executable_hash",
            json!(format!("sha256:{}", "1".repeat(64))),
        ),
        (
            "resume_started",
            "/body/outcome/previous_suspend_count",
            json!(2),
        ),
        ("resume_failed", "/body/outcome/win32_error", json!(0)),
        (
            "consumed",
            "/body/settlement_sequence",
            json!(i64::MAX as u64 + 1),
        ),
    ] {
        reject(name, pointer, bad);
    }
}

#[test]
fn strict_scalar_spellings_and_numbers_refuse_aliases() {
    use serde_json::json;
    for pointer in ["/context/run", "/context/invocation", "/context/occurrence"] {
        for bad in [
            "00000000000000000000000000",
            "run-00000000000000000000000001",
            "01arz3ndektsv4rrffq69g5fav",
            "81ARZ3NDEKTSV4RRFFQ69G5FAV",
        ] {
            reject("genesis", pointer, json!(bad));
        }
    }
    for bad in [
        "",
        "0",
        "00000000000000000000000000000000",
        "ABCDEF00000000000000000000000001",
    ] {
        reject("genesis", "/operation", json!(bad));
    }
    for bad in [
        "0".repeat(64),
        format!("sha256:{}", "A".repeat(64)),
        format!("SHA256:{}", "0".repeat(64)),
    ] {
        reject("bound", "/predecessor", json!(bad));
    }
    for pointer in ["/lease", "/body/registry_generation"] {
        for bad in [json!(0), json!(-1), json!(1.5), json!("1")] {
            reject("bound", pointer, bad);
        }
    }
    for bad in [json!(0), json!(2), json!(1.0), json!("1")] {
        reject("genesis", "/version", bad);
    }
    let literal = serde_json::to_string(&fixture("genesis")).unwrap();
    let overflow = literal.replace(
        "\"registry_generation\":6",
        "\"registry_generation\":18446744073709551616",
    );
    assert_ne!(overflow, literal);
    assert!(serde_json::from_str::<WindowsGuardianRecord>(&overflow).is_err());
    let duplicate = literal.replacen("\"version\":1", "\"version\":1,\"version\":1", 1);
    assert!(serde_json::from_str::<WindowsGuardianRecord>(&duplicate).is_err());
}

#[test]
fn scalar_constructors_and_valid_extremes_match_the_wire_contract() {
    assert!(GuardianOperationId::new([0; 16]).is_err());
    assert!(GuardianLease::new(0).is_err());
    assert!(GuardianRegistryGeneration::new(0).is_err());
    assert!(GuardianBarrierGeneration::new(0).is_err());
    assert!(GuardianQueryGeneration::new(0).is_err());
    assert!(GuardianRevocationGeneration::new(0).is_err());
    assert!(GuardianHostObservationGeneration::new(0).is_err());
    assert!(GuardianJournalSequence::new(0).is_err());
    assert!(GuardianJournalSequence::new(i64::MAX as u64 + 1).is_err());
    assert_eq!(
        GuardianJournalSequence::new(i64::MAX as u64).unwrap().get(),
        i64::MAX as u64
    );
    assert_eq!(GuardianLease::new(u64::MAX).unwrap().get(), u64::MAX);
    let mut wire = fixture("takeover");
    wire["body"]["host"]["pid"] = wire["body"]["previous_host"]["pid"].clone();
    assert!(
        serde_json::from_value::<WindowsGuardianRecord>(wire).is_ok(),
        "PID reuse with changed FILETIME is only shape-valid"
    );
}

#[test]
fn every_literal_hash_is_stable_across_json_layout_and_sensitive_to_operation() {
    for vector in vectors() {
        let wire = vector["record"].clone();
        let compact: WindowsGuardianRecord = serde_json::from_str(&wire.to_string()).unwrap();
        let pretty: WindowsGuardianRecord =
            serde_json::from_str(&serde_json::to_string_pretty(&wire).unwrap()).unwrap();
        assert_eq!(compact.hash(), pretty.hash());
        let mut changed = wire;
        changed["operation"] = Value::String("ffffffffffffffffffffffffffffffff".into());
        let changed: WindowsGuardianRecord = serde_json::from_value(changed).unwrap();
        assert_ne!(changed.hash(), compact.hash());
    }
}
