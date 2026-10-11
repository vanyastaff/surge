//! CLI smoke: `surge feature` exposes the roadmap amendment entrypoint.

mod runtime_home_fixture {
    #[cfg(windows)]
    use surge_persistence::RuntimeHomeOwner;
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../scripts/test-support/runtime_home.rs"
    ));
}
use runtime_home_fixture::FixtureHome;

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
    let home = FixtureHome::new().unwrap();
    let surge_home = home.path();

    Command::cargo_bin("surge")
        .unwrap()
        .args(["feature", "list", "--all-projects", "--json"])
        .env("SURGE_HOME", surge_home)
        .assert()
        .success()
        .stdout(contains("[]"));
    home.close().unwrap();
}

#[test]
fn feature_show_missing_patch_reports_error() {
    let home = FixtureHome::new().unwrap();
    let surge_home = home.path();

    Command::cargo_bin("surge")
        .unwrap()
        .args(["feature", "show", "rpatch-missing"])
        .env("SURGE_HOME", surge_home)
        .assert()
        .failure()
        .stderr(contains("roadmap patch not found"));
    home.close().unwrap();
}
