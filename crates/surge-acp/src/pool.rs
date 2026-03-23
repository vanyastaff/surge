//! AgentPool — multi-agent management.
//!
//! This module provides a pool for managing multiple agent connections,
//! handling lazy initialization, session creation, and agent routing.

use agent_client_protocol::{
    Agent, ContentBlock, NewSessionRequest, NewSessionResponse, PromptRequest, PromptResponse,
    SetSessionModeRequest,
};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;
use surge_core::config::AgentConfig;
use surge_core::SurgeError;
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, info, warn};

use crate::client::PermissionPolicy;
use crate::connection::AgentConnection;
use crate::health::HealthMonitor;

/// Handle to an active agent session.
#[derive(Debug, Clone)]
pub struct SessionHandle {
    /// Session identifier from the agent.
    pub session_id: String,
    /// Name of the agent hosting this session.
    pub agent_name: String,
}

/// Pool for managing multiple agent connections.
///
/// Provides lazy initialization of agent connections, session management,
/// and routing of tasks to appropriate agents.
pub struct AgentPool {
    /// Configuration for available agents.
    configs: HashMap<String, AgentConfig>,

    /// Active agent connections (lazily initialized).
    connections: Arc<RwLock<HashMap<String, AgentConnection>>>,

    /// Default agent name for tasks without explicit agent specification.
    default_agent: String,

    /// Root directory for worktree operations.
    worktree_root: std::path::PathBuf,

    /// Default permission policy for agent connections.
    permission_policy: PermissionPolicy,

    /// Health monitor for tracking agent reliability and fallback routing.
    health: Arc<Mutex<HealthMonitor>>,
}

impl AgentPool {
    /// Create a new agent pool.
    ///
    /// # Arguments
    ///
    /// * `configs` - Map of agent name to configuration
    /// * `default_agent` - Name of the default agent
    /// * `worktree_root` - Root directory for file operations
    /// * `permission_policy` - Policy for controlling agent permissions
    ///
    /// # Errors
    ///
    /// Returns error if default agent is not found in configs.
    pub fn new(
        configs: HashMap<String, AgentConfig>,
        default_agent: String,
        worktree_root: std::path::PathBuf,
        permission_policy: PermissionPolicy,
    ) -> Result<Self, SurgeError> {
        // Validate that default agent exists in configs
        if !configs.contains_key(&default_agent) {
            return Err(SurgeError::Config(format!(
                "Default agent '{}' not found in agent configurations",
                default_agent
            )));
        }

        let mut health_monitor = HealthMonitor::new();
        for name in configs.keys() {
            health_monitor.register(name);
        }

        Ok(Self {
            configs,
            connections: Arc::new(RwLock::new(HashMap::new())),
            default_agent,
            worktree_root,
            permission_policy,
            health: Arc::new(Mutex::new(health_monitor)),
        })
    }

    /// Get the default agent name.
    #[must_use]
    pub fn default_agent(&self) -> &str {
        &self.default_agent
    }

    /// Get list of available agent names.
    #[must_use]
    pub fn available_agents(&self) -> Vec<&str> {
        self.configs.keys().map(String::as_str).collect()
    }

    /// Get or create a connection to an agent.
    ///
    /// If the agent is not already connected, spawns the agent process and
    /// establishes an ACP connection. Subsequent calls return the existing connection.
    ///
    /// # Arguments
    ///
    /// * `name` - Name of the agent to connect to
    ///
    /// # Errors
    ///
    /// Returns error if:
    /// - Agent name is not found in configurations
    /// - Agent spawn or initialization fails
    pub async fn get_or_connect(&self, name: &str) -> Result<(), SurgeError> {
        // Fast path: check if already connected
        {
            let connections = self.connections.read().await;
            if connections.contains_key(name) {
                debug!("Agent '{}' already connected", name);
                return Ok(());
            }
        }

        // Slow path: spawn new connection
        info!("Connecting to agent '{}'", name);

        let config = self
            .configs
            .get(name)
            .ok_or_else(|| SurgeError::AgentNotFound(name.to_string()))?;

        let connection = AgentConnection::spawn(
            name.to_string(),
            config,
            self.worktree_root.clone(),
            self.permission_policy.clone(),
        )
        .await?;

        self.connections
            .write()
            .await
            .insert(name.to_string(), connection);

        info!("Agent '{}' connected successfully", name);
        Ok(())
    }

