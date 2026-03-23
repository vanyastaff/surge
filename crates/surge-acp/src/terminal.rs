//! Terminal manager for ACP terminal operations.
//!
//! Manages child processes spawned by agents, tracking their output
//! and lifecycle within worktree boundaries. Each terminal is independently
//! lockable to avoid blocking concurrent operations.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::AsyncReadExt;
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tracing::debug;

/// Managed terminal process.
pub(crate) struct Terminal {
    /// Child process handle.
    child: Child,
    /// Accumulated output.
    output: Arc<Mutex<String>>,
    /// Maximum output bytes to retain.
    output_byte_limit: Option<u64>,
    /// Cached exit status (set once on exit).
    exit_status: Option<ExitStatus>,
}

/// Terminal exit information.
#[derive(Debug, Clone)]
pub struct ExitStatus {
    /// Process exit code (None if killed by signal).
    pub exit_code: Option<u32>,
    /// Signal name (None if exited normally).
    pub signal: Option<String>,
}

/// Manages terminal processes for a single client.
///
/// Each terminal lives behind its own `Arc<Mutex<_>>` so that operations
/// on one terminal (e.g. `wait_for_exit`) don't block operations on others.
pub struct Terminals {
    terminals: HashMap<String, Arc<Mutex<Terminal>>>,
    next_id: u64,
    worktree_root: PathBuf,
}

impl Terminals {
    /// Create a new terminal manager.
    #[must_use]
    pub fn new(worktree_root: PathBuf) -> Self {
        Self {
            terminals: HashMap::new(),
            next_id: 0,
            worktree_root,
        }
    }

    /// Spawn a new terminal process. Returns the terminal ID.
    ///
    /// This method only needs `&mut self` briefly to insert into the map;
    /// all async work happens in background tasks or per-terminal locks.
    ///
    /// # Errors
    ///
    /// Returns error string if process spawn fails or cwd is outside worktree.
    pub fn spawn(
        &mut self,
        command: &str,
        args: &[String],
        env: &[(String, String)],
        cwd: Option<&PathBuf>,
        output_byte_limit: Option<u64>,
    ) -> Result<String, String> {
        let work_dir = cwd
            .cloned()
            .unwrap_or_else(|| self.worktree_root.clone());

        // Validate cwd is within worktree. Fail closed: if we cannot
        // canonicalize either path, refuse to spawn.
        let canonical_work_dir = work_dir.canonicalize().map_err(|e| {
            format!(
                "Cannot canonicalize terminal cwd {}: {e}",
                work_dir.display()
            )
        })?;
        let canonical_root = self.worktree_root.canonicalize().map_err(|e| {
            format!(
                "Cannot canonicalize worktree root {}: {e}",
                self.worktree_root.display()
            )
        })?;
        if !canonical_work_dir.starts_with(&canonical_root) {
            return Err(format!(
                "Terminal cwd {} is outside worktree bounds",
                work_dir.display()
            ));
        }

        self.next_id += 1;
        let terminal_id = format!("term-{}", self.next_id);

        debug!(
            terminal_id = terminal_id.as_str(),
            command,
            "spawning terminal process"
        );

        let mut cmd = Command::new(command);
        cmd.args(args)
            .current_dir(&work_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null());

        // On Windows, create process without a visible console window
        #[cfg(windows)]
        cmd.creation_flags(0x00000008); // CREATE_NO_WINDOW

        for (key, value) in env {
            cmd.env(key, value);
        }

        let mut child = cmd.spawn().map_err(|e| {
            format!("Failed to spawn terminal command '{command}': {e}")
        })?;

        let output = Arc::new(Mutex::new(String::new()));

        // Spawn background tasks to collect stdout and stderr
        if let Some(stdout) = child.stdout.take() {
            let out = Arc::clone(&output);
            let limit = output_byte_limit;
            tokio::spawn(async move {
                collect_output(stdout, out, limit).await;
            });
        }
        if let Some(stderr) = child.stderr.take() {
            let out = Arc::clone(&output);
            let limit = output_byte_limit;
            tokio::spawn(async move {
                collect_output(stderr, out, limit).await;
            });
        }

        let terminal = Terminal {
            child,
            output,
            output_byte_limit,
            exit_status: None,
        };

        self.terminals
            .insert(terminal_id.clone(), Arc::new(Mutex::new(terminal)));

        Ok(terminal_id)
    }

    /// Get an `Arc` handle to a terminal. The caller locks it independently.
    #[must_use]
    pub(crate) fn get_terminal(&self, terminal_id: &str) -> Option<Arc<Mutex<Terminal>>> {
        self.terminals.get(terminal_id).cloned()
    }

    /// Remove a terminal from the map (for release). Returns the handle if found.
    pub(crate) fn remove_terminal(&mut self, terminal_id: &str) -> Option<Arc<Mutex<Terminal>>> {
        self.terminals.remove(terminal_id)
    }
}

