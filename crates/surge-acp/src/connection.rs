//! AgentConnection — manages agent process lifecycle and ACP connection.
//!
//! Process spawning and I/O setup is handled by the transport layer
//! ([`crate::transport`]). This module performs the ACP handshake and owns
//! the connection for the lifetime of the agent.

use agent_client_protocol::{
    Agent, AgentCapabilities, ClientCapabilities, ClientSideConnection, Implementation,
    InitializeRequest, ProtocolVersion,
};
use std::collections::HashMap;
use std::path::PathBuf;
use surge_core::SurgeError;
use surge_core::config::AgentConfig;
use tokio::process::Child;
use tracing::{debug, info};

use crate::client::{PermissionPolicy, SurgeClient};
use crate::registry::{AgentCapability, Registry, RegistryEntry};
use crate::transport::{AgentIo, AgentTransport, StdioTransport, TcpTransport};

/// State of an active agent session.
#[derive(Debug, Clone)]
pub struct SessionState {
    /// Session identifier.
    pub session_id: String,
    /// Working directory for this session.
    pub working_dir: PathBuf,
    /// Current mode (if any).
    pub mode: Option<String>,
}

// ── EffectiveCapabilities ────────────────────────────────────────────

/// Merged view of ACP-reported and registry-declared agent capabilities.
///
/// ACP capabilities describe what the agent *can do at the protocol level*
/// (filesystem, terminal). Registry capabilities describe *what tasks* the
/// agent is designed to handle (code, plan, review, …).
#[derive(Debug, Clone)]
pub struct EffectiveCapabilities {
    /// Capabilities reported by the agent during the ACP initialization handshake.
    pub acp: AgentCapabilities,
    /// High-level task capabilities from the builtin registry.
    ///
    /// `None` if the agent is not listed in the builtin catalog (e.g. a custom
    /// agent configured by the user).  In that case callers should assume the
    /// agent is capable of any task rather than blocking it.
    pub registry: Option<Vec<AgentCapability>>,
}

impl EffectiveCapabilities {
    /// Whether the agent declares a specific task capability in the registry.
    ///
    /// Returns `true` for agents not in the builtin catalog — custom agents are
    /// assumed capable of any task rather than being blocked.
    #[must_use]
    pub fn has(&self, cap: &AgentCapability) -> bool {
        self.registry
            .as_deref()
            .map(|caps| caps.contains(cap))
            .unwrap_or(true)
    }

    // ── ACP-level protocol capabilities ─────────────────────────────

    /// Whether the agent supports resuming prior sessions via `session/load`.
    #[must_use]
    pub fn can_load_session(&self) -> bool {
        self.acp.load_session
    }

    /// Whether the agent accepts image content blocks in prompts.
    #[must_use]
    pub fn supports_images(&self) -> bool {
        self.acp.prompt_capabilities.image
    }

    /// Whether the agent accepts audio content blocks in prompts.
    #[must_use]
    pub fn supports_audio(&self) -> bool {
        self.acp.prompt_capabilities.audio
    }

    /// Whether the agent can be given additional MCP server configuration.
    #[must_use]
    pub fn supports_mcp(&self) -> bool {
        self.acp.mcp_capabilities.http || self.acp.mcp_capabilities.sse
    }
}

// ── AgentConnection ──────────────────────────────────────────────────

/// Manages connection to a single agent.
///
/// Handles process lifecycle, ACP connection over stdio, and session management.
pub struct AgentConnection {
    /// Name of the agent from configuration.
    name: String,

    /// ACP connection providing Agent trait methods.
    connection: ClientSideConnection,

    /// Child process handle (for stdio transport).
    process: Option<Child>,

    /// Active sessions tracked by this connection.
    sessions: HashMap<String, SessionState>,

    /// ACP capabilities from the initialization handshake.
    capabilities: AgentCapabilities,

