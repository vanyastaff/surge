use std::path::PathBuf;
use std::sync::Arc;

use gpui::EventEmitter;
use surge_acp::{
    AgentHealth, AgentPool, DetectedAgent, HealthTracker, PermissionPolicy, Registry, RegistryEntry,
};
use surge_core::id::RunId;
use surge_core::{Spec, SpecId, SurgeConfig, SurgeEvent, TaskId, TaskState};
use surge_orchestrator::engine::handle::{RunStatus, RunSummary};
use surge_orchestrator::engine::ipc::GlobalDaemonEvent;

use crate::daemon_link::ConnectionState;

/// Central application state shared across all UI screens.
///
/// Screens hold `Entity<AppState>` and read data from it.
/// When data changes, `cx.notify()` triggers UI re-render.
pub struct AppState {
    // ── Project ──
    pub project_path: Option<PathBuf>,
    pub project_name: String,
    pub config: Option<SurgeConfig>,
    pub current_branch: String,

    // ── Agents ──
    /// Full registry catalog (builtin agents).
    pub registry: Registry,
    /// Agents detected on system PATH.
    pub installed_agents: Vec<DetectedAgent>,
    /// Health metrics per agent.
    pub health: HealthTracker,

    // ── Tasks ──
    pub tasks: Vec<TaskEntry>,

    // ── Specs ──
    pub specs: Vec<Spec>,

    // ── Worktrees ──
    pub worktrees: Vec<WorktreeEntry>,

    // ── ACP ──
    /// Agent pool for ACP connections (created when project has agents configured).
    pub agent_pool: Option<Arc<AgentPool>>,

    // ── Daemon (runtime UI client) ──
    /// Connection state with `surge-daemon`. The runtime UI is a daemon
    /// client: it lists / watches runs that the daemon hosts rather than
    /// running them in-process. See `crate::daemon_link::try_connect`.
    pub daemon_state: ConnectionState,
    /// Run summaries last fetched / observed from the daemon. Updated by
    /// the connect task on `ListRuns` and incrementally by the global-event
    /// subscription as `RunAccepted` / `RunFinished` arrive.
    ///
    /// We store our own `UiRun` rather than `RunSummary` directly because
    /// the latter is `#[non_exhaustive]` (engine can grow new fields) and
    /// would prevent us from synthesising a stub when a `RunAccepted` for
    /// an unknown id arrives ahead of a `ListRuns` refresh.
    pub runs: Vec<UiRun>,

    // ── Events ──
    pub _event_tx: tokio::sync::broadcast::Sender<SurgeEvent>,
    pub recent_events: Vec<SurgeEvent>,
}

/// A task tracked in the UI (in-memory, SQLite later).
#[derive(Debug, Clone)]
pub struct TaskEntry {
    pub id: TaskId,
    pub _spec_id: SpecId,
    pub title: String,
    pub description: String,
    pub state: TaskState,
    pub agent: Option<String>,
    pub complexity: String,
    pub _created_at: String,
    pub updated_at: String,
}

/// A git worktree entry for display.
#[derive(Debug, Clone)]
pub struct WorktreeEntry {
    pub spec_id: String,
    pub branch: String,
    pub path: PathBuf,
    pub exists: bool,
}

/// UI-side projection of a daemon-hosted run. Mirrors the orchestrator's
/// `RunSummary` (which is `#[non_exhaustive]` and therefore not
/// constructible from outside) but with only the fields the runtime UI
/// reads today. Add fields here as the UI starts surfacing more state.
#[derive(Debug, Clone)]
pub struct UiRun {
    pub run_id: RunId,
    pub status: RunStatus,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub last_event_seq: Option<u64>,
}

impl From<&RunSummary> for UiRun {
    fn from(s: &RunSummary) -> Self {
        Self {
            run_id: s.run_id,
            status: s.status,
            started_at: s.started_at,
            last_event_seq: s.last_event_seq,
        }
    }
}

