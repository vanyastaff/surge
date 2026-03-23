//! AgentPool — multi-agent management with resilience.
//!
//! All ACP operations run on a dedicated thread with a `LocalSet` to satisfy
//! the `!Send` requirement of `ClientSideConnection`. The pool API is fully
//! `Send`-safe and can be called from any tokio context.
//!
//! Internally uses `tokio::sync::mpsc` for zero-polling async communication
//! and `spawn_local` per-operation for concurrent ACP I/O.

use agent_client_protocol::{
    Agent, ContentBlock, NewSessionRequest, PromptRequest, PromptResponse, SetSessionModeRequest,
    SessionModeId,
};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};
use surge_core::config::{AgentConfig, ResilienceConfig};
use surge_core::{SurgeError, SurgeEvent};
use tokio::sync::{broadcast, mpsc, oneshot, Mutex};
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

type Reply<T> = oneshot::Sender<Result<T, SurgeError>>;

enum PoolOp {
    Connect {
        name: String,
        tx: Reply<()>,
    },
    CreateSession {
        agent_name: String,
        mode: Option<String>,
        working_dir: PathBuf,
        tx: Reply<SessionHandle>,
    },
    Prompt {
        session: SessionHandle,
        content: Vec<ContentBlock>,
        tx: Reply<PromptResponse>,
    },
    Shutdown {
        tx: oneshot::Sender<()>,
    },
}

/// Shared mutable state inside the LocalSet worker.
struct WorkerState {
    connections: HashMap<String, AgentConnection>,
    spawning: HashSet<String>,
    configs: HashMap<String, AgentConfig>,
    worktree_root: PathBuf,
    permission_policy: PermissionPolicy,
    resilience: ResilienceConfig,
    health: Arc<Mutex<HealthTracker>>,
    event_tx: broadcast::Sender<SurgeEvent>,
}

/// Pool for managing multiple agent connections.
///
/// Provides lazy initialization of agent connections, session management,
/// health-based fallback routing, and configurable timeouts.
///
/// Internally runs a dedicated thread with `tokio::task::LocalSet` for ACP
/// operations, so all public methods are `Send`-safe.
pub struct AgentPool {
    /// Async channel to the LocalSet worker.
    op_tx: mpsc::UnboundedSender<PoolOp>,

    /// Worker thread handle.
    _worker: Option<std::thread::JoinHandle<()>>,

    /// Default agent name.
    default_agent: String,

    /// Health tracker (Send-safe, shared with worker).
    health: Arc<Mutex<HealthTracker>>,

    /// Event broadcast sender for subscribing to agent events.
    event_tx: broadcast::Sender<SurgeEvent>,
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
        let (event_tx, _) = broadcast::channel::<SurgeEvent>(256);

        let (op_tx, op_rx) = mpsc::unbounded_channel::<PoolOp>();

        let worker_health = Arc::clone(&health);
        let worker_event_tx = event_tx.clone();

        let worker = std::thread::Builder::new()
            .name("surge-acp-pool".into())
            .spawn(move || {
                run_worker(
                    op_rx,
                    configs,
                    worktree_root,
                    permission_policy,
                    resilience,
                    worker_health,
                    worker_event_tx,
                );
            })
            .map_err(|e| {
                SurgeError::AgentConnection(format!("Failed to spawn pool worker: {e}"))
            })?;

