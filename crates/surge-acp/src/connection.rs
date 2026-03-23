//! AgentConnection — manages agent process lifecycle and ACP connection.
//!
//! This module handles spawning agent processes, establishing ACP connections
//! over stdio transport, and managing the agent lifecycle.

use agent_client_protocol::{
    Agent, AgentCapabilities, ClientCapabilities, ClientSideConnection, FileSystemCapability,
    InitializeRequest,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use surge_core::config::{AgentConfig, Transport};
use surge_core::SurgeError;
use tracing::{debug, info};

use crate::client::{PermissionPolicy, SurgeClient};

// Note: agent_client_protocol re-exports VERSION constant from schema

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
    /// - Only stdio transport is currently supported
    pub async fn spawn(
        name: String,
        config: &AgentConfig,
        worktree_root: PathBuf,
        permission_policy: PermissionPolicy,
    ) -> Result<Self, SurgeError> {
        info!("Spawning agent '{}' with command: {}", name, config.command);

        // Currently only stdio transport is supported
        match &config.transport {
            Transport::Stdio => {
                Self::spawn_stdio(name, config, worktree_root, permission_policy).await
            }
            Transport::Tcp { host, port } => {
                Err(SurgeError::AgentConnection(format!(
                    "TCP transport not yet implemented ({}:{})",
                    host, port
                )))
            }
        }
    }

    /// Spawn agent with stdio transport.
    async fn spawn_stdio(
        name: String,
        config: &AgentConfig,
        worktree_root: PathBuf,
        permission_policy: PermissionPolicy,
    ) -> Result<Self, SurgeError> {
        // Build command
        let mut cmd = Command::new(&config.command);
        cmd.args(&config.args);
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        // Set working directory to worktree root
        cmd.current_dir(&worktree_root);

        debug!("Spawning command: {:?}", cmd);

        // Spawn process
        let mut child = cmd.spawn().map_err(|e| {
            SurgeError::AgentConnection(format!(
                "Failed to spawn agent '{}' ({}): {}",
                name, config.command, e
            ))
        })?;

        // Extract stdio handles
        let stdin = child.stdin.take().ok_or_else(|| {
            SurgeError::AgentConnection("Failed to capture agent stdin".to_string())
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            SurgeError::AgentConnection("Failed to capture agent stdout".to_string())
        })?;

        // Wrap in tokio async I/O and then use futures AsyncRead/AsyncWrite compat
        use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

        let async_stdin = tokio::process::ChildStdin::from_std(stdin).map_err(|e| {
            SurgeError::AgentConnection(format!("Failed to create async stdin: {}", e))
        })?.compat_write();

        let async_stdout = tokio::process::ChildStdout::from_std(stdout).map_err(|e| {
            SurgeError::AgentConnection(format!("Failed to create async stdout: {}", e))
        })?.compat();

        // Create SurgeClient for this connection
        let client = SurgeClient::new(worktree_root.clone(), permission_policy);

        // Establish ACP connection
        // ClientSideConnection::new returns (connection, io_task)
        // The spawn function is used by the library to spawn internal tasks
        let (connection, io_task) = ClientSideConnection::new(
            client,
            async_stdin,
            async_stdout,
            // Use tokio::task::spawn_local for LocalBoxFuture tasks
            // These are protocol-internal tasks that don't need to be Send
            // Caller must ensure a LocalSet is active (e.g. via tokio::task::LocalSet)
            |fut| {
                #[allow(clippy::let_underscore_future)]
                let _ = tokio::task::spawn_local(fut);
            },
        );

        // Spawn the IO task to handle the RPC communication
        tokio::task::spawn_local(async move {
            if let Err(e) = io_task.await {
                tracing::error!("ACP IO task failed: {:?}", e);
            }
        });

        // Perform ACP initialization handshake
        info!("Performing ACP initialization handshake for '{}'", name);

        let init_request = InitializeRequest {
            protocol_version: agent_client_protocol::VERSION,
            client_capabilities: surge_client_capabilities(),
            meta: None,
        };

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
    ///
    /// This provides access to all Agent trait methods (ping, prompt, new_session, etc.).
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
            // try_wait returns None if still running, Some(ExitStatus) if exited
            process.try_wait().ok().flatten().is_none()
        } else {
            // No process means TCP transport (not yet implemented), assume connected
            true
        }
    }

    /// Kill the agent process.
    ///
    /// # Errors
    ///
    /// Returns error if process kill fails.
    pub fn kill(&mut self) -> Result<(), SurgeError> {
        if let Some(process) = &mut self.process {
            process
                .kill()
                .map_err(|e| SurgeError::AgentConnection(format!("Failed to kill agent: {}", e)))?;
        }
        Ok(())
    }
}

impl Drop for AgentConnection {
    fn drop(&mut self) {
        // Attempt to cleanly terminate the agent process
        if let Some(mut process) = self.process.take() {
            debug!("Dropping AgentConnection '{}', killing process", self.name);
            let _ = process.kill();
        }
    }
}

/// Build Surge client capabilities for ACP initialization.
fn surge_client_capabilities() -> ClientCapabilities {
    ClientCapabilities {
        fs: FileSystemCapability {
            read_text_file: true,
            write_text_file: true,
            meta: None,
        },
        terminal: true,
        meta: None,
    }
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

    // Note: Full integration tests require a real agent binary and are tested
    // in integration tests or through CLI commands.
}