impl AppState {
    /// Create initial state with empty data and real registry.
    pub fn new() -> Self {
        let (event_tx, _) = tokio::sync::broadcast::channel(256);
        let registry = Registry::builtin();
        let installed_agents = registry.detect_installed_with_paths();

        // Register installed agents in health monitor.
        let mut health = HealthTracker::new();
        for agent in &installed_agents {
            health.register(&agent.entry.id);
        }

        Self {
            project_path: None,
            project_name: String::new(),
            config: None,
            current_branch: "main".to_string(),
            registry,
            installed_agents,
            health,
            tasks: Vec::new(),
            specs: Vec::new(),
            worktrees: Vec::new(),
            agent_pool: None,
            daemon_state: ConnectionState::default(),
            runs: Vec::new(),
            _event_tx: event_tx,
            recent_events: Vec::new(),
        }
    }

    /// Apply a `GlobalDaemonEvent` to the run list.
    ///
    /// `RunAccepted` inserts a fresh `RunSummary` if the id isn't
    /// already known (or marks an existing one as `Active`).
    /// `RunFinished` flips an existing run's status to a terminal
    /// variant matching the outcome — keeps the entry around so the
    /// UI can show "recently finished" runs without re-querying
    /// `ListRuns`.
    ///
    /// Future variants are tolerated by ignoring (the wire enum is
    /// `#[non_exhaustive]`).
    pub fn apply_global_event(&mut self, event: &GlobalDaemonEvent) {
        use surge_orchestrator::engine::handle::RunOutcome;

        match event {
            GlobalDaemonEvent::RunAccepted { run_id } => {
                if let Some(existing) = self.runs.iter_mut().find(|r| &r.run_id == run_id) {
                    existing.status = RunStatus::Active;
                } else {
                    self.runs.push(UiRun {
                        run_id: *run_id,
                        status: RunStatus::Active,
                        started_at: chrono::Utc::now(),
                        last_event_seq: None,
                    });
                }
            },
            GlobalDaemonEvent::RunFinished { run_id, outcome } => {
                // `RunOutcome` is `#[non_exhaustive]`. For known variants
                // we map to the matching `RunStatus`. For an unknown
                // future variant we deliberately do NOT collapse to
                // `Aborted` — that would misrepresent e.g. a future
                // `TimedOut` outcome as user-cancelled. Instead, leave
                // the existing status untouched (typical case: it was
                // `Active`, so it stays `Active`; the UI will surface
                // the staleness when it next refreshes via `ListRuns`)
                // and emit a tracing::debug so the gap is visible.
                let new_status = match outcome {
                    RunOutcome::Completed { .. } => Some(RunStatus::Completed),
                    RunOutcome::Failed { .. } => Some(RunStatus::Failed),
                    RunOutcome::Aborted { .. } => Some(RunStatus::Aborted),
                    _ => {
                        tracing::debug!(
                            run_id = %run_id,
                            "RunFinished with unknown RunOutcome variant; keeping current status"
                        );
                        None
                    },
                };
                if let Some(existing) = self.runs.iter_mut().find(|r| &r.run_id == run_id) {
                    if let Some(status) = new_status {
                        existing.status = status;
                    }
                } else {
                    // Unknown run id AND unknown outcome — synthesize a
                    // stub with our best guess (Aborted is least
                    // misleading for "we don't know what happened, but
                    // we know it ended"). Real `started_at` is
                    // unrecoverable here.
                    let stub_status = new_status.unwrap_or(RunStatus::Aborted);
                    self.runs.push(UiRun {
                        run_id: *run_id,
                        status: stub_status,
                        started_at: chrono::Utc::now(),
                        last_event_seq: None,
                    });
                }
            },
            _ => {
                // `GlobalDaemonEvent` is `#[non_exhaustive]`. Future
                // variants are silently dropped here; add cases as
                // they're introduced.
            },
        }
    }

    /// Replace the run list from a fresh `ListRuns` response.
    pub fn set_runs_from_summaries(&mut self, summaries: &[RunSummary]) {
        self.runs = summaries.iter().map(UiRun::from).collect();
    }

    /// Load project from a directory path.
    pub fn load_project(&mut self, path: &std::path::Path) {
        self.project_path = Some(path.to_path_buf());
        self.project_name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "project".to_string());

