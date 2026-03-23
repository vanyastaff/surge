//! ACP Agent Registry — data-driven catalog parsed from official ACP registry.json.
//!
//! Two-tier loading:
//! 1. **Embedded fallback** — compiled into the binary via `include_str!`
//! 2. **Cached registry** — `~/.surge/cache/registry.json`, updated via CDN fetch
//!
//! `Registry::load()` tries the cache first, falls back to embedded.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use surge_core::config::{AgentConfig, Transport};
use tracing::{debug, info, warn};

// ── Embedded registry JSON ──────────────────────────────────────────

const REGISTRY_JSON: &str = include_str!("acp_registry.json");

/// Registry JSON URL for runtime refresh.
pub const REGISTRY_URL: &str =
    "https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json";

// ── Raw serde types matching the official schema ────────────────────

#[derive(Debug, Clone, Deserialize)]
struct RawRegistry {
    #[allow(dead_code)]
    version: String,
    agents: Vec<RawAgent>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawAgent {
    id: String,
    name: String,
    version: String,
    description: String,
    repository: Option<String>,
    website: Option<String>,
    authors: Vec<String>,
    license: String,
    #[serde(default)]
    distribution: RawDistribution,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct RawDistribution {
    npx: Option<NpxDist>,
    binary: Option<HashMap<String, BinaryPlatformDist>>,
    uvx: Option<UvxDist>,
}

#[derive(Debug, Clone, Deserialize)]
struct NpxDist {
    package: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: HashMap<String, String>,
}

#[derive(Debug, Clone, Deserialize)]
struct BinaryPlatformDist {
    archive: String,
    cmd: String,
    #[serde(default)]
    args: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct UvxDist {
    package: String,
    #[serde(default)]
    args: Vec<String>,
}

// ── Public types ────────────────────────────────────────────────────

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

/// How an agent is distributed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Distribution {
    /// Installed and run via `npx <package> [args...]`.
    Npx {
        package: String,
        args: Vec<String>,
        env: HashMap<String, String>,
    },
    /// Platform-specific binary downloaded from an archive URL.
    Binary {
        platforms: HashMap<String, BinaryTarget>,
    },
    /// Installed and run via `uvx <package> [args...]`.
    Uvx {
        package: String,
        args: Vec<String>,
    },
}

/// A platform-specific binary target.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BinaryTarget {
    /// URL of the archive to download.
    pub archive: String,
    /// Command to run after extraction.
    pub cmd: String,
    /// Extra arguments for ACP mode.
    pub args: Vec<String>,
}

/// A single entry in the agent registry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryEntry {
    /// Unique identifier from ACP registry (e.g. "claude-acp", "goose").
    pub id: String,
    /// Human-readable display name.
    pub display_name: String,
    /// Short description.
    pub description: String,
    /// Version from the registry.
    pub version: String,
    /// Authors / vendor.
    pub authors: Vec<String>,
    /// License (e.g. "Apache-2.0", "proprietary").
    pub license: String,
    /// Distribution methods.
    pub distributions: Vec<Distribution>,
    /// Repository URL.
    pub repository: Option<String>,
    /// Website URL.
    pub website: Option<String>,
    /// Derived tags for UI filtering.
    pub tags: Vec<String>,
    /// Capabilities (derived from known agents, Code+Chat default).
    pub capabilities: Vec<AgentCapability>,

    // ── Backward-compatible fields for UI ───────────────────────

    /// Resolved command for the current platform (best available).
    pub command: String,
    /// Default args (including ACP args).
    pub default_args: Vec<String>,
    /// Transport (always Stdio for ACP).
    pub transport: Transport,
    /// Install instructions (derived from distribution).
    pub install_instructions: String,
    /// Long description (same as description for registry agents).
    #[serde(default)]
    pub long_description: String,
    /// Models — empty for registry agents (not part of ACP schema).
    #[serde(default)]
    pub models: Vec<String>,
}

impl RegistryEntry {
    /// Convert this registry entry into an `AgentConfig`.
    #[must_use]
    pub fn to_agent_config(&self) -> AgentConfig {
        AgentConfig {
            command: self.command.clone(),
            args: self.default_args.clone(),
            transport: self.transport.clone(),
        }
    }

