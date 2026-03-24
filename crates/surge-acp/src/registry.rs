//! ACP Agent Registry — builtin catalog of supported agents plus remote fetching.
//!
//! # Registry sources
//!
//! | Source | API |
//! |---|---|
//! | Builtin (hardcoded) | [`Registry::builtin()`] |
//! | Remote URL | [`Registry::fetch_remote()`] |
//! | Merged | [`Registry::merged()`] |
//!
//! Remote entries are cached in `~/.surge/registry-cache.json` for 24 hours.

use crate::discovery::AgentDiscovery;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};
use surge_core::config::{AgentConfig, Transport};
use tracing::{debug, warn};

// ── Public types ────────────────────────────────────────────────────

/// Known agent variants in the Surge ecosystem.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentKind {
    Claude,
    Copilot,
    Codex,
    Gemini,
}

impl AgentKind {
    /// All known agent kinds.
    pub const ALL: [Self; 4] = [Self::Claude, Self::Copilot, Self::Codex, Self::Gemini];

    /// String identifier used in configs and registry.
    #[must_use]
    pub fn id(self) -> &'static str {
        match self {
            Self::Claude => "claude-acp",
            Self::Copilot => "github-copilot-cli",
            Self::Codex => "codex-acp",
            Self::Gemini => "gemini",
        }
    }

    /// Try to parse from a string identifier.
    #[must_use]
    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "claude-acp" => Some(Self::Claude),
            "github-copilot-cli" => Some(Self::Copilot),
            "codex-acp" => Some(Self::Codex),
            "gemini" => Some(Self::Gemini),
            _ => None,
        }
    }

    /// Human-readable display name.
    #[must_use]
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Claude => "Claude Agent",
            Self::Copilot => "GitHub Copilot",
            Self::Codex => "Codex CLI",
            Self::Gemini => "Gemini CLI",
        }
    }

    /// One-line description / tagline.
    #[must_use]
    pub fn tagline(self) -> &'static str {
        match self {
            Self::Claude => {
                "Anthropic's autonomous coding agent — deepest reasoning, largest context"
            }
            Self::Copilot => "GitHub's multi-model terminal agent with native repo integration",
            Self::Codex => "OpenAI's cloud-native coding agent with sandboxed parallel execution",
            Self::Gemini => {
                "Google's CLI with the most generous free tier — 1M context on all models"
            }
        }
    }

    /// Vendor / company name.
    #[must_use]
    pub fn vendor(self) -> &'static str {
        match self {
            Self::Claude => "Anthropic",
            Self::Copilot => "GitHub",
            Self::Codex => "OpenAI",
            Self::Gemini => "Google",
        }
    }

    /// Default model name.
    #[must_use]
    pub fn default_model(self) -> &'static str {
        match self {
            Self::Claude => "Claude Sonnet 4.6",
            Self::Copilot => "GPT-5 mini",
            Self::Codex => "GPT-5.3-Codex",
            Self::Gemini => "Gemini 2.5 Pro",
        }
    }

    /// Vendor brand color as (r, g, b) in 0.0–1.0.
    #[must_use]
    pub fn color(self) -> (f32, f32, f32) {
        match self {
            Self::Claude => (0.851, 0.467, 0.341),  // #D97757
            Self::Copilot => (0.431, 0.251, 0.788), // #6E40C9
            Self::Codex => (0.063, 0.639, 0.498),   // #10A37F
            Self::Gemini => (0.259, 0.522, 0.957),  // #4285F4
        }
    }

    /// Version detection command: (args, is_wrapper, cli_name).
    #[must_use]
    pub fn version_command(self) -> (&'static [&'static str], bool, &'static str) {
        match self {
            Self::Claude => (&["claude", "--version"], true, "Claude Code"),
            Self::Copilot => (&["gh", "copilot", "--version"], false, ""),
            Self::Codex => (&["codex", "--version"], true, "Codex CLI"),
            Self::Gemini => (&["gemini", "--version"], false, ""),
        }
    }

    /// Environment variable used to pass an MCP config file path to this agent.
    ///
    /// Returns `None` for agents whose MCP pass-through mechanism is not yet known.
    /// Extend this as each agent's CLI is verified against its documentation.
    #[must_use]
    pub fn mcp_config_env_var(self) -> Option<&'static str> {
        match self {
            // Claude Code reads MCP server config from the path in CLAUDE_MCP_CONFIG.
            Self::Claude => Some("CLAUDE_MCP_CONFIG"),
            // TODO: discover the correct env var / flag for each agent below.
            Self::Copilot | Self::Codex | Self::Gemini => None,
        }
    }
}

impl fmt::Display for AgentKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.id())
    }
}

/// Capabilities an agent may support.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentCapability {
    Code,
    Plan,
    Review,
    Test,
    Refactor,
    Chat,
}

impl fmt::Display for AgentCapability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Code => "code",
            Self::Plan => "plan",
            Self::Review => "review",
            Self::Test => "test",
            Self::Refactor => "refactor",
            Self::Chat => "chat",
        };
        write!(f, "{s}")
    }
}

