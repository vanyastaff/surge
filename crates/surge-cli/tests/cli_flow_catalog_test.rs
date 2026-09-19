//! Integration tests for `surge flow` (ADR-0020, T8).
//!
//! The catalog is project → home → bundled; these tests pin the listing, the
//! shadowing order and the named failure for an unresolvable reference.

use assert_cmd::Command;
use predicates::str::contains;
use std::path::Path;

fn surge(home: &Path) -> Command {
    let mut command = Command::cargo_bin("surge").unwrap();
    command.env("HOME", home).env("USERPROFILE", home);
    command
}

fn write_template(dir: &Path, file: &str, name: &str, when: &str) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(
        dir.join(file),
        format!(
            r#"schema_version = 1
start = "only"

[metadata]
name = "{name}"
created_at = "2026-01-01T00:00:00Z"
when_to_use = "{when}"

[nodes.only]
id = "only"
nodes.only.declared_outcomes = []

[nodes.only.position]
x = 0.0
y = 0.0

[nodes.only.config]
node_kind = "terminal"

[nodes.only.config.kind]
type = "success"

[[edges]]
id = "e1"
to = "only"
kind = "forward"

[edges.from]
node = "only"
outcome = "done"

[edges.policy]
on_max_exceeded = "escalate"
"#
        ),
    )
    .unwrap();
}

#[test]
fn list_shows_bundled_templates_with_when_to_use() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();

    surge(&home)
        .args(["flow", "list"])
        .current_dir(temp.path())
        .assert()
        .success()
        .stdout(contains("bug-fix@1.0"))
        .stdout(contains("A bug with a reproduction"));
}

#[test]
fn project_template_shadows_bundled_and_is_listed() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    write_template(
        &temp.path().join(".surge/flows"),
        "bug-fix-1.0.toml",
        "bug-fix",
        "our own bug flow",
    );

    surge(&home)
        .args(["flow", "show", "bug-fix@1"])
        .current_dir(temp.path())
        .assert()
        .success()
        .stdout(contains("layer:       project"))
        .stdout(contains("when_to_use: our own bug flow"));
}

#[test]
fn unknown_reference_is_a_named_error() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();

    surge(&home)
        .args(["flow", "show", "nope@1"])
        .current_dir(temp.path())
        .assert()
        .failure()
        .stderr(contains("no flow template matches nope@1"));
}

#[test]
fn malformed_reference_is_refused_before_lookup() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();

    surge(&home)
        .args(["flow", "show", "bug-fix"])
        .current_dir(temp.path())
        .assert()
        .failure()
        .stderr(contains("empty version portion"));
}

#[test]
fn json_listing_carries_layer_and_path() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    write_template(
        &temp.path().join(".surge/flows"),
        "custom-2.0.toml",
        "custom",
        "custom fit",
    );

    let output = surge(&home)
        .args(["flow", "list", "--json"])
        .current_dir(temp.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let rows = parsed.as_array().unwrap();
    let custom = rows
        .iter()
        .find(|row| row["reference"] == "custom@2.0")
        .expect("custom@2.0 listed");
    assert_eq!(custom["layer"], "project");
    assert_eq!(custom["when_to_use"], "custom fit");
    assert!(
        custom["path"]
            .as_str()
            .unwrap()
            .ends_with("custom-2.0.toml")
    );
}
