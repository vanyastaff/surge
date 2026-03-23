//! Terminal manager for ACP terminal operations.
//!
//! Manages child processes spawned by agents, tracking their output
//! and lifecycle within worktree boundaries.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::AsyncReadExt;
use tokio::process::{Child, Command};
use tokio::sync::{Mutex, Notify};
use tracing::{debug, warn};

/// Managed terminal process.
struct ManagedTerminal {
    /// Child process handle.
    child: Child,
    /// Accumulated output.
    output: Arc<Mutex<String>>,
    /// Maximum output bytes to retain.
    output_byte_limit: Option<u64>,
    /// Whether the process has exited.
    exit_status: Arc<Mutex<Option<TerminalExit>>>,
    /// Notification for exit event.
    exit_notify: Arc<Notify>,
}

/// Terminal exit information.
#[derive(Debug, Clone)]
pub struct TerminalExit {
    /// Process exit code (None if killed by signal).
    pub exit_code: Option<u32>,
    /// Signal name (None if exited normally).
    pub signal: Option<String>,
}

/// Manages terminal processes for a single client.
pub struct TerminalManager {
    terminals: HashMap<String, ManagedTerminal>,
    next_id: u64,
    worktree_root: PathBuf,
}

impl TerminalManager {
    /// Create a new terminal manager.
    #[must_use]
    pub fn new(worktree_root: PathBuf) -> Self {
        Self {
            terminals: HashMap::new(),
            next_id: 0,
            worktree_root,
        }
    }

    /// Spawn a new terminal process.
    ///
    /// # Errors
    ///
    /// Returns error string if process spawn fails.
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

        // Validate cwd is within worktree
        if let Ok(canonical) = work_dir.canonicalize() {
            if let Ok(root_canonical) = self.worktree_root.canonicalize() {
                if !canonical.starts_with(&root_canonical) {
                    return Err(format!(
                        "Terminal cwd {} is outside worktree bounds",
                        work_dir.display()
                    ));
                }
            }
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
        let exit_status: Arc<Mutex<Option<TerminalExit>>> = Arc::new(Mutex::new(None));
        let exit_notify = Arc::new(Notify::new());

        // Spawn background task to collect stdout
        if let Some(stdout) = child.stdout.take() {
            let out = Arc::clone(&output);
            let limit = output_byte_limit;
            tokio::spawn(async move {
                collect_output(stdout, out, limit).await;
            });
        }

        // Spawn background task to collect stderr (merged into output)
        if let Some(stderr) = child.stderr.take() {
            let out = Arc::clone(&output);
            let limit = output_byte_limit;
            tokio::spawn(async move {
                collect_output(stderr, out, limit).await;
            });
        }

        // Exit detection happens lazily in get_output() and wait_for_exit()
        // rather than via a background polling task, since we can't move
        // the child handle out of ManagedTerminal.

        let terminal = ManagedTerminal {
            child,
            output,
            output_byte_limit,
            exit_status,
            exit_notify,
        };

        self.terminals.insert(terminal_id.clone(), terminal);

        Ok(terminal_id)
    }

    /// Get accumulated output from a terminal.
    ///
    /// Also checks if the process has exited and updates status.
    pub async fn get_output(
        &mut self,
        terminal_id: &str,
    ) -> Result<(String, bool, Option<TerminalExit>), String> {
        let terminal = self
            .terminals
            .get_mut(terminal_id)
            .ok_or_else(|| format!("Terminal '{terminal_id}' not found"))?;

        // Try to check if child has exited (non-blocking)
        let exit = match terminal.child.try_wait() {
            Ok(Some(status)) => {
                let exit = TerminalExit {
                    exit_code: status.code().map(|c| c as u32),
                    signal: None,
                };
                *terminal.exit_status.lock().await = Some(exit.clone());
                terminal.exit_notify.notify_waiters();
                Some(exit)
            }
            Ok(None) => None,
            Err(e) => {
                warn!(terminal_id, "failed to check terminal status: {e}");
                None
            }
        };

        let output = terminal.output.lock().await;
        let truncated = terminal
            .output_byte_limit
            .is_some_and(|limit| output.len() as u64 >= limit);

        Ok((output.clone(), truncated, exit))
    }

    /// Wait for a terminal to exit.
    pub async fn wait_for_exit(
        &mut self,
        terminal_id: &str,
    ) -> Result<TerminalExit, String> {
        let terminal = self
            .terminals
            .get_mut(terminal_id)
            .ok_or_else(|| format!("Terminal '{terminal_id}' not found"))?;

        // Check if already exited
        {
            let status = terminal.exit_status.lock().await;
            if let Some(exit) = status.as_ref() {
                return Ok(exit.clone());
            }
        }

        // Wait for the child process
        let status = terminal
            .child
            .wait()
            .await
            .map_err(|e| format!("Failed to wait for terminal '{terminal_id}': {e}"))?;

        let exit = TerminalExit {
            exit_code: status.code().map(|c| c as u32),
            signal: None,
        };

        *terminal.exit_status.lock().await = Some(exit.clone());
        terminal.exit_notify.notify_waiters();

        Ok(exit)
    }