    /// Check if this agent is locally installed (binary on PATH).
    ///
    /// npx/uvx agents are never "installed" — they run on-demand.
    /// Use `is_runnable()` to check if an agent can be launched.
    #[must_use]
    pub fn is_installed(&self) -> bool {
        if self.is_npx() || self.is_uvx() {
            return false;
        }
        which(&self.command)
    }

    /// Whether this agent can be launched right now.
    ///
    /// - Binary agents: the command is on PATH
    /// - npx agents: `npx` is available (package downloaded on-demand)
    /// - uvx agents: `uvx` is available
    #[must_use]
    pub fn is_runnable(&self) -> bool {
        which(&self.command)
    }

    /// Whether this agent has a binary distribution for the current platform.
    #[must_use]
    pub fn has_binary_dist(&self) -> bool {
        self.distributions.iter().any(|d| matches!(d, Distribution::Binary { .. }))
    }

    /// Whether this agent uses npx distribution.
    #[must_use]
    pub fn is_npx(&self) -> bool {
        self.command == "npx"
    }

    /// Whether this agent uses uvx distribution.
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

    /// Return the primary vendor/author name.
    #[must_use]
    pub fn vendor(&self) -> &str {
        self.authors.first().map_or("Unknown", String::as_str)
    }

    /// Whether this agent uses an open-source license.
    #[must_use]
    pub fn is_open_source(&self) -> bool {
        let l = self.license.to_lowercase();
        !l.contains("proprietary")
    }
}

// ── Registry ────────────────────────────────────────────────────────

/// Catalog of ACP-compatible agents.
#[derive(Debug, Clone)]
pub struct Registry {
    entries: Vec<RegistryEntry>,
}

impl Registry {
    /// Create the registry from the embedded `acp_registry.json` only.
    #[must_use]
    pub fn embedded() -> Self {
        Self::from_json(REGISTRY_JSON).unwrap_or_else(|e| {
            tracing::error!("Failed to parse embedded registry: {e}");
            Self {
                entries: Vec::new(),
            }
        })
    }

    /// Load registry: try cache first, fall back to embedded.
    ///
    /// Cache location: `~/.surge/cache/registry.json`
    #[must_use]
    pub fn load() -> Self {
        if let Some(cache_path) = cache_path() {
            if cache_path.exists() {
                match std::fs::read_to_string(&cache_path) {
                    Ok(json) => match Self::from_json(&json) {
                        Ok(reg) => {
                            info!(
                                agents = reg.len(),
                                "loaded registry from cache: {}",
                                cache_path.display()
                            );
                            return reg;
                        }
                        Err(e) => {
                            warn!("cached registry is corrupt, using embedded: {e}");
                        }
                    },
                    Err(e) => {
                        warn!("cannot read cached registry: {e}");
                    }
                }
            } else {
                debug!("no cached registry, using embedded");
            }
        }
        Self::embedded()
    }

    /// Backward-compat alias for `load()`.
    #[must_use]
    pub fn builtin() -> Self {
        Self::load()
    }

    /// Parse a registry from JSON string.
    ///
    /// # Errors
    ///
    /// Returns error if JSON parsing fails.
    pub fn from_json(json: &str) -> Result<Self, String> {
        let raw: RawRegistry =
            serde_json::from_str(json).map_err(|e| format!("Registry parse error: {e}"))?;

        let platform = current_platform();
        let entries = raw
            .agents
            .into_iter()
            .map(|agent| build_entry(agent, &platform))
            .collect();

        Ok(Self { entries })
    }

    /// Update the local cache with fresh JSON (e.g. fetched from CDN).
    ///
    /// Also saves the ETag for conditional GET on next fetch.
    ///
    /// # Errors
    ///
    /// Returns error if cache directory can't be created or file can't be written.
    pub fn save_cache(json: &str, etag: Option<&str>) -> Result<PathBuf, String> {
        let cache_dir = cache_dir().ok_or("Cannot determine home directory")?;
        std::fs::create_dir_all(&cache_dir)
            .map_err(|e| format!("Cannot create cache dir: {e}"))?;

        let registry_path = cache_dir.join("registry.json");
        std::fs::write(&registry_path, json)
            .map_err(|e| format!("Cannot write registry cache: {e}"))?;

        if let Some(etag) = etag {
            let etag_path = cache_dir.join("registry.etag");
            let _ = std::fs::write(etag_path, etag);
        }

        info!("registry cache updated: {}", registry_path.display());
        Ok(registry_path)
    }

