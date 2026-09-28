//! AgentConnection — manages agent process lifecycle and ACP connection.
//!
//! Process spawning and I/O setup is handled by the transport layer
//! ([`crate::transport`]). This module performs the ACP handshake and owns
//! the connection for the lifetime of the agent.
// pre-existing per M2 precedent; not in scope for M3
#![allow(clippy::excessive_nesting)]

use crate::sdk_v1::ClientConnection;
use agent_client_protocol::schema::ProtocolVersion;

use agent_client_protocol::schema::v1::{
    AgentCapabilities, ClientCapabilities, Implementation, InitializeRequest,
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
    connection: ClientConnection,

    driver: OwnedDriver,

    /// Child process handle (for stdio transport).
    process: Option<Child>,

    /// Active sessions tracked by this connection.
    sessions: HashMap<String, SessionState>,

    /// ACP capabilities from the initialization handshake.
    capabilities: AgentCapabilities,

    /// Builtin registry entry for this agent, if found.
    registry_entry: Option<RegistryEntry>,
}

/// Retains the task even when a caller cancels an awaited shutdown.
struct OwnedDriver(Option<tokio::task::JoinHandle<agent_client_protocol::Result<()>>>);
impl OwnedDriver {
    async fn join(&mut self, limit: std::time::Duration) -> Result<(), SurgeError> {
        let Some(task) = self.0.as_mut() else {
            return Ok(());
        };
        let result = tokio::time::timeout(limit, &mut *task).await;
        match result {
            Ok(result) => {
                self.0.take();
                result
                    .map_err(|_| SurgeError::AgentConnection("ACP driver task failed".into()))?
                    .map_err(|_| SurgeError::AgentConnection("ACP driver failed".into()))
            },
            Err(_) => {
                task.abort();
                let _ = task.await;
                self.0.take();
                Err(SurgeError::AgentConnection(
                    "ACP driver shutdown timed out; cleanup could not be confirmed".into(),
                ))
            },
        }
    }
}
impl Drop for OwnedDriver {
    fn drop(&mut self) {
        if let Some(task) = &self.0 {
            task.abort();
        }
    }
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
    /// `ClientConnection`) must run inside a `tokio::task::LocalSet`
    /// because Surge owns local callback tasks on that executor.
    /// Use `AgentPool` (M8+) which handles this automatically.
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
            },
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

        let mut client = SurgeClient::new(worktree_root.clone(), permission_policy);
        if let Some(tx) = event_tx {
            client = client.with_events(tx);
        }

        Self::connect_with_client(name, io, declared_caps, client).await
    }

    async fn connect_with_client(
        name: String,
        io: AgentIo,
        declared_caps: ClientCapabilities,
        client: impl crate::sdk_v1::ClientCallbacks + 'static,
    ) -> Result<Self, SurgeError> {
        let AgentIo {
            reader,
            writer,
            child,
        } = io;
        // Establish ACP connection over the transport I/O.
        let (connection, io_task) = ClientConnection::new(client, writer, reader);

        let driver = OwnedDriver(Some(tokio::task::spawn_local(io_task)));
        let registry_entry = Registry::builtin().find(&name).cloned();
        // Construct the owner before awaiting handshake: cancellation drops all resources.
        let mut owner = Self {
            name: name.clone(),
            connection,
            driver,
            process: child,
            sessions: HashMap::new(),
            capabilities: AgentCapabilities::default(),
            registry_entry,
        };

        // ACP initialization handshake.
        info!("Performing ACP initialization handshake for '{}'", name);

        let mut init_request = InitializeRequest::new(ProtocolVersion::V1);
        init_request.client_capabilities = declared_caps;
        init_request.client_info = Some(Implementation::new("surge", env!("CARGO_PKG_VERSION")));

        let init_response = match owner.connection.initialize(init_request).await {
            Ok(response) => response,
            Err(_) => {
                owner.kill().await?;
                return Err(SurgeError::Acp("ACP initialization failed".into()));
            },
        };

        info!(
            "Agent '{}' initialized successfully. Capabilities: {:?}",
            name, init_response.agent_capabilities
        );

        owner.capabilities = init_response.agent_capabilities;
        Ok(owner)
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
    pub(crate) fn connection(&self) -> &ClientConnection {
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

    /// Close the transport and join callbacks, then wait for child exit or force kill.
    ///
    /// # Errors
    /// Returns an error if driver termination or child reaping cannot be confirmed.
    pub async fn wait_or_kill(&mut self, grace: std::time::Duration) -> Result<(), SurgeError> {
        self.connection.stop();
        let driver_result = self
            .driver
            .join(crate::sdk_v1::DRIVER_CLEANUP_TIMEOUT)
            .await;
        let child_result = if let Some(process) = self.process.as_mut() {
            if matches!(tokio::time::timeout(grace, process.wait()).await, Ok(Ok(_))) {
                self.process = None;
                Ok(())
            } else {
                self.kill_process().await
            }
        } else {
            Ok(())
        };
        child_result?;
        driver_result
    }

    /// Kill the agent process forcefully with platform-specific handling.
    ///
    /// On Windows, this kills the entire process tree (needed because agents are
    /// spawned via `cmd /C` which creates child processes). On Unix, kills and
    /// reaps the direct child; this does not guarantee descendant termination.
    ///
    /// # Errors
    ///
    /// Returns error if process kill fails.
    pub async fn kill(&mut self) -> Result<(), SurgeError> {
        self.connection.stop();
        let driver_result = self
            .driver
            .join(crate::sdk_v1::DRIVER_CLEANUP_TIMEOUT)
            .await;
        self.kill_process().await?;
        driver_result
    }

    async fn kill_process(&mut self) -> Result<(), SurgeError> {
        if let Some(process) = self.process.as_mut() {
            let pid = process.id();

            #[cfg(windows)]
            {
                // On Windows, use taskkill /F /T to kill the entire process tree.
                // This is necessary because agents spawned via cmd.exe create child
                // processes that won't be killed by tokio's kill() alone.
                if let Some(pid) = pid {
                    debug!("Killing Windows process tree for PID {}", pid);
                    let output = tokio::time::timeout(
                        std::time::Duration::from_secs(2),
                        tokio::process::Command::new("taskkill")
                            .kill_on_drop(true)
                            .args(["/F", "/T", "/PID", &pid.to_string()])
                            .output(),
                    )
                    .await;

                    match output {
                        Ok(Ok(out)) if out.status.success() => {
                            debug!("Successfully killed process tree for PID {}", pid);
                        },
                        Ok(Ok(out)) => {
                            // taskkill failed, fall back to tokio kill
                            debug!(
                                "taskkill failed (exit code {:?}), falling back to tokio kill",
                                out.status.code()
                            );
                            process.start_kill().map_err(|e| {
                                SurgeError::AgentConnection(format!("Agent kill failed: {e}"))
                            })?;
                        },
                        _ => {
                            process.start_kill().map_err(|e| {
                                SurgeError::AgentConnection(format!("Agent kill failed: {e}"))
                            })?;
                        },
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
                process.start_kill().map_err(|e| {
                    SurgeError::AgentConnection(format!("Failed to kill agent: {}", e))
                })?;
            }

            // Reap the process to prevent zombies
            tokio::time::timeout(std::time::Duration::from_secs(2), process.wait())
                .await
                .map_err(|_| SurgeError::AgentConnection("Agent reap timed out".into()))?
                .map_err(|e| SurgeError::AgentConnection(format!("Agent reap failed: {e}")))?;
        }
        self.process = None;
        Ok(())
    }
}

impl Drop for AgentConnection {
    fn drop(&mut self) {
        // Best effort only; explicit shutdown is required for verified cleanup.
        self.connection.stop();
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
pub(crate) fn surge_client_capabilities(policy: &PermissionPolicy) -> ClientCapabilities {
    use agent_client_protocol::schema::v1::FileSystemCapabilities;

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
    struct HeldCallback {
        entered: std::sync::Arc<tokio::sync::Notify>,
        dropped: std::sync::Arc<std::sync::atomic::AtomicBool>,
    }
    impl Drop for HeldCallback {
        fn drop(&mut self) {
            self.dropped
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }
    #[async_trait::async_trait(?Send)]
    impl crate::sdk_v1::ClientCallbacks for HeldCallback {
        async fn request(
            &self,
            _: agent_client_protocol::schema::v1::AgentRequest,
        ) -> agent_client_protocol::Result<serde_json::Value> {
            std::future::pending().await
        }
        async fn notification(
            &self,
            _: agent_client_protocol::schema::v1::AgentNotification,
        ) -> agent_client_protocol::Result<()> {
            self.entered.notify_one();
            std::future::pending().await
        }
    }

    #[tokio::test]
    async fn cleanup_driver_errors_reach_connection_caller() {
        use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
        tokio::task::LocalSet::new()
            .run_until(async {
                for stall in [false, true] {
                    let (local, _peer) = tokio::io::duplex(32);
                    let (read, write) = tokio::io::split(local);
                    let (connection, _) = ClientConnection::new(
                        SurgeClient::new(PathBuf::from("."), PermissionPolicy::AutoApprove),
                        write.compat_write(),
                        read.compat(),
                    );
                    let driver = OwnedDriver(Some(tokio::task::spawn_local(async move {
                        if stall {
                            std::future::pending::<()>().await;
                        }
                        Err(agent_client_protocol::Error::internal_error())
                    })));
                    let mut owner = AgentConnection {
                        name: "fixture".into(),
                        connection,
                        driver,
                        process: None,
                        sessions: HashMap::new(),
                        capabilities: AgentCapabilities::default(),
                        registry_entry: None,
                    };
                    let error = owner
                        .wait_or_kill(std::time::Duration::ZERO)
                        .await
                        .unwrap_err()
                        .to_string();
                    assert!(
                        error.contains(if stall { "timed out" } else { "driver failed" }),
                        "{error}"
                    );
                    assert!(owner.driver.0.is_none());
                }
            })
            .await;
    }

    #[tokio::test]
    async fn kill_joins_legacy_driver_and_closes_transport() {
        use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
        use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
        tokio::task::LocalSet::new().run_until(async {
            let (local, peer) = tokio::io::duplex(4096);
            let (read, write) = tokio::io::split(local);
            let (peer_read, mut peer_write) = tokio::io::split(peer);
            let peer_task = tokio::task::spawn_local(async move {
                let mut reader = BufReader::new(peer_read);
                let mut line = String::new();
                reader.read_line(&mut line).await.unwrap();
                let request: serde_json::Value = serde_json::from_str(&line).unwrap();
                let response = serde_json::json!({"jsonrpc":"2.0", "id":request["id"], "result":{"protocolVersion":1,"agentCapabilities":{}}});
                peer_write.write_all(format!("{response}\n").as_bytes()).await.unwrap();
                let notification = serde_json::json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"hello"}}}});
                peer_write.write_all(format!("{notification}\n").as_bytes()).await.unwrap();
                let mut rest = Vec::new();
                reader.read_to_end(&mut rest).await.unwrap();
            });
            #[cfg(unix)]
            let child = Some(tokio::process::Command::new("sleep").arg("60").kill_on_drop(true).spawn().unwrap());
            #[cfg(not(unix))]
            let child = None;
            let io = AgentIo { reader: Box::new(read.compat()), writer: Box::new(write.compat_write()), child };
            let entered = std::sync::Arc::new(tokio::sync::Notify::new());
            let dropped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let callbacks = HeldCallback { entered: entered.clone(), dropped: dropped.clone() };
            let mut connection = AgentConnection::connect_with_client("test".into(), io, ClientCapabilities::default(), callbacks).await.unwrap();
            tokio::time::timeout(std::time::Duration::from_secs(1), entered.notified()).await.unwrap();
            connection.kill().await.unwrap();
            assert!(connection.driver.0.is_none());
            assert!(connection.process.is_none());
            assert!(dropped.load(std::sync::atomic::Ordering::SeqCst));
            tokio::time::timeout(std::time::Duration::from_secs(1), peer_task).await.expect("successful kill must join driver and close transport").unwrap();
        }).await;
    }

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
            let mut child = Command::new("powershell")
                .args(["-NoProfile", "-Command", "Start-Sleep -Seconds 100"])
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
                elapsed >= std::time::Duration::from_millis(80),
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
                    #[cfg(windows)]
                    {
                        if let Some(pid) = process.id() {
                            let output = tokio::process::Command::new("taskkill")
                                .args(["/F", "/T", "/PID", &pid.to_string()])
                                .output()
                                .await;

                            match output {
                                Ok(Ok(out)) if out.status.success() => {},
                                _ => {
                                    process.start_kill().map_err(|e| {
                                        SurgeError::AgentConnection(format!(
                                            "Agent kill failed: {e}"
                                        ))
                                    })?;
                                },
                            }
                        }
                    }

                    #[cfg(not(windows))]
                    {
                        process.start_kill().map_err(|e| {
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
                    },
                    _ => {
                        // Timed out or wait error — force kill and reap
                        let _ = process.kill().await;
                        let _ = process.wait().await;
                    },
                }
                self.process = None;
            }
        }
    }
}
