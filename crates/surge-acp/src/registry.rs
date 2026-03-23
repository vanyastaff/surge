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
    /// Longer description with details about the agent.
    #[serde(default)]
    pub long_description: String,
    pub command: String,
    pub default_args: Vec<String>,
    pub transport: Transport,
    pub capabilities: Vec<AgentCapability>,
    /// Models/LLMs this agent can use.
    #[serde(default)]
    pub models: Vec<String>,
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
                description: "Anthropic's autonomous coding agent".into(),
                long_description: "Full-featured agentic coding tool. Edits files, runs commands, searches codebases, manages git — all autonomously. Best-in-class for complex multi-file tasks. Supports extended thinking and tool use.".into(),
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
                models: vec![
                    "Claude Opus 4.6".into(),
                    "Claude Sonnet 4.6".into(),
                    "Claude Opus 4.5".into(),
                    "Claude Sonnet 4.5".into(),
                    "Claude Haiku 4.5".into(),
                ],
                install_instructions: "npm install -g @anthropic-ai/claude-code".into(),
                website: Some("https://claude.ai/claude-code".into()),
                tags: vec!["anthropic".into(), "claude".into(), "ai".into(), "full-featured".into()],
            },
            RegistryEntry {
                id: "copilot-cli".into(),
                display_name: "GitHub Copilot CLI".into(),
                description: "GitHub's AI coding assistant in the terminal".into(),
                long_description: "Code suggestions and completions powered by GitHub Copilot. Integrates with GitHub ecosystem — issues, PRs, repos. Fast completions for common patterns.".into(),
                command: "gh".into(),
                default_args: vec!["copilot".into()],
                transport: Transport::Stdio,
                capabilities: vec![AgentCapability::Code, AgentCapability::Chat],
                models: vec![
                    "GPT-5 mini".into(),
                    "GPT-4.1".into(),
                    "GPT-5.3-Codex".into(),
                    "Claude Opus 4.6".into(),
                    "Claude Sonnet 4.6".into(),
                    "Gemini 3.1 Pro".into(),
                ],
                install_instructions: "gh extension install github/gh-copilot".into(),
                website: Some("https://github.com/features/copilot".into()),
                tags: vec!["github".into(), "copilot".into(), "ai".into()],
            },
            RegistryEntry {
                id: "zed-agent".into(),
                display_name: "Zed Agent".into(),
                description: "Fast AI agent built into Zed editor".into(),
                long_description: "Native AI assistant in the Zed editor. Extremely fast, low-latency completions. Great for quick edits and refactoring. Uses structured tool calls for file operations.".into(),
                command: "zed".into(),
                default_args: vec!["--agent".into()],
                transport: Transport::Stdio,
                capabilities: vec![
                    AgentCapability::Code,
                    AgentCapability::Refactor,
                    AgentCapability::Chat,
                ],
                models: vec![
                    "Claude Sonnet 4.6".into(),
                    "Claude Opus 4.6".into(),
                    "GPT-4.1".into(),
                    "Gemini 2.5 Pro".into(),
                ],
                install_instructions: "Install Zed from https://zed.dev".into(),
                website: Some("https://zed.dev".into()),
                tags: vec!["zed".into(), "editor".into(), "ai".into(), "fast".into()],
            },
            RegistryEntry {
                id: "aider".into(),
                display_name: "Aider".into(),
                description: "Open-source AI pair programmer".into(),
                long_description: "Terminal-based AI pair programming. Works with any LLM provider — OpenAI, Anthropic, local models. Understands git repos, edits multiple files, creates commits automatically.".into(),
                command: "aider".into(),
                default_args: vec!["--no-auto-commits".into()],
                transport: Transport::Stdio,
                capabilities: vec![
                    AgentCapability::Code,
                    AgentCapability::Refactor,
                    AgentCapability::Chat,
                ],
                models: vec![
                    "Claude Opus 4.6".into(),
                    "Claude Sonnet 4.6".into(),
                    "GPT-4.1".into(),
                    "GPT-5".into(),
                    "Gemini 2.5 Pro".into(),
                    "DeepSeek V3".into(),
                    "Qwen3-Coder".into(),
                ],
                install_instructions: "pip install aider-chat".into(),
                website: Some("https://aider.chat".into()),
                tags: vec!["aider".into(), "open-source".into(), "multi-llm".into()],
            },
            RegistryEntry {
                id: "codex-cli".into(),
                display_name: "Codex CLI".into(),
                description: "OpenAI's lightweight coding agent".into(),
                long_description: "Terminal-based coding agent from OpenAI. Sandboxed execution, code generation and editing. Optimized for fast iteration on small-to-medium tasks.".into(),
                command: "codex".into(),
                default_args: vec![],
                transport: Transport::Stdio,
                capabilities: vec![AgentCapability::Code, AgentCapability::Chat],
                models: vec![
                    "o4-mini".into(),
                    "o3".into(),
                    "GPT-4.1".into(),
                ],
                install_instructions: "npm install -g @openai/codex".into(),
                website: Some("https://openai.com".into()),
                tags: vec!["openai".into(), "codex".into(), "ai".into(), "lightweight".into()],
            },
            RegistryEntry {
                id: "gemini-cli".into(),
                display_name: "Gemini CLI".into(),
                description: "Google's AI coding agent with free tier".into(),
                long_description: "Terminal-based coding agent from Google. Free tier with 100 RPD for Gemini 2.5 Pro. Auto-fallback from Pro to Flash when quota exhausted. 1M context window on all models.".into(),
                command: "gemini".into(),
                default_args: vec![],
                transport: Transport::Stdio,
                capabilities: vec![
                    AgentCapability::Code,
                    AgentCapability::Chat,
                    AgentCapability::Refactor,
                ],
                models: vec![
                    "Gemini 2.5 Pro".into(),
                    "Gemini 2.5 Flash".into(),
                    "Gemini 3 Flash".into(),
                    "Gemini 3.1 Flash-Lite".into(),
                ],
                install_instructions: "npm install -g @anthropic-ai/gemini-cli || pip install gemini-cli".into(),
                website: Some("https://ai.google.dev".into()),
                tags: vec!["google".into(), "free".into(), "popular".into()],
            },
            RegistryEntry {
                id: "goose".into(),
                display_name: "Goose".into(),
                description: "Open-source autonomous coding agent by Block".into(),
                long_description: "Extensible autonomous agent from Block (Square). Supports any LLM provider — Anthropic, OpenAI, Google, Ollama. Plugin-based architecture with MCP support. Can run fully local.".into(),
                command: "goose".into(),
                default_args: vec![],
                transport: Transport::Stdio,
                capabilities: vec![
                    AgentCapability::Code,
                    AgentCapability::Chat,
                    AgentCapability::Refactor,
                ],
                models: vec![
                    "Claude Sonnet 4.6".into(),
                    "GPT-4.1".into(),
                    "Gemini 2.5 Pro".into(),
                ],
                install_instructions: "brew install goose || pip install goose-ai".into(),
                website: Some("https://block.github.io/goose".into()),
                tags: vec!["block".into(), "open-source".into(), "popular".into()],
            },
            RegistryEntry {
                id: "cline".into(),
                display_name: "Cline".into(),
                description: "Open-source AI coding agent with multi-provider support".into(),
                long_description: "Autonomous coding agent supporting any LLM provider. Works with Anthropic, OpenAI, Google, Azure, AWS Bedrock, Ollama, and OpenRouter. Can run fully local.".into(),
                command: "cline".into(),
                default_args: vec![],
                transport: Transport::Stdio,
                capabilities: vec![
                    AgentCapability::Code,
                    AgentCapability::Chat,
                ],
                models: vec![
                    "Claude Sonnet 4.6".into(),
                    "GPT-4.1".into(),
                    "Gemini 2.5 Pro".into(),
                ],
                install_instructions: "npm install -g cline".into(),
                website: Some("https://cline.bot".into()),
                tags: vec!["open-source".into()],
            },
            RegistryEntry {
                id: "amp".into(),
                display_name: "Amp".into(),
                description: "Codebase-aware AI agent by Sourcegraph".into(),
                long_description: "Terminal coding agent from Sourcegraph. Leverages Sourcegraph's code intelligence for deep codebase understanding. Free tier available.".into(),
                command: "amp".into(),
                default_args: vec![],
                transport: Transport::Stdio,
                capabilities: vec![
                    AgentCapability::Code,
                    AgentCapability::Chat,
                    AgentCapability::Review,
                ],
                models: vec![
                    "Claude Sonnet 4.6".into(),
                    "Claude Opus 4.6".into(),
                ],
                install_instructions: "npm install -g @anthropic-ai/amp || brew install amp".into(),
                website: Some("https://sourcegraph.com/amp".into()),
                tags: vec!["sourcegraph".into(), "free".into()],
            },
            RegistryEntry {
                id: "devstral".into(),
                display_name: "Devstral".into(),
                description: "Mistral's coding-focused model agent".into(),
                long_description: "Lightweight coding agent from Mistral. Optimized for code generation with small footprint. Runs fully local via Ollama — no API key required.".into(),
                command: "devstral".into(),
                default_args: vec![],
                transport: Transport::Stdio,
                capabilities: vec![
                    AgentCapability::Code,
                    AgentCapability::Chat,
                ],
                models: vec![
                    "Devstral".into(),
                ],
                install_instructions: "ollama pull devstral".into(),
                website: Some("https://mistral.ai".into()),
                tags: vec!["mistral".into(), "free".into(), "open-source".into()],
            },
            RegistryEntry {
                id: "qwen3-coder".into(),
                display_name: "Qwen3-Coder".into(),
                description: "Alibaba's fully local coding agent".into(),
                long_description: "Local-first coding agent from Alibaba. Runs entirely on your machine via Ollama — no internet, no rate limits, completely free. 128K context.".into(),
                command: "qwen3-coder".into(),
                default_args: vec![],
                transport: Transport::Stdio,
                capabilities: vec![
                    AgentCapability::Code,
                    AgentCapability::Chat,
                ],
                models: vec![
                    "Qwen3-Coder".into(),
                    "Qwen3-Coder-Plus".into(),
                ],
                install_instructions: "ollama pull qwen3-coder".into(),
                website: Some("https://qwen.ai".into()),
                tags: vec!["alibaba".into(), "free".into(), "open-source".into()],
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
        assert_eq!(reg.list().len(), 11);
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
        let results = reg.search("open-source");
        assert!(results.len() >= 1);
        assert!(results.iter().any(|e| e.id == "aider"));
    }

    #[test]
    fn test_by_capability() {
        let reg = Registry::builtin();
        let planners = reg.by_capability(&AgentCapability::Plan);
        assert_eq!(planners.len(), 1);
        assert_eq!(planners[0].id, "claude-code");

        let coders = reg.by_capability(&AgentCapability::Code);
        assert_eq!(coders.len(), 11);
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