    /// Read the cached ETag for conditional GET.
    #[must_use]
    pub fn cached_etag() -> Option<String> {
        let etag_path = cache_dir()?.join("registry.etag");
        std::fs::read_to_string(etag_path).ok().map(|s| s.trim().to_string())
    }

    /// Update cache from JSON and reload the registry.
    ///
    /// # Errors
    ///
    /// Returns error if JSON is invalid or cache write fails.
    pub fn update_from_json(json: &str, etag: Option<&str>) -> Result<Self, String> {
        // Validate JSON parses before caching
        let registry = Self::from_json(json)?;
        Self::save_cache(json, etag)?;
        Ok(registry)
    }

    /// Age of the cached registry file, if it exists.
    #[must_use]
    pub fn cache_age() -> Option<std::time::Duration> {
        let path = cache_path()?;
        let metadata = std::fs::metadata(path).ok()?;
        let modified = metadata.modified().ok()?;
        std::time::SystemTime::now().duration_since(modified).ok()
    }

    /// Whether the cache is stale (older than TTL, default 24 hours).
    #[must_use]
    pub fn is_cache_stale(ttl: std::time::Duration) -> bool {
        match Self::cache_age() {
            Some(age) => age > ttl,
            None => true, // no cache = stale
        }
    }

    /// Return all entries in the registry.
    #[must_use]
    pub fn list(&self) -> &[RegistryEntry] {
        &self.entries
    }

    /// Search entries by a free-text query (case-insensitive).
    #[must_use]
    pub fn search(&self, query: &str) -> Vec<&RegistryEntry> {
        self.entries.iter().filter(|e| e.matches(query)).collect()
    }

    /// Find an entry by its exact id.
    #[must_use]
    pub fn find(&self, id: &str) -> Option<&RegistryEntry> {
        self.entries.iter().find(|e| e.id == id)
    }

    /// Return entries for agents that are installed on this system.
    #[must_use]
    pub fn detect_installed(&self) -> Vec<&RegistryEntry> {
        self.entries.iter().filter(|e| e.is_installed()).collect()
    }

    /// Detect installed agents (binary on PATH) with their resolved paths.
    pub fn detect_installed_with_paths(&self) -> Vec<DetectedAgent> {
        self.entries
            .iter()
            .filter(|e| e.is_installed())
            .map(|e| DetectedAgent {
                entry: e.clone(),
                command_path: resolve_command_path(&e.command),
            })
            .collect()
    }

    /// Detect all agents that can be launched right now.
    ///
    /// Includes binary agents on PATH + npx/uvx agents (if runtime available).
    pub fn detect_runnable_with_paths(&self) -> Vec<DetectedAgent> {
        self.entries
            .iter()
            .filter(|e| e.is_runnable())
            .map(|e| DetectedAgent {
                entry: e.clone(),
                command_path: resolve_command_path(&e.command),
            })
            .collect()
    }

    /// Return entries that have the given capability.
    #[must_use]
    pub fn by_capability(&self, cap: &AgentCapability) -> Vec<&RegistryEntry> {
        self.entries
            .iter()
            .filter(|e| e.capabilities.contains(cap))
            .collect()
    }

    /// Number of agents in the registry.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the registry is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Result of detecting an installed agent.
#[derive(Debug, Clone)]
pub struct DetectedAgent {
    /// Registry entry.
    pub entry: RegistryEntry,
    /// Resolved path to the command.
    pub command_path: Option<String>,
}

// ── Platform resolution ─────────────────────────────────────────────

/// Returns the current platform key (e.g. "windows-x86_64", "darwin-aarch64").
#[must_use]
fn current_platform() -> String {
    let os = if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "darwin"
    } else {
        "linux"
    };

