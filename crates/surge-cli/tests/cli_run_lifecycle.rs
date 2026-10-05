//! Foreground commands report durable outcomes, not merely successful startup.

#[path = "common/owned_flow.rs"]
mod owned_flow;

use assert_cmd::Command;
use std::time::Duration;

fn terminal_run(watch: bool, failure: bool) {
    let temp = tempfile::tempdir().unwrap();
    let home_dir = tempfile::tempdir().unwrap();
    let home = home_dir.path();
    let source = include_str!("../../../examples/flow_terminal_only.toml");
    let source = if failure {
        source.replace("type = \"success\"", "type = \"failure\"\nexit_code = 1")
    } else {
        source.to_owned()
    };
    let flow = temp.path().join("flow.toml");
    std::fs::write(&flow, source).unwrap();
    owned_flow::commit_project(temp.path());
    let _daemon = owned_flow::start_daemon(home, temp.path(), None);
    let mut command = Command::cargo_bin("surge").unwrap();
    command
        .args(["engine", "run"])
        .arg("flow.toml")
        .current_dir(temp.path())
        .env("SURGE_HOME", home)
        .timeout(Duration::from_secs(15));
    if watch {
        command.arg("--watch");
    }
    let output = command.output().unwrap();
    assert_eq!(output.status.success(), !failure, "{output:?}");
    let run_id = String::from_utf8(output.stdout).unwrap();
    let disk_watch = Command::cargo_bin("surge")
        .unwrap()
        .args(["engine", "watch", run_id.trim()])
        .env("SURGE_HOME", home)
        .current_dir(temp.path())
        .timeout(Duration::from_secs(15))
        .output()
        .unwrap();
    assert_eq!(disk_watch.status.success(), !failure, "{disk_watch:?}");
    let replay = Command::cargo_bin("surge")
        .unwrap()
        .args(["engine", "replay", run_id.trim(), "--format", "json"])
        .env("SURGE_HOME", home)
        .current_dir(temp.path())
        .timeout(Duration::from_secs(15))
        .assert()
        .success();
    let state: serde_json::Value = serde_json::from_slice(&replay.get_output().stdout).unwrap();
    assert_eq!(state["terminal"], true, "{state}");
    assert_eq!(state["failed"], failure, "{state}");
}

#[test]
fn foreground_success_is_durable() {
    terminal_run(false, false);
}

#[test]
fn foreground_failure_is_nonzero_and_durable() {
    terminal_run(false, true);
}

#[test]
fn watched_success_is_durable() {
    terminal_run(true, false);
}

#[test]
fn watched_failure_is_nonzero_and_durable() {
    terminal_run(true, true);
}

#[test]
fn user_template_executes_to_completion() {
    let temp = tempfile::tempdir().unwrap();
    let home_dir = tempfile::tempdir().unwrap();
    let home = home_dir.path();
    std::fs::create_dir_all(home.join("templates")).unwrap();
    std::fs::write(
        home.join("templates/quick.toml"),
        include_str!("../../../examples/flow_terminal_only.toml"),
    )
    .unwrap();
    owned_flow::commit_project(temp.path());
    let _daemon = owned_flow::start_daemon(home, temp.path(), None);
    let run = Command::cargo_bin("surge")
        .unwrap()
        .args(["engine", "run", "--template", "quick"])
        .env("SURGE_HOME", home)
        .current_dir(temp.path())
        .timeout(Duration::from_secs(15))
        .assert()
        .success();
    let id = String::from_utf8(run.get_output().stdout.clone()).unwrap();
    let replay = Command::cargo_bin("surge")
        .unwrap()
        .args(["engine", "replay", id.trim(), "--format", "json"])
        .env("SURGE_HOME", home)
        .current_dir(temp.path())
        .timeout(Duration::from_secs(15))
        .assert()
        .success();
    let state: serde_json::Value = serde_json::from_slice(&replay.get_output().stdout).unwrap();
    assert_eq!(state["view"]["terminal"], "completed");
}

