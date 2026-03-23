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
use surge_core::config::AgentConfig;
use surge_core::SurgeError;
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
                ))
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

        let AgentIo { reader, writer, child } = io;

        let mut client = SurgeClient::new(worktree_root.clone(), permission_policy);
        if let Some(tx) = event_tx {
            client = client.with_events(tx);
        }

        // Establish ACP connection over the transport I/O.
        let (connection, io_task) = ClientSideConnection::new(
            client,
            writer,
            reader,
            |fut| {
                #[allow(clippy::let_underscore_future)]
                let _ = tokio::task::spawn_local(fut);
            },
        );

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
        .fs(
            FileSystemCapabilities::new()
                .read_text_file(allow_read)
                .write_text_file(allow_write),
        )
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
}
