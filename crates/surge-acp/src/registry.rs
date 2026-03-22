//! ACP Agent Registry — built-in catalog of known agents.

use serde::{Deserialize, Serialize};
use std::fmt;
use surge_core::config::{AgentConfig, Transport};

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
    pub id: String,
    pub display_name: String,
    pub description: String,
    pub command: String,
    pub default_args: Vec<String>,
    pub transport: Transport,
    pub capabilities: Vec<AgentCapability>,
    pub install_instructions: String,
    pub website: Option<String>,
    pub tags: Vec<String>,
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

    /// Check if this agent's command is available on the system PATH.
    #[must_use]
    pub fn is_installed(&self) -> bool {
        which(&self.command)
    }

    /// Return `true` if the entry matches a case-insensitive search query.
    #[must_use]
    pub fn matches(&self, query: &str) -> bool {
        let q = query.to_lowercase();
        self.id.to_lowercase().contains(&q)
            || self.display_name.to_lowercase().contains(&q)
            || self.description.to_lowercase().contains(&q)
            || self.tags.iter().any(|t| t.to_lowercase().contains(&q))
    }
}

/// Catalog of known ACP-compatible agents.
#[derive(Debug, Clone)]
pub struct Registry {
    entries: Vec<RegistryEntry>,
}

impl Registry {
    /// Create the built-in registry with well-known agents.
    #[must_use]
    pub fn builtin() -> Self {
        let entries = vec![
            RegistryEntry {
                id: "claude-code".into(),
                display_name: "Claude Code".into(),
                description: "Anthropic's CLI coding agent".into(),
                command: "claude".into(),
                default_args: vec![
                    "--print".into(),
                    "--output-format".into(),
                    "stream-json".into(),
                ],
                transport: Transport::Stdio,
                capabilities: vec![
                    AgentCapability::Code,
                    AgentCapability::Plan,
                    AgentCapability::Review,
                    AgentCapability::Test,
                    AgentCapability::Refactor,
                    AgentCapability::Chat,
                ],
                install_instructions: "npm install -g @anthropic-ai/claude-code".into(),
                website: Some("https://claude.ai".into()),
                tags: vec!["anthropic".into(), "claude".into(), "ai".into()],
            },
            RegistryEntry {
                id: "copilot-cli".into(),
                display_name: "GitHub Copilot CLI".into(),
                description: "GitHub Copilot in the terminal".into(),
                command: "gh".into(),
                default_args: vec!["copilot".into()],
                transport: Transport::Stdio,
                capabilities: vec![AgentCapability::Code, AgentCapability::Chat],
                install_instructions: "gh extension install github/gh-copilot".into(),
                website: Some("https://github.com/features/copilot".into()),
                tags: vec!["github".into(), "copilot".into(), "ai".into()],
            },
            RegistryEntry {
                id: "zed-agent".into(),
                display_name: "Zed Agent".into(),
                description: "Zed editor's built-in coding agent".into(),
                command: "zed".into(),
                default_args: vec!["--agent".into()],
                transport: Transport::Stdio,
                capabilities: vec![
                    AgentCapability::Code,
                    AgentCapability::Refactor,
                    AgentCapability::Chat,
                ],
                install_instructions: "Install Zed from https://zed.dev".into(),
                website: Some("https://zed.dev".into()),
                tags: vec!["zed".into(), "editor".into(), "ai".into()],
            },
            RegistryEntry {
                id: "aider".into(),
                display_name: "Aider".into(),
                description: "AI pair programming in the terminal".into(),
                command: "aider".into(),
                default_args: vec!["--no-auto-commits".into()],
                transport: Transport::Stdio,
                capabilities: vec![
                    AgentCapability::Code,
                    AgentCapability::Refactor,
                    AgentCapability::Chat,
                ],
                install_instructions: "pip install aider-chat".into(),
                website: Some("https://aider.chat".into()),
                tags: vec!["aider".into(), "python".into(), "ai".into()],
            },
            RegistryEntry {
                id: "codex-cli".into(),
                display_name: "Codex CLI".into(),
                description: "OpenAI's Codex CLI agent".into(),
                command: "codex".into(),
                default_args: vec![],
                transport: Transport::Stdio,
                capabilities: vec![AgentCapability::Code, AgentCapability::Chat],
                install_instructions: "npm install -g @openai/codex".into(),
                website: Some("https://openai.com".into()),
                tags: vec!["openai".into(), "codex".into(), "ai".into()],
            },
        ];

        Self { entries }
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

    /// Detect installed agents with their resolved paths.
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

    /// Return entries that have the given capability.
    #[must_use]
    pub fn by_capability(&self, cap: &AgentCapability) -> Vec<&RegistryEntry> {
        self.entries
            .iter()
            .filter(|e| e.capabilities.contains(cap))
            .collect()
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

/// Check if a command exists on PATH.
fn which(command: &str) -> bool {
    use std::process::Command;

    // On Windows, use `where`; on Unix, use `which`
    #[cfg(windows)]
    let result = Command::new("where")
        .arg(command)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();

    #[cfg(not(windows))]
    let result = Command::new("which")
        .arg(command)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();

    result.is_ok_and(|s| s.success())
}

/// Try to resolve the full path of a command.
fn resolve_command_path(command: &str) -> Option<String> {
    use std::process::Command;

    #[cfg(windows)]
    let output = Command::new("where")
        .arg(command)
        .output()
        .ok()?;

    #[cfg(not(windows))]
    let output = Command::new("which")
        .arg(command)
        .output()
        .ok()?;

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
    fn test_builtin_catalog_not_empty() {
        let reg = Registry::builtin();
        assert!(!reg.list().is_empty());
        assert_eq!(reg.list().len(), 5);
    }

    #[test]
    fn test_find_by_id() {
        let reg = Registry::builtin();
        let entry = reg.find("claude-code");
        assert!(entry.is_some());
        assert_eq!(entry.unwrap().display_name, "Claude Code");

        assert!(reg.find("nonexistent").is_none());
    }

    #[test]
    fn test_search() {
        let reg = Registry::builtin();
        let results = reg.search("anthropic");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "claude-code");
    }

    #[test]
    fn test_search_by_tag() {
        let reg = Registry::builtin();
        let results = reg.search("python");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "aider");
    }

    #[test]
    fn test_by_capability() {
        let reg = Registry::builtin();
        let planners = reg.by_capability(&AgentCapability::Plan);
        assert_eq!(planners.len(), 1);
        assert_eq!(planners[0].id, "claude-code");

        let coders = reg.by_capability(&AgentCapability::Code);
        assert_eq!(coders.len(), 5);
    }

    #[test]
    fn test_to_agent_config() {
        let reg = Registry::builtin();
        let entry = reg.find("claude-code").unwrap();
        let config = entry.to_agent_config();

        assert_eq!(config.command, "claude");
        assert_eq!(
            config.args,
            vec!["--print", "--output-format", "stream-json"]
        );
        assert!(matches!(config.transport, Transport::Stdio));
    }

    #[test]
    fn test_search_case_insensitive() {
        let reg = Registry::builtin();
        let results = reg.search("CLAUDE");
        assert!(!results.is_empty());
        assert_eq!(results[0].id, "claude-code");
    }

    #[test]
    fn test_which_finds_git() {
        // git should be available on any dev machine
        assert!(which("git"));
    }

    #[test]
    fn test_which_not_found() {
        assert!(!which("nonexistent_binary_12345"));
    }

    #[test]
    fn test_detect_installed_returns_subset() {
        let reg = Registry::builtin();
        let installed = reg.detect_installed();
        // We can't know exactly what's installed, but it should be <= total
        assert!(installed.len() <= reg.list().len());
    }

    #[test]
    fn test_resolve_command_path_git() {
        let path = resolve_command_path("git");
        assert!(path.is_some());
        assert!(path.unwrap().contains("git"));
    }

    #[test]
    fn test_capability_display() {
        assert_eq!(AgentCapability::Code.to_string(), "code");
        assert_eq!(AgentCapability::Plan.to_string(), "plan");
        assert_eq!(AgentCapability::Review.to_string(), "review");
        assert_eq!(AgentCapability::Test.to_string(), "test");
        assert_eq!(AgentCapability::Refactor.to_string(), "refactor");
        assert_eq!(AgentCapability::Chat.to_string(), "chat");
    }
}
