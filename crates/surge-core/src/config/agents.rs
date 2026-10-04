//! Agents, their launch environment and transport, and routing between them.

use super::*;

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

/// Value of one environment variable passed to a spawned agent process.
///
/// Secret values must not be written into `surge.toml`. The [`Self::Inject`]
/// form stores only the **name** of the variable to read at spawn time — the
/// same convention `[[task_sources]]` (`api_token_env`) and `[telegram]`
/// (`bot_token_env`) already use. [`Self::Literal`] is for non-secret values
/// (an empty `ANTHROPIC_API_KEY=""`, a base URL), never for credentials.
///
/// The wire form is untagged: a bare TOML string is a literal, a table is an
/// injection spec. This keeps the common literal case (`KEY = "value"`)
/// readable while giving injected secrets an explicit shape.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AgentEnvValue {
    /// A value written verbatim into the spawned agent's environment.
    Literal(String),
    /// Read the value from the operator's process environment at spawn time.
    Inject {
        /// Name of the environment variable to read (e.g. `OLLAMA_API_KEY`).
        from: String,
        /// Value used when `from` is unset. `None` means the variable is
        /// required unless `required = false`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        default: Option<String>,
        /// When `true` (the default), a missing `from` variable with no
        /// `default` is a spawn-time error naming the variable — the agent
        /// never launches with a silently absent credential.
        #[serde(default = "default_agent_env_required")]
        required: bool,
    },
}

fn default_agent_env_required() -> bool {
    true
}

/// A file an agent needs in its working directory before it can start.
///
/// Some ACP agents read a project-local settings file at session setup and
/// reject values inherited from the operator's global configuration (most
/// commonly a non-interactive permission mode). Which file, and what it must
/// contain, is a property of the *agent*, not of surge — so it is declared
/// as data on the provider's registry entry (or `[agents.<id>]` in
/// `surge.toml`) and materialised generically by
/// `surge_acp::settings_seed`. Surge has no per-vendor branch for this.
///
/// `path` is relative to the run's worktree; absolute paths and `..` are
/// refused at seed time. An existing file is never clobbered — the
/// operator's explicit project choice wins.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSettingsFile {
    /// Worktree-relative path, e.g. `.claude/settings.json`.
    pub path: String,
    /// Exact file content to write when the file is absent.
    pub content: String,
}

impl AgentSettingsFile {
    /// Constructor for callers outside this crate (`#[non_exhaustive]`).
    #[must_use]
    pub fn new(path: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            content: content.into(),
        }
    }
}

/// Explicit credential sources managed by this configured launch route.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AuthSource {
    /// Effective child environment key; its value is never persisted by capacity routing.
    Env {
        /// Environment variable name explicitly managed for this launch.
        target_key: String,
    },
    /// Provider-owned file, relative to the explicit launch working directory when relative.
    File {
        /// Declared source locator resolved against the explicit launch directory.
        path: std::path::PathBuf,
    },
}
/// Completeness is a declaration about configured sources, not an observed provider account.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfiguredSourceCompleteness {
    /// Operator declares every configured authentication source; inaccessible sources remain opaque.
    CompleteConfiguredSources,
    /// Authentication discovery is incomplete, so historical capacity cannot authorize a skip.
    Opaque,
}
/// Nonsecret operator declaration identifying a distinct configured provider route.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapacityRoute {
    /// Explicit provider family; runtime kind and display tags are not evidence.
    pub provider_family: String,
    /// Nonsecret name distinguishing configured routes within one family.
    pub configured_route: String,
    /// Typed declared source locators, containing no credential values.
    #[serde(default)]
    pub auth_sources: Vec<AuthSource>,
    /// Whether all configured authentication sources have been declared.
    pub completeness: ConfiguredSourceCompleteness,
}

/// Configuration for a single coding agent (Claude Code, Copilot CLI, etc.).
///
/// Specifies the command to spawn, transport layer, MCP servers to inject,
/// and declared capabilities for routing decisions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default = "default_transport")]
    pub transport: Transport,
    /// MCP servers to pass to the agent at startup.
    ///
    /// If non-empty, Surge serialises these to a temporary JSON file and sets
    /// the agent-specific environment variable (e.g. `CLAUDE_MCP_CONFIG` for
    /// Claude Code) before spawning the process.
    #[serde(default)]
    pub mcp_servers: Vec<McpServerConfig>,
    /// Capabilities this agent supports.
    #[serde(default)]
    pub capabilities: Vec<AgentCapability>,
    /// Environment variables to set on the spawned agent process, keyed by
    /// variable name. Values are either literals or `from`-injections of the
    /// operator's environment (see [`AgentEnvValue`]).
    ///
    /// Read at spawn time by `surge-acp`'s env resolver — never by
    /// `surge-core`, which is I/O-free. This is the per-agent half of the
    /// config surface; a builtin registry entry carries the same shape so a
    /// registry-launched runtime reaches the same resolution.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub env: std::collections::BTreeMap<String, AgentEnvValue>,
    /// Files to materialise in the run's worktree before the agent starts
    /// (see [`AgentSettingsFile`]). Data, so a new provider needs no code.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub settings_files: Vec<AgentSettingsFile>,
    /// Explicit capacity identity; absent entries remain opaque.
    #[serde(default)]
    pub capacity_route: Option<CapacityRoute>,
}

impl AgentConfig {
    /// Validate the agent configuration.
    pub(super) fn validate(&self, agent_name: &str) -> Result<(), crate::SurgeError> {
        // Validate command is not empty
        if self.command.trim().is_empty() {
            return Err(crate::SurgeError::Config(format!(
                "Agent '{}' has empty command. Command must be a non-empty string",
                agent_name
            )));
        }

        // Validate TCP transport has non-empty host
        if let Transport::Tcp { host, port } = &self.transport {
            if host.trim().is_empty() {
                return Err(crate::SurgeError::Config(format!(
                    "Agent '{}' TCP transport has empty host. Host must be a non-empty string",
                    agent_name
                )));
            }
            if *port == 0 {
                return Err(crate::SurgeError::Config(format!(
                    "Agent '{}' TCP transport has invalid port 0. Port must be between 1 and 65535",
                    agent_name
                )));
            }
        }

        if let Transport::WebSocket { .. } = &self.transport {
            return Err(crate::SurgeError::Config(
                "WebSocket transport not yet supported".to_string(),
            ));
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Transport {
    #[default]
    Stdio,
    Tcp {
        host: String,
        port: u16,
    },
    /// WebSocket transport for remote agents (reserved, not yet implemented).
    #[serde(rename = "ws")]
    WebSocket {
        url: String,
    },
}

fn default_transport() -> Transport {
    Transport::Stdio
}

/// Strategy for routing tasks to agents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum RoutingStrategy {
    /// Use the default agent for all tasks.
    #[default]
    Default,
    /// Route based on task complexity.
    Complexity,
    /// Round-robin across available agents.
    RoundRobin,
}

/// Configuration for agent routing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingConfig {
    /// Routing strategy.
    #[serde(default)]
    pub strategy: RoutingStrategy,
    /// Per-complexity agent preferences (e.g. {"complex": "claude"}).
    #[serde(default)]
    pub agent_preferences: HashMap<String, String>,
}

impl Default for RoutingConfig {
    fn default() -> Self {
        Self {
            strategy: RoutingStrategy::Default,
            agent_preferences: HashMap::new(),
        }
    }
}