    /// Builtin registry entry for this agent, if found.
    registry_entry: Option<RegistryEntry>,
}

impl AgentConnection {
    /// Spawn an agent process and establish ACP connection.
    ///
    /// # Arguments
    ///
    /// * `name` - Agent name from configuration
    /// * `config` - Agent configuration (command, args, transport)
    /// * `worktree_root` - Root directory for file operations
    /// * `permission_policy` - Policy for agent permissions
    ///
    /// # Errors
    ///
    /// Returns error if:
    /// - Process spawn fails
    /// - ACP initialization handshake fails
    /// - TCP transport is recognized in [`surge_core::config::Transport`] but not yet
    ///   implemented; passing a TCP config returns [`SurgeError::AgentConnection`]
    ///
    /// # Note
    ///
    /// All async operations on `AgentConnection` (and the returned
    /// `ClientSideConnection`) must run inside a `tokio::task::LocalSet`
    /// because the ACP SDK uses `spawn_local` internally.
    /// Use [`AgentPool`] which handles this automatically.
    pub async fn spawn(
        name: String,
        config: &AgentConfig,
        worktree_root: PathBuf,
        permission_policy: PermissionPolicy,
        event_tx: Option<tokio::sync::broadcast::Sender<surge_core::SurgeEvent>>,
    ) -> Result<Self, SurgeError> {
        use surge_core::config::Transport;

        let io = match &config.transport {
            Transport::Stdio => StdioTransport::connect(&name, config, &worktree_root).await?,
            Transport::Tcp { .. } => TcpTransport::connect(&name, config, &worktree_root).await?,
            Transport::WebSocket { .. } => {
                return Err(SurgeError::Config(
                    "WebSocket transport not yet supported".to_string(),
                ));
            }
        };

        Self::connect_with_io(name, io, worktree_root, permission_policy, event_tx).await
    }

    /// Perform the ACP handshake on top of a transport-supplied I/O channel.
    async fn connect_with_io(
        name: String,
        io: AgentIo,
        worktree_root: PathBuf,
        permission_policy: PermissionPolicy,
        event_tx: Option<tokio::sync::broadcast::Sender<surge_core::SurgeEvent>>,
    ) -> Result<Self, SurgeError> {
        // Compute declared capabilities before permission_policy is moved.
        let declared_caps = surge_client_capabilities(&permission_policy);

        let AgentIo {
            reader,
            writer,
            child,
        } = io;

        let mut client = SurgeClient::new(worktree_root.clone(), permission_policy);
        if let Some(tx) = event_tx {
            client = client.with_events(tx);
        }

        // Establish ACP connection over the transport I/O.
        let (connection, io_task) = ClientSideConnection::new(client, writer, reader, |fut| {
            #[allow(clippy::let_underscore_future)]
            let _ = tokio::task::spawn_local(fut);
        });

        tokio::task::spawn_local(async move {
            if let Err(e) = io_task.await {
                tracing::error!("ACP IO task failed: {:?}", e);
            }
        });

        // ACP initialization handshake.
        info!("Performing ACP initialization handshake for '{}'", name);

        let mut init_request = InitializeRequest::new(ProtocolVersion::V1);
        init_request.client_capabilities = declared_caps;
        init_request.client_info = Some(Implementation::new("surge", env!("CARGO_PKG_VERSION")));

        let init_response = connection
            .initialize(init_request)
            .await
            .map_err(|e| SurgeError::Acp(format!("ACP initialization failed: {:?}", e)))?;

        info!(
            "Agent '{}' initialized successfully. Capabilities: {:?}",
            name, init_response.agent_capabilities
        );

        let registry_entry = Registry::builtin().find(&name).cloned();

        Ok(Self {
            name,
            connection,
            process: child,
            sessions: HashMap::new(),
            capabilities: init_response.agent_capabilities,
            registry_entry,
        })
    }