        // Try loading surge.toml config and create AgentPool.
        let config_path = path.join("surge.toml");
        if config_path.exists() {
            if let Ok(config) = SurgeConfig::load(&config_path) {
                // Create AgentPool from config if agents are configured.
                if !config.agents.is_empty() {
                    if let Ok(pool) = AgentPool::new(
                        config.agents.clone(),
                        config.default_agent.clone(),
                        path.to_path_buf(),
                        PermissionPolicy::default(),
                        config.resilience.clone(),
                    ) {
                        self.agent_pool = Some(Arc::new(pool));
                    }
                }
                self.config = Some(config);
            }
        }

        // Re-detect installed agents (might have changed).
        self.installed_agents = self.registry.detect_installed_with_paths();

        // If no pool yet (no surge.toml), create one from installed agents.
        if self.agent_pool.is_none() && !self.installed_agents.is_empty() {
            let mut agents = std::collections::HashMap::new();
            let mut default_agent = String::new();
            for detected in &self.installed_agents {
                let config = detected.entry.to_agent_config();
                if default_agent.is_empty() {
                    default_agent = detected.entry.id.clone();
                }
                agents.insert(detected.entry.id.clone(), config);
            }
            if let Ok(pool) = AgentPool::new(
                agents,
                default_agent,
                path.to_path_buf(),
                PermissionPolicy::default(),
                surge_core::config::ResilienceConfig::default(),
            ) {
                self.agent_pool = Some(Arc::new(pool));
            }
        }

        // Try detecting current git branch.
        self.current_branch = detect_branch(path).unwrap_or_else(|| "main".to_string());
    }

    /// Update config in-place and save to disk.
    ///
    /// Validates + persists BEFORE replacing the in-memory config so a
    /// validation or IO failure leaves the UI holding the previous,
    /// known-good state instead of an invalid one that was never
    /// written. Requires `project_path` to be set — otherwise there is
    /// no place to save and silently mutating in-memory only would
    /// diverge from disk.
    pub fn update_config(&mut self, config: SurgeConfig) -> Result<(), surge_core::SurgeError> {
        let project_path = self.project_path.as_ref().ok_or_else(|| {
            surge_core::SurgeError::Config(
                "No project loaded; cannot save config without project_path".into(),
            )
        })?;
        config.save(&project_path.join("surge.toml"))?;
        self.config = Some(config);
        Ok(())
    }

    /// Handle a SurgeEvent — update state and emit for UI subscribers.
    pub fn handle_event(&mut self, event: SurgeEvent, cx: &mut gpui::Context<Self>) {
        // Keep last 100 events for recent activity.
        self.recent_events.push(event.clone());
        if self.recent_events.len() > 100 {
            self.recent_events.remove(0);
        }

        match &event {
            SurgeEvent::TaskStateChanged {
                task_id, new_state, ..
            } => {
                if let Some(task) = self.tasks.iter_mut().find(|t| &t.id == task_id) {
                    task.state = new_state.clone();
                }
            },
            SurgeEvent::AgentConnected { agent_name } => {
                self.health.register(agent_name);
            },
            _ => {},
        }

        cx.emit(event);
    }

    // ── Computed accessors ──

    /// Agents that are installed (for Configured tab).
    pub fn configured_agents(&self) -> &[DetectedAgent] {
        &self.installed_agents
    }

    /// Registry entries NOT installed (for Available tab).
    pub fn available_agents(&self) -> Vec<&RegistryEntry> {
        let installed_ids: Vec<&str> = self
            .installed_agents
            .iter()
            .map(|a| a.entry.id.as_str())
            .collect();
        self.registry
            .list()
            .iter()
            .filter(|e| !installed_ids.contains(&e.id.as_str()))
            .collect()
    }

    /// Get health for a specific agent.
    pub fn agent_health(&self, name: &str) -> Option<&AgentHealth> {
        self.health.get_health(name)
    }

    /// Count tasks by state.
    pub fn task_count_by_state(&self, state_match: fn(&TaskState) -> bool) -> usize {
        self.tasks.iter().filter(|t| state_match(&t.state)).count()
    }
}

impl EventEmitter<SurgeEvent> for AppState {}

/// Detect current git branch name from a path.
fn detect_branch(path: &std::path::Path) -> Option<String> {
    let head_file = path.join(".git").join("HEAD");
    if let Ok(content) = std::fs::read_to_string(head_file) {
        if let Some(ref_str) = content.strip_prefix("ref: refs/heads/") {
            return Some(ref_str.trim().to_string());
        }
    }
    None
}