        Ok(Self {
            op_tx,
            _worker: Some(worker),
            default_agent,
            health,
            event_tx,
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
        self.send(PoolOp::Connect {
            name: name.to_string(),
            tx,
        })?;
        Self::recv(rx).await
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
        self.send(PoolOp::CreateSession {
            agent_name,
            mode: mode.map(String::from),
            working_dir: working_dir.to_path_buf(),
            tx,
        })?;
        Self::recv(rx).await
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
        self.send(PoolOp::Prompt {
            session: session.clone(),
            content,
            tx,
        })?;
        Self::recv(rx).await
    }

    /// Gracefully shutdown all agents.
    pub async fn shutdown(&self) {
        info!("Shutting down agent pool");
        let (tx, rx) = oneshot::channel();
        if self.send(PoolOp::Shutdown { tx }).is_ok() {
            let _ = rx.await;
        }
        info!("Agent pool shutdown complete");
    }

    /// Get a reference to the health tracker.
    #[must_use]
    pub fn health(&self) -> &Arc<Mutex<HealthTracker>> {
        &self.health
    }

    /// Subscribe to agent events (message chunks, file ops, terminal events, etc.).
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<SurgeEvent> {
        self.event_tx.subscribe()
    }

    /// Configure a fallback agent for a primary agent.
    pub async fn set_fallback(&self, primary: &str, fallback: &str) {
        self.health.lock().await.set_fallback(primary, fallback);
    }

    /// Pre-connect to the default agent in the background.
    ///
    /// Call immediately after pool creation to absorb agent startup latency
    /// before the first [`create_session`][Self::create_session] call.
    ///
    /// Fire-and-forget — connection errors are silently ignored (the pool will
    /// reconnect on the next real operation).
    pub fn warm_up(&self) {
        // Drop the receiver — the worker will try to respond but that's fine.
        let (tx, _rx) = tokio::sync::oneshot::channel::<Result<(), SurgeError>>();
        let _ = self.op_tx.send(PoolOp::Connect {
            name: self.default_agent.clone(),
            tx,
        });
    }

    // ── Private helpers ─────────────────────────────────────────────

    fn send(&self, op: PoolOp) -> Result<(), SurgeError> {
        self.op_tx
            .send(op)
            .map_err(|_| SurgeError::AgentConnection("Pool worker stopped".to_string()))
    }

    async fn recv<T>(rx: oneshot::Receiver<Result<T, SurgeError>>) -> Result<T, SurgeError> {
        rx.await
            .map_err(|_| SurgeError::AgentConnection("Pool worker dropped response".to_string()))?
    }
}

impl Drop for AgentPool {
    fn drop(&mut self) {
        debug!("AgentPool dropped");
        if let Some(worker) = self._worker.take() {
            // Signal the worker to shut down, then block until it exits so that
            // agent processes are reaped before the pool disappears.
            let (tx, _) = tokio::sync::oneshot::channel::<()>();
            let _ = self.op_tx.send(PoolOp::Shutdown { tx });
            let _ = worker.join();
        }
    }
}

impl std::fmt::Debug for AgentPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentPool")
            .field("default_agent", &self.default_agent)
            .finish_non_exhaustive()
    }
}

// ── Worker thread ───────────────────────────────────────────────────

fn run_worker(
    mut op_rx: mpsc::UnboundedReceiver<PoolOp>,
    configs: HashMap<String, AgentConfig>,
    worktree_root: PathBuf,
    permission_policy: PermissionPolicy,
    resilience: ResilienceConfig,
    health: Arc<Mutex<HealthTracker>>,
    event_tx: broadcast::Sender<SurgeEvent>,
) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("failed to build ACP pool runtime");

    let local = tokio::task::LocalSet::new();
    local.block_on(&rt, async move {
        let state = Rc::new(RefCell::new(WorkerState {
            connections: HashMap::new(),
            spawning: HashSet::new(),
            configs,
            worktree_root,
            permission_policy,
            resilience,
            health,
            event_tx,
        }));

        while let Some(op) = op_rx.recv().await {
            match op {
                PoolOp::Shutdown { tx } => {
                    let grace = Duration::from_secs(
                        state.borrow().resilience.shutdown_grace_secs,
                    );
                    let conns: Vec<_> = state.borrow_mut().connections.drain().collect();
                    for (name, mut conn) in conns {
                        debug!(agent = name.as_str(), "shutting down agent");
                        conn.wait_or_kill(grace).await;
                    }
                    let _ = tx.send(());
                    break;
                }

                PoolOp::Connect { name, tx } => {
                    let st = Rc::clone(&state);
                    tokio::task::spawn_local(async move {
                        let result = connect(&st, &name).await;
                        let _ = tx.send(result);
                    });
                }

                PoolOp::CreateSession {
                    agent_name,
                    mode,
                    working_dir,
                    tx,
                } => {
                    let st = Rc::clone(&state);
                    tokio::task::spawn_local(async move {
                        let result =
                            create_session(&st, &agent_name, mode.as_deref(), &working_dir).await;
                        let _ = tx.send(result);
                    });
                }

                PoolOp::Prompt {
                    session,
                    content,
                    tx,
                } => {
                    let st = Rc::clone(&state);
                    tokio::task::spawn_local(async move {
                        let result = prompt(&st, &session, content).await;
                        let _ = tx.send(result);
                    });
                }
            }
        }
    });
}