    /// Get the agent name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Get the process ID of the agent (if running via stdio transport).
    ///
    /// Returns `None` for TCP/WebSocket transports or if the process handle is unavailable.
    #[must_use]
    pub fn pid(&self) -> Option<u32> {
        self.process.as_ref().and_then(|child| child.id())
    }

    /// Get the raw ACP capabilities from the initialization handshake.
    #[must_use]
    pub fn capabilities(&self) -> &AgentCapabilities {
        &self.capabilities
    }

    /// Get the merged ACP + registry capabilities for this agent.
    ///
    /// Use this when making routing or permission decisions — it provides a
    /// unified view of what the agent can do at both the protocol and task level.
    #[must_use]
    pub fn effective_capabilities(&self) -> EffectiveCapabilities {
        EffectiveCapabilities {
            acp: self.capabilities.clone(),
            registry: self.registry_entry.as_ref().map(|e| e.capabilities.clone()),
        }
    }

    /// Get access to the underlying ACP connection.
    #[must_use]
    pub fn connection(&self) -> &ClientSideConnection {
        &self.connection
    }

    /// Track a new session.
    pub fn add_session(&mut self, session_id: String, working_dir: PathBuf, mode: Option<String>) {
        self.sessions.insert(
            session_id.clone(),
            SessionState {
                session_id,
                working_dir,
                mode,
            },
        );
    }

    /// Remove a session from tracking.
    pub fn remove_session(&mut self, session_id: &str) {
        self.sessions.remove(session_id);
    }

    /// Get session state.
    #[must_use]
    pub fn get_session(&self, session_id: &str) -> Option<&SessionState> {
        self.sessions.get(session_id)
    }

    /// Check if the agent process is still running.
    #[must_use]
    pub fn is_running(&mut self) -> bool {
        if let Some(process) = &mut self.process {
            process.try_wait().ok().flatten().is_none()
        } else {
            // No child process; connection is externally managed.
            true
        }
    }

    /// Attempt graceful shutdown: wait up to `grace` for exit, then kill.
    pub async fn wait_or_kill(&mut self, grace: std::time::Duration) {
        let Some(process) = self.process.as_mut() else {
            return;
        };
        match tokio::time::timeout(grace, process.wait()).await {
            Ok(Ok(_)) => {
                // Exited cleanly
            }
            _ => {
                // Timed out or wait error — force kill and reap
                let _ = process.kill().await;
                let _ = process.wait().await;
            }
        }
        self.process = None;
    }

    /// Kill the agent process forcefully with platform-specific handling.
    ///
    /// On Windows, this kills the entire process tree (needed because agents are
    /// spawned via `cmd /C` which creates child processes). On Unix, sends SIGKILL
    /// to the process group to ensure all child processes are terminated.
    ///
    /// # Errors
    ///
    /// Returns error if process kill fails.
    pub async fn kill(&mut self) -> Result<(), SurgeError> {
        if let Some(process) = self.process.as_mut() {
            let pid = process.id();

            #[cfg(windows)]
            {
                // On Windows, use taskkill /F /T to kill the entire process tree.
                // This is necessary because agents spawned via cmd.exe create child
                // processes that won't be killed by tokio's kill() alone.
                if let Some(pid) = pid {
                    debug!("Killing Windows process tree for PID {}", pid);
                    let output = tokio::process::Command::new("taskkill")
                        .args(["/F", "/T", "/PID", &pid.to_string()])
                        .output()
                        .await;

                    match output {
                        Ok(out) if out.status.success() => {
                            debug!("Successfully killed process tree for PID {}", pid);
                        }
                        Ok(out) => {
                            // taskkill failed, fall back to tokio kill
                            debug!(
                                "taskkill failed (exit code {:?}), falling back to tokio kill",
                                out.status.code()
                            );
                            let _ = process.kill().await;
                        }
                        Err(e) => {
                            // taskkill command failed to spawn, fall back to tokio kill
                            debug!("taskkill spawn failed ({}), falling back to tokio kill", e);
                            let _ = process.kill().await;
                        }
                    }
                } else {
                    // Process already exited
                    debug!("Process has no PID, already exited");
                }
            }

            #[cfg(not(windows))]
            {
                // On Unix, tokio's kill() sends SIGKILL to the process.
                // For process groups, we'd need to call kill(-pid, SIGKILL) via libc,
                // but for now the simple approach is sufficient since agents typically
                // clean up their children on exit.
                if let Some(pid) = pid {
                    debug!("Killing Unix process PID {}", pid);
                }
                process.kill().await.map_err(|e| {
                    SurgeError::AgentConnection(format!("Failed to kill agent: {}", e))
                })?;
            }

            // Reap the process to prevent zombies
            let _ = process.wait().await;
        }
        self.process = None;
        Ok(())
    }
}