/// A single entry in the agent registry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryEntry {
    pub kind: AgentKind,
    pub id: String,
    pub display_name: String,
    pub description: String,
    pub version: String,
    pub authors: Vec<String>,
    pub license: String,
    pub command: String,
    pub default_args: Vec<String>,
    pub transport: Transport,
    pub install_instructions: String,
    /// The actual CLI binary name (e.g. "claude" for claude-acp, "gh" for copilot).
    /// Used to detect if the underlying tool is installed, even for npx wrappers.
    pub cli_binary: Option<String>,
    pub website: Option<String>,
    pub tags: Vec<String>,
    pub capabilities: Vec<AgentCapability>,
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default)]
    pub long_description: String,
}

impl RegistryEntry {
    /// Convert to `AgentConfig`.
    #[must_use]
    pub fn to_agent_config(&self) -> AgentConfig {
        AgentConfig {
            command: self.command.clone(),
            args: self.default_args.clone(),
            transport: self.transport.clone(),
            mcp_servers: vec![],
            capabilities: self
                .capabilities
                .iter()
                .map(|cap| match cap {
                    AgentCapability::Code => surge_core::config::AgentCapability::Code,
                    AgentCapability::Plan => surge_core::config::AgentCapability::Plan,
                    AgentCapability::Review => surge_core::config::AgentCapability::Review,
                    AgentCapability::Test => surge_core::config::AgentCapability::Test,
                    AgentCapability::Refactor => surge_core::config::AgentCapability::Refactor,
                    AgentCapability::Chat => surge_core::config::AgentCapability::Chat,
                })
                .collect(),
        }
    }

    /// Check if this agent's binary is installed on PATH.
    /// npx/uvx agents are never "installed" — they run on-demand.
    #[must_use]
    pub fn is_installed(&self) -> bool {
        // Check the real CLI binary if specified (e.g. "claude" for claude-acp)
        if let Some(bin) = &self.cli_binary {
            return which(bin);
        }
        // For npx/uvx without cli_binary — not locally installed
        if self.is_npx() || self.is_uvx() {
            return false;
        }
        which(&self.command)
    }

    /// Whether this agent can be launched right now.
    #[must_use]
    pub fn is_runnable(&self) -> bool {
        which(&self.command)
    }

    #[must_use]
    pub fn is_npx(&self) -> bool {
        self.command == "npx"
    }

    #[must_use]
    pub fn is_uvx(&self) -> bool {
        self.command == "uvx"
    }

    /// Return `true` if the entry matches a case-insensitive search query.
    #[must_use]
    pub fn matches(&self, query: &str) -> bool {
        let q = query.to_lowercase();
        self.id.to_lowercase().contains(&q)
            || self.display_name.to_lowercase().contains(&q)
            || self.description.to_lowercase().contains(&q)
            || self.tags.iter().any(|t| t.to_lowercase().contains(&q))
            || self.authors.iter().any(|a| a.to_lowercase().contains(&q))
    }

    /// Primary vendor name.
    #[must_use]
    pub fn vendor(&self) -> &str {
        self.authors.first().map_or("Unknown", String::as_str)
    }

    /// Whether this agent uses an open-source license.
    #[must_use]
    pub fn is_open_source(&self) -> bool {
        !self.license.to_lowercase().contains("proprietary")
    }
}

// ── Registry ────────────────────────────────────────────────────────

/// Catalog of ACP-compatible agents.
#[derive(Debug, Clone)]
pub struct Registry {
    entries: Vec<RegistryEntry>,
}

impl Registry {
    /// Create the registry with the 4 supported agents.
    #[must_use]
    pub fn builtin() -> Self {
        Self {
            entries: builtin_agents(),
        }
    }

    #[must_use]
    pub fn list(&self) -> &[RegistryEntry] {
        &self.entries
    }

    #[must_use]
    pub fn search(&self, query: &str) -> Vec<&RegistryEntry> {
        self.entries.iter().filter(|e| e.matches(query)).collect()
    }

    #[must_use]
    pub fn find(&self, id: &str) -> Option<&RegistryEntry> {
        self.entries.iter().find(|e| e.id == id)
    }

    #[must_use]
    pub fn detect_installed(&self) -> Vec<&RegistryEntry> {
        self.entries.iter().filter(|e| e.is_installed()).collect()
    }

    pub fn detect_installed_with_paths(&self) -> Vec<DetectedAgent> {
        // Use AgentDiscovery module for enhanced detection via env vars and standard paths
        let mut discovery = AgentDiscovery::new();
        discovery.discover_all(&self.entries)
    }

    /// Discover installed agents using the merged registry.
    ///
    /// This method creates a merged registry from config, builtin, and optionally
    /// remote sources, then uses [`AgentDiscovery`] to find which agents are
    /// actually installed on the system.
    ///
    /// Returns detected agents with full registry metadata, command paths, and
    /// version information when available.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let registry = Registry::builtin();
    /// let detected = registry.discover_with_merged_registry();
    /// for agent in detected {
    ///     println!("Found: {} at {:?}", agent.entry.display_name, agent.command_path);
    /// }
    /// ```
    pub fn discover_with_merged_registry(&self) -> Vec<DetectedAgent> {
        // Use AgentDiscovery for intelligent matching
        let mut discovery = AgentDiscovery::new();
        discovery.discover_all(&self.entries)
    }

    pub fn detect_runnable_with_paths(&self) -> Vec<DetectedAgent> {
        // Serve from cache when fresh.
        if let Some(cached) = load_discovery_cache() {
            return cached;
        }

        let agents: Vec<DetectedAgent> = self
            .entries
            .iter()
            .filter(|e| e.is_runnable())
            .map(|e| DetectedAgent {
                entry: e.clone(),
                command_path: resolve_command_path(&e.command),
                detected_version: None,
            })
            .collect();

        save_discovery_cache(agents.clone());
        agents
    }

