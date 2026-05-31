//! Durability fault-injection harness (v0.2 M4, slice 1).
//!
//! Proves the per-run SQLite event log survives an **unclean** process death
//! mid-run: a real `surge engine run` subprocess is aborted via the
//! `SURGE_CHECKPOINT_EXIT` seam (`std::process::exit(99)` — no async teardown,
//! no `Drop`, like a `kill -9`) the instant it enters the first stage, right
//! after the `StageEntered` event is durably committed to the WAL. A fresh
//! `surge engine replay` then folds that log and must see the precise mid-run
//! state — proving the committed event was not lost and the log is not
//! corrupt.
//!
//! The seam is `#[cfg(debug_assertions)]`, so this test only runs in debug.

#![cfg(debug_assertions)]

use assert_cmd::Command;
use predicates::str::contains;

/// Absolute path to the bundled two-node example flow (`impl_1` → terminal).
fn minimal_flow() -> std::path::PathBuf {
    let flow = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("examples")
        .join("flow_minimal_agent.toml");
    assert!(flow.exists(), "flow fixture missing: {}", flow.display());
    flow
}

#[test]
fn unclean_exit_mid_run_preserves_and_folds_event_log() {
    let temp = tempfile::tempdir().unwrap();
    let surge_home = temp.path().join("surge_home");
    let worktree = temp.path().join("wt");
    std::fs::create_dir_all(&surge_home).unwrap();
    std::fs::create_dir_all(&worktree).unwrap();
    let flow = minimal_flow();

    // Run the flow, but abort the process uncleanly the instant it enters the
    // first stage (`impl_1`), after StageEntered is committed. `--watch` keeps
    // the CLI alive long enough for the spawned run task to reach the stage.
    let out = Command::cargo_bin("surge")
        .unwrap()
        .args(["engine", "run"])
        .arg(&flow)
        .arg("--watch")
        .arg("--worktree")
        .arg(&worktree)
        .env("HOME", &surge_home)
        .env("USERPROFILE", &surge_home)
        .env("SURGE_HOME", &surge_home)
        .env("SURGE_CHECKPOINT_EXIT", "impl_1")
        .output()
        .expect("spawn surge engine run");

    assert_eq!(
        out.status.code(),
        Some(99),
        "expected unclean checkpoint exit (99); got {:?}. stderr:\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );

    // The run id is printed before the run starts (first stdout line).
    let stdout = String::from_utf8_lossy(&out.stdout);
    let run_id = stdout
        .lines()
        .next()
        .expect("run id on first stdout line")
        .trim()
        .to_string();
    assert!(
        run_id.starts_with("run"),
        "unexpected run id {run_id:?}; stdout:\n{stdout}"
    );

    // The log left by the unclean kill must fold cleanly (no corruption) and
    // show the run mid-flight: entered `impl_1`, not terminal.
    Command::cargo_bin("surge")
        .unwrap()
        .args(["engine", "replay"])
        .arg(&run_id)
        .env("HOME", &surge_home)
        .env("USERPROFILE", &surge_home)
        .env("SURGE_HOME", &surge_home)
        .assert()
        .success()
        .stdout(contains("active node:   impl_1"))
        .stdout(contains("terminal:      no"));
}
