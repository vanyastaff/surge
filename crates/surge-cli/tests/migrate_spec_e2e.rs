//! End-to-end tests for `surge migrate-spec`.
//!
//! Each test builds a legacy `Spec` programmatically, persists it as a
//! `.spec.toml` via a local DTO that mirrors the on-disk shape the
//! `surge-spec` crate used to own, runs the `surge` binary against it, and
//! snapshots the resulting `flow.toml` via `insta`. The mapping reassigns
//! subtask IDs to `sN`, so output is deterministic regardless of the input
//! ULIDs.

use std::path::Path;

use assert_cmd::Command;
use serde::Serialize;
use surge_core::spec::{Complexity, Spec, Subtask};
use surge_core::id::SubtaskId;
use tempfile::TempDir;

/// Minimal serializer for the legacy `.spec.toml` shape — duplicated here
/// because the binary's `legacy_spec` module is not exported.
#[derive(Serialize)]
struct LegacySpecFile {
    spec: Spec,
}

fn write_spec(dir: &TempDir, name: &str, spec: Spec) -> std::path::PathBuf {
    let path = dir.path().join(format!("{name}.spec.toml"));
    let content = toml::to_string_pretty(&LegacySpecFile { spec }).unwrap();
    std::fs::write(&path, content).unwrap();
    path
}

fn run_migrate(input: &Path) -> (i32, String) {
    let output = Command::cargo_bin("surge")
        .unwrap()
        .arg("migrate-spec")
        .arg(input)
        .output()
        .unwrap();

    let code = output.status.code().unwrap_or(-1);
    let stdout = String::from_utf8(output.stdout).unwrap();
    (code, stdout)
}

fn subtask_with(title: &str, agent: Option<&str>, deps: Vec<SubtaskId>) -> Subtask {
    let mut s = Subtask::new(title, format!("Body of {title}"), Complexity::Simple);
    s.depends_on = deps;
    s.agent = agent.map(str::to_string);
    s
}

fn build_spec(title: &str, subtasks: Vec<Subtask>) -> Spec {
    let mut spec = Spec::new(title, format!("{title} description"), Complexity::Standard);
    spec.subtasks = subtasks;
    spec
}

#[test]
fn single_subtask() {
    let dir = TempDir::new().unwrap();
    let spec = build_spec(
        "Single",
        vec![subtask_with("Build", Some("implementer@1.0"), vec![])],
    );
    let path = write_spec(&dir, "single", spec);

    let (code, stdout) = run_migrate(&path);

    assert_eq!(code, 0, "expected exit 0 (no warnings); got {code}\n{stdout}");
    insta::assert_snapshot!("single", stdout);
}

#[test]
fn linear_chain() {
    let dir = TempDir::new().unwrap();
    let a = subtask_with("Step A", Some("implementer@1.0"), vec![]);
    let b = subtask_with("Step B", Some("implementer@1.0"), vec![a.id]);
    let c = subtask_with("Step C", Some("implementer@1.0"), vec![b.id]);
    let spec = build_spec("Linear", vec![a, b, c]);
    let path = write_spec(&dir, "linear", spec);

    let (code, stdout) = run_migrate(&path);

    assert_eq!(code, 0, "expected exit 0; got {code}\n{stdout}");
    insta::assert_snapshot!("linear_chain", stdout);
}

#[test]
fn fan_out() {
    let dir = TempDir::new().unwrap();
    let a = subtask_with("Root", Some("implementer@1.0"), vec![]);
    let b = subtask_with("Left", Some("implementer@1.0"), vec![a.id]);
    let c = subtask_with("Right", Some("implementer@1.0"), vec![a.id]);
    let spec = build_spec("FanOut", vec![a, b, c]);
    let path = write_spec(&dir, "fan_out", spec);

    let (code, stdout) = run_migrate(&path);

    assert_eq!(code, 0, "expected exit 0; got {code}\n{stdout}");
    insta::assert_snapshot!("fan_out", stdout);
}

#[test]
fn diamond_emits_warning_exit_2() {
    let dir = TempDir::new().unwrap();
    let a = subtask_with("Root", Some("implementer@1.0"), vec![]);
    let b = subtask_with("Left", Some("implementer@1.0"), vec![a.id]);
    let c = subtask_with("Right", Some("implementer@1.0"), vec![a.id]);
    let b_id = b.id;
    let c_id = c.id;
    let d = subtask_with("Merge", Some("implementer@1.0"), vec![b_id, c_id]);
    let spec = build_spec("Diamond", vec![a, b, c, d]);
    let path = write_spec(&dir, "diamond", spec);

    let (code, stdout) = run_migrate(&path);

    assert_eq!(
        code, 2,
        "expected exit 2 (fan-in warning without --allow-warnings); got {code}\n{stdout}",
    );
    insta::assert_snapshot!("diamond", stdout);
}

#[test]
fn diamond_allow_warnings_exit_0() {
    let dir = TempDir::new().unwrap();
    let a = subtask_with("Root", Some("implementer@1.0"), vec![]);
    let b = subtask_with("Left", Some("implementer@1.0"), vec![a.id]);
    let c = subtask_with("Right", Some("implementer@1.0"), vec![a.id]);
    let b_id = b.id;
    let c_id = c.id;
    let d = subtask_with("Merge", Some("implementer@1.0"), vec![b_id, c_id]);
    let spec = build_spec("Diamond", vec![a, b, c, d]);
    let path = write_spec(&dir, "diamond_allow", spec);

    let output = Command::cargo_bin("surge")
        .unwrap()
        .arg("migrate-spec")
        .arg(&path)
        .arg("--allow-warnings")
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "expected exit 0 with --allow-warnings; got {}",
        output.status,
    );
}

#[test]
fn no_profile_defaults_with_warning() {
    let dir = TempDir::new().unwrap();
    let spec = build_spec("NoProfile", vec![subtask_with("Solo", None, vec![])]);
    let path = write_spec(&dir, "no_profile", spec);

    let (code, stdout) = run_migrate(&path);

    assert_eq!(
        code, 2,
        "expected exit 2 (profile defaulted warning); got {code}\n{stdout}",
    );
    insta::assert_snapshot!("no_profile", stdout);
}

#[test]
fn output_flag_writes_file() {
    let dir = TempDir::new().unwrap();
    let spec = build_spec(
        "Outfile",
        vec![subtask_with("Only", Some("implementer@1.0"), vec![])],
    );
    let spec_path = write_spec(&dir, "outfile", spec);
    let out_path = dir.path().join("out.flow.toml");

    let output = Command::cargo_bin("surge")
        .unwrap()
        .arg("migrate-spec")
        .arg(&spec_path)
        .arg("--output")
        .arg(&out_path)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "expected exit 0; got {}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(out_path.exists(), "output file was not created");
    let body = std::fs::read_to_string(&out_path).unwrap();
    assert!(body.contains("schema_version = 1"));
    assert!(body.contains("[nodes.s1]"));
}

#[test]
fn missing_input_fails() {
    let output = Command::cargo_bin("surge")
        .unwrap()
        .arg("migrate-spec")
        .arg("does-not-exist.spec.toml")
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "expected non-zero exit for missing input; got {}",
        output.status,
    );
}