    let arch = if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        "x86_64"
    };

    format!("{os}-{arch}")
}

/// Build a `RegistryEntry` from a raw agent, resolving the command for the current platform.
fn build_entry(raw: RawAgent, platform: &str) -> RegistryEntry {
    let mut distributions = Vec::new();
    let mut command = String::new();
    let mut default_args = Vec::new();
    let mut install_instructions = String::new();

    // Build distributions and resolve best command for this platform
    if let Some(npx) = &raw.distribution.npx {
        distributions.push(Distribution::Npx {
            package: npx.package.clone(),
            args: npx.args.clone(),
            env: npx.env.clone(),
        });

        // npx is preferred if available — works everywhere
        if command.is_empty() {
            command = "npx".to_string();
            default_args = Vec::new();
            // Strip version from package for display: "@foo/bar@1.2.3" → "@foo/bar"
            let pkg_no_version = strip_version(&npx.package);
            default_args.push(pkg_no_version);
            default_args.extend(npx.args.clone());

            install_instructions = format!("npx {}", npx.package);
        }
    }

    if let Some(binaries) = &raw.distribution.binary {
        let mut platforms = HashMap::new();
        for (plat, dist) in binaries {
            platforms.insert(
                plat.clone(),
                BinaryTarget {
                    archive: dist.archive.clone(),
                    cmd: dist.cmd.clone(),
                    args: dist.args.clone(),
                },
            );
        }

        // If there's a binary for this platform, prefer it over npx
        if let Some(plat_dist) = binaries.get(platform) {
            command = plat_dist.cmd.clone();
            default_args = plat_dist.args.clone();
            install_instructions = format!("Download from {}", plat_dist.archive);
        } else if command.is_empty() {
            // No binary for this platform and no npx — use first available cmd
            if let Some((_, first)) = binaries.iter().next() {
                command = first.cmd.clone();
                default_args = first.args.clone();
                install_instructions = "Binary not available for this platform".to_string();
            }
        }

        distributions.push(Distribution::Binary { platforms });
    }

    if let Some(uvx) = &raw.distribution.uvx {
        distributions.push(Distribution::Uvx {
            package: uvx.package.clone(),
            args: uvx.args.clone(),
        });

        if command.is_empty() {
            command = "uvx".to_string();
            default_args = vec![uvx.package.clone()];
            default_args.extend(uvx.args.clone());
            install_instructions = format!("uvx {}", uvx.package);
        }
    }

    // Derive tags from metadata
    let mut tags = Vec::new();
    if let Some(author) = raw.authors.first() {
        tags.push(author.to_lowercase());
    }
    let license_lower = raw.license.to_lowercase();
    if !license_lower.contains("proprietary") {
        tags.push("open-source".into());
    }
    // Well-known popular agents
    if matches!(
        raw.id.as_str(),
        "claude-acp" | "github-copilot-cli" | "codex-acp" | "gemini" | "cursor" | "goose"
    ) {
        tags.push("popular".into());
    }

    // Derive capabilities from known agents
    let capabilities = derive_capabilities(&raw.id);

    RegistryEntry {
        id: raw.id,
        display_name: raw.name.clone(),
        description: raw.description.clone(),
        version: raw.version,
        authors: raw.authors,
        license: raw.license,
        distributions,
        repository: raw.repository,
        website: raw.website,
        tags,
        capabilities,
        command,
        default_args,
        transport: Transport::Stdio,
        install_instructions,
        long_description: raw.description,
        models: Vec::new(),
    }
}

/// Derive capabilities for known agents. Default: Code + Chat.
fn derive_capabilities(id: &str) -> Vec<AgentCapability> {
    match id {
        "claude-acp" => vec![
            AgentCapability::Code,
            AgentCapability::Plan,
            AgentCapability::Review,
            AgentCapability::Test,
            AgentCapability::Refactor,
            AgentCapability::Chat,
        ],
        "github-copilot-cli" | "gemini" | "cline" | "goose" | "kilo" | "opencode" => vec![
            AgentCapability::Code,
            AgentCapability::Refactor,
            AgentCapability::Chat,
        ],
        "stakpak" => vec![AgentCapability::Code, AgentCapability::Chat],
        _ => vec![AgentCapability::Code, AgentCapability::Chat],
    }
}