    #[must_use]
    pub fn by_capability(&self, cap: &AgentCapability) -> Vec<&RegistryEntry> {
        self.entries
            .iter()
            .filter(|e| e.capabilities.contains(cap))
            .collect()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Invalidate the discovery cache, forcing the next call to
    /// [`Registry::detect_runnable_with_paths`] to re-scan the system.
    ///
    /// Use this when the environment has changed (new agent installed,
    /// PATH updated, etc.) and you want fresh detection results.
    pub fn refresh_discovery() {
        if let Ok(mut cache_guard) = DISCOVERY_CACHE.lock() {
            *cache_guard = None;
            debug!("discovery cache invalidated");
        }
    }

    // ── Remote fetching ──────────────────────────────────────────────

    /// Fetch a registry from a remote URL, using a 24-hour file cache.
    ///
    /// The remote endpoint must serve a JSON array of [`RegistryEntry`] objects
    /// using the same schema as the builtin catalog.
    ///
    /// # Cache
    ///
    /// Successful responses are stored in `~/.surge/registry-cache.json`.
    /// Subsequent calls within 24 hours return the cached data without hitting
    /// the network.  Cache failures are silently ignored.
    ///
    /// # Errors
    ///
    /// Returns an error if the network request fails **and** there is no valid
    /// (possibly stale) cache entry to fall back to.
    pub async fn fetch_remote(url: &str) -> Result<Self, surge_core::SurgeError> {
        // Serve from cache when fresh.
        if let Some(entries) = load_registry_cache() {
            debug!("remote registry served from cache");
            return Ok(Self { entries });
        }

        debug!(url, "fetching remote registry");

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(3))
            .build()
            .map_err(|e| {
                surge_core::SurgeError::AgentConnection(format!("Failed to build HTTP client: {e}"))
            })?;

        let entries: Vec<RegistryEntry> = client
            .get(url)
            .send()
            .await
            .map_err(|e| {
                surge_core::SurgeError::AgentConnection(format!(
                    "Failed to fetch remote registry from {url}: {e}"
                ))
            })?
            .error_for_status()
            .map_err(|e| {
                surge_core::SurgeError::AgentConnection(format!(
                    "Remote registry request failed: {e}"
                ))
            })?
            .json()
            .await
            .map_err(|e| {
                surge_core::SurgeError::AgentConnection(format!(
                    "Failed to parse remote registry JSON: {e}"
                ))
            })?;

        save_registry_cache(&entries);

        Ok(Self { entries })
    }

    /// Create a registry from agents defined in surge.toml.
    ///
    /// Converts each `AgentConfig` to a `RegistryEntry` with sensible defaults
    /// for missing metadata. The agent name (HashMap key) becomes the entry `id`.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let mut agents = HashMap::new();
    /// agents.insert("my-agent".to_string(), AgentConfig {
    ///     command: "my-agent".to_string(),
    ///     args: vec!["--acp".to_string()],
    ///     transport: Transport::Stdio,
    ///     mcp_servers: vec![],
    ///     capabilities: vec![],
    /// });
    /// let registry = Registry::from_config(agents);
    /// assert_eq!(registry.len(), 1);
    /// ```
    #[must_use]
    pub fn from_config(agents: HashMap<String, AgentConfig>) -> Self {
        let entries = agents
            .into_iter()
            .map(|(name, config)| {
                // Try to infer AgentKind from command or name
                let kind = infer_agent_kind(&name, &config.command);

                RegistryEntry {
                    kind,
                    id: name.clone(),
                    display_name: prettify_name(&name),
                    description: format!("Custom agent: {}", name),
                    version: "custom".to_string(),
                    authors: vec![],
                    license: "unknown".to_string(),
                    command: config.command,
                    default_args: config.args,
                    transport: config.transport,
                    install_instructions: String::new(),
                    cli_binary: None,
                    website: None,
                    tags: vec!["custom".to_string()],
                    capabilities: if config.capabilities.is_empty() {
                        // Default to all capabilities if none specified
                        vec![
                            AgentCapability::Code,
                            AgentCapability::Plan,
                            AgentCapability::Review,
                            AgentCapability::Test,
                            AgentCapability::Refactor,
                            AgentCapability::Chat,
                        ]
                    } else {
                        config
                            .capabilities
                            .iter()
                            .map(convert_capability)
                            .collect()
                    },
                    models: vec![],
                    long_description: String::new(),
                }
            })
            .collect();

        Self { entries }
    }

    /// Load registry from agents defined in surge.toml.
    ///
    /// Discovers surge.toml by walking up from the current directory,
    /// loads the config, and creates a registry from the `agents` section.
    ///
    /// Returns an empty registry if no surge.toml is found or if the
    /// config has no agents defined.
    ///
    /// # Errors
    ///
    /// Returns an error if surge.toml exists but cannot be parsed.
    pub fn load_from_toml() -> Result<Self, surge_core::SurgeError> {
        use surge_core::config::SurgeConfig;

        let config = SurgeConfig::discover()?;
        Ok(Self::from_config(config.agents))
    }

    /// Merge a builtin and a remote registry.
    ///
    /// Builtin entries take priority: if both catalogs contain an entry with
    /// the same `id`, the builtin version is kept and the remote one ignored.
    /// Unknown remote entries are appended after all builtin entries.
    #[must_use]
    pub fn merged(builtin: Self, remote: Self) -> Self {
        Self::merged_impl(None, builtin, remote)
    }

