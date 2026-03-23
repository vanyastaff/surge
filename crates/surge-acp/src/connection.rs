//! AgentConnection — manages agent process lifecycle and ACP connection.
//!
//! This module handles spawning agent processes, establishing ACP connections
//! over stdio transport, and managing the agent lifecycle.

use agent_client_protocol::{
    Agent, AgentCapabilities, ClientCapabilities, ClientSideConnection, Implementation,
    InitializeRequest, ProtocolVersion,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use surge_core::config::{AgentConfig, Transport};
use surge_core::SurgeError;
use tokio::process::{Child, Command};
use tracing::{debug, info};

use crate::client::{PermissionPolicy, SurgeClient};

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

    /// Agent capabilities from initialization handshake.
    capabilities: AgentCapabilities,
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
        info!("Spawning agent '{}' with command: {}", name, config.command);

        match &config.transport {
            Transport::Stdio => {
                Self::spawn_stdio(name, config, worktree_root, permission_policy, event_tx).await
            }
            Transport::Tcp { host, port } => Err(SurgeError::AgentConnection(format!(
                "TCP transport not yet implemented ({}:{})",
                host, port
            ))),
        }
    }

    /// Spawn agent with stdio transport.
    async fn spawn_stdio(
        name: String,
        config: &AgentConfig,
        worktree_root: PathBuf,
        permission_policy: PermissionPolicy,
        event_tx: Option<tokio::sync::broadcast::Sender<surge_core::SurgeEvent>>,
    ) -> Result<Self, SurgeError> {
        // On Windows, script-based commands (npx, npm, etc.) need cmd /C
        // because CreateProcessW doesn't resolve .cmd/.bat via PATHEXT.
        #[cfg(windows)]
        let mut cmd = {
            let mut c = Command::new("cmd");
            c.arg("/C").arg(&config.command);
            c.args(&config.args);
            c.creation_flags(0x08000000); // CREATE_NO_WINDOW
            c
        };
        #[cfg(not(windows))]
        let mut cmd = {
            let mut c = Command::new(&config.command);
            c.args(&config.args);
            c
        };

        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        cmd.current_dir(&worktree_root);

        debug!("Spawning command: {:?}", cmd);

        let mut child = cmd.spawn().map_err(|e| {
            SurgeError::AgentConnection(format!(
                "Failed to spawn agent '{}' ({}): {}",
                name, config.command, e
            ))
        })?;

        // Extract stdio handles — tokio::process::Child yields async handles directly
        let stdin = child.stdin.take().ok_or_else(|| {
            SurgeError::AgentConnection("Failed to capture agent stdin".to_string())
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            SurgeError::AgentConnection("Failed to capture agent stdout".to_string())
        })?;

        // Drain stderr to tracing::warn in background.
        // Uses tokio::spawn (not spawn_local) since ChildStderr is Send.
        if let Some(stderr) = child.stderr.take() {
            let agent_name = name.clone();
            tokio::spawn(async move {
                use tokio::io::{AsyncBufReadExt, BufReader};
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    tracing::warn!(agent = %agent_name, "[stderr] {}", line);
                }
            });
        }

        // Wrap in futures AsyncRead/AsyncWrite as required by ACP SDK
        use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
        let async_stdin = stdin.compat_write();
        let async_stdout = stdout.compat();

        // Create SurgeClient for this connection
        let mut client = SurgeClient::new(worktree_root.clone(), permission_policy);
        if let Some(tx) = event_tx {
            client = client.with_events(tx);
        }

        // Establish ACP connection
        let (connection, io_task) = ClientSideConnection::new(
            client,
            async_stdin,
            async_stdout,
            |fut| {
                #[allow(clippy::let_underscore_future)]
                let _ = tokio::task::spawn_local(fut);
            },
        );

        // Spawn the IO task to handle RPC communication
        tokio::task::spawn_local(async move {
            if let Err(e) = io_task.await {
                tracing::error!("ACP IO task failed: {:?}", e);
            }
        });

        // Perform ACP initialization handshake
        info!("Performing ACP initialization handshake for '{}'", name);

        let mut init_request = InitializeRequest::new(ProtocolVersion::V1);
        init_request.client_capabilities = surge_client_capabilities();
        init_request.client_info = Some(Implementation::new("surge", env!("CARGO_PKG_VERSION")));

        let init_response = connection
            .initialize(init_request)
            .await
            .map_err(|e| SurgeError::Acp(format!("ACP initialization failed: {:?}", e)))?;

        info!(
            "Agent '{}' initialized successfully. Capabilities: {:?}",
            name, init_response.agent_capabilities
        );

        Ok(Self {
            name,
            connection,
            process: Some(child),
            sessions: HashMap::new(),
            capabilities: init_response.agent_capabilities,
        })
    }

    /// Get the agent name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Get the agent capabilities.
    #[must_use]
    pub fn capabilities(&self) -> &AgentCapabilities {
        &self.capabilities
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

    /// Kill the agent process.
    ///
    /// # Errors
    ///
    /// Returns error if process kill fails.
    pub async fn kill(&mut self) -> Result<(), SurgeError> {
        if let Some(process) = self.process.as_mut() {
            process
                .kill()
                .await
                .map_err(|e| SurgeError::AgentConnection(format!("Failed to kill agent: {}", e)))?;
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
fn surge_client_capabilities() -> ClientCapabilities {
    use agent_client_protocol::FileSystemCapabilities;
    ClientCapabilities::new()
        .fs(
            FileSystemCapabilities::new()
                .read_text_file(true)
                .write_text_file(true),
        )
        .terminal(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_surge_client_capabilities() {
        let caps = surge_client_capabilities();
        assert!(caps.fs.read_text_file);
        assert!(caps.fs.write_text_file);
        assert!(caps.terminal);
    }
}