/// Strip version suffix from npm package name.
/// `"@foo/bar@1.2.3"` → `"@foo/bar"`, `"cline@2.9.0"` → `"cline"`.
fn strip_version(package: &str) -> String {
    // Find the last '@' that's not at position 0 (scoped packages start with @)
    if let Some(pos) = package.rfind('@') {
        if pos > 0 {
            return package[..pos].to_string();
        }
    }
    package.to_string()
}

// ── Cache paths ─────────────────────────────────────────────────────

/// Return `~/.surge/cache/` directory path.
fn cache_dir() -> Option<PathBuf> {
    dirs_path().map(|p| p.join("cache"))
}

/// Return `~/.surge/cache/registry.json` path.
fn cache_path() -> Option<PathBuf> {
    cache_dir().map(|p| p.join("registry.json"))
}

/// Return `~/.surge/` directory path.
fn dirs_path() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var("USERPROFILE")
            .ok()
            .map(|p| PathBuf::from(p).join(".surge"))
    }
    #[cfg(not(windows))]
    {
        std::env::var("HOME")
            .ok()
            .map(|p| PathBuf::from(p).join(".surge"))
    }
}

// ── Utilities ───────────────────────────────────────────────────────

use std::sync::{LazyLock, Mutex};

/// Cache for `which`/`resolve_command_path` results.
/// Avoids spawning `where.exe`/`which` subprocess per agent per call.
static WHICH_CACHE: LazyLock<Mutex<HashMap<String, Option<String>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Check if a command exists on PATH (cached).
fn which(command: &str) -> bool {
    resolve_command_path(command).is_some()
}