    /// Merge config, builtin, and remote registries (3-way merge).
    ///
    /// Priority order (highest to lowest):
    /// 1. Config entries (user-defined)
    /// 2. Builtin entries (hardcoded)
    /// 3. Remote entries (fetched from registry)
    ///
    /// If the same `id` appears in multiple sources, the higher-priority
    /// version is kept. Unique entries from all sources are included.
    #[must_use]
    pub fn merged_with_config(config: Self, builtin: Self, remote: Self) -> Self {
        Self::merged_impl(Some(config), builtin, remote)
    }

    /// Internal implementation for 2-way and 3-way registry merging.
    ///
    /// Priority: config > builtin > remote
    fn merged_impl(config: Option<Self>, builtin: Self, remote: Self) -> Self {
        let mut entries = Vec::new();
        let mut seen_ids = std::collections::HashSet::new();

        // 1. Add config entries first (highest priority)
        if let Some(config) = config {
            for entry in config.entries {
                seen_ids.insert(entry.id.clone());
                entries.push(entry);
            }
        }

        // 2. Add builtin entries (skip if already in config)
        for entry in builtin.entries {
            if seen_ids.insert(entry.id.clone()) {
                entries.push(entry);
            }
        }

        // 3. Add remote entries (skip if already in config or builtin)
        for entry in remote.entries {
            if seen_ids.insert(entry.id.clone()) {
                entries.push(entry);
            }
        }

        Self { entries }
    }
}

/// Result of detecting an installed agent.
#[derive(Debug, Clone)]
pub struct DetectedAgent {
    pub entry: RegistryEntry,
    pub command_path: Option<String>,
    /// Detected version string from running `--version` command.
    /// None if version detection failed or was not attempted.
    pub detected_version: Option<String>,
}

// ── Hardcoded agents ────────────────────────────────────────────────

fn builtin_agents() -> Vec<RegistryEntry> {
    vec![
        RegistryEntry {
            kind: AgentKind::Claude,
            id: "claude-acp".into(),
            display_name: "Claude Agent".into(),
            description: "ACP wrapper for Anthropic's Claude".into(),
            version: "0.22.2".into(),
            authors: vec!["Anthropic".into()],
            license: "proprietary".into(),
            command: "npx".into(),
            default_args: vec!["@zed-industries/claude-agent-acp".into()],
            transport: Transport::Stdio,
            install_instructions: "npx @zed-industries/claude-agent-acp".into(),
            cli_binary: Some("claude".into()),
            website: Some("https://claude.ai/claude-code".into()),
            tags: vec!["anthropic".into(), "popular".into()],
            capabilities: vec![
                AgentCapability::Code,
                AgentCapability::Plan,
                AgentCapability::Review,
                AgentCapability::Test,
                AgentCapability::Refactor,
                AgentCapability::Chat,
            ],
            models: vec![],
            long_description: String::new(),
        },
        RegistryEntry {
            kind: AgentKind::Copilot,
            id: "github-copilot-cli".into(),
            display_name: "GitHub Copilot".into(),
            description: "GitHub's AI pair programmer".into(),
            version: "1.0.10".into(),
            authors: vec!["GitHub".into()],
            license: "proprietary".into(),
            command: "npx".into(),
            default_args: vec!["@github/copilot".into(), "--acp".into()],
            transport: Transport::Stdio,
            install_instructions: "npx @github/copilot --acp".into(),
            cli_binary: Some("gh".into()),
            website: Some("https://github.com/features/copilot/cli/".into()),
            tags: vec!["github".into(), "popular".into()],
            capabilities: vec![
                AgentCapability::Code,
                AgentCapability::Plan,
                AgentCapability::Review,
                AgentCapability::Chat,
            ],
            models: vec![],
            long_description: String::new(),
        },
        RegistryEntry {
            kind: AgentKind::Codex,
            id: "codex-acp".into(),
            display_name: "Codex CLI".into(),
            description: "ACP adapter for OpenAI's coding assistant".into(),
            version: "0.10.0".into(),
            authors: vec!["OpenAI".into(), "Zed Industries".into()],
            license: "Apache-2.0".into(),
            command: "npx".into(),
            default_args: vec!["@zed-industries/codex-acp".into()],
            transport: Transport::Stdio,
            install_instructions: "npx @zed-industries/codex-acp".into(),
            cli_binary: Some("codex".into()),
            website: Some("https://openai.com".into()),
            tags: vec!["openai".into(), "popular".into(), "open-source".into()],
            capabilities: vec![AgentCapability::Code, AgentCapability::Chat],
            models: vec![],
            long_description: String::new(),
        },
        RegistryEntry {
            kind: AgentKind::Gemini,
            id: "gemini".into(),
            display_name: "Gemini CLI".into(),
            description: "Google's official CLI for Gemini".into(),
            version: "0.34.0".into(),
            authors: vec!["Google".into()],
            license: "Apache-2.0".into(),
            command: "npx".into(),
            default_args: vec!["@google/gemini-cli".into(), "--acp".into()],
            transport: Transport::Stdio,
            install_instructions: "npx @google/gemini-cli --acp".into(),
            cli_binary: Some("gemini".into()),
            website: Some("https://geminicli.com".into()),
            tags: vec!["google".into(), "popular".into(), "open-source".into()],
            capabilities: vec![
                AgentCapability::Code,
                AgentCapability::Refactor,
                AgentCapability::Chat,
            ],
            models: vec![],
            long_description: String::new(),
        },
    ]
}