impl std::fmt::Debug for Terminals {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Terminals")
            .field("active_terminals", &self.terminals.len())
            .field("worktree_root", &self.worktree_root)
            .finish()
    }
}

// ── Per-terminal operations (called on the locked Terminal) ───

impl Terminal {
    /// Get accumulated output. Also non-blocking check for exit.
    async fn get_output(&mut self) -> (String, bool, Option<ExitStatus>) {
        // Non-blocking exit check
        if self.exit_status.is_none()
            && let Ok(Some(status)) = self.child.try_wait()
        {
            self.exit_status = Some(ExitStatus {
                exit_code: status.code().map(|c| c as u32),
                signal: None,
            });
        }

        let output = self.output.lock().await;
        let truncated = self
            .output_byte_limit
            .is_some_and(|limit| output.len() as u64 >= limit);

        (output.clone(), truncated, self.exit_status.clone())
    }

    /// Block until child exits.
    async fn wait_for_exit(&mut self) -> Result<ExitStatus, String> {
        if let Some(exit) = &self.exit_status {
            return Ok(exit.clone());
        }

        let status = self
            .child
            .wait()
            .await
            .map_err(|e| format!("Failed to wait for terminal: {e}"))?;

        let exit = ExitStatus {
            exit_code: status.code().map(|c| c as u32),
            signal: None,
        };
        self.exit_status = Some(exit.clone());
        Ok(exit)
    }

    /// Kill the child process.
    async fn kill(&mut self) -> Result<(), String> {
        self.child
            .kill()
            .await
            .map_err(|e| format!("Failed to kill terminal: {e}"))?;

        self.exit_status = Some(ExitStatus {
            exit_code: None,
            signal: Some("SIGKILL".to_string()),
        });
        Ok(())
    }
}

// ── Free functions used by client.rs ────────────────────────────────

/// Get output from a terminal by ID. Acquires only the per-terminal lock.
pub async fn terminal_get_output(
    mgr: &Mutex<Terminals>,
    terminal_id: &str,
) -> Result<(String, bool, Option<ExitStatus>), String> {
    let handle = {
        let m = mgr.lock().await;
        m.get_terminal(terminal_id)
            .ok_or_else(|| format!("Terminal '{terminal_id}' not found"))?
    };
    // Manager lock dropped — only per-terminal lock held
    let mut term = handle.lock().await;
    Ok(term.get_output().await)
}

/// Wait for terminal exit by ID. Acquires only the per-terminal lock.
pub async fn terminal_wait_for_exit(
    mgr: &Mutex<Terminals>,
    terminal_id: &str,
) -> Result<ExitStatus, String> {
    let handle = {
        let m = mgr.lock().await;
        m.get_terminal(terminal_id)
            .ok_or_else(|| format!("Terminal '{terminal_id}' not found"))?
    };
    let mut term = handle.lock().await;
    term.wait_for_exit().await
}

/// Kill terminal by ID. Acquires only the per-terminal lock.
pub async fn terminal_kill(
    mgr: &Mutex<Terminals>,
    terminal_id: &str,
) -> Result<(), String> {
    let handle = {
        let m = mgr.lock().await;
        m.get_terminal(terminal_id)
            .ok_or_else(|| format!("Terminal '{terminal_id}' not found"))?
    };
    let mut term = handle.lock().await;
    term.kill().await
}

/// Release (remove + kill if running) a terminal by ID.
pub async fn terminal_release(
    mgr: &Mutex<Terminals>,
    terminal_id: &str,
) -> Result<(), String> {
    let handle = {
        let mut m = mgr.lock().await;
        m.remove_terminal(terminal_id)
            .ok_or_else(|| format!("Terminal '{terminal_id}' not found"))?
    };
    let mut term = handle.lock().await;
    if term.exit_status.is_none()
        && term.child.try_wait().ok().flatten().is_none()
    {
        let _ = term.child.kill().await;
    }
    debug!(terminal_id, "terminal released");
    Ok(())
}

// ── Output collection ───────────────────────────────────────────────