    /// Check if an agent is responsive by ensuring connection.
    ///
    /// Automatically connects to the agent if not already connected.
    ///
    /// # Arguments
    ///
    /// * `name` - Name of the agent to check
    ///
    /// # Errors
    ///
    /// Returns error if agent is not found or connection fails.
    pub async fn ping(&self, name: &str) -> Result<(), SurgeError> {
        // Simply ensure connection is established - that's our "ping"
        self.get_or_connect(name).await
    }

    /// Create a new session with an agent.
    ///
    /// # Arguments
    ///
    /// * `agent_name` - Name of the agent (or None to use default)
    /// * `mode` - Optional session mode (e.g., "code", "plan")
    /// * `working_dir` - Working directory for the session
    ///
    /// # Errors
    ///
    /// Returns error if:
    /// - Agent connection fails
    /// - Session creation fails
    /// - Mode setting fails (if requested and supported)
    pub async fn create_session(
        &self,
        agent_name: Option<&str>,
        mode: Option<&str>,
        working_dir: &Path,
    ) -> Result<SessionHandle, SurgeError> {
        let agent_name = agent_name.unwrap_or(&self.default_agent);

        self.get_or_connect(agent_name).await?;

        let mut connections = self.connections.write().await;
        let connection = connections.get_mut(agent_name).ok_or_else(|| {
            SurgeError::AgentConnection("Connection disappeared".to_string())
        })?;

        // Create new session
        debug!(
            "Creating session with agent '{}' in {}",
            agent_name,
            working_dir.display()
        );

        let request = NewSessionRequest {
            cwd: working_dir.to_path_buf(),
            mcp_servers: vec![],
            meta: None,
        };

        let response: NewSessionResponse = connection
            .connection()
            .new_session(request)
            .await
            .map_err(|e| SurgeError::Acp(format!("Failed to create session: {:?}", e)))?;

        let session_id = response.session_id.to_string();

        // Set mode if requested
        // Note: In ACP, mode setting is a separate method and capability checking
        // would require inspecting prompt_capabilities.modes if available
        if let Some(mode) = mode {
            debug!("Setting session mode to '{}'", mode);

            let _: () = connection
                .connection()
                .set_session_mode(SetSessionModeRequest {
                    session_id: session_id.clone().into(),
                    mode_id: agent_client_protocol::SessionModeId(mode.to_string().into()),
                    meta: None,
                })
                .await
                .map(|_| ())
                .or_else(|e| {
                    // If mode setting fails, log but don't fail the whole session creation
                    debug!(
                        "Failed to set session mode '{}': {:?}, continuing anyway",
                        mode, e
                    );
                    Ok::<(), SurgeError>(())
                })?;
        }

        // Track session in connection
        connection.add_session(session_id.clone(), working_dir.to_path_buf(), mode.map(String::from));

        Ok(SessionHandle {
            session_id,
            agent_name: agent_name.to_string(),
        })
    }

