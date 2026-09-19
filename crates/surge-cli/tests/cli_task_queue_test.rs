//! Integration tests for `surge project` and `surge task` (ADR-0020, T7).
//!
//! These drive the CLI against a temporary git repository with a real
//! `.surge/roadmap.toml`, and assert the queue semantics the acceptance
//! ledger names: `surge project start` registers and mirrors the roadmap;
//! `surge task priority` reorders the queue; `surge task pause`/`resume`
//! move a task in and out of the ready set; `surge ready` reports only
//! unblocked tasks and `blocked_by_failed` for a failed dependency.

use assert_cmd::Command;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;
use std::path::Path;
use std::process::Command as StdCommand;

fn surge(home: &Path) -> Command {
    let mut command = Command::cargo_bin("surge").unwrap();
    command.env("HOME", home).env("USERPROFILE", home);
    command
}

fn init_repo(root: &Path) {
    for args in [
        vec!["init"],
        vec!["config", "user.email", "test@test.com"],
        vec!["config", "user.name", "Test"],
    ] {
        StdCommand::new("git")
            .args(&args)
            .current_dir(root)
            .output()
            .unwrap();
    }
    std::fs::write(root.join("README.md"), "# Test\n").unwrap();
}

fn write_roadmap(root: &Path, roadmap: &str) {
    let dir = root.join(".surge");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("roadmap.toml"), roadmap).unwrap();
}

/// Three tasks: t1 → t2 chain, t3 independent. Sizes are required at v2.
const ROADMAP: &str = r#"
schema_version = 2

[[milestones]]
id = "m1"
title = "M1"

[[milestones.tasks]]
id = "t1"
title = "First"
size = "s"
priority = "medium"

[[milestones.tasks]]
id = "t2"
title = "Second"
size = "s"
depends_on = ["t1"]

[[milestones.tasks]]
id = "t3"
title = "Third"
size = "s"
"#;

fn fixture() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let repo = temp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    init_repo(&repo);
    write_roadmap(&repo, ROADMAP);
    (temp, home, repo)
}

#[test]
fn project_start_registers_and_ready_filters_blocked() {
    let (_temp, home, repo) = fixture();

    surge(&home)
        .args(["project", "start"])
        .current_dir(&repo)
        .assert()
        .success()
        .stdout(contains("3 task(s)"))
        .stdout(contains("registered"));

    // t2 is blocked by t1; t1 and t3 are ready.
    surge(&home)
        .args(["ready"])
        .current_dir(&repo)
        .assert()
        .success()
        .stdout(contains("t1"))
        .stdout(contains("t3"))
        .stdout(predicates::str::is_match(r"(?m)^t2\b").unwrap().not());
}

#[test]
fn task_list_shows_dependencies_and_ready_set() {
    let (_temp, home, repo) = fixture();
    surge(&home)
        .args(["project", "start"])
        .current_dir(&repo)
        .assert()
        .success();

    surge(&home)
        .args(["task", "list"])
        .current_dir(&repo)
        .assert()
        .success()
        .stdout(contains("t1"))
        .stdout(contains("t2"))
        .stdout(contains("t3"))
        .stdout(contains("Ready: t1, t3"))
        .stdout(contains("DEPENDS_ON"));
}