/// Collect output from a reader into a shared buffer, respecting byte limit.
/// Uses `floor_char_boundary` for safe UTF-8 truncation.
async fn collect_output<R: tokio::io::AsyncRead + Unpin>(
    mut reader: R,
    output: Arc<Mutex<String>>,
    byte_limit: Option<u64>,
) {
    let mut buf = [0u8; 4096];
    loop {
        match reader.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => {
                let chunk = String::from_utf8_lossy(&buf[..n]);
                let mut out = output.lock().await;

                if let Some(limit) = byte_limit {
                    let remaining = limit.saturating_sub(out.len() as u64) as usize;
                    if remaining == 0 {
                        break;
                    }
                    // Safe UTF-8 truncation — never split a multi-byte char
                    let take = chunk.floor_char_boundary(remaining);
                    out.push_str(&chunk[..take]);
                    if take < chunk.len() {
                        break;
                    }
                } else {
                    out.push_str(&chunk);
                }
            }
            Err(e) => {
                debug!("terminal output reader error: {e}");
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_dir() -> PathBuf {
        std::env::temp_dir()
    }

    #[tokio::test]
    async fn test_spawn_and_wait() {
        let mgr = Arc::new(Mutex::new(Terminals::new(temp_dir())));

        #[cfg(windows)]
        let (cmd, args) = ("cmd", vec!["/C".into(), "echo hello".into()]);
        #[cfg(not(windows))]
        let (cmd, args) = ("echo", vec!["hello".into()]);

        let id = mgr.lock().await.spawn(cmd, &args, &[], None, None).unwrap();
        let exit = terminal_wait_for_exit(&mgr, &id).await.unwrap();
        assert_eq!(exit.exit_code, Some(0));

        // Give collector time to finish
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let (output, truncated, _) = terminal_get_output(&mgr, &id).await.unwrap();
        assert!(output.contains("hello"));
        assert!(!truncated);
    }

    #[tokio::test]
    async fn test_kill_terminal() {
        let mgr = Arc::new(Mutex::new(Terminals::new(temp_dir())));

        #[cfg(windows)]
        let (cmd, args) = ("cmd", vec!["/C".into(), "timeout /t 60".into()]);
        #[cfg(not(windows))]
        let (cmd, args) = ("sleep", vec!["60".into()]);

        let id = mgr.lock().await.spawn(cmd, &args, &[], None, None).unwrap();
        terminal_kill(&mgr, &id).await.unwrap();

        let exit = terminal_wait_for_exit(&mgr, &id).await.unwrap();
        assert!(exit.signal.is_some() || exit.exit_code.is_some());
    }

    #[tokio::test]
    async fn test_release_terminal() {
        let mgr = Arc::new(Mutex::new(Terminals::new(temp_dir())));

        #[cfg(windows)]
        let (cmd, args) = ("cmd", vec!["/C".into(), "echo test".into()]);
        #[cfg(not(windows))]
        let (cmd, args) = ("echo", vec!["test".into()]);

        let id = mgr.lock().await.spawn(cmd, &args, &[], None, None).unwrap();
        terminal_release(&mgr, &id).await.unwrap();

        // Terminal should be gone
        assert!(terminal_get_output(&mgr, &id).await.is_err());
    }

    #[tokio::test]
    async fn test_output_byte_limit() {
        let mgr = Arc::new(Mutex::new(Terminals::new(temp_dir())));

        #[cfg(windows)]
        let (cmd, args) = (
            "cmd",
            vec!["/C".into(), "echo aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into()],
        );
        #[cfg(not(windows))]
        let (cmd, args) = (
            "echo",
            vec!["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into()],
        );

        let id = mgr.lock().await.spawn(cmd, &args, &[], None, Some(10)).unwrap();
        let _ = terminal_wait_for_exit(&mgr, &id).await;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let (output, truncated, _) = terminal_get_output(&mgr, &id).await.unwrap();
        assert!(output.len() <= 10);
        assert!(truncated);
    }

    #[tokio::test]
    async fn test_not_found() {
        let mgr = Arc::new(Mutex::new(Terminals::new(temp_dir())));
        assert!(terminal_get_output(&mgr, "nonexistent").await.is_err());
        assert!(terminal_kill(&mgr, "nonexistent").await.is_err());
        assert!(terminal_wait_for_exit(&mgr, "nonexistent").await.is_err());
    }

    #[tokio::test]
    async fn test_concurrent_terminals() {
        let mgr = Arc::new(Mutex::new(Terminals::new(temp_dir())));

        #[cfg(windows)]
        let (cmd1, args1) = ("cmd", vec!["/C".into(), "echo first".into()]);
        #[cfg(not(windows))]
        let (cmd1, args1) = ("echo", vec!["first".into()]);

        #[cfg(windows)]
        let (cmd2, args2) = ("cmd", vec!["/C".into(), "echo second".into()]);
        #[cfg(not(windows))]
        let (cmd2, args2) = ("echo", vec!["second".into()]);

        let id1 = mgr.lock().await.spawn(cmd1, &args1, &[], None, None).unwrap();
        let id2 = mgr.lock().await.spawn(cmd2, &args2, &[], None, None).unwrap();

        // Wait on both concurrently — no deadlock
        let mgr1 = Arc::clone(&mgr);
        let mgr2 = Arc::clone(&mgr);
        let id1c = id1.clone();
        let id2c = id2.clone();

        let (r1, r2) = tokio::join!(
            terminal_wait_for_exit(&mgr1, &id1c),
            terminal_wait_for_exit(&mgr2, &id2c),
        );

        assert_eq!(r1.unwrap().exit_code, Some(0));
        assert_eq!(r2.unwrap().exit_code, Some(0));
    }
}
