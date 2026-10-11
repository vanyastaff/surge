//! Startup failures must be visible to process supervisors.
#![cfg(unix)]

use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[test]
fn socket_bind_failure_exits_unsuccessfully() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    let socket = home.join("daemon/daemon.sock");
    std::fs::create_dir_all(&socket).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_surge-daemon"))
        .args(["--shutdown-grace", "100ms"])
        .env("SURGE_HOME", &home)
        .env_remove("SURGE_PROFILES_DIR")
        .current_dir(root.path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            child.kill().unwrap();
            let output = child.wait_with_output().unwrap();
            panic!(
                "daemon did not exit: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let output = child.wait_with_output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("server exited with error"), "{stderr}");
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(socket.is_dir(), "startup must preserve an unrelated path");
}

#[test]
fn failed_startup_preserves_regular_socket_path_and_secures_parent() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    let parent = home.join("daemon");
    std::fs::create_dir_all(&parent).unwrap();
    std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o755)).unwrap();
    let socket = parent.join("daemon.sock");
    std::fs::write(&socket, "user data").unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_surge-daemon"))
        .args(["--shutdown-grace", "100ms"])
        .env("SURGE_HOME", &home)
        .current_dir(root.path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while child.try_wait().unwrap().is_none()
        && !std::fs::symlink_metadata(&socket).is_ok_and(|meta| meta.file_type().is_socket())
    {
        if Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let _ = child.kill();
    let _ = child.wait();
    assert_eq!(std::fs::read_to_string(&socket).unwrap(), "user data");
    assert_eq!(
        std::fs::metadata(parent).unwrap().permissions().mode() & 0o777,
        0o700
    );
}
