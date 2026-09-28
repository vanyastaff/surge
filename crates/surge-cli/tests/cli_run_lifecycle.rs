//! Foreground commands report durable outcomes, not merely successful startup.

use assert_cmd::Command;
use std::time::Duration;

fn terminal_run(watch: bool, failure: bool) {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("surge-home");
    let source = include_str!("../../../examples/flow_terminal_only.toml");
    let source = if failure {
        source.replace("type = \"success\"", "type = \"failure\"\nexit_code = 1")
    } else {
        source.to_owned()
    };
    let flow = temp.path().join("flow.toml");
    std::fs::write(&flow, source).unwrap();
    let mut command = Command::cargo_bin("surge").unwrap();
    command
        .args(["engine", "run"])
        .arg(&flow)
        .current_dir(temp.path())
        .env("SURGE_HOME", &home)
        .timeout(Duration::from_secs(15));
    if watch {
        command.arg("--watch");
    }
    let output = command.output().unwrap();
    assert_eq!(output.status.success(), !failure, "{output:?}");
    let run_id = String::from_utf8(output.stdout).unwrap();
    let replay = Command::cargo_bin("surge")
        .unwrap()
        .args(["engine", "replay", run_id.trim(), "--format", "json"])
        .env("SURGE_HOME", &home)
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
    let home = temp.path().join("home");
    std::fs::create_dir_all(home.join("templates")).unwrap();
    std::fs::write(
        home.join("templates/quick.toml"),
        include_str!("../../../examples/flow_terminal_only.toml"),
    )
    .unwrap();
    let run = Command::cargo_bin("surge")
        .unwrap()
        .args(["engine", "run", "--template", "quick"])
        .env("SURGE_HOME", &home)
        .current_dir(temp.path())
        .timeout(Duration::from_secs(15))
        .assert()
        .success();
    let id = String::from_utf8(run.get_output().stdout.clone()).unwrap();
    let replay = Command::cargo_bin("surge")
        .unwrap()
        .args(["engine", "replay", id.trim(), "--format", "json"])
        .env("SURGE_HOME", &home)
        .current_dir(temp.path())
        .timeout(Duration::from_secs(15))
        .assert()
        .success();
    let state: serde_json::Value = serde_json::from_slice(&replay.get_output().stdout).unwrap();
    assert_eq!(state["view"]["terminal"], "completed");
}

#[test]
fn local_gate_aborts_with_actionable_error() {
    for watch in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
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
        let mut command = Command::cargo_bin("surge").unwrap();
        command
            .args(["engine", "run"])
            .arg(&flow)
            .env("SURGE_HOME", &home)
            .current_dir(temp.path())
            .timeout(Duration::from_secs(15));
        if watch {
            command.arg("--watch");
        }
        let run = command
            .assert()
            .failure()
            .stderr(predicates::str::contains("needs human input"))
            .stderr(predicates::str::contains("--daemon"));
        let id = String::from_utf8(run.get_output().stdout.clone()).unwrap();
        let replay = Command::cargo_bin("surge")
            .unwrap()
            .args(["engine", "replay", id.trim(), "--format", "json"])
            .env("SURGE_HOME", &home)
            .current_dir(temp.path())
            .timeout(Duration::from_secs(15))
            .assert()
            .success();
        let state: serde_json::Value = serde_json::from_slice(&replay.get_output().stdout).unwrap();
        assert_eq!(state["view"]["terminal"], "aborted");
    }
}