    /// Send a prompt to an agent session.
    ///
    /// # Arguments
    ///
    /// * `session` - Session handle from create_session
    /// * `content` - Prompt content blocks to send
    ///
    /// # Errors
    ///
    /// Returns error if agent connection is not found or prompt fails.
    pub async fn prompt(
        &self,
        session: &SessionHandle,
        content: Vec<ContentBlock>,
    ) -> Result<PromptResponse, SurgeError> {
        let connections = self.connections.read().await;
        let connection = connections.get(&session.agent_name).ok_or_else(|| {
            SurgeError::AgentConnection(format!(
                "Agent '{}' not connected",
                session.agent_name
            ))
        })?;

        debug!(
            "Sending prompt to agent '{}' session '{}'",
            session.agent_name, session.session_id
        );

        let request = PromptRequest {
            session_id: session.session_id.clone().into(),
            prompt: content.clone(),
            meta: None,
        };

        let start = Instant::now();
        match connection.connection().prompt(request).await {
            Ok(response) => {
                self.health
                    .lock()
                    .await
                    .record_success(&session.agent_name, start.elapsed());
                Ok(response)
            }
            Err(e) => {
                let error_msg = format!("{e:?}");
                {
                    let mut health = self.health.lock().await;
                    health.record_failure(&session.agent_name, &error_msg);

                    let fallback = health.resolve_agent(&session.agent_name).to_string();
                    if fallback != session.agent_name {
                        warn!(
                            primary = session.agent_name.as_str(),
                            fallback = fallback.as_str(),
                            "primary agent unhealthy, attempting fallback"
                        );
                    }
                }
                Err(SurgeError::Acp(format!("Prompt failed: {error_msg}")))
            }
        }
    }

    /// Gracefully shutdown all agents.
    ///
    /// Terminates all active agent connections and cleans up resources.
    pub async fn shutdown(&self) {
        info!("Shutting down agent pool");

        let mut connections = self.connections.write().await;
        for (name, mut conn) in connections.drain() {
            debug!("Shutting down agent '{}'", name);
            let _ = conn.kill();
        }

        info!("Agent pool shutdown complete");
    }

    /// Get the number of active connections.
    #[must_use]
    pub async fn active_connections(&self) -> usize {
        self.connections.read().await.len()
    }

    /// Check if a specific agent is connected.
    #[must_use]
    pub async fn is_connected(&self, name: &str) -> bool {
        self.connections.read().await.contains_key(name)
    }

    /// Get a reference to the health monitor.
    #[must_use]
    pub fn health(&self) -> &Arc<Mutex<HealthMonitor>> {
        &self.health
    }

    /// Configure a fallback agent for a primary agent.
    pub async fn set_fallback(&self, primary: &str, fallback: &str) {
        self.health.lock().await.set_fallback(primary, fallback);
    }
}

impl Drop for AgentPool {
    fn drop(&mut self) {
        // Note: Drop cannot be async, so we can't call shutdown() here.
        // Connections will be cleaned up by their own Drop implementations.
        debug!("AgentPool dropped");
    }
}

impl std::fmt::Debug for AgentPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentPool")
            .field("default_agent", &self.default_agent)
            .field("worktree_root", &self.worktree_root)
            .field("available_agents", &self.configs.keys().collect::<Vec<_>>())
            .field("permission_policy", &self.permission_policy)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use surge_core::config::Transport;

    fn test_agent_config() -> AgentConfig {
        AgentConfig {
            command: "echo".to_string(),
            args: vec!["test".to_string()],
            transport: Transport::Stdio,
        }
    }

    #[test]
    fn test_agent_pool_creation() {
        let mut configs = HashMap::new();
        configs.insert("test-agent".to_string(), test_agent_config());

        let pool = AgentPool::new(
            configs,
            "test-agent".to_string(),
            PathBuf::from("/tmp/test"),
            PermissionPolicy::default(),
        );

        assert!(pool.is_ok());
        let pool = pool.unwrap();
        assert_eq!(pool.default_agent(), "test-agent");
        assert_eq!(pool.available_agents(), vec!["test-agent"]);
    }

    #[test]
    fn test_agent_pool_invalid_default() {
        let configs = HashMap::new();

        let pool = AgentPool::new(
            configs,
            "nonexistent".to_string(),
            PathBuf::from("/tmp/test"),
            PermissionPolicy::default(),
        );

        assert!(pool.is_err());
        assert!(matches!(pool.unwrap_err(), SurgeError::Config(_)));
    }

    #[tokio::test]
    async fn test_session_handle() {
        let handle = SessionHandle {
            session_id: "test-session".to_string(),
            agent_name: "test-agent".to_string(),
        };

        assert_eq!(handle.session_id, "test-session");
        assert_eq!(handle.agent_name, "test-agent");
    }
}