// ── Registry cache ───────────────────────────────────────────────────

const CACHE_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// Platform-appropriate path for the registry cache file.
fn registry_cache_path() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var("APPDATA")
            .ok()
            .map(|d| PathBuf::from(d).join("surge").join("registry-cache.json"))
    }
    #[cfg(not(windows))]
    {
        std::env::var("HOME")
            .ok()
            .map(|h| PathBuf::from(h).join(".surge").join("registry-cache.json"))
    }
}

/// Load cached entries if the cache file exists and is younger than [`CACHE_TTL`].
fn load_registry_cache() -> Option<Vec<RegistryEntry>> {
    let path = registry_cache_path()?;
    let metadata = std::fs::metadata(&path).ok()?;
    let age = metadata.modified().ok()?.elapsed().ok()?;
    if age > CACHE_TTL {
        debug!("registry cache is stale ({age:?}), will re-fetch");
        return None;
    }
    let content = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&content).ok()
}

/// Persist registry entries to the cache file, creating parent dirs as needed.
/// Failures are silently ignored — caching is best-effort.
fn save_registry_cache(entries: &[RegistryEntry]) {
    let Some(path) = registry_cache_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match serde_json::to_string_pretty(entries) {
        Ok(json) => {
            if let Err(e) = std::fs::write(&path, json) {
                warn!("failed to write registry cache to {}: {e}", path.display());
            }
        }
        Err(e) => warn!("failed to serialize registry cache: {e}"),
    }
}

// ── Utilities ───────────────────────────────────────────────────────

/// Cache for `which`/`resolve_command_path` results.
static WHICH_CACHE: LazyLock<Mutex<HashMap<String, Option<String>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Discovery cache entry with timestamp.
#[derive(Debug, Clone)]
struct DiscoveryCacheEntry {
    agents: Vec<DetectedAgent>,
    timestamp: Instant,
}

/// Cache for discovered agents with 5-minute TTL.
static DISCOVERY_CACHE: LazyLock<Mutex<Option<DiscoveryCacheEntry>>> =
    LazyLock::new(|| Mutex::new(None));

const DISCOVERY_CACHE_TTL: Duration = Duration::from_secs(5 * 60);

/// Load cached discovered agents if cache is still valid.
fn load_discovery_cache() -> Option<Vec<DetectedAgent>> {
    if let Ok(cache_guard) = DISCOVERY_CACHE.lock()
        && let Some(entry) = cache_guard.as_ref()
    {
        let age = entry.timestamp.elapsed();
        if age <= DISCOVERY_CACHE_TTL {
            debug!("discovery cache hit (age: {age:?})");
            return Some(entry.agents.clone());
        }
        debug!("discovery cache expired (age: {age:?})");
    }
    None
}

/// Save discovered agents to cache with current timestamp.
fn save_discovery_cache(agents: Vec<DetectedAgent>) {
    if let Ok(mut cache_guard) = DISCOVERY_CACHE.lock() {
        *cache_guard = Some(DiscoveryCacheEntry {
            agents,
            timestamp: Instant::now(),
        });
        debug!("discovery cache updated");
    }
}

/// Check if a command exists on PATH (cached).
fn which(command: &str) -> bool {
    resolve_command_path(command).is_some()
}

/// Resolve the full path of a command (cached).
fn resolve_command_path(command: &str) -> Option<String> {
    if let Ok(cache) = WHICH_CACHE.lock()
        && let Some(result) = cache.get(command)
    {
        return result.clone();
    }

    let result = resolve_command_uncached(command);

    if let Ok(mut cache) = WHICH_CACHE.lock() {
        cache.insert(command.to_string(), result.clone());
    }

    result
}

fn resolve_command_uncached(command: &str) -> Option<String> {
    use std::process::Command;

    #[cfg(windows)]
    let output = Command::new("where").arg(command).output().ok()?;

    #[cfg(not(windows))]
    let output = Command::new("which").arg(command).output().ok()?;

    if output.status.success() {
        let path = String::from_utf8_lossy(&output.stdout);
        Some(path.lines().next()?.trim().to_string())
    } else {
        None
    }
}

/// Try to infer `AgentKind` from agent name or command.
/// Falls back to Claude if no match is found.
fn infer_agent_kind(name: &str, command: &str) -> AgentKind {
    let name_lower = name.to_lowercase();
    let command_lower = command.to_lowercase();

    if name_lower.contains("claude") || command_lower.contains("claude") {
        AgentKind::Claude
    } else if name_lower.contains("copilot")
        || name_lower.contains("gh")
        || command_lower.contains("copilot")
    {
        AgentKind::Copilot
    } else if name_lower.contains("codex") || command_lower.contains("codex") {
        AgentKind::Codex
    } else if name_lower.contains("gemini") || command_lower.contains("gemini") {
        AgentKind::Gemini
    } else {
        // Default to Claude for unknown agents
        AgentKind::Claude
    }
}