    /// Kill a terminal process.
    pub async fn kill(&mut self, terminal_id: &str) -> Result<(), String> {
        let terminal = self
            .terminals
            .get_mut(terminal_id)
            .ok_or_else(|| format!("Terminal '{terminal_id}' not found"))?;

        debug!(terminal_id, "killing terminal process");

        terminal
            .child
            .kill()
            .await
            .map_err(|e| format!("Failed to kill terminal '{terminal_id}': {e}"))?;

        let exit = TerminalExit {
            exit_code: None,
            signal: Some("SIGKILL".to_string()),
        };
        *terminal.exit_status.lock().await = Some(exit);
        terminal.exit_notify.notify_waiters();

        Ok(())
    }

    /// Release (remove) a terminal, killing it if still running.
    pub async fn release(&mut self, terminal_id: &str) -> Result<(), String> {
        if let Some(mut terminal) = self.terminals.remove(terminal_id) {
            debug!(terminal_id, "releasing terminal");
            // Kill if still running
            if terminal.child.try_wait().ok().flatten().is_none() {
                let _ = terminal.child.kill().await;
            }
            Ok(())
        } else {
            Err(format!("Terminal '{terminal_id}' not found"))
        }
    }
}

impl std::fmt::Debug for TerminalManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TerminalManager")
            .field("active_terminals", &self.terminals.len())
            .field("worktree_root", &self.worktree_root)
            .finish()
    }
}

/// Collect output from a reader into a shared buffer, respecting byte limit.
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
                    let take = chunk.len().min(remaining);
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
        let mut mgr = TerminalManager::new(temp_dir());

        #[cfg(windows)]
        let (cmd, args) = ("cmd", vec!["/C".into(), "echo hello".into()]);
        #[cfg(not(windows))]
        let (cmd, args) = ("echo", vec!["hello".into()]);

        let id = mgr.spawn(cmd, &args, &[], None, None).unwrap();
        let exit = mgr.wait_for_exit(&id).await.unwrap();
        assert_eq!(exit.exit_code, Some(0));

        // Give collector time to finish
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let (output, truncated, _) = mgr.get_output(&id).await.unwrap();
        assert!(output.contains("hello"));
        assert!(!truncated);
    }

    #[tokio::test]
    async fn test_kill_terminal() {
        let mut mgr = TerminalManager::new(temp_dir());

        #[cfg(windows)]
        let (cmd, args) = ("cmd", vec!["/C".into(), "timeout /t 60".into()]);
        #[cfg(not(windows))]
        let (cmd, args) = ("sleep", vec!["60".into()]);

        let id = mgr.spawn(cmd, &args, &[], None, None).unwrap();
        mgr.kill(&id).await.unwrap();

        let exit = mgr.wait_for_exit(&id).await.unwrap();
        // Killed processes may have non-zero or no exit code
        assert!(exit.signal.is_some() || exit.exit_code.is_some());
    }

    #[tokio::test]
    async fn test_release_terminal() {
        let mut mgr = TerminalManager::new(temp_dir());

        #[cfg(windows)]
        let (cmd, args) = ("cmd", vec!["/C".into(), "echo test".into()]);
        #[cfg(not(windows))]
        let (cmd, args) = ("echo", vec!["test".into()]);

        let id = mgr.spawn(cmd, &args, &[], None, None).unwrap();
        mgr.release(&id).await.unwrap();

        // Terminal should be gone
        assert!(mgr.get_output(&id).await.is_err());
    }

    #[tokio::test]
    async fn test_output_byte_limit() {
        let mut mgr = TerminalManager::new(temp_dir());

        // Generate output exceeding limit
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

        let id = mgr.spawn(cmd, &args, &[], None, Some(10)).unwrap();
        let _ = mgr.wait_for_exit(&id).await;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let (output, truncated, _) = mgr.get_output(&id).await.unwrap();
        assert!(output.len() <= 10);
        assert!(truncated);
    }

    #[tokio::test]
    async fn test_not_found() {
        let mut mgr = TerminalManager::new(temp_dir());
        assert!(mgr.get_output("nonexistent").await.is_err());
        assert!(mgr.kill("nonexistent").await.is_err());
        assert!(mgr.wait_for_exit("nonexistent").await.is_err());
    }
}
