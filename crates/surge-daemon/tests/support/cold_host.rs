//! Owned test-host process lifecycle. Readiness files carry verification IDs only.
use std::{
    ffi::OsString,
    fs::{File, OpenOptions},
    io::Read,
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    time::Duration,
};

pub struct ColdHost {
    child: Option<Child>,
    ready_file: PathBuf,
    stdout_path: PathBuf,
    stderr_path: PathBuf,
}
impl ColdHost {
    pub fn spawn(
        probe_exact: &str,
        vars: impl IntoIterator<Item = (OsString, OsString)>,
        ready_file: PathBuf,
        diagnostics_dir: &Path,
    ) -> Result<Self, String> {
        match std::fs::symlink_metadata(&ready_file) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
            _ => return Err("cold host readiness marker already exists or is inaccessible".into()),
        }
        let (stdout, stdout_path) = diagnostic_file(diagnostics_dir, "cold-host-stdout-")?;
        let (stderr, stderr_path) = diagnostic_file(diagnostics_dir, "cold-host-stderr-")?;
        let executable =
            std::env::current_exe().map_err(|_| "current test executable unavailable")?;
        let child = Command::new(executable)
            .args(["--exact", probe_exact, "--nocapture"])
            // Crash injection is opt-in for this exact child, never inherited accidentally.
            .env_remove("SURGE_ROUTE_COMMIT_EXIT")
            .env_remove("SURGE_START_PREPARATION_ITEM")
            .env_remove("SURGE_START_PREPARATION_READY")
            .env_remove("SURGE_START_PREPARATION_RELEASE")
            .env_remove("SURGE_START_PREPARATION_ACCEPTED")
            .env_remove("SURGE_START_PREPARATION_MANIFEST")
            .envs(vars)
            .stdin(Stdio::null())
            .stdout(stdout)
            .stderr(stderr)
            .spawn()
            .map_err(|_| "owned cold host spawn failed")?;
        Ok(Self {
            child: Some(child),
            ready_file,
            stdout_path,
            stderr_path,
        })
    }
    pub fn pid(&self) -> Option<u32> {
        self.child.as_ref().map(Child::id)
    }
    pub fn diagnostics_paths(&self) -> (&Path, &Path) {
        (&self.stdout_path, &self.stderr_path)
    }
    /// Await a newly published verification marker. Probes should publish atomically.
    /// Errors and timeouts terminate and reap this owned child before returning.
    pub async fn wait_ready(&mut self, timeout: Duration) -> Result<String, String> {
        let started = tokio::time::Instant::now();
        loop {
            if started.elapsed() >= timeout {
                let failure = self.cleanup_failure("cold host readiness timeout");
                return Err(format!("{failure}{}", self.stderr_excerpt()));
            }
            match read_readiness(&self.ready_file) {
                Ok(Some(marker)) => return Ok(marker),
                Ok(None) => {},
                Err(reason) => return Err(self.cleanup_failure(&reason)),
            }
            let Some(child) = self.child.as_mut() else {
                return Err("cold host is already stopped".into());
            };
            match child.try_wait() {
                Ok(Some(status)) => {
                    self.child.take(); // try_wait has already reaped this exact child.
                    return Err(format!(
                        "cold host exited before readiness: {status}{}",
                        self.stderr_excerpt()
                    ));
                },
                Ok(None) => {},
                Err(_) => return Err(self.cleanup_failure("cold host status inspection failed")),
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
    /// Await this owned child's actual exit, preserving its exact OS status.
    pub async fn wait_exit(&mut self, timeout: Duration) -> Result<ExitStatus, String> {
        let started = tokio::time::Instant::now();
        loop {
            let Some(child) = self.child.as_mut() else {
                return Err("cold host is already stopped".into());
            };
            match child.try_wait() {
                Ok(Some(status)) => {
                    self.child.take(); // try_wait has reaped this exact child.
                    return Ok(status);
                },
                Ok(None) => {},
                Err(_) => return Err(self.cleanup_failure("cold host exit inspection failed")),
            }
            if started.elapsed() >= timeout {
                return Err(self.cleanup_failure("cold host exit timeout"));
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
    /// Bounded excerpt of the child's panic block and WARN/ERROR lines.
    /// CI logs otherwise show only the capture path, which is gone with the runner.
    fn stderr_excerpt(&self) -> String {
        const TAIL: usize = 256 * 1024;
        const PANIC_CONTEXT: usize = 6;
        const MAX_LINES: usize = 24;
        let Ok(bytes) = std::fs::read(&self.stderr_path) else {
            return "; stderr capture unreadable".into();
        };
        let text = String::from_utf8_lossy(&bytes[bytes.len().saturating_sub(TAIL)..]);
        let mut excerpt = Vec::new();
        let mut panic_lines = 0;
        for line in text.lines() {
            if line.contains(" panicked at ") {
                panic_lines = PANIC_CONTEXT + 1;
            }
            if panic_lines > 0 || line.contains(" WARN ") || line.contains(" ERROR ") {
                excerpt.push(line);
            }
            panic_lines = panic_lines.saturating_sub(1);
        }
        let skipped = excerpt.len().saturating_sub(MAX_LINES);
        let mut rendered = String::from("; stderr excerpt:");
        if skipped > 0 {
            rendered.push_str(&format!("\n    ... {skipped} earlier lines"));
        }
        for line in &excerpt[skipped..] {
            rendered.push_str("\n    ");
            rendered.push_str(line);
        }
        rendered
    }
    fn cleanup_failure(&mut self, reason: &str) -> String {
        if self.stop_and_wait().is_err() {
            format!("{reason}; owned child cleanup failed")
        } else {
            reason.to_owned()
        }
    }
    pub fn stop_and_wait(&mut self) -> Result<(), String> {
        let Some(child) = self.child.as_mut() else {
            return Ok(());
        };
        if child
            .try_wait()
            .map_err(|_| "owned cold host status failed")?
            .is_none()
        {
            if child.kill().is_err()
                && child
                    .try_wait()
                    .map_err(|_| "owned cold host status failed")?
                    .is_none()
            {
                return Err("owned cold host termination failed".into());
            }
            child.wait().map_err(|_| "owned cold host reap failed")?;
        }
        self.child.take();
        Ok(())
    }
}
impl Drop for ColdHost {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
#[cfg(test)]
type ReadinessInterposition = Box<dyn FnOnce(&Path)>;
#[cfg(test)]
std::thread_local! {
    static READINESS_INTERPOSITION: std::cell::RefCell<Option<ReadinessInterposition>> = const { std::cell::RefCell::new(None) };
}
#[cfg(test)]
fn set_readiness_interposition(interposition: Option<ReadinessInterposition>) {
    READINESS_INTERPOSITION.with(|slot| *slot.borrow_mut() = interposition);
}
fn read_readiness(path: &Path) -> Result<Option<String>, String> {
    const LIMIT: u64 = 64 * 1024;
    let original = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("cold host readiness inspection failed".into()),
    };
    if !original.is_file() || original.len() > LIMIT {
        return Err("cold host readiness marker is not a bounded regular file".into());
    }
    if original.len() == 0 {
        return Ok(None);
    }
    #[cfg(test)]
    READINESS_INTERPOSITION.with(|slot| {
        if let Some(interposition) = slot.borrow_mut().take() {
            interposition(path);
        }
    });
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // A marker can change after metadata inspection. Never follow a replacement
        // symlink or block on a replacement FIFO before the held-descriptor checks.
        options.custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK);
    }
    let file = options
        .open(path)
        .map_err(|_| "cold host readiness open failed")?;
    let before = file
        .metadata()
        .map_err(|_| "cold host readiness descriptor inspection failed")?;
    if !before.is_file() || before.len() > LIMIT {
        return Err("cold host readiness descriptor is not a bounded regular file".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if before.dev() != original.dev() || before.ino() != original.ino() {
            return Err("cold host readiness marker changed before open".into());
        }
    }
    let after_file = file
        .try_clone()
        .map_err(|_| "cold host readiness descriptor clone failed")?;
    let mut marker = String::new();
    file.take(LIMIT + 1)
        .read_to_string(&mut marker)
        .map_err(|_| "cold host readiness marker is not readable UTF-8")?;
    if marker.len() as u64 > LIMIT {
        return Err("cold host readiness marker exceeds limit".into());
    }
    let after = after_file
        .metadata()
        .map_err(|_| "cold host readiness final inspection failed")?;
    if marker.is_empty()
        || marker.len() as u64 != before.len()
        || before.len() != after.len()
        || before.modified().ok() != after.modified().ok()
    {
        return Ok(None);
    }
    Ok(Some(marker))
}
fn diagnostic_file(directory: &Path, prefix: &str) -> Result<(File, PathBuf), String> {
    tempfile::Builder::new()
        .prefix(prefix)
        .tempfile_in(directory)
        .map_err(|_| "cold host diagnostic creation failed")?
        .keep()
        .map_err(|_| "cold host diagnostic preservation failed".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn spawn(directory: &Path, mode: &str) -> ColdHost {
        let ready = directory.join("owned-ready.json");
        ColdHost::spawn(
            "cold_host::tests::owned_child_probe",
            [
                (
                    OsString::from("SURGE_COLD_HELPER_READY"),
                    ready.clone().into_os_string(),
                ),
                (
                    OsString::from("SURGE_COLD_HELPER_MODE"),
                    OsString::from(mode),
                ),
            ],
            ready,
            directory,
        )
        .unwrap()
    }
    #[test]
    fn owned_child_probe() {
        let Some(ready) = std::env::var_os("SURGE_COLD_HELPER_READY") else {
            return;
        };
        match std::env::var("SURGE_COLD_HELPER_MODE").unwrap().as_str() {
            "ready" => std::fs::write(ready, b"{\"verification\":\"owned-child\"}").unwrap(),
            "exit" => std::process::exit(23),
            "panic" => panic!("probe-visible child failure"),
            "exit99" => std::process::exit(99),
            _ => {},
        }
        loop {
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    #[tokio::test]
    async fn readiness_requires_a_fresh_marker_and_owned_process_can_be_reaped() {
        let directory = tempfile::tempdir().unwrap();
        let mut host = spawn(directory.path(), "ready");
        assert_ne!(host.pid(), Some(std::process::id()));
        let observed = host.wait_ready(Duration::from_secs(5)).await.unwrap();
        assert_eq!(observed, "{\"verification\":\"owned-child\"}");
        let (stdout, stderr) = host.diagnostics_paths();
        assert!(stdout.is_file() && stderr.is_file());
        host.stop_and_wait().unwrap();
        assert_eq!(host.pid(), None);
        assert!(
            ColdHost::spawn(
                "cold_host::tests::owned_child_probe",
                [],
                directory.path().join("owned-ready.json"),
                directory.path()
            )
            .is_err()
        );
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn regular_marker_swapped_to_fifo_errors_without_blocking_and_reaps_child() {
        use std::{os::unix::fs::OpenOptionsExt, sync::mpsc, time::Instant};
        let directory = tempfile::tempdir().unwrap();
        let mut host = spawn(directory.path(), "silent");
        std::fs::write(&host.ready_file, b"verification-only marker").unwrap();
        let (interposed_tx, interposed_rx) = mpsc::channel();
        set_readiness_interposition(Some(Box::new(move |path| {
            std::fs::remove_file(path).unwrap();
            assert!(Command::new("mkfifo").arg(path).status().unwrap().success());
            interposed_tx.send(()).unwrap();
        })));
        let fifo = host.ready_file.clone();
        // A watchdog releases the old blocking implementation, so the RED cannot hang the suite.
        // The repaired implementation returns before this writer opens the owned fixture FIFO.
        let release = std::thread::spawn(move || {
            interposed_rx.recv_timeout(Duration::from_secs(2)).unwrap();
            std::thread::sleep(Duration::from_millis(500));
            OpenOptions::new()
                .read(true)
                .write(true)
                .custom_flags(nix::libc::O_NONBLOCK)
                .open(fifo)
                .unwrap()
        });
        let started = Instant::now();
        let observed = host.wait_ready(Duration::from_millis(100)).await;
        let elapsed = started.elapsed();
        set_readiness_interposition(None);
        drop(release.join().unwrap());
        assert!(observed.is_err());
        assert_eq!(
            host.pid(),
            None,
            "failed readiness must reap the owned child"
        );
        assert!(
            elapsed < Duration::from_millis(200),
            "FIFO swap blocked past readiness timeout: {elapsed:?}"
        );
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn dropping_the_owner_during_panic_reaps_its_actual_child() {
        let directory = tempfile::tempdir().unwrap();
        let mut host = spawn(directory.path(), "ready");
        host.wait_ready(Duration::from_secs(5)).await.unwrap();
        let pid = host.pid().unwrap();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _owner = host;
            panic!("exercise owned-child cleanup during unwind");
        }));
        assert!(result.is_err());
        // Read-only OS oracle for the PID returned by the owned Child; no guessed process is killed.
        let observed = Command::new("ps")
            .args(["-p", &pid.to_string(), "-o", "pid="])
            .output()
            .unwrap();
        assert!(
            observed.stdout.is_empty(),
            "owned child remains after panic cleanup"
        );
    }
    #[tokio::test]
    async fn natural_exit_status_is_preserved_and_exit_timeout_reaps_owned_child() {
        let directory = tempfile::tempdir().unwrap();
        let mut exited = spawn(directory.path(), "exit99");
        let status = exited.wait_exit(Duration::from_secs(5)).await.unwrap();
        assert_eq!(status.code(), Some(99));
        assert_eq!(exited.pid(), None);
        let mut silent = spawn(directory.path(), "silent");
        assert!(silent.wait_exit(Duration::from_millis(50)).await.is_err());
        assert_eq!(
            silent.pid(),
            None,
            "exit timeout must kill and reap its owned child"
        );
    }
    #[tokio::test]
    async fn readiness_timeout_and_early_exit_reap_the_owned_child() {
        let directory = tempfile::tempdir().unwrap();
        let mut silent = spawn(directory.path(), "silent");
        assert!(silent.wait_ready(Duration::from_millis(50)).await.is_err());
        assert_eq!(
            silent.pid(),
            None,
            "timeout must kill and reap its owned child"
        );
        let mut exited = spawn(directory.path(), "exit");
        assert!(
            exited
                .wait_ready(Duration::from_secs(5))
                .await
                .unwrap_err()
                .contains("23")
        );
        assert_eq!(exited.pid(), None);
    }
    #[tokio::test]
    async fn early_exit_error_carries_the_child_panic_message() {
        let directory = tempfile::tempdir().unwrap();
        let mut panicked = spawn(directory.path(), "panic");
        let error = panicked
            .wait_ready(Duration::from_secs(5))
            .await
            .unwrap_err();
        assert!(error.contains("stderr excerpt"), "{error}");
        assert!(error.contains("probe-visible child failure"), "{error}");
        assert_eq!(panicked.pid(), None);
    }
}
