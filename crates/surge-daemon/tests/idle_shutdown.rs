//! Exercise the real daemon process, including an idle IPC connection.
#![cfg(unix)]

use std::{
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

struct OwnedDaemon(Child);

impl Drop for OwnedDaemon {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}

#[test]
fn idle_connection_does_not_force_full_shutdown_grace() {
    let directory = tempfile::tempdir().unwrap();
    let home = directory.path().join("home");
    let mut daemon = OwnedDaemon(
        Command::new(env!("CARGO_BIN_EXE_surge-daemon"))
            .args(["--shutdown-grace", "30s"])
            .env("SURGE_HOME", &home)
            .current_dir(directory.path())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let socket = home.join("daemon/daemon.sock");
    let ready_deadline = Instant::now() + Duration::from_secs(15);
    let idle = loop {
        if let Ok(connection) = UnixStream::connect(&socket) {
            break connection;
        }
        assert!(
            daemon.0.try_wait().unwrap().is_none(),
            "daemon exited at startup"
        );
        assert!(Instant::now() < ready_deadline, "daemon startup timed out");
        std::thread::sleep(Duration::from_millis(10));
    };
    let mut control = UnixStream::connect(&socket).unwrap();
    control
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let started = Instant::now();
    control
        .write_all(b"{\"method\":\"shutdown\",\"request_id\":1}\n")
        .unwrap();
    let mut reply = String::new();
    BufReader::new(control).read_line(&mut reply).unwrap();
    let reply: serde_json::Value = serde_json::from_str(&reply).unwrap();
    assert_eq!(reply["method"], "shutdown_ok");
    loop {
        if let Some(status) = daemon.0.try_wait().unwrap() {
            assert!(status.success(), "daemon exit: {status}");
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "idle drain waited for grace"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    // Keep this connection alive until after the process exits.
    drop(idle);
}
