//! Real daemon restart across a shutdown grace longer than the old CLI deadline.
#![cfg(unix)]

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

struct DaemonCleanup {
    initial: Child,
    home: PathBuf,
}

impl Drop for DaemonCleanup {
    fn drop(&mut self) {
        // Only processes recorded in this test's fresh, isolated home are killed.
        if let Ok(Some(pid)) = surge_daemon::pidfile::read_pid(&self.home.join("daemon/daemon.pid"))
        {
            let _ = nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(i32::try_from(pid).unwrap()),
                nix::sys::signal::Signal::SIGKILL,
            );
        }
        let _ = self.initial.kill();
        let _ = self.initial.wait();
    }
}

#[test]
#[ignore = "requires cargo build -p surge-daemon; runs a real daemon with an 11-second grace"]
fn restart_waits_for_grace_then_starts_a_new_healthy_daemon() {
    let cli = PathBuf::from(env!("CARGO_BIN_EXE_surge"));
    let daemon = cli.parent().unwrap().join("surge-daemon");
    assert!(daemon.is_file(), "build surge-daemon before this test");
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let child = Command::new(daemon)
        .args(["--shutdown-grace", "11s"])
        .env("SURGE_HOME", &home)
        .current_dir(temp.path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let original_pid = child.id();
    let mut cleanup = DaemonCleanup {
        initial: child,
        home,
    };
    let deadline = Instant::now() + Duration::from_secs(15);
    while !cleanup.home.join("daemon/daemon.sock").exists() {
        assert!(
            cleanup.initial.try_wait().unwrap().is_none(),
            "daemon exited early"
        );
        assert!(Instant::now() < deadline, "daemon readiness timed out");
        std::thread::sleep(Duration::from_millis(50));
    }

    let started = Instant::now();
    assert_cmd::Command::new(&cli)
        .args(["daemon", "restart"])
        .env("SURGE_HOME", &cleanup.home)
        .current_dir(temp.path())
        .timeout(Duration::from_secs(25))
        .assert()
        .success();
    assert!(
        started.elapsed() >= Duration::from_secs(11),
        "grace was bypassed"
    );
    let restarted_pid = surge_daemon::pidfile::read_pid(&cleanup.home.join("daemon/daemon.pid"))
        .unwrap()
        .unwrap();
    assert_ne!(original_pid, restarted_pid);
    assert_cmd::Command::new(cli)
        .args(["daemon", "status"])
        .env("SURGE_HOME", &cleanup.home)
        .current_dir(temp.path())
        .timeout(Duration::from_secs(5))
        .assert()
        .success()
        .stdout(predicates::str::contains("ping:   ok"));
}
