use std::path::PathBuf;
use std::sync::Arc;

use surge_acp::{
    AgentHealth, AgentPool, DetectedAgent, HealthTracker, PermissionPolicy, Registry, RegistryEntry,
};
use surge_core::{Spec, SpecId, SurgeConfig, SurgeEvent, TaskId, TaskState};

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
            _event_tx: event_tx,
            recent_events: Vec::new(),
        }
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

    /// Handle a SurgeEvent — update state accordingly.
    pub fn handle_event(&mut self, event: SurgeEvent) {
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
            }
            SurgeEvent::AgentConnected { agent_name } => {
                self.health.register(agent_name);
            }
            _ => {}
        }
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
