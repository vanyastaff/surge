//! Real daemon restart: idle owners settle promptly and delayed owners get time to exit.
#![cfg(unix)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

struct DaemonCleanup {
    initial: Child,
    home: PathBuf,
    resume: Option<(mpsc::Sender<()>, JoinHandle<()>)>,
    successor: Option<UnixStream>,
}

impl DaemonCleanup {
    fn shutdown_successor(&mut self) -> std::io::Result<()> {
        let socket = self.home.join("daemon/daemon.sock");
        let mut connection = match self.successor.take() {
            Some(connection) => connection,
            None if socket.exists() => UnixStream::connect(&socket)?,
            None => return Ok(()),
        };
        connection.set_read_timeout(Some(Duration::from_secs(1)))?;
        connection.set_write_timeout(Some(Duration::from_secs(1)))?;
        connection.write_all(b"{\"method\":\"shutdown\",\"request_id\":1}\n")?;
        let mut reply = String::new();
        BufReader::new(connection).read_line(&mut reply)?;
        let reply: serde_json::Value = serde_json::from_str(&reply)?;
        if reply["method"] != "shutdown_ok" || reply["request_id"] != 1 {
            return Err(std::io::Error::other(
                "unexpected fixture shutdown response",
            ));
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while socket.exists() {
            if Instant::now() >= deadline {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "fixture daemon socket survived shutdown",
                ));
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        Ok(())
    }
}

impl Drop for DaemonCleanup {
    fn drop(&mut self) {
        // Join the delayed signal owner before reaping its original child, so
        // no delayed SIGCONT can outlive ownership and hit a reused PID.
        if let Some((cancel, resume)) = self.resume.take() {
            let _ = cancel.send(());
            let _ = resume.join();
        }
        // The successor is addressed only through this private runtime's IPC
        // endpoint. Never signal a process named by mutable pidfile contents.
        if let Err(error) = self.shutdown_successor() {
            eprintln!("fixture IPC shutdown failed: {error}");
        }
        let _ = self.initial.kill();
        let _ = self.initial.wait();
    }
}

fn start_daemon(cli: &Path, project: &Path, home: PathBuf) -> DaemonCleanup {
    let daemon = cli.parent().unwrap().join("surge-daemon");
    assert!(daemon.is_file(), "build surge-daemon before this test");
    let child = Command::new(daemon)
        .args(["--shutdown-grace", "30s"])
        .env("SURGE_HOME", &home)
        .current_dir(project)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut cleanup = DaemonCleanup {
        initial: child,
        home,
        resume: None,
        successor: None,
    };
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        assert!(
            cleanup.initial.try_wait().unwrap().is_none(),
            "daemon exited early"
        );
        let status = Command::new(cli)
            .args(["daemon", "status"])
            .env("SURGE_HOME", &cleanup.home)
            .current_dir(project)
            .output()
            .unwrap();
        if status.status.success() && String::from_utf8_lossy(&status.stdout).contains("ping:   ok")
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "daemon readiness timed out: {status:?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    cleanup
}

fn restart_and_check(cli: &Path, project: &Path, cleanup: &mut DaemonCleanup) {
    let original_pid = cleanup.initial.id();
    assert_cmd::Command::new(cli)
        .args(["daemon", "restart"])
        .env("SURGE_HOME", &cleanup.home)
        .current_dir(project)
        .timeout(Duration::from_secs(25))
        .assert()
        .success();
    if let Some((_, resume)) = cleanup.resume.take() {
        resume.join().unwrap();
    }
    let initial_exit = cleanup
        .initial
        .try_wait()
        .unwrap()
        .expect("original daemon exited");
    assert!(
        initial_exit.success(),
        "original daemon exit: {initial_exit}"
    );
    let restarted_pid = surge_daemon::pidfile::read_pid(&cleanup.home.join("daemon/daemon.pid"))
        .unwrap()
        .unwrap();
    assert_ne!(original_pid, restarted_pid);
    assert_cmd::Command::new(cli)
        .args(["daemon", "status"])
        .env("SURGE_HOME", &cleanup.home)
        .current_dir(project)
        .timeout(Duration::from_secs(5))
        .assert()
        .success()
        .stdout(predicates::str::contains("ping:   ok"));
    cleanup.successor = Some(UnixStream::connect(cleanup.home.join("daemon/daemon.sock")).unwrap());
}

#[test]
#[ignore = "requires cargo build -p surge-daemon; starts and restarts a real idle daemon"]
fn idle_restart_does_not_wait_for_full_grace() {
    let cli = PathBuf::from(env!("CARGO_BIN_EXE_surge"));
    let temp = tempfile::tempdir().unwrap();
    let mut cleanup = start_daemon(&cli, temp.path(), temp.path().join("home"));
    let started = Instant::now();
    restart_and_check(&cli, temp.path(), &mut cleanup);
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "idle restart waited for grace"
    );
    cleanup.shutdown_successor().unwrap();
}

#[test]
#[ignore = "requires cargo build -p surge-daemon; holds the real daemon stopped for 11 seconds"]
fn restart_waits_beyond_old_deadline_for_delayed_owner() {
    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid;

    let cli = PathBuf::from(env!("CARGO_BIN_EXE_surge"));
    let temp = tempfile::tempdir().unwrap();
    let mut cleanup = start_daemon(&cli, temp.path(), temp.path().join("home"));
    let pid = Pid::from_raw(i32::try_from(cleanup.initial.id()).unwrap());
    kill(pid, Signal::SIGSTOP).unwrap();
    let (cancel, receiver) = mpsc::channel();
    let started = Instant::now();
    cleanup.resume = Some((
        cancel,
        std::thread::spawn(move || {
            if matches!(
                receiver.recv_timeout(Duration::from_secs(11)),
                Err(mpsc::RecvTimeoutError::Timeout)
            ) {
                kill(pid, Signal::SIGCONT).unwrap();
            }
        }),
    ));
    restart_and_check(&cli, temp.path(), &mut cleanup);
    assert!(
        started.elapsed() >= Duration::from_secs(11),
        "delayed owner was bypassed"
    );
    cleanup.shutdown_successor().unwrap();
}
