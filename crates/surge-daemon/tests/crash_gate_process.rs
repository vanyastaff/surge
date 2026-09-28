//! Real-process recovery of a custom-directory run and stale approval rejection.
#![cfg(unix)]
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
use surge_core::{RunId, graph::Graph};
use surge_orchestrator::engine::EngineRunConfig;

struct Daemon {
    child: Child,
    socket: PathBuf,
}
impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
impl Daemon {
    fn start(root: &Path) -> Self {
        let child = Command::new(env!("CARGO_BIN_EXE_surge-daemon"))
            .args(["--shutdown-grace", "1s"])
            .env("SURGE_HOME", root.join("home"))
            .current_dir(root)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut daemon = Self {
            child,
            socket: root.join("home/daemon/daemon.sock"),
        };
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if UnixStream::connect(&daemon.socket).is_ok() {
                return daemon;
            }
            assert!(daemon.child.try_wait().unwrap().is_none(), "startup exited");
            assert!(Instant::now() < deadline, "startup timeout");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    fn rpc(&self, request: Value) -> Value {
        let mut socket = UnixStream::connect(&self.socket).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        writeln!(socket, "{request}").unwrap();
        let mut line = String::new();
        BufReader::new(socket).read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    }
    fn crash(mut self) {
        self.child.kill().unwrap();
        self.child.wait().unwrap();
    }
}
fn events(path: &Path) -> Vec<Value> {
    let connection =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let mut query = connection
        .prepare("SELECT cast(payload AS TEXT) FROM events ORDER BY seq")
        .unwrap();
    query
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .map(|row| serde_json::from_str::<Value>(&row.unwrap()).unwrap()["payload"].clone())
        .collect()
}
fn wait_for(path: &Path, kind: &str, count: usize) -> Vec<Value> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if path.exists() {
            let observed = events(path);
            if observed
                .iter()
                .filter(|event| event["type"] == kind)
                .count()
                >= count
            {
                return observed;
            }
        }
        assert!(Instant::now() < deadline, "waiting for {kind} x{count}");
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn gate(events: &[Value]) -> &Value {
    events
        .iter()
        .rev()
        .find(|event| event["type"] == "human_input_requested")
        .unwrap()
}
fn approve(daemon: &Daemon, run: RunId, gate: &Value) -> Value {
    daemon.rpc(json!({"method":"resolve_gate_input", "request_id":3,"run_id":run,
        "node":gate["node"],"gate_request_id":gate["call_id"].as_str().unwrap().strip_prefix("gate-").unwrap(),
        "response":{"outcome":"approve"}}))
}
#[test]
fn custom_worktree_recovers_and_only_fresh_gate_can_complete_run() {
    let root = tempfile::tempdir().unwrap();
    let worktree = root.path().join("custom-checkout");
    std::fs::create_dir(&worktree).unwrap();
    let daemon = Daemon::start(root.path());
    let run = RunId::new();
    let graph: Graph = toml::from_str(include_str!("fixtures/recovery-gate.toml")).unwrap();
    let result = daemon.rpc(json!({"method":"start_run","request_id":2,"run_id":run,
        "graph":graph,"worktree_path":worktree,"run_config":EngineRunConfig::default()}));
    assert_ne!(result["method"], "error", "{result}");
    let database = root
        .path()
        .join("home/runs")
        .join(run.to_string())
        .join("events.sqlite");
    let before = wait_for(&database, "human_input_requested", 1);
    daemon.crash();
    // Remove only the stale test socket so readiness cannot observe its old inode.
    let _ = std::fs::remove_file(root.path().join("home/daemon/daemon.sock"));
    let recovered = Daemon::start(root.path());
    let after = wait_for(&database, "human_input_requested", 2);
    assert_ne!(gate(&before)["call_id"], gate(&after)["call_id"]);
    assert_eq!(approve(&recovered, run, gate(&before))["method"], "error");
    assert_eq!(
        approve(&recovered, run, gate(&after))["method"],
        "resolve_human_input_ok"
    );
    let completed = wait_for(&database, "run_completed", 1);
    for kind in ["run_started", "human_input_resolved", "run_completed"] {
        assert_eq!(
            completed
                .iter()
                .filter(|event| event["type"] == kind)
                .count(),
            1,
            "{kind}"
        );
    }
    assert!(!completed.iter().any(|event| event["type"] == "run_failed"));
}
