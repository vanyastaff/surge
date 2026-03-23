//! AgentPool — multi-agent management with resilience.
//!
//! This module provides a pool for managing multiple agent connections,
//! handling lazy initialization with spawn-gating, session creation,
//! health-based fallback routing, and configurable timeouts.

use agent_client_protocol::{
    Agent, ContentBlock, NewSessionRequest, PromptRequest, PromptResponse, SetSessionModeRequest,
    SessionModeId,
};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};
use surge_core::config::{AgentConfig, ResilienceConfig};
use surge_core::SurgeError;
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, info, warn};

use crate::client::PermissionPolicy;
use crate::connection::AgentConnection;
use crate::health::HealthTracker;

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
/// health-based fallback routing, and configurable timeouts.
pub struct AgentPool {
    /// Configuration for available agents.
    configs: HashMap<String, AgentConfig>,

    /// Active agent connections (lazily initialized).
    connections: Arc<RwLock<HashMap<String, AgentConnection>>>,

    /// Guards against concurrent spawn of the same agent name.
    spawning: Arc<Mutex<HashSet<String>>>,

    /// Default agent name for tasks without explicit agent specification.
    default_agent: String,

    /// Root directory for worktree operations.
    worktree_root: std::path::PathBuf,

    /// Default permission policy for agent connections.
    permission_policy: PermissionPolicy,

    /// Health monitor for tracking agent reliability and fallback routing.
    health: Arc<Mutex<HealthTracker>>,

