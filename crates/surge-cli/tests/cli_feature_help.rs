//! CLI smoke: `surge feature` exposes the roadmap amendment entrypoint.

use assert_cmd::Command;
use predicates::str::contains;

#[test]
fn feature_help_lists_describe() {
    Command::cargo_bin("surge")
        .unwrap()
        .args(["feature", "--help"])
        .assert()
        .success()
        .stdout(contains("describe"))
        .stdout(contains("list"))
        .stdout(contains("show"))
        .stdout(contains("reject"));
}

#[test]
fn feature_describe_help_lists_target_and_output_flags() {
    Command::cargo_bin("surge")
        .unwrap()
        .args(["feature", "describe", "--help"])
        .assert()
        .success()
        .stdout(contains("--run"))
        .stdout(contains("--project"))
        .stdout(contains("--worktree"))
        .stdout(contains("--approval"))
        .stdout(contains("--conflict-choice"))
        .stdout(contains("--json"));
}

#[test]
fn feature_list_empty_registry_json_works() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();

    Command::cargo_bin("surge")
        .unwrap()
        .args(["feature", "list", "--all-projects", "--json"])
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .assert()
        .success()
        .stdout(contains("[]"));
}

#[test]
fn feature_show_missing_patch_reports_error() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();

    Command::cargo_bin("surge")
        .unwrap()
        .args(["feature", "show", "rpatch-missing"])
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .assert()
        .failure()
        .stderr(contains("roadmap patch not found"));
}