#[test]
fn task_priority_changes_the_order() {
    let (_temp, home, repo) = fixture();
    surge(&home)
        .args(["project", "start"])
        .current_dir(&repo)
        .assert()
        .success();

    // t3 (medium) currently outranks nothing, but raise it above t1.
    surge(&home)
        .args(["task", "priority", "t3", "critical"])
        .current_dir(&repo)
        .assert()
        .success()
        .stdout(contains("t3 → critical"));

    // The roadmap file itself must carry the change (planning truth).
    let written = std::fs::read_to_string(repo.join(".surge/roadmap.toml")).unwrap();
    assert!(written.contains(r#"priority = "critical""#));

    surge(&home)
        .args(["task", "list"])
        .current_dir(&repo)
        .assert()
        .success()
        .stdout(contains("Ready: t3, t1"));
}

#[test]
fn task_pause_removes_it_from_the_ready_set() {
    let (_temp, home, repo) = fixture();
    surge(&home)
        .args(["project", "start"])
        .current_dir(&repo)
        .assert()
        .success();

    surge(&home)
        .args(["task", "pause", "t1"])
        .current_dir(&repo)
        .assert()
        .success()
        .stdout(contains("paused t1"));

    // t1 out; t2 was already blocked by t1 and stays blocked (a paused
    // dependency is waiting, not failed, so no blocked_by_failed line).
    surge(&home)
        .args(["ready"])
        .current_dir(&repo)
        .assert()
        .success()
        .stdout(contains("t3"))
        .stdout(predicates::str::is_match(r"(?m)^t1\b").unwrap().not());

    surge(&home)
        .args(["task", "resume", "t1"])
        .current_dir(&repo)
        .assert()
        .success()
        .stdout(contains("resumed t1"));

    surge(&home)
        .args(["task", "list"])
        .current_dir(&repo)
        .assert()
        .success()
        .stdout(contains("Ready: t1, t3"));
}

#[tokio::test(flavor = "multi_thread")]
async fn failed_dependency_is_reported_as_blocked_by_failed() {
    let (_temp, home, repo) = fixture();
    surge(&home)
        .args(["project", "start"])
        .current_dir(&repo)
        .assert()
        .success();

    // The dispatcher would mark a failed run `failed`; simulate that state
    // through the same store the scheduler uses, then observe `surge ready`.
    let storage = surge_persistence::runs::Storage::open(&home.join(".surge"))
        .await
        .unwrap();
    storage
        .task_queue_store()
        .skip(
            &repo.canonicalize().unwrap(),
            "t1",
            chrono::Utc::now().timestamp_millis(),
        )
        .unwrap();
    drop(storage);

    surge(&home)
        .args(["ready"])
        .current_dir(&repo)
        .assert()
        .success()
        .stdout(contains("blocked_by_failed: t2"))
        .stdout(contains("t1 (failed)"));
}

#[test]
fn project_pause_and_resume_round_trip() {
    let (_temp, home, repo) = fixture();
    surge(&home)
        .args(["project", "start"])
        .current_dir(&repo)
        .assert()
        .success();

    surge(&home)
        .args(["project", "status"])
        .current_dir(&repo)
        .assert()
        .success()
        .stdout(contains("running"));

    surge(&home)
        .args(["project", "pause"])
        .current_dir(&repo)
        .assert()
        .success()
        .stdout(contains("paused"));

    surge(&home)
        .args(["project", "status"])
        .current_dir(&repo)
        .assert()
        .success()
        .stdout(contains("paused"));

    surge(&home)
        .args(["project", "resume"])
        .current_dir(&repo)
        .assert()
        .success()
        .stdout(contains("resumed"));
}

#[tokio::test(flavor = "multi_thread")]
async fn task_requeue_unblocks_a_failed_dependency() {
    let (_temp, home, repo) = fixture();
    surge(&home)
        .args(["project", "start"])
        .current_dir(&repo)
        .assert()
        .success();

    let storage = surge_persistence::runs::Storage::open(&home.join(".surge"))
        .await
        .unwrap();
    storage
        .task_queue_store()
        .skip(
            &repo.canonicalize().unwrap(),
            "t1",
            chrono::Utc::now().timestamp_millis(),
        )
        .unwrap();
    drop(storage);

    surge(&home)
        .args(["task", "requeue", "t1"])
        .current_dir(&repo)
        .assert()
        .success()
        .stdout(contains("requeued t1"));

    surge(&home)
        .args(["task", "list"])
        .current_dir(&repo)
        .assert()
        .success()
        .stdout(contains("Ready: t1, t3"));
}
