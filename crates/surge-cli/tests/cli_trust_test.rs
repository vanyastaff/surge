//! Integration tests for `surge trust` (ADR-0020, T13).
//!
//! A fresh clone's `.surge/` files are unpinned; `trust list` reports it and
//! `trust accept` pins the content, which is the operator action that
//! unblocks a dispatched run.

use assert_cmd::Command;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;
use std::path::Path;

fn surge(home: &Path) -> Command {
    let mut command = Command::cargo_bin("surge").unwrap();
    command.env("HOME", home).env("USERPROFILE", home);
    command
}

fn fixture() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let repo = temp.path().join("repo");
    std::fs::create_dir_all(repo.join(".surge/profiles")).unwrap();
    std::fs::write(
        repo.join(".surge/profiles/custom-1.0.toml"),
        "role = { id = \"custom\", version = \"1.0.0\" }\n",
    )
    .unwrap();
    std::fs::write(repo.join(".surge/roadmap.toml"), "schema_version = 2\n").unwrap();
    (temp, home, repo)
}

#[test]
fn unpinned_file_is_listed_then_accept_pins_it() {
    let (_temp, home, repo) = fixture();

    surge(&home)
        .args(["trust", "list"])
        .current_dir(&repo)
        .assert()
        .success()
        .stdout(contains(".surge/profiles/custom-1.0.toml"))
        .stdout(contains("unpinned"))
        // Planning data is not gated: the roadmap must not appear.
        .stdout(predicates::str::is_match(r"(?m)^\.surge/roadmap\.toml").unwrap().not());

    surge(&home)
        .args(["trust", "accept", ".surge/profiles/custom-1.0.toml"])
        .current_dir(&repo)
        .assert()
        .success()
        .stdout(contains("pinned"));

    surge(&home)
        .args(["trust", "list"])
        .current_dir(&repo)
        .assert()
        .success()
        .stdout(contains("pinned"))
        .stdout(predicates::str::is_match("unpinned").unwrap().not());
}

#[test]
fn changed_file_reads_as_changed_until_reaccepted() {
    let (_temp, home, repo) = fixture();
    surge(&home)
        .args(["trust", "accept", "--all"])
        .current_dir(&repo)
        .assert()
        .success();

    std::fs::write(
        repo.join(".surge/profiles/custom-1.0.toml"),
        "role = { id = \"custom\", version = \"2.0.0\" }\n",
    )
    .unwrap();

    surge(&home)
        .args(["trust", "list"])
        .current_dir(&repo)
        .assert()
        .success()
        .stdout(contains("changed"));

    surge(&home)
        .args(["trust", "accept", "--all"])
        .current_dir(&repo)
        .assert()
        .success();
    surge(&home)
        .args(["trust", "list"])
        .current_dir(&repo)
        .assert()
        .success()
        .stdout(predicates::str::is_match("changed").unwrap().not());
}

#[test]
fn path_escaping_the_project_is_refused() {
    let (_temp, home, repo) = fixture();
    surge(&home)
        .args(["trust", "accept", "../outside.toml"])
        .current_dir(&repo)
        .assert()
        .failure()
        .stderr(contains("invalid path"));
}

#[test]
fn missing_file_is_refused() {
    let (_temp, home, repo) = fixture();
    surge(&home)
        .args(["trust", "accept", ".surge/profiles/nope-1.0.toml"])
        .current_dir(&repo)
        .assert()
        .failure()
        .stderr(contains("does not exist"));
}
