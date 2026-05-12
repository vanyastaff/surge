//! CLI smoke tests for `surge artifact schema`.

use assert_cmd::Command;
use predicates::str::contains;

#[test]
fn schema_spec_emits_valid_json_with_expected_keys() {
    let output = Command::cargo_bin("surge")
        .unwrap()
        .args(["artifact", "schema", "spec"])
        .output()
        .expect("invoke surge artifact schema spec");
    assert!(output.status.success(), "command failed: {output:?}");

    let stdout = String::from_utf8(output.stdout).expect("stdout is UTF-8");
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("stdout is valid JSON");
    let object = value.as_object().expect("schema is a JSON object");

    assert_eq!(
        object.get("title").and_then(|v| v.as_str()),
        Some("SpecArtifact")
    );
    assert!(object.contains_key("$schema"));
    assert!(
        object
            .get("$id")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .starts_with("https://surge.dev/schema/v1/"),
        "expected canonical Surge $id namespace"
    );
    assert!(
        object
            .get("properties")
            .and_then(|p| p.get("schema_version"))
            .is_some(),
        "spec schema must describe the schema_version envelope field"
    );
    assert!(object.contains_key("$defs"), "spec schema must inline Spec/Subtask/etc.");
    assert_eq!(
        object.get("x-surge-schema-version").and_then(|v| v.as_u64()),
        Some(u64::from(surge_core::ARTIFACT_SCHEMA_VERSION))
    );
}

#[test]
fn schema_all_emits_every_kind_in_one_object() {
    let output = Command::cargo_bin("surge")
        .unwrap()
        .args(["artifact", "schema", "--all"])
        .output()
        .expect("invoke surge artifact schema --all");
    assert!(output.status.success(), "command failed: {output:?}");

    let stdout = String::from_utf8(output.stdout).expect("stdout is UTF-8");
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("stdout is valid JSON");
    let object = value.as_object().expect("--all returns a JSON object");

    for key in [
        "description",
        "requirements",
        "roadmap",
        "roadmap-patch",
        "spec",
        "adr",
        "story",
        "plan",
        "flow",
    ] {
        assert!(object.contains_key(key), "--all must include key {key}");
    }

    let spec = object.get("spec").and_then(|v| v.as_object()).unwrap();
    assert!(spec.contains_key("$id"));

    let plan = object.get("plan").and_then(|v| v.as_object()).unwrap();
    assert_eq!(
        plan.get("x-surge-no-json-schema").and_then(|v| v.as_bool()),
        Some(true),
        "markdown-only kinds report no-schema sentinel under --all"
    );
}

#[test]
fn schema_plan_reports_markdown_only_error() {
    Command::cargo_bin("surge")
        .unwrap()
        .args(["artifact", "schema", "plan"])
        .assert()
        .failure()
        .stderr(contains("no JSON schema for plan"))
        .stderr(contains("## Settings"))
        .stderr(contains("## Tasks"));
}