// ── Worker operations (run inside LocalSet via spawn_local) ─────────

async fn connect(state: &Rc<RefCell<WorkerState>>, name: &str) -> Result<(), SurgeError> {
    // Fast path
    if state.borrow().connections.contains_key(name) {
        return Ok(());
    }

    // Atomically check-and-mark as spawning in a single borrow_mut
    {
        let mut s = state.borrow_mut();
        if s.spawning.contains(name) {
            return Err(SurgeError::AgentConnection(format!(
                "Agent '{name}' is already being spawned"
            )));
        }
        s.spawning.insert(name.to_string());
    }

    let (config, worktree_root, permission_policy, timeout) = {
        let s = state.borrow();
        let config = s
            .configs
            .get(name)
            .ok_or_else(|| SurgeError::AgentNotFound(name.to_string()))?
            .clone();
        (
            config,
            s.worktree_root.clone(),
            s.permission_policy.clone(),
            Duration::from_secs(s.resilience.connect_timeout_secs),
        )
    };

    info!("Connecting to agent '{}'", name);

    let event_tx = state.borrow().event_tx.clone();

    let result = tokio::time::timeout(
        timeout,
        AgentConnection::spawn(
            name.to_string(),
            &config,
            worktree_root,
            permission_policy,
            Some(event_tx),
        ),
    )
    .await
    .map_err(|_| SurgeError::Timeout(format!("connecting to agent '{name}'")))
    .and_then(|r| r);

    state.borrow_mut().spawning.remove(name);

    let connection = result?;
    state
        .borrow_mut()
        .connections
        .insert(name.to_string(), connection);
    info!("Agent '{}' connected successfully", name);
    Ok(())
}