#[test]
fn daemon_gate_remains_pending_until_explicit_suspension() {
    {
        let temp = tempfile::tempdir().unwrap();
        let home_dir = tempfile::tempdir().unwrap();
        let home = home_dir.path();
        let flow = temp.path().join("gate.toml");
        let mut source = include_str!("../../../examples/flow_terminal_only.toml")
            .replace("start = \"end\"", "start = \"gate\"")
            .replace("edges = []", "");
        source.push_str(
            r#"
[nodes.gate]
id = "gate"
[nodes.gate.position]
x = 0.0
y = 0.0
[[nodes.gate.declared_outcomes]]
id = "approve"
description = "Approved"
edge_kind_hint = "forward"
is_terminal = false
[nodes.gate.config]
node_kind = "human_gate"
delivery_channels = []
[nodes.gate.config.summary]
title = "Approval"
body = "Decide"
[[nodes.gate.config.options]]
outcome = "approve"
label = "Approve"
[[edges]]
id = "approved"
to = "end"
kind = "forward"
[edges.from]
node = "gate"
outcome = "approve"
[edges.policy]
on_max_exceeded = "escalate"
"#,
        );
        std::fs::write(&flow, source).unwrap();
        owned_flow::commit_project(temp.path());
        let _daemon = owned_flow::start_daemon(home, temp.path(), None);
        let run = Command::cargo_bin("surge")
            .unwrap()
            .args(["engine", "run", "gate.toml", "--daemon"])
            .env("SURGE_HOME", home)
            .current_dir(temp.path())
            .timeout(Duration::from_secs(15))
            .assert()
            .success();
        let id = String::from_utf8(run.get_output().stdout.clone()).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        loop {
            let log = Command::cargo_bin("surge")
                .unwrap()
                .args(["engine", "logs", id.trim()])
                .env("SURGE_HOME", home)
                .current_dir(temp.path())
                .timeout(Duration::from_secs(15))
                .output()
                .unwrap();
            if log.status.success()
                && String::from_utf8_lossy(&log.stderr).contains("HumanInputRequested")
            {
                // Read the projection only after observing the durable request.
                // A replay fetched before the log can legitimately predate StageEntered.
                let replay = Command::cargo_bin("surge")
                    .unwrap()
                    .args(["engine", "replay", id.trim(), "--format", "json"])
                    .env("SURGE_HOME", home)
                    .current_dir(temp.path())
                    .timeout(Duration::from_secs(15))
                    .assert()
                    .success();
                let state: serde_json::Value =
                    serde_json::from_slice(&replay.get_output().stdout).unwrap();
                assert_eq!(state["view"]["active_node"], "gate", "{state}");
                assert_eq!(state["terminal"], false, "{state}");
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "gate did not become pending"
            );
            std::thread::sleep(Duration::from_millis(25));
        }
        Command::cargo_bin("surge")
            .unwrap()
            .args(["engine", "watch", id.trim()])
            .env("SURGE_HOME", home)
            .current_dir(temp.path())
            .timeout(Duration::from_secs(15))
            .assert()
            .failure()
            .stderr(predicates::str::contains("no durable terminal"));
        Command::cargo_bin("surge")
            .unwrap()
            .args(["engine", "stop", id.trim(), "--daemon"])
            .env("SURGE_HOME", home)
            .current_dir(temp.path())
            .timeout(Duration::from_secs(15))
            .assert()
            .success();
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        loop {
            let log = Command::cargo_bin("surge")
                .unwrap()
                .args(["engine", "logs", id.trim()])
                .env("SURGE_HOME", home)
                .current_dir(temp.path())
                .timeout(Duration::from_secs(15))
                .output()
                .unwrap();
            if log.status.success() && String::from_utf8_lossy(&log.stderr).contains("RunSuspended")
            {
                let replay = Command::cargo_bin("surge")
                    .unwrap()
                    .args(["engine", "replay", id.trim(), "--format", "json"])
                    .env("SURGE_HOME", home)
                    .current_dir(temp.path())
                    .timeout(Duration::from_secs(15))
                    .assert()
                    .success();
                let state: serde_json::Value =
                    serde_json::from_slice(&replay.get_output().stdout).unwrap();
                assert_eq!(state["terminal"], false, "{state}");
                assert!(state["view"]["terminal"].is_null(), "{state}");
                Command::cargo_bin("surge")
                    .unwrap()
                    .args(["engine", "watch", id.trim()])
                    .env("SURGE_HOME", home)
                    .current_dir(temp.path())
                    .timeout(Duration::from_secs(15))
                    .assert()
                    .failure()
                    .stderr(predicates::str::contains("no durable terminal"));
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "gate suspension not durable"
            );
            std::thread::sleep(Duration::from_millis(25));
        }
    }
}
