//! AgentPool — multi-agent management with resilience.
//!
//! All ACP operations run on a dedicated thread with a `LocalSet` to satisfy
//! the `!Send` requirement of `ClientSideConnection`. The pool API is fully
//! `Send`-safe and can be called from any tokio context.

use agent_client_protocol::{
    Agent, ContentBlock, NewSessionRequest, PromptRequest, PromptResponse, SetSessionModeRequest,
    SessionModeId,
};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use surge_core::config::{AgentConfig, ResilienceConfig};
use surge_core::SurgeError;
use tokio::sync::{oneshot, Mutex};
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

// ── Internal message types for the LocalSet worker ──────────────────

enum PoolOp {
    Connect {
        name: String,
        tx: oneshot::Sender<Result<(), SurgeError>>,
    },
    CreateSession {
        agent_name: String,
        mode: Option<String>,
        working_dir: PathBuf,
        tx: oneshot::Sender<Result<SessionHandle, SurgeError>>,
    },
    Prompt {
        session: SessionHandle,
        content: Vec<ContentBlock>,
        tx: oneshot::Sender<Result<PromptResponse, SurgeError>>,
    },
    Shutdown {
        tx: oneshot::Sender<()>,
    },
}

/// Pool for managing multiple agent connections.
///
/// Provides lazy initialization of agent connections, session management,
/// health-based fallback routing, and configurable timeouts.
///
/// Internally runs a dedicated thread with `tokio::task::LocalSet` for ACP
/// operations, so all public methods are `Send`-safe.
pub struct AgentPool {
    /// Channel to the LocalSet worker.
    op_tx: std::sync::mpsc::Sender<PoolOp>,

    /// Worker thread handle.
    _worker: Option<std::thread::JoinHandle<()>>,

    /// Default agent name.
    default_agent: String,

    /// Health monitor (Send-safe, shared).
    health: Arc<Mutex<HealthTracker>>,
}

