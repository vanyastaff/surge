//! ACP Agent Registry — hardcoded catalog of supported agents.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::sync::{LazyLock, Mutex};
use surge_core::config::{AgentConfig, Transport};

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
            Self::Claude => "Anthropic's autonomous coding agent — deepest reasoning, largest context",
            Self::Copilot => "GitHub's multi-model terminal agent with native repo integration",
            Self::Codex => "OpenAI's cloud-native coding agent with sandboxed parallel execution",
            Self::Gemini => "Google's CLI with the most generous free tier — 1M context on all models",
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
        self.entries
            .iter()
            .filter(|e| e.is_installed())
            .map(|e| DetectedAgent {
                entry: e.clone(),
                command_path: resolve_command_path(&e.command),
            })
            .collect()
    }

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
}

/// Result of detecting an installed agent.
#[derive(Debug, Clone)]
pub struct DetectedAgent {
    pub entry: RegistryEntry,
    pub command_path: Option<String>,
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
            default_args: vec![
                "@zed-industries/claude-agent-acp".into(),
            ],
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
            default_args: vec![
                "@github/copilot".into(),
                "--acp".into(),
            ],
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
            default_args: vec![
                "@zed-industries/codex-acp".into(),
            ],
            transport: Transport::Stdio,
            install_instructions: "npx @zed-industries/codex-acp".into(),
            cli_binary: Some("codex".into()),
            website: Some("https://openai.com".into()),
            tags: vec!["openai".into(), "popular".into(), "open-source".into()],
            capabilities: vec![
                AgentCapability::Code,
                AgentCapability::Chat,
            ],
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
            default_args: vec![
                "@google/gemini-cli".into(),
                "--acp".into(),
            ],
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

// ── Utilities ───────────────────────────────────────────────────────

/// Cache for `which`/`resolve_command_path` results.
static WHICH_CACHE: LazyLock<Mutex<HashMap<String, Option<String>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

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
}