async fn create_session(
    state: &Rc<RefCell<WorkerState>>,
    agent_name: &str,
    mode: Option<&str>,
    working_dir: &Path,
) -> Result<SessionHandle, SurgeError> {
    connect(state, agent_name).await?;

    let session_timeout = Duration::from_secs(state.borrow().resilience.session_timeout_secs);

    debug!(
        "Creating session with agent '{}' in {}",
        agent_name,
        working_dir.display()
    );

    let request = NewSessionRequest::new(working_dir);

    // Temporarily remove the connection from the map so no RefCell borrow
    // is held across the async ACP calls below.
    let mut conn = {
        let mut s = state.borrow_mut();
        s.connections
            .remove(agent_name)
            .ok_or_else(|| SurgeError::AgentConnection("Connection disappeared".to_string()))?
    };

    let session_id_result: Result<String, SurgeError> = async {
        let response =
            tokio::time::timeout(session_timeout, conn.connection().new_session(request))
                .await
                .map_err(|_| SurgeError::Timeout("new_session".to_string()))?
                .map_err(|e| SurgeError::Acp(format!("Failed to create session: {:?}", e)))?;

        let session_id = response.session_id.to_string();

        if let Some(m) = mode {
            debug!("Setting session mode to '{}'", m);
            match tokio::time::timeout(
                session_timeout,
                conn.connection().set_session_mode(
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

        Ok(session_id)
    }
    .await;

    // Track session on success, then restore the connection regardless of outcome.
    if let Ok(ref session_id) = session_id_result {
        conn.add_session(session_id.clone(), working_dir.to_path_buf(), mode.map(String::from));
    }
    state
        .borrow_mut()
        .connections
        .insert(agent_name.to_string(), conn);

    Ok(SessionHandle {
        session_id: session_id_result?,
        agent_name: agent_name.to_string(),
    })
}

async fn prompt(
    state: &Rc<RefCell<WorkerState>>,
    session: &SessionHandle,
    content: Vec<ContentBlock>,
) -> Result<PromptResponse, SurgeError> {
    let (prompt_timeout, max_retries) = {
        let s = state.borrow();
        (
            Duration::from_secs(s.resilience.prompt_timeout_secs),
            s.resilience.prompt_retries,
        )
    };

    // Resolve fallback — clone Arc before awaiting the lock so no RefCell
    // borrow is held across the await.
    let resolved_agent = {
        let health = state.borrow().health.clone();
        health.lock().await.resolve_agent(&session.agent_name).to_string()
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
            if let Err(e) = connect(state, agent_name).await {
                last_error = Some(e);
                continue;
            }

            let request = PromptRequest::new(session.session_id.clone(), content.clone());

            let start = Instant::now();

            // Temporarily remove connection to avoid holding a RefCell borrow
            // across the prompt await.
            let conn = match state.borrow_mut().connections.remove(agent_name) {
                Some(c) => c,
                None => continue,
            };

            let result =
                tokio::time::timeout(prompt_timeout, conn.connection().prompt(request)).await;

            // Restore connection before processing result.
            state
                .borrow_mut()
                .connections
                .insert(agent_name.to_string(), conn);

            let health = state.borrow().health.clone();

            match result {
                Ok(Ok(response)) => {
                    health.lock().await.record_success(agent_name, start.elapsed());

                    // Emit token usage if the agent reported it.
                    if let Some(usage) = &response.usage {
                        let event_tx = state.borrow().event_tx.clone();
                        let _ = event_tx.send(SurgeEvent::TokensConsumed {
                            session_id: session.session_id.clone(),
                            agent_name: agent_name.clone(),
                            input_tokens: usage.input_tokens,
                            output_tokens: usage.output_tokens,
                            thought_tokens: usage.thought_tokens,
                            cached_read_tokens: usage.cached_read_tokens,
                            cached_write_tokens: usage.cached_write_tokens,
                            estimated_cost_usd: None,
                        });
                    }

                    return Ok(response);
                }
                Ok(Err(e)) => {
                    let msg = format!("{e:?}");
                    health.lock().await.record_failure(agent_name, &msg);
                    last_error = Some(SurgeError::Acp(format!(
                        "Prompt failed on '{agent_name}': {msg}"
                    )));
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

    Err(last_error
        .unwrap_or_else(|| SurgeError::Acp("All prompt candidates failed".to_string())))
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

    /// `warm_up` must not panic or block — it fires and forgets.
    #[test]
    fn test_warm_up_does_not_block() {
        let mut configs = HashMap::new();
        configs.insert("test-agent".to_string(), test_agent_config());

        let pool = AgentPool::new(
            configs,
            "test-agent".to_string(),
            PathBuf::from("/tmp/test"),
            PermissionPolicy::default(),
            ResilienceConfig::default(),
        )
        .unwrap();

        // warm_up is synchronous and non-blocking — it just enqueues a connect op.
        pool.warm_up();
        // The agent won't actually connect (echo isn't an ACP agent), but the
        // call must return immediately without panicking.
    }

    /// Dropping an AgentPool must join the worker thread, not detach it.
    ///
    /// If `Drop` hangs, the test runner will time out. If it panics, the test
    /// fails. Either way we catch regressions in the shutdown path.
    #[test]
    fn test_pool_drop_joins_worker() {
        let mut configs = HashMap::new();
        configs.insert("test-agent".to_string(), test_agent_config());

        let pool = AgentPool::new(
            configs,
            "test-agent".to_string(),
            PathBuf::from("/tmp/test"),
            PermissionPolicy::default(),
            ResilienceConfig::default(),
        )
        .unwrap();

        // No connections were established, so Shutdown drains nothing.
        // The worker should exit and join almost immediately.
        drop(pool);
    }
}