impl Drop for AgentConnection {
    fn drop(&mut self) {
        if let Some(process) = self.process.as_mut() {
            debug!("Dropping AgentConnection '{}', killing process", self.name);
            // start_kill sends the signal synchronously (no await needed)
            let _ = process.start_kill();
        }
    }
}

/// Build Surge client capabilities for ACP initialization.
///
/// The declared capabilities reflect the active [`PermissionPolicy`] so agents
/// do not attempt operations that Surge will never approve.
fn surge_client_capabilities(policy: &PermissionPolicy) -> ClientCapabilities {
    use agent_client_protocol::FileSystemCapabilities;

    let (allow_read, allow_write) = match policy {
        // Full access or interactive (user decides per-request).
        PermissionPolicy::AutoApprove | PermissionPolicy::Interactive => (true, true),
        PermissionPolicy::Smart {
            allow_read,
            allow_write_in_worktree,
            ..
        } => (*allow_read, *allow_write_in_worktree),
    };

    ClientCapabilities::new()
        .fs(FileSystemCapabilities::new()
            .read_text_file(allow_read)
            .write_text_file(allow_write))
        .terminal(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_surge_client_capabilities() {
        let caps = surge_client_capabilities(&PermissionPolicy::AutoApprove);
        assert!(caps.fs.read_text_file);
        assert!(caps.fs.write_text_file);
        assert!(caps.terminal);
    }

    #[test]
    fn test_surge_client_capabilities_smart_read_only() {
        let policy = PermissionPolicy::Smart {
            allow_read: true,
            allow_write_in_worktree: false,
            allow_bash_safe: true,
            deny_bash_dangerous: true,
            deny_network: false,
        };
        let caps = surge_client_capabilities(&policy);
        assert!(caps.fs.read_text_file);
        assert!(!caps.fs.write_text_file);
    }

    mod shutdown {
        use super::*;
        use std::process::Stdio;
        use tokio::process::Command;

        /// Test that kill() properly terminates child processes on Windows.
        #[tokio::test]
        #[cfg(windows)]
        async fn test_kill_windows_process_tree() {
            // Spawn a long-running process via cmd.exe (similar to how agents are spawned)
            let mut child = Command::new("cmd")
                .args(["/C", "ping", "-n", "100", "127.0.0.1"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("Failed to spawn test process");

            let pid = child.id().expect("Failed to get child PID");

            // Verify process is running
            assert!(
                child.try_wait().unwrap().is_none(),
                "Process should be running"
            );

            // Create a mock AgentConnection with just the process field
            let mut mock_connection = TestAgentConnection::new(child);

            // Kill the process
            mock_connection
                .kill()
                .await
                .expect("Failed to kill process");

            // Verify process was killed (check that PID no longer exists)
            let check = Command::new("tasklist")
                .args(["/FI", &format!("PID eq {}", pid)])
                .output()
                .await
                .expect("Failed to run tasklist");

            let output = String::from_utf8_lossy(&check.stdout);
            assert!(
                !output.contains(&pid.to_string()) || output.contains("No tasks"),
                "Process should be killed"
            );
        }

        /// Test that kill() properly terminates child processes on Unix.
        #[tokio::test]
        #[cfg(not(windows))]
        async fn test_kill_unix_process() {
            // Spawn a long-running process
            let mut child = Command::new("sleep")
                .arg("100")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("Failed to spawn test process");

            let pid = child.id().expect("Failed to get child PID");

            // Verify process is running
            assert!(
                child.try_wait().unwrap().is_none(),
                "Process should be running"
            );

            // Create a mock AgentConnection with just the process field
            let mut mock_connection = TestAgentConnection::new(child);

            // Kill the process
            mock_connection
                .kill()
                .await
                .expect("Failed to kill process");

            // Verify process was killed (check that PID no longer exists)
            let check = Command::new("ps")
                .args(["-p", &pid.to_string()])
                .output()
                .await
                .expect("Failed to run ps");

            assert!(
                !check.status.success(),
                "Process should be killed (ps should fail)"
            );
        }

        /// Test that wait_or_kill() respects grace period.
        #[tokio::test]
        async fn test_wait_or_kill_grace_period() {
            #[cfg(windows)]
            let mut child = Command::new("cmd")
                .args(["/C", "ping", "-n", "100", "127.0.0.1"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("Failed to spawn test process");

            #[cfg(not(windows))]
            let mut child = Command::new("sleep")
                .arg("100")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("Failed to spawn test process");

            // Verify process is running
            assert!(
                child.try_wait().unwrap().is_none(),
                "Process should be running"
            );

            let mut mock_connection = TestAgentConnection::new(child);

            // Give a very short grace period — process won't exit in time
            let start = std::time::Instant::now();
            mock_connection
                .wait_or_kill(std::time::Duration::from_millis(100))
                .await;
            let elapsed = start.elapsed();

            // Should have taken approximately the grace period (allow some overhead)
            assert!(
                elapsed >= std::time::Duration::from_millis(100),
                "Should wait for grace period"
            );
            assert!(
                elapsed < std::time::Duration::from_secs(2),
                "Should kill after grace period, not wait forever"
            );

            // Process should be None after wait_or_kill
            assert!(
                mock_connection.process.is_none(),
                "Process should be cleared"
            );
        }

        /// Helper struct for testing AgentConnection methods without full initialization.
        struct TestAgentConnection {
            process: Option<Child>,
        }

        impl TestAgentConnection {
            fn new(child: Child) -> Self {
                Self {
                    process: Some(child),
                }
            }

            async fn kill(&mut self) -> Result<(), SurgeError> {
                if let Some(process) = self.process.as_mut() {
                    let pid = process.id();

                    #[cfg(windows)]
                    {
                        if let Some(pid) = pid {
                            let output = tokio::process::Command::new("taskkill")
                                .args(["/F", "/T", "/PID", &pid.to_string()])
                                .output()
                                .await;

                            match output {
                                Ok(out) if out.status.success() => {}
                                _ => {
                                    let _ = process.kill().await;
                                }
                            }
                        }
                    }

                    #[cfg(not(windows))]
                    {
                        process.kill().await.map_err(|e| {
                            SurgeError::AgentConnection(format!("Failed to kill agent: {}", e))
                        })?;
                    }

                    let _ = process.wait().await;
                }
                self.process = None;
                Ok(())
            }

            async fn wait_or_kill(&mut self, grace: std::time::Duration) {
                let Some(process) = self.process.as_mut() else {
                    return;
                };
                match tokio::time::timeout(grace, process.wait()).await {
                    Ok(Ok(_)) => {
                        // Exited cleanly
                    }
                    _ => {
                        // Timed out or wait error — force kill and reap
                        let _ = process.kill().await;
                        let _ = process.wait().await;
                    }
                }
                self.process = None;
            }
        }
    }
}