    /// Resilience configuration (timeouts, retries, shutdown grace).
    resilience: ResilienceConfig,
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
    /// * `resilience` - Resilience configuration for timeouts and retries
    ///
    /// # Errors
    ///
    /// Returns error if default agent is not found in configs.
    pub fn new(
        configs: HashMap<String, AgentConfig>,
        default_agent: String,
        worktree_root: std::path::PathBuf,
        permission_policy: PermissionPolicy,
        resilience: ResilienceConfig,
    ) -> Result<Self, SurgeError> {
        if !configs.contains_key(&default_agent) {
            return Err(SurgeError::Config(format!(
                "Default agent '{}' not found in agent configurations",
                default_agent
            )));
        }

        let mut health_monitor = HealthTracker::new();
        for name in configs.keys() {
            health_monitor.register(name);
        }

        Ok(Self {
            configs,
            connections: Arc::new(RwLock::new(HashMap::new())),
            spawning: Arc::new(Mutex::new(HashSet::new())),
            default_agent,
            worktree_root,
            permission_policy,
            health: Arc::new(Mutex::new(health_monitor)),
            resilience,
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
    /// Uses a spawn-gate to prevent TOCTOU races: only one caller can
    /// spawn a given agent name at a time.
    ///
    /// # Errors
    ///
    /// Returns error if agent is not found, spawn fails, or connection times out.
    pub async fn get_or_connect(&self, name: &str) -> Result<(), SurgeError> {
        // Fast path: already connected
        if self.connections.read().await.contains_key(name) {
            return Ok(());
        }

        // Acquire spawn-gate
        let mut spawning = self.spawning.lock().await;
        if spawning.contains(name) {
            // Another task is spawning this agent — wait for it with bounded polling
            drop(spawning);
            let max_wait = Duration::from_secs(self.resilience.connect_timeout_secs);
            let deadline = Instant::now() + max_wait;
            loop {
                tokio::time::sleep(Duration::from_millis(50)).await;
                if self.connections.read().await.contains_key(name) {
                    return Ok(());
                }
                if Instant::now() >= deadline {
                    return Err(SurgeError::Timeout(format!(
                        "waiting for another task to connect agent '{name}'"
                    )));
                }
                let s = self.spawning.lock().await;
                if !s.contains(name) {
                    break;
                }
            }
            if self.connections.read().await.contains_key(name) {
                return Ok(());
            }
            spawning = self.spawning.lock().await;
        }

        // Double-check after acquiring gate
        if self.connections.read().await.contains_key(name) {
            return Ok(());
        }

        spawning.insert(name.to_string());
        drop(spawning);

        info!("Connecting to agent '{}'", name);

        let config = self
            .configs
            .get(name)
            .ok_or_else(|| SurgeError::AgentNotFound(name.to_string()))?;

        let timeout = Duration::from_secs(self.resilience.connect_timeout_secs);
        let spawn_result = tokio::time::timeout(
            timeout,
            AgentConnection::spawn(
                name.to_string(),
                config,
                self.worktree_root.clone(),
                self.permission_policy.clone(),
            ),
        )
        .await
        .map_err(|_| SurgeError::Timeout(format!("connecting to agent '{name}'")))
        .and_then(|r| r);

        // Always remove from spawning set
        self.spawning.lock().await.remove(name);

        let connection = spawn_result?;
        self.connections
            .write()
            .await
            .insert(name.to_string(), connection);

        info!("Agent '{}' connected successfully", name);
        Ok(())
    }

    /// Check if an agent is responsive by ensuring connection.
    ///
    /// # Errors
    ///
    /// Returns error if agent is not found or connection fails.
    pub async fn ping(&self, name: &str) -> Result<(), SurgeError> {
        self.get_or_connect(name).await
    }

    /// Create a new session with an agent.
    ///
    /// Uses read-lock for ACP calls, write-lock only for session tracking insertion.
    ///
    /// # Errors
    ///
    /// Returns error if connection, session creation, or timeout fails.
    pub async fn create_session(
        &self,
        agent_name: Option<&str>,
        mode: Option<&str>,
        working_dir: &Path,
    ) -> Result<SessionHandle, SurgeError> {
        let agent_name = agent_name.unwrap_or(&self.default_agent).to_string();
        self.get_or_connect(&agent_name).await?;

        let session_timeout = Duration::from_secs(self.resilience.session_timeout_secs);

        // ACP calls under read-lock — concurrent sessions allowed
        let (session_id, mode_string) = {
            let connections = self.connections.read().await;
            let connection = connections.get(&agent_name).ok_or_else(|| {
                SurgeError::AgentConnection("Connection disappeared".to_string())
            })?;

            debug!(
                "Creating session with agent '{}' in {}",
                agent_name,
                working_dir.display()
            );

            let request = NewSessionRequest::new(working_dir);

            let response = tokio::time::timeout(
                session_timeout,
                connection.connection().new_session(request),
            )
            .await
            .map_err(|_| SurgeError::Timeout("new_session".to_string()))?
            .map_err(|e| SurgeError::Acp(format!("Failed to create session: {:?}", e)))?;

            let session_id = response.session_id.to_string();

            if let Some(m) = mode {
                debug!("Setting session mode to '{}'", m);
                match tokio::time::timeout(
                    session_timeout,
                    connection.connection().set_session_mode(
                        SetSessionModeRequest::new(
                            session_id.clone(),
                            SessionModeId::new(m),
                        ),
                    ),
                )
                .await
                {
                    Ok(Ok(_)) => {}
                    Ok(Err(e)) => warn!("Failed to set session mode '{}': {:?}", m, e),
                    Err(_) => warn!("Timeout setting session mode '{}'", m),
                }
            }

            (session_id, mode.map(String::from))
        }; // read-lock dropped

        // Write-lock only for cheap in-memory insertion
        self.connections
            .write()
            .await
            .get_mut(&agent_name)
            .ok_or_else(|| SurgeError::AgentConnection("Connection disappeared".to_string()))?
            .add_session(session_id.clone(), working_dir.to_path_buf(), mode_string);

        Ok(SessionHandle {
            session_id,
            agent_name,
        })
    }

    /// Send a prompt to an agent session.
    ///
    /// Attempts health-based fallback routing and retries with exponential backoff.
    ///
    /// # Errors
    ///
    /// Returns error if all candidates and retries fail.
    pub async fn prompt(
        &self,
        session: &SessionHandle,
        content: Vec<ContentBlock>,
    ) -> Result<PromptResponse, SurgeError> {
        let prompt_timeout = Duration::from_secs(self.resilience.prompt_timeout_secs);
        let max_retries = self.resilience.prompt_retries;

        // Resolve which agent to use (may be fallback)
        let resolved_agent = {
            let health = self.health.lock().await;
            health.resolve_agent(&session.agent_name).to_string()
        };

        let candidates: Vec<String> = if resolved_agent != session.agent_name {
            warn!(
                primary = session.agent_name.as_str(),
                fallback = resolved_agent.as_str(),
                "primary agent unhealthy, trying fallback first"
            );
            vec![resolved_agent, session.agent_name.clone()]
        } else {
            vec![session.agent_name.clone()]
        };

        let mut last_error: Option<SurgeError> = None;

        for attempt in 0..=max_retries {
            for agent_name in &candidates {
                if let Err(e) = self.get_or_connect(agent_name).await {
                    last_error = Some(e);
                    continue;
                }

                let connections = self.connections.read().await;
                let Some(connection) = connections.get(agent_name) else {
                    continue;
                };

                let request = PromptRequest::new(
                    session.session_id.clone(),
                    content.clone(),
                );

                let start = Instant::now();
                let result = tokio::time::timeout(
                    prompt_timeout,
                    connection.connection().prompt(request),
                )
                .await;

                match result {
                    Ok(Ok(response)) => {
                        let elapsed = start.elapsed();
                        drop(connections);
                        self.health.lock().await.record_success(agent_name, elapsed);
                        return Ok(response);
                    }
                    Ok(Err(e)) => {
                        let msg = format!("{e:?}");
                        drop(connections);
                        self.health.lock().await.record_failure(agent_name, &msg);
                        last_error =
                            Some(SurgeError::Acp(format!("Prompt failed on '{agent_name}': {msg}")));
                        warn!(agent = agent_name.as_str(), attempt, "prompt failed");
                    }
                    Err(_) => {
                        drop(connections);
                        let msg = format!("prompt timed out on '{agent_name}'");
                        self.health.lock().await.record_failure(agent_name, &msg);
                        last_error = Some(SurgeError::Timeout(msg));
                        warn!(agent = agent_name.as_str(), attempt, "prompt timed out");
                    }
                }
            }

            if attempt < max_retries {
                tokio::time::sleep(Duration::from_millis(500 * 2u64.pow(attempt))).await;
            }
        }

        Err(last_error.unwrap_or_else(|| SurgeError::Acp("All prompt candidates failed".to_string())))
    }

    /// Gracefully shutdown all agents.
    ///
    /// Waits up to `shutdown_grace_secs` for each process to exit, then kills.
    pub async fn shutdown(&self) {
        info!("Shutting down agent pool");
        let grace = Duration::from_secs(self.resilience.shutdown_grace_secs);

        let mut connections = self.connections.write().await;
        for (name, mut conn) in connections.drain() {
            debug!(agent = name.as_str(), "waiting for agent to exit");
            conn.wait_or_kill(grace).await;
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
    pub fn health(&self) -> &Arc<Mutex<HealthTracker>> {
        &self.health
    }

    /// Configure a fallback agent for a primary agent.
    pub async fn set_fallback(&self, primary: &str, fallback: &str) {
        self.health.lock().await.set_fallback(primary, fallback);
    }
}

impl Drop for AgentPool {
    fn drop(&mut self) {
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
            ResilienceConfig::default(),
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
            ResilienceConfig::default(),
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