impl AgentPool {
    /// Create a new agent pool.
    ///
    /// Spawns a dedicated worker thread with a `LocalSet` for ACP operations.
    ///
    /// # Errors
    ///
    /// Returns error if default agent is not found in configs.
    pub fn new(
        configs: HashMap<String, AgentConfig>,
        default_agent: String,
        worktree_root: PathBuf,
        permission_policy: PermissionPolicy,
        resilience: ResilienceConfig,
    ) -> Result<Self, SurgeError> {
        if !configs.contains_key(&default_agent) {
            return Err(SurgeError::Config(format!(
                "Default agent '{}' not found in agent configurations",
                default_agent
            )));
        }

        let mut health_tracker = HealthTracker::new();
        for name in configs.keys() {
            health_tracker.register(name);
        }
        let health = Arc::new(Mutex::new(health_tracker));

        let (op_tx, op_rx) = std::sync::mpsc::channel::<PoolOp>();

        let worker_health = Arc::clone(&health);
        let worker_resilience = resilience.clone();

        let worker = std::thread::Builder::new()
            .name("surge-acp-pool".into())
            .spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("failed to build ACP runtime");

                let local = tokio::task::LocalSet::new();
                local.block_on(&rt, async move {
                    let mut connections: HashMap<String, AgentConnection> = HashMap::new();
                    let mut spawning: HashSet<String> = HashSet::new();

                    loop {
                        // Yield to let ACP IO tasks run between operations
                        tokio::task::yield_now().await;

                        let op = match op_rx.try_recv() {
                            Ok(op) => op,
                            Err(std::sync::mpsc::TryRecvError::Empty) => {
                                tokio::time::sleep(Duration::from_millis(1)).await;
                                continue;
                            }
                            Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
                        };

                        match op {
                            PoolOp::Connect { name, tx } => {
                                let result = do_connect(
                                    &name,
                                    &configs,
                                    &mut connections,
                                    &mut spawning,
                                    &worktree_root,
                                    &permission_policy,
                                    &worker_resilience,
                                )
                                .await;
                                let _ = tx.send(result);
                            }
                            PoolOp::CreateSession {
                                agent_name,
                                mode,
                                working_dir,
                                tx,
                            } => {
                                let result = do_create_session(
                                    &agent_name,
                                    mode.as_deref(),
                                    &working_dir,
                                    &configs,
                                    &mut connections,
                                    &mut spawning,
                                    &worktree_root,
                                    &permission_policy,
                                    &worker_resilience,
                                )
                                .await;
                                let _ = tx.send(result);
                            }
                            PoolOp::Prompt {
                                session,
                                content,
                                tx,
                            } => {
                                let result = do_prompt(
                                    &session,
                                    content,
                                    &configs,
                                    &mut connections,
                                    &mut spawning,
                                    &worktree_root,
                                    &permission_policy,
                                    &worker_resilience,
                                    &worker_health,
                                )
                                .await;
                                let _ = tx.send(result);
                            }
                            PoolOp::Shutdown { tx } => {
                                let grace =
                                    Duration::from_secs(worker_resilience.shutdown_grace_secs);
                                for (name, mut conn) in connections.drain() {
                                    debug!(agent = name.as_str(), "shutting down agent");
                                    conn.wait_or_kill(grace).await;
                                }
                                let _ = tx.send(());
                                break;
                            }
                        }
                    }
                });
            })
            .map_err(|e| {
                SurgeError::AgentConnection(format!("Failed to spawn pool worker: {e}"))
            })?;

        Ok(Self {
            op_tx,
            _worker: Some(worker),
            default_agent,
            health,
        })
    }

    /// Get the default agent name.
    #[must_use]
    pub fn default_agent(&self) -> &str {
        &self.default_agent
    }

    /// Check if an agent is responsive by ensuring connection.
    ///
    /// # Errors
    ///
    /// Returns error if agent is not found or connection fails.
    pub async fn ping(&self, name: &str) -> Result<(), SurgeError> {
        self.get_or_connect(name).await
    }

    /// Get or create a connection to an agent.
    ///
    /// # Errors
    ///
    /// Returns error if agent is not found, spawn fails, or timeout.
    pub async fn get_or_connect(&self, name: &str) -> Result<(), SurgeError> {
        let (tx, rx) = oneshot::channel();
        self.op_tx
            .send(PoolOp::Connect {
                name: name.to_string(),
                tx,
            })
            .map_err(|_| SurgeError::AgentConnection("Pool worker stopped".to_string()))?;
        rx.await
            .map_err(|_| SurgeError::AgentConnection("Pool worker dropped response".to_string()))?
    }

    /// Create a new session with an agent.
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
        let (tx, rx) = oneshot::channel();
        self.op_tx
            .send(PoolOp::CreateSession {
                agent_name,
                mode: mode.map(String::from),
                working_dir: working_dir.to_path_buf(),
                tx,
            })
            .map_err(|_| SurgeError::AgentConnection("Pool worker stopped".to_string()))?;
        rx.await
            .map_err(|_| SurgeError::AgentConnection("Pool worker dropped response".to_string()))?
    }

    /// Send a prompt to an agent session.
    ///
    /// # Errors
    ///
    /// Returns error if all candidates and retries fail.
    pub async fn prompt(
        &self,
        session: &SessionHandle,
        content: Vec<ContentBlock>,
    ) -> Result<PromptResponse, SurgeError> {
        let (tx, rx) = oneshot::channel();
        self.op_tx
            .send(PoolOp::Prompt {
                session: session.clone(),
                content,
                tx,
            })
            .map_err(|_| SurgeError::AgentConnection("Pool worker stopped".to_string()))?;
        rx.await
            .map_err(|_| SurgeError::AgentConnection("Pool worker dropped response".to_string()))?
    }

    /// Gracefully shutdown all agents.
    pub async fn shutdown(&self) {
        info!("Shutting down agent pool");
        let (tx, rx) = oneshot::channel();
        if self.op_tx.send(PoolOp::Shutdown { tx }).is_ok() {
            let _ = rx.await;
        }
        info!("Agent pool shutdown complete");
    }

    /// Get a reference to the health tracker.
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
            .finish_non_exhaustive()
    }
}

// ── Worker-internal operations (run inside LocalSet) ────────────────