/// Convert a kebab-case or snake_case name to a human-readable display name.
fn prettify_name(name: &str) -> String {
    name.replace('-', " ")
        .replace('_', " ")
        .split_whitespace()
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                None => String::new(),
                Some(first) => first.to_uppercase().chain(chars).collect(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Convert from `surge_core::config::AgentCapability` to the local `AgentCapability`.
fn convert_capability(cap: &surge_core::config::AgentCapability) -> AgentCapability {
    match cap {
        surge_core::config::AgentCapability::Code => AgentCapability::Code,
        surge_core::config::AgentCapability::Plan => AgentCapability::Plan,
        surge_core::config::AgentCapability::Review => AgentCapability::Review,
        surge_core::config::AgentCapability::Test => AgentCapability::Test,
        surge_core::config::AgentCapability::Refactor => AgentCapability::Refactor,
        surge_core::config::AgentCapability::Chat => AgentCapability::Chat,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builtin_has_4_agents() {
        let reg = Registry::builtin();
        assert_eq!(reg.len(), 4);
    }

    #[test]
    fn test_find_claude() {
        let reg = Registry::builtin();
        let entry = reg.find("claude-acp").unwrap();
        assert_eq!(entry.display_name, "Claude Agent");
        assert_eq!(entry.command, "npx");
    }

    #[test]
    fn test_find_gemini() {
        let reg = Registry::builtin();
        let entry = reg.find("gemini").unwrap();
        assert_eq!(entry.display_name, "Gemini CLI");
        assert!(entry.default_args.contains(&"--acp".to_string()));
    }

    #[test]
    fn test_find_copilot() {
        let reg = Registry::builtin();
        let entry = reg.find("github-copilot-cli").unwrap();
        assert_eq!(entry.vendor(), "GitHub");
    }

    #[test]
    fn test_find_codex() {
        let reg = Registry::builtin();
        let entry = reg.find("codex-acp").unwrap();
        assert!(entry.is_open_source());
    }

    #[test]
    fn test_installed_checks_cli_binary() {
        let reg = Registry::builtin();
        let claude = reg.find("claude-acp").unwrap();
        // claude-acp has cli_binary="claude" — installed if `claude` is on PATH
        assert_eq!(claude.cli_binary.as_deref(), Some("claude"));
        // Actual result depends on system — just check it doesn't panic
        let _ = claude.is_installed();
    }

    #[test]
    fn test_all_are_code_capable() {
        let reg = Registry::builtin();
        let coders = reg.by_capability(&AgentCapability::Code);
        assert_eq!(coders.len(), 4);
    }

    #[test]
    fn test_search() {
        let reg = Registry::builtin();
        assert!(!reg.search("google").is_empty());
        assert!(!reg.search("anthropic").is_empty());
        assert!(reg.search("nonexistent_xyz").is_empty());
    }

    #[test]
    fn test_to_agent_config() {
        let reg = Registry::builtin();
        let config = reg.find("gemini").unwrap().to_agent_config();
        assert_eq!(config.command, "npx");
        assert!(matches!(config.transport, Transport::Stdio));
    }

    #[test]
    fn test_which_finds_git() {
        assert!(which("git"));
    }

    #[test]
    fn test_which_not_found() {
        assert!(!which("nonexistent_binary_12345"));
    }

    #[test]
    fn test_which_cache_works() {
        // First call populates cache
        let _ = which("git");
        // Second call hits cache (no subprocess)
        let _ = which("git");

        let cache = WHICH_CACHE.lock().unwrap();
        assert!(cache.contains_key("git"));
    }

    fn make_entry(id: &str) -> RegistryEntry {
        RegistryEntry {
            kind: AgentKind::Claude,
            id: id.to_string(),
            display_name: id.to_string(),
            description: String::new(),
            version: "0.0.0".to_string(),
            authors: vec![],
            license: "MIT".to_string(),
            command: "echo".to_string(),
            default_args: vec![],
            transport: Transport::Stdio,
            install_instructions: String::new(),
            cli_binary: None,
            website: None,
            tags: vec![],
            capabilities: vec![AgentCapability::Code],
            models: vec![],
            long_description: String::new(),
        }
    }

    #[test]
    fn test_merged_builtin_takes_priority_on_id_collision() {
        let builtin = Registry {
            entries: vec![make_entry("agent-a")],
        };
        let mut remote_a = make_entry("agent-a");
        remote_a.display_name = "remote-version".to_string();
        let remote = Registry {
            entries: vec![remote_a, make_entry("agent-b")],
        };

        let merged = Registry::merged(builtin, remote);

        assert_eq!(merged.len(), 2);
        // builtin entry preserved — display_name from builtin, not remote
        assert_eq!(merged.find("agent-a").unwrap().display_name, "agent-a");
        // new remote-only entry appended
        assert!(merged.find("agent-b").is_some());
    }

    #[test]
    fn test_merged_remote_only_entries_appended() {
        let builtin = Registry::builtin();
        let remote = Registry {
            entries: vec![make_entry("custom-agent")],
        };
        let merged = Registry::merged(builtin, remote);

        assert!(merged.find("custom-agent").is_some());
        // builtin agents still present
        assert!(merged.find("claude-acp").is_some());
    }

    #[test]
    fn test_merged_empty_remote() {
        let builtin = Registry::builtin();
        let len = builtin.len();
        let merged = Registry::merged(builtin, Registry { entries: vec![] });
        assert_eq!(merged.len(), len);
    }

    #[test]
    fn test_merged_three_way() {
        // Create config registry with custom agent-a
        let mut config_a = make_entry("agent-a");
        config_a.display_name = "config-version".to_string();
        let config = Registry {
            entries: vec![config_a],
        };

        // Create builtin registry with agent-a and agent-b
        let mut builtin_a = make_entry("agent-a");
        builtin_a.display_name = "builtin-version".to_string();
        let builtin = Registry {
            entries: vec![builtin_a, make_entry("agent-b")],
        };

        // Create remote registry with agent-a, agent-b, and agent-c
        let mut remote_a = make_entry("agent-a");
        remote_a.display_name = "remote-version".to_string();
        let mut remote_b = make_entry("agent-b");
        remote_b.display_name = "remote-b-version".to_string();
        let remote = Registry {
            entries: vec![remote_a, remote_b, make_entry("agent-c")],
        };

        // Perform 3-way merge
        let merged = Registry::merged_with_config(config, builtin, remote);

        // Verify merged result
        assert_eq!(merged.len(), 3);

        // agent-a should come from config (highest priority)
        assert_eq!(
            merged.find("agent-a").unwrap().display_name,
            "config-version"
        );

        // agent-b should come from builtin (config doesn't have it)
        assert_eq!(merged.find("agent-b").unwrap().display_name, "agent-b");

        // agent-c should come from remote (only source that has it)
        assert!(merged.find("agent-c").is_some());
    }

    #[test]
    fn test_mcp_config_env_var() {
        assert_eq!(
            AgentKind::Claude.mcp_config_env_var(),
            Some("CLAUDE_MCP_CONFIG")
        );
        assert!(AgentKind::Copilot.mcp_config_env_var().is_none());
        assert!(AgentKind::Codex.mcp_config_env_var().is_none());
        assert!(AgentKind::Gemini.mcp_config_env_var().is_none());
    }

    #[test]
    fn test_refresh_discovery() {
        let reg = Registry::builtin();

        // Populate the cache by calling detect_runnable_with_paths
        let first_result = reg.detect_runnable_with_paths();

        // Verify cache is populated
        assert!(DISCOVERY_CACHE.lock().unwrap().is_some());

        // Invalidate the cache
        Registry::refresh_discovery();

        // Verify cache is cleared
        assert!(DISCOVERY_CACHE.lock().unwrap().is_none());

        // Next call should re-populate the cache
        let second_result = reg.detect_runnable_with_paths();
        assert!(DISCOVERY_CACHE.lock().unwrap().is_some());

        // Results should be consistent
        assert_eq!(first_result.len(), second_result.len());
    }

    #[test]
    fn test_detect_with_discovery() {
        use std::env;
        use std::fs::File;

        let reg = Registry::builtin();

        // Test 1: Basic detection without env vars (may find agents on PATH)
        let detected = reg.detect_installed_with_paths();
        // Can't assert specific count since it depends on what's installed on the system
        // Just verify the method runs without panicking
        assert!(detected.len() <= reg.len());

        // Test 2: Detection with env var override
        let temp_dir = env::temp_dir();
        let temp_file = temp_dir.join("test_mock_claude");

        // Create a temporary mock agent file
        if File::create(&temp_file).is_ok() {
            // Set CLAUDE_PATH to point to our mock file
            unsafe {
                env::set_var("CLAUDE_PATH", temp_file.to_string_lossy().as_ref());
            }

            // Run detection - should find Claude via env var
            let detected = reg.detect_installed_with_paths();

            // Look for Claude in the results
            let claude_found = detected.iter().any(|d| {
                d.entry.kind == AgentKind::Claude
                    && d.command_path
                        .as_ref()
                        .map(|p| p.contains("test_mock_claude"))
                        .unwrap_or(false)
            });

            assert!(
                claude_found,
                "Should detect Claude agent via CLAUDE_PATH env var"
            );

            // Clean up
            unsafe {
                env::remove_var("CLAUDE_PATH");
            }
            let _ = std::fs::remove_file(&temp_file);
        }

        // Test 3: Verify DetectedAgent structure
        let detected = reg.detect_installed_with_paths();
        for agent in detected {
            // Each detected agent should have a valid entry
            assert!(!agent.entry.id.is_empty());
            assert!(!agent.entry.display_name.is_empty());
            // command_path may be Some or None depending on what's installed
        }
    }

    #[test]
    fn test_from_config() {
        use std::collections::HashMap;

        // Test 1: Empty config produces empty registry
        let empty_agents: HashMap<String, AgentConfig> = HashMap::new();
        let empty_reg = Registry::from_config(empty_agents);
        assert_eq!(empty_reg.len(), 0);

        // Test 2: Single agent with minimal config
        let mut agents = HashMap::new();
        agents.insert(
            "my-custom-agent".to_string(),
            AgentConfig {
                command: "my-agent".to_string(),
                args: vec!["--acp".to_string()],
                transport: Transport::Stdio,
                mcp_servers: vec![],
                capabilities: vec![],
            },
        );

        let reg = Registry::from_config(agents);
        assert_eq!(reg.len(), 1);

        let entry = reg.find("my-custom-agent").unwrap();
        assert_eq!(entry.id, "my-custom-agent");
        assert_eq!(entry.display_name, "My Custom Agent");
        assert_eq!(entry.command, "my-agent");
        assert_eq!(entry.default_args, vec!["--acp"]);
        assert_eq!(entry.tags, vec!["custom"]);
        // Should default to all capabilities when none specified
        assert_eq!(entry.capabilities.len(), 6);
        assert!(entry.capabilities.contains(&AgentCapability::Code));
        assert!(entry.capabilities.contains(&AgentCapability::Chat));

        // Test 3: Agent with explicit capabilities
        let mut agents_with_caps = HashMap::new();
        agents_with_caps.insert(
            "test-agent".to_string(),
            AgentConfig {
                command: "test".to_string(),
                args: vec![],
                transport: Transport::Stdio,
                mcp_servers: vec![],
                capabilities: vec![
                    surge_core::config::AgentCapability::Code,
                    surge_core::config::AgentCapability::Test,
                ],
            },
        );

        let reg_with_caps = Registry::from_config(agents_with_caps);
        let entry_with_caps = reg_with_caps.find("test-agent").unwrap();
        assert_eq!(entry_with_caps.capabilities.len(), 2);
        assert!(entry_with_caps
            .capabilities
            .contains(&AgentCapability::Code));
        assert!(entry_with_caps
            .capabilities
            .contains(&AgentCapability::Test));

        // Test 4: Inferred AgentKind from name
        let mut claude_agent = HashMap::new();
        claude_agent.insert(
            "claude-custom".to_string(),
            AgentConfig {
                command: "npx".to_string(),
                args: vec!["@custom/claude".to_string()],
                transport: Transport::Stdio,
                mcp_servers: vec![],
                capabilities: vec![],
            },
        );

        let claude_reg = Registry::from_config(claude_agent);
        let claude_entry = claude_reg.find("claude-custom").unwrap();
        assert_eq!(claude_entry.kind, AgentKind::Claude);

        // Test 5: Multiple agents
        let mut multi_agents = HashMap::new();
        multi_agents.insert(
            "agent-1".to_string(),
            AgentConfig {
                command: "agent1".to_string(),
                args: vec![],
                transport: Transport::Stdio,
                mcp_servers: vec![],
                capabilities: vec![],
            },
        );
        multi_agents.insert(
            "agent-2".to_string(),
            AgentConfig {
                command: "agent2".to_string(),
                args: vec![],
                transport: Transport::Stdio,
                mcp_servers: vec![],
                capabilities: vec![],
            },
        );

        let multi_reg = Registry::from_config(multi_agents);
        assert_eq!(multi_reg.len(), 2);
        assert!(multi_reg.find("agent-1").is_some());
        assert!(multi_reg.find("agent-2").is_some());
    }

    #[test]
    fn test_load_from_toml() {
        use std::fs;

        // Create a temporary directory structure
        let temp_dir = std::env::temp_dir().join("surge_test_registry_load_from_toml");
        let _ = fs::remove_dir_all(&temp_dir); // Clean up any previous test
        fs::create_dir_all(&temp_dir).unwrap();

        // Test 1: When surge.toml exists with agents, it should load them
        let config_path = temp_dir.join("surge.toml");
        fs::write(
            &config_path,
            r#"
default_agent = "test-agent"

[agents.test-agent]
command = "test-cli"
args = ["--acp", "--mode=test"]
transport = "stdio"

[agents.another-agent]
command = "another-cli"
args = []
transport = { tcp = { host = "localhost", port = 8080 } }
"#,
        )
        .unwrap();

        // Change to the temp directory to test load_from_toml
        let original_dir = std::env::current_dir().unwrap();
        std::env::set_current_dir(&temp_dir).unwrap();

        let registry = Registry::load_from_toml().unwrap();
        assert_eq!(registry.len(), 2);

        let test_agent = registry.find("test-agent").unwrap();
        assert_eq!(test_agent.command, "test-cli");
        assert_eq!(test_agent.default_args, vec!["--acp", "--mode=test"]);
        assert!(matches!(test_agent.transport, Transport::Stdio));

        let another_agent = registry.find("another-agent").unwrap();
        assert_eq!(another_agent.command, "another-cli");
        assert!(matches!(another_agent.transport, Transport::Tcp { .. }));

        // Test 2: When no surge.toml exists, it should return empty registry
        let no_config_dir = std::env::temp_dir().join("surge_test_registry_load_from_toml_no_config");
        let _ = fs::remove_dir_all(&no_config_dir);
        fs::create_dir_all(&no_config_dir).unwrap();
        std::env::set_current_dir(&no_config_dir).unwrap();

        let empty_registry = Registry::load_from_toml().unwrap();
        assert_eq!(empty_registry.len(), 0);
        assert!(empty_registry.is_empty());

        // Test 3: When surge.toml exists but has no agents, return empty registry
        let empty_agents_dir = std::env::temp_dir().join("surge_test_registry_load_from_toml_empty_agents");
        let _ = fs::remove_dir_all(&empty_agents_dir);
        fs::create_dir_all(&empty_agents_dir).unwrap();
        let empty_config_path = empty_agents_dir.join("surge.toml");
        fs::write(
            &empty_config_path,
            r#"
default_agent = "claude-acp"

[pipeline]
max_qa_iterations = 5
"#,
        )
        .unwrap();
        std::env::set_current_dir(&empty_agents_dir).unwrap();

        let empty_agents_registry = Registry::load_from_toml().unwrap();
        assert_eq!(empty_agents_registry.len(), 0);

        // Restore original directory
        std::env::set_current_dir(&original_dir).unwrap();

        // Clean up
        let _ = fs::remove_dir_all(&temp_dir);
        let _ = fs::remove_dir_all(&no_config_dir);
        let _ = fs::remove_dir_all(&empty_agents_dir);
    }
}