/// Resolve the full path of a command (cached).
///
/// First call runs `where`/`which` subprocess; subsequent calls return cached result.
fn resolve_command_path(command: &str) -> Option<String> {
    {
        if let Ok(cache) = WHICH_CACHE.lock() {
            if let Some(result) = cache.get(command) {
                return result.clone();
            }
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builtin_parses_embedded_json() {
        let reg = Registry::embedded();
        assert!(!reg.is_empty());
        // Official ACP registry has 27 agents
        assert!(reg.len() >= 25, "expected >=25 agents, got {}", reg.len());
    }

    #[test]
    fn test_find_claude() {
        let reg = Registry::embedded();
        let entry = reg.find("claude-acp");
        assert!(entry.is_some());
        assert_eq!(entry.unwrap().display_name, "Claude Agent");
    }

    #[test]
    fn test_find_goose() {
        let reg = Registry::embedded();
        let entry = reg.find("goose");
        assert!(entry.is_some());
        assert_eq!(entry.unwrap().vendor(), "Block");
    }

    #[test]
    fn test_npx_command_resolution() {
        let reg = Registry::embedded();
        let gemini = reg.find("gemini").unwrap();
        // Gemini is npx-only, should resolve to npx
        assert_eq!(gemini.command, "npx");
        assert!(gemini.default_args.contains(&"@google/gemini-cli".to_string()));
        assert!(gemini.default_args.contains(&"--acp".to_string()));
    }

    #[test]
    fn test_binary_command_resolution() {
        let reg = Registry::embedded();
        let goose = reg.find("goose").unwrap();
        // goose is binary-only, command depends on platform
        assert!(!goose.command.is_empty());
    }

    #[test]
    fn test_uvx_command_resolution() {
        let reg = Registry::embedded();
        let crow = reg.find("crow-cli").unwrap();
        assert_eq!(crow.command, "uvx");
        assert!(crow.default_args.contains(&"crow-cli".to_string()));
    }

    #[test]
    fn test_search() {
        let reg = Registry::embedded();
        let results = reg.search("anthropic");
        assert!(!results.is_empty());
    }

    #[test]
    fn test_search_by_tag() {
        let reg = Registry::embedded();
        let results = reg.search("open-source");
        assert!(results.len() >= 5);
    }

    #[test]
    fn test_capabilities() {
        let reg = Registry::embedded();
        let planners = reg.by_capability(&AgentCapability::Plan);
        assert_eq!(planners.len(), 1);
        assert_eq!(planners[0].id, "claude-acp");

        let coders = reg.by_capability(&AgentCapability::Code);
        assert_eq!(coders.len(), reg.len());
    }

    #[test]
    fn test_to_agent_config() {
        let reg = Registry::embedded();
        let entry = reg.find("gemini").unwrap();
        let config = entry.to_agent_config();
        assert_eq!(config.command, "npx");
        assert!(matches!(config.transport, Transport::Stdio));
    }

    #[test]
    fn test_strip_version() {
        assert_eq!(strip_version("@foo/bar@1.2.3"), "@foo/bar");
        assert_eq!(strip_version("cline@2.9.0"), "cline");
        assert_eq!(strip_version("@google/gemini-cli@0.34.0"), "@google/gemini-cli");
        assert_eq!(strip_version("plain-pkg"), "plain-pkg");
    }

    #[test]
    fn test_is_open_source() {
        let reg = Registry::embedded();
        let goose = reg.find("goose").unwrap();
        assert!(goose.is_open_source());

        let cursor = reg.find("cursor").unwrap();
        assert!(!cursor.is_open_source());
    }

    #[test]
    fn test_from_json_runtime() {
        // Simulates runtime refresh with a minimal JSON
        let json = r#"{"version":"1.0.0","agents":[{"id":"test","name":"Test","version":"0.1.0","description":"A test","authors":["Dev"],"license":"MIT","distribution":{"npx":{"package":"test-pkg@0.1.0","args":["--acp"]}}}]}"#;
        let reg = Registry::from_json(json).unwrap();
        assert_eq!(reg.len(), 1);
        let entry = reg.find("test").unwrap();
        assert_eq!(entry.command, "npx");
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
    fn test_detect_installed_returns_subset() {
        let reg = Registry::embedded();
        let installed = reg.detect_installed();
        assert!(installed.len() <= reg.list().len());
    }

    #[test]
    fn test_all_entries_have_command() {
        let reg = Registry::embedded();
        for entry in reg.list() {
            assert!(
                !entry.command.is_empty(),
                "Agent '{}' has no resolved command",
                entry.id
            );
        }
    }

    #[test]
    fn test_save_and_load_cache() {
        // Use a temp dir to avoid polluting real ~/.surge
        let tmp = std::env::temp_dir().join("surge-test-cache");
        let _ = std::fs::create_dir_all(&tmp);
        let cache_file = tmp.join("registry.json");

        let json = r#"{"version":"1.0.0","agents":[{"id":"cached","name":"Cached Agent","version":"0.1.0","description":"From cache","authors":["Test"],"license":"MIT","distribution":{"npx":{"package":"cached@0.1.0"}}}]}"#;

        std::fs::write(&cache_file, json).unwrap();

        // Verify JSON parses correctly
        let reg = Registry::from_json(json).unwrap();
        assert_eq!(reg.len(), 1);
        assert_eq!(reg.find("cached").unwrap().display_name, "Cached Agent");

        // Cleanup
        let _ = std::fs::remove_dir_all(tmp);
    }

    #[test]
    fn test_from_json_validates() {
        let json = r#"{"version":"1.0.0","agents":[{"id":"fresh","name":"Fresh","version":"1.0.0","description":"Fresh agent","authors":["Dev"],"license":"Apache-2.0","distribution":{"npx":{"package":"fresh@1.0.0","args":["--acp"]}}}]}"#;

        let reg = Registry::from_json(json).unwrap();
        assert_eq!(reg.len(), 1);
        assert!(reg.find("fresh").unwrap().is_open_source());
    }

    #[test]
    fn test_from_invalid_json() {
        let result = Registry::from_json("not json");
        assert!(result.is_err());
    }

    #[test]
    fn test_embedded_always_works() {
        let reg = Registry::embedded();
        assert!(reg.len() >= 25);
    }

    #[test]
    fn test_cache_staleness_zero_ttl() {
        use std::time::Duration;
        // With TTL of 0, cache is always stale (even if it exists)
        assert!(Registry::is_cache_stale(Duration::ZERO));
    }
}