#[allow(clippy::too_many_arguments)]
async fn do_connect(
    name: &str,
    configs: &HashMap<String, AgentConfig>,
    connections: &mut HashMap<String, AgentConnection>,
    spawning: &mut HashSet<String>,
    worktree_root: &Path,
    permission_policy: &PermissionPolicy,
    resilience: &ResilienceConfig,
) -> Result<(), SurgeError> {
    if connections.contains_key(name) {
        return Ok(());
    }

    if spawning.contains(name) {
        return Err(SurgeError::AgentConnection(format!(
            "Agent '{name}' is already being spawned"
        )));
    }

    let config = configs
        .get(name)
        .ok_or_else(|| SurgeError::AgentNotFound(name.to_string()))?;

    info!("Connecting to agent '{}'", name);
    spawning.insert(name.to_string());

    let timeout = Duration::from_secs(resilience.connect_timeout_secs);
    let result = tokio::time::timeout(
        timeout,
        AgentConnection::spawn(
            name.to_string(),
            config,
            worktree_root.to_path_buf(),
            permission_policy.clone(),
        ),
    )
    .await
    .map_err(|_| SurgeError::Timeout(format!("connecting to agent '{name}'")))
    .and_then(|r| r);

    spawning.remove(name);

    let connection = result?;
    connections.insert(name.to_string(), connection);
    info!("Agent '{}' connected successfully", name);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn do_create_session(
    agent_name: &str,
    mode: Option<&str>,
    working_dir: &Path,
    configs: &HashMap<String, AgentConfig>,
    connections: &mut HashMap<String, AgentConnection>,
    spawning: &mut HashSet<String>,
    worktree_root: &Path,
    permission_policy: &PermissionPolicy,
    resilience: &ResilienceConfig,
) -> Result<SessionHandle, SurgeError> {
    do_connect(
        agent_name,
        configs,
        connections,
        spawning,
        worktree_root,
        permission_policy,
        resilience,
    )
    .await?;

    let session_timeout = Duration::from_secs(resilience.session_timeout_secs);

    let connection = connections.get(agent_name).ok_or_else(|| {
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
                SetSessionModeRequest::new(session_id.clone(), SessionModeId::new(m)),
            ),
        )
        .await
        {
            Ok(Ok(_)) => {}
            Ok(Err(e)) => warn!("Failed to set session mode '{}': {:?}", m, e),
            Err(_) => warn!("Timeout setting session mode '{}'", m),
        }
    }

    // Track session
    connections
        .get_mut(agent_name)
        .ok_or_else(|| SurgeError::AgentConnection("Connection disappeared".to_string()))?
        .add_session(session_id.clone(), working_dir.to_path_buf(), mode.map(String::from));

    Ok(SessionHandle {
        session_id,
        agent_name: agent_name.to_string(),
    })
}

#[allow(clippy::too_many_arguments)]
async fn do_prompt(
    session: &SessionHandle,
    content: Vec<ContentBlock>,
    configs: &HashMap<String, AgentConfig>,
    connections: &mut HashMap<String, AgentConnection>,
    spawning: &mut HashSet<String>,
    worktree_root: &Path,
    permission_policy: &PermissionPolicy,
    resilience: &ResilienceConfig,
    health: &Arc<Mutex<HealthTracker>>,
) -> Result<PromptResponse, SurgeError> {
    let prompt_timeout = Duration::from_secs(resilience.prompt_timeout_secs);
    let max_retries = resilience.prompt_retries;

    let resolved_agent = {
        let h = health.lock().await;
        h.resolve_agent(&session.agent_name).to_string()
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
            if let Err(e) = do_connect(
                agent_name,
                configs,
                connections,
                spawning,
                worktree_root,
                permission_policy,
                resilience,
            )
            .await
            {
                last_error = Some(e);
                continue;
            }

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
                    health.lock().await.record_success(agent_name, elapsed);
                    return Ok(response);
                }
                Ok(Err(e)) => {
                    let msg = format!("{e:?}");
                    health.lock().await.record_failure(agent_name, &msg);
                    last_error =
                        Some(SurgeError::Acp(format!("Prompt failed on '{agent_name}': {msg}")));
                    warn!(agent = agent_name.as_str(), attempt, "prompt failed");
                }
                Err(_) => {
                    let msg = format!("prompt timed out on '{agent_name}'");
                    health.lock().await.record_failure(agent_name, &msg);
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

#[cfg(test)]
mod tests {
    use super::*;
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
