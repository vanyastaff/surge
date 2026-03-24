//! Surge configuration.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

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

/// Pricing information for agent cost estimation.
///
/// Used to track and estimate costs for agent operations based on token usage.
/// All costs are per million tokens.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PricingInfo {
    /// Cost per million input tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_cost_per_million_tokens: Option<f64>,
    /// Cost per million output tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_cost_per_million_tokens: Option<f64>,
    /// Currency code (default: "USD").
    #[serde(default = "default_currency")]
    pub currency: String,
}

fn default_currency() -> String {
    "USD".to_string()
}

impl Default for PricingInfo {
    fn default() -> Self {
        Self {
            input_cost_per_million_tokens: None,
            output_cost_per_million_tokens: None,
            currency: default_currency(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SurgeConfig {
    pub default_agent: String,
    #[serde(default)]
    pub agents: HashMap<String, AgentConfig>,
    #[serde(default)]
    pub pipeline: PipelineConfig,
    #[serde(default)]
    pub routing: RoutingConfig,
    #[serde(default)]
    pub cleanup: CleanupPolicy,
    #[serde(default)]
    pub ide: IdeConfig,
    #[serde(default)]
    pub resilience: ResilienceConfig,
    #[serde(default)]
    pub log: LogConfig,
}

/// Configuration for a single MCP (Model Context Protocol) server passed to an agent.
///
/// When non-empty, Surge writes these servers to a temporary JSON config file and
/// hands the path to the agent via an agent-specific environment variable so the
/// agent can forward them to its underlying model.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct McpServerConfig {
    /// Identifier used as the key in the `mcpServers` JSON object.
    pub name: String,
    /// Command that runs the MCP server process.
    pub command: String,
    /// Arguments passed to the MCP server command.
    #[serde(default)]
    pub args: Vec<String>,
    /// Extra environment variables for the MCP server process.
    #[serde(default)]
    pub env: HashMap<String, String>,
}

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
}

impl AgentConfig {
    /// Validate the agent configuration.
    fn validate(&self, agent_name: &str) -> Result<(), crate::SurgeError> {
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineConfig {
    #[serde(default = "default_max_qa_iterations")]
    pub max_qa_iterations: u32,
    #[serde(default = "default_max_parallel")]
    pub max_parallel: usize,
    #[serde(default)]
    pub gates: GateConfig,
    /// Stop pipeline if estimated cost exceeds this (USD). None = unlimited.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_cost_usd: Option<f64>,
    /// Stop pipeline if total tokens exceed this. None = unlimited.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            max_qa_iterations: default_max_qa_iterations(),
            max_parallel: default_max_parallel(),
            gates: GateConfig::default(),
            max_cost_usd: None,
            max_tokens: None,
        }
    }
}

impl PipelineConfig {
    /// Validate the pipeline configuration.
    fn validate(&self) -> Result<(), crate::SurgeError> {
        // Validate max_qa_iterations is positive
        if self.max_qa_iterations == 0 {
            return Err(crate::SurgeError::Config(
                "pipeline.max_qa_iterations must be greater than 0".to_string(),
            ));
        }

        // Validate max_parallel is positive
        if self.max_parallel == 0 {
            return Err(crate::SurgeError::Config(
                "pipeline.max_parallel must be greater than 0".to_string(),
            ));
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateConfig {
    #[serde(default = "default_true")]
    pub after_spec: bool,
    #[serde(default = "default_true")]
    pub after_plan: bool,
    #[serde(default)]
    pub after_each_subtask: bool,
    #[serde(default = "default_true")]
    pub after_qa: bool,
}

impl Default for GateConfig {
    fn default() -> Self {
        Self {
            after_spec: true,
            after_plan: true,
            after_each_subtask: false,
            after_qa: true,
        }
    }
}

fn default_max_qa_iterations() -> u32 {
    10
}
fn default_max_parallel() -> usize {
    3
}
fn default_true() -> bool {
    true
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

/// Policy for cleaning up git worktrees and branches.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CleanupPolicy {
    /// Remove worktrees when task completes.
    #[serde(default = "default_true")]
    pub remove_worktrees_on_complete: bool,
    /// Days to keep merged branches before cleanup.
    #[serde(default = "default_keep_branches_days")]
    pub keep_branches_days: u32,
}

impl Default for CleanupPolicy {
    fn default() -> Self {
        Self {
            remove_worktrees_on_complete: true,
            keep_branches_days: default_keep_branches_days(),
        }
    }
}

fn default_keep_branches_days() -> u32 {
    7
}

/// IDE integration configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct IdeConfig {
    /// Editor name (e.g. "vscode", "rustrover", "zed").
    #[serde(default)]
    pub editor: Option<String>,
    /// Command to open a file: substitutes `{path}` and `{line}`.
    /// Auto-detected from `editor` if not set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open_file_cmd: Option<String>,
    /// Open worktree in IDE automatically after spec starts executing.
    #[serde(default)]
    pub auto_open_worktree: bool,
}

/// Logging configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogConfig {
    /// Log level: error, warn, info, debug, trace.
    #[serde(default = "default_log_level")]
    pub level: String,
    /// Write logs to this file in addition to stderr.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<std::path::PathBuf>,
    /// Max log file size in MB before rotation.
    #[serde(default = "default_log_max_mb")]
    pub max_size_mb: u64,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            level: default_log_level(),
            file: None,
            max_size_mb: default_log_max_mb(),
        }
    }
}

fn default_log_level() -> String {
    "info".to_string()
}
fn default_log_max_mb() -> u64 {
    50
}

/// Backoff strategy for retry delays.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum BackoffStrategy {
    /// Fixed delay between retries.
    Linear,
    /// Exponentially increasing delay (delay *= 2 each retry).
    #[default]
    Exponential,
    /// Exponential backoff with random jitter to avoid thundering herd.
    #[serde(rename = "exponential_jitter")]
    ExponentialWithJitter,
}

/// Retry policy configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryPolicy {
    /// Maximum number of retry attempts.
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
    /// Initial delay before first retry (milliseconds).
    #[serde(default = "default_initial_delay_ms")]
    pub initial_delay_ms: u64,
    /// Maximum delay between retries (milliseconds).
    #[serde(default = "default_max_delay_ms")]
    pub max_delay_ms: u64,
    /// Backoff strategy for calculating delays.
    #[serde(default)]
    pub backoff_strategy: BackoffStrategy,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: default_max_retries(),
            initial_delay_ms: default_initial_delay_ms(),
            max_delay_ms: default_max_delay_ms(),
            backoff_strategy: BackoffStrategy::default(),
        }
    }
}

fn default_max_retries() -> u32 {
    3
}
fn default_initial_delay_ms() -> u64 {
    1000
}
fn default_max_delay_ms() -> u64 {
    60000
}

/// Resilience configuration for agent connections.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResilienceConfig {
    /// Timeout for spawning and ACP-initializing an agent process (seconds).
    #[serde(default = "default_connect_timeout_secs")]
    pub connect_timeout_secs: u64,
    /// Timeout for a single `new_session` ACP call (seconds).
    #[serde(default = "default_session_timeout_secs")]
    pub session_timeout_secs: u64,
    /// Timeout for a single `prompt` ACP call (seconds).
    #[serde(default = "default_prompt_timeout_secs")]
    pub prompt_timeout_secs: u64,
    /// How many times to retry a failed prompt before giving up.
    #[serde(default = "default_prompt_retries")]
    pub prompt_retries: u32,
    /// Seconds to wait for a process to exit cleanly before SIGKILL.
    #[serde(default = "default_shutdown_grace_secs")]
    pub shutdown_grace_secs: u64,
    /// Retry policy configuration with backoff strategies.
    #[serde(default)]
    pub retry_policy: RetryPolicy,
    /// Number of consecutive failures before circuit breaker trips.
    #[serde(default = "default_circuit_breaker_threshold")]
    pub circuit_breaker_threshold: u32,
    /// If true, auth failures (401) fail immediately without retry.
    #[serde(default = "default_auth_failure_immediate_fail")]
    pub auth_failure_immediate_fail: bool,
}

impl Default for ResilienceConfig {
    fn default() -> Self {
        Self {
            connect_timeout_secs: default_connect_timeout_secs(),
            session_timeout_secs: default_session_timeout_secs(),
            prompt_timeout_secs: default_prompt_timeout_secs(),
            prompt_retries: default_prompt_retries(),
            shutdown_grace_secs: default_shutdown_grace_secs(),
            retry_policy: RetryPolicy::default(),
            circuit_breaker_threshold: default_circuit_breaker_threshold(),
            auth_failure_immediate_fail: default_auth_failure_immediate_fail(),
        }
    }
}

fn default_connect_timeout_secs() -> u64 {
    120
}
fn default_session_timeout_secs() -> u64 {
    10
}
fn default_prompt_timeout_secs() -> u64 {
    300
}
fn default_prompt_retries() -> u32 {
    1
}
fn default_shutdown_grace_secs() -> u64 {
    5
}
fn default_circuit_breaker_threshold() -> u32 {
    5
}
fn default_auth_failure_immediate_fail() -> bool {
    true
}

impl Default for SurgeConfig {
    fn default() -> Self {
        Self {
            default_agent: "claude-acp".to_string(),
            agents: HashMap::new(),
            pipeline: PipelineConfig::default(),
            routing: RoutingConfig::default(),
            cleanup: CleanupPolicy::default(),
            ide: IdeConfig::default(),
            resilience: ResilienceConfig::default(),
            log: LogConfig::default(),
        }
    }
}

impl SurgeConfig {
    /// Load config from a TOML file at the given path.
    pub fn load(path: &PathBuf) -> Result<Self, crate::SurgeError> {
        let content = std::fs::read_to_string(path).map_err(|e| {
            crate::SurgeError::Config(format!("Failed to read {}: {e}", path.display()))
        })?;
        let config: Self = toml::from_str(&content).map_err(|e| {
            crate::SurgeError::Config(format!("Failed to parse {}: {e}", path.display()))
        })?;
        config.validate()?;
        Ok(config)
    }

    /// Validate the configuration and return helpful error messages.
    pub fn validate(&self) -> Result<(), crate::SurgeError> {
        // Validate default_agent exists in agents map (when agents are configured)
        if !self.agents.is_empty() && !self.agents.contains_key(&self.default_agent) {
            return Err(crate::SurgeError::Config(format!(
                "default_agent '{}' not found in agents. Available agents: {}",
                self.default_agent,
                self.agents
                    .keys()
                    .map(|k| k.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }

        // Validate each agent configuration
        for (name, agent) in &self.agents {
            agent.validate(name)?;
        }

        // Validate pipeline configuration
        self.pipeline.validate()?;

        // Validate log level
        const VALID_LEVELS: &[&str] = &["error", "warn", "info", "debug", "trace"];
        if !VALID_LEVELS.contains(&self.log.level.as_str()) {
            return Err(crate::SurgeError::Config(format!(
                "log.level '{}' is invalid. Must be one of: {}",
                self.log.level,
                VALID_LEVELS.join(", ")
            )));
        }

        Ok(())
    }

    /// Discover surge.toml by searching current directory and parent directories.
    /// Returns a default configuration if no file is found.
    pub fn discover() -> Result<Self, crate::SurgeError> {
        let start_dir = std::env::current_dir().map_err(|e| {
            crate::SurgeError::Config(format!("Failed to get current directory: {e}"))
        })?;

        match Self::find_config_file(&start_dir) {
            Ok(config_path) => Self::load(&config_path),
            Err(_) => Ok(Self::default()),
        }
    }

    /// Apply environment variable overrides to the configuration.
    /// Environment variables with the SURGE_* prefix override config values:
    /// - SURGE_DEFAULT_AGENT
    /// - SURGE_MAX_QA_ITERATIONS
    /// - SURGE_MAX_PARALLEL
    /// - SURGE_GATE_AFTER_SPEC
    /// - SURGE_GATE_AFTER_PLAN
    /// - SURGE_GATE_AFTER_EACH_SUBTASK
    /// - SURGE_GATE_AFTER_QA
    pub fn apply_env_overrides(&mut self) {
        // Override default_agent
        if let Ok(value) = std::env::var("SURGE_DEFAULT_AGENT") {
            self.default_agent = value;
        }

        // Override pipeline.max_qa_iterations
        if let Ok(value) = std::env::var("SURGE_MAX_QA_ITERATIONS")
            && let Ok(parsed) = value.parse::<u32>()
        {
            self.pipeline.max_qa_iterations = parsed;
        }

        // Override pipeline.max_parallel
        if let Ok(value) = std::env::var("SURGE_MAX_PARALLEL")
            && let Ok(parsed) = value.parse::<usize>()
        {
            self.pipeline.max_parallel = parsed;
        }

        // Override pipeline.gates.after_spec
        if let Ok(value) = std::env::var("SURGE_GATE_AFTER_SPEC")
            && let Ok(parsed) = value.parse::<bool>()
        {
            self.pipeline.gates.after_spec = parsed;
        }

        // Override pipeline.gates.after_plan
        if let Ok(value) = std::env::var("SURGE_GATE_AFTER_PLAN")
            && let Ok(parsed) = value.parse::<bool>()
        {
            self.pipeline.gates.after_plan = parsed;
        }

        // Override pipeline.gates.after_each_subtask
        if let Ok(value) = std::env::var("SURGE_GATE_AFTER_EACH_SUBTASK")
            && let Ok(parsed) = value.parse::<bool>()
        {
            self.pipeline.gates.after_each_subtask = parsed;
        }

        // Override pipeline.gates.after_qa
        if let Ok(value) = std::env::var("SURGE_GATE_AFTER_QA")
            && let Ok(parsed) = value.parse::<bool>()
        {
            self.pipeline.gates.after_qa = parsed;
        }
    }

    /// Load config by discovering surge.toml, or return default if not found.
    /// This combines discovery and default fallback in a single convenient method.
    pub fn load_or_default() -> Result<Self, crate::SurgeError> {
        Self::discover()
    }

    /// Find surge.toml by walking up from the given directory.
    fn find_config_file(start_dir: &Path) -> Result<PathBuf, crate::SurgeError> {
        let mut current = start_dir;

        loop {
            let candidate = current.join("surge.toml");
            if candidate.exists() {
                return Ok(candidate);
            }

            // Move to parent directory
            match current.parent() {
                Some(parent) => current = parent,
                None => {
                    return Err(crate::SurgeError::Config(format!(
                        "surge.toml not found in {} or any parent directory",
                        start_dir.display()
                    )));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_config_discovery() {
        // Create a temporary directory structure
        let temp_dir = std::env::temp_dir().join("surge_test_discovery");
        let _ = fs::remove_dir_all(&temp_dir); // Clean up any previous test
        fs::create_dir_all(&temp_dir).unwrap();

        let nested_dir = temp_dir.join("subdir").join("nested");
        fs::create_dir_all(&nested_dir).unwrap();

        // Create a surge.toml in the temp_dir
        let config_path = temp_dir.join("surge.toml");
        fs::write(
            &config_path,
            r#"
default_agent = "test-agent"

[agents.test-agent]
command = "test"
"#,
        )
        .unwrap();

        // Test finding from nested directory
        let found_path = SurgeConfig::find_config_file(&nested_dir).unwrap();
        assert_eq!(found_path, config_path);

        // Test finding from the directory containing surge.toml
        let found_path = SurgeConfig::find_config_file(&temp_dir).unwrap();
        assert_eq!(found_path, config_path);

        // Test error when not found
        let non_existent_dir = std::env::temp_dir().join("surge_test_no_config");
        fs::create_dir_all(&non_existent_dir).unwrap();
        let result = SurgeConfig::find_config_file(&non_existent_dir);
        assert!(result.is_err());

        // Clean up
        let _ = fs::remove_dir_all(&temp_dir);
        let _ = fs::remove_dir_all(&non_existent_dir);
    }

    #[test]
    fn test_default_config() {
        // Test that Default provides sensible values
        let config = SurgeConfig::default();

        assert_eq!(config.default_agent, "claude-acp");
        assert!(config.agents.is_empty());
        assert_eq!(config.pipeline.max_qa_iterations, 10);
        assert_eq!(config.pipeline.max_parallel, 3);
        assert!(config.pipeline.gates.after_spec);
        assert!(config.pipeline.gates.after_plan);
        assert!(!config.pipeline.gates.after_each_subtask);
        assert!(config.pipeline.gates.after_qa);
    }

    #[test]
    fn test_load_or_default() {
        // Create a temporary directory structure
        let temp_dir = std::env::temp_dir().join("surge_test_load_or_default");
        let _ = fs::remove_dir_all(&temp_dir); // Clean up any previous test
        fs::create_dir_all(&temp_dir).unwrap();

        // Test 1: When surge.toml exists, it should load it
        let config_path = temp_dir.join("surge.toml");
        fs::write(
            &config_path,
            r#"
default_agent = "custom-agent"

[pipeline]
max_qa_iterations = 5
max_parallel = 2
"#,
        )
        .unwrap();

        // Change to the temp directory to test load_or_default
        let original_dir = std::env::current_dir().unwrap();
        std::env::set_current_dir(&temp_dir).unwrap();

        let config = SurgeConfig::load_or_default().unwrap();
        assert_eq!(config.default_agent, "custom-agent");
        assert_eq!(config.pipeline.max_qa_iterations, 5);
        assert_eq!(config.pipeline.max_parallel, 2);

        // Test 2: When no surge.toml exists, it should return default
        let no_config_dir = std::env::temp_dir().join("surge_test_load_or_default_no_config");
        let _ = fs::remove_dir_all(&no_config_dir);
        fs::create_dir_all(&no_config_dir).unwrap();
        std::env::set_current_dir(&no_config_dir).unwrap();

        let config = SurgeConfig::load_or_default().unwrap();
        assert_eq!(config.default_agent, "claude-acp");
        assert_eq!(config.pipeline.max_qa_iterations, 10);
        assert_eq!(config.pipeline.max_parallel, 3);

        // Restore original directory
        std::env::set_current_dir(&original_dir).unwrap();

        // Clean up
        let _ = fs::remove_dir_all(&temp_dir);
        let _ = fs::remove_dir_all(&no_config_dir);
    }

    #[test]
    fn test_config_validation() {
        // Test 1: Valid configuration passes validation
        let mut valid_config = SurgeConfig::default();
        valid_config.agents.insert(
            "claude-acp".to_string(),
            AgentConfig {
                command: "claude".to_string(),
                args: vec![],
                transport: Transport::Stdio,
                mcp_servers: vec![],
                capabilities: vec![],
            },
        );
        assert!(valid_config.validate().is_ok());

        // Test 2: default_agent not in agents map fails
        let mut invalid_config = SurgeConfig {
            default_agent: "nonexistent".to_string(),
            ..Default::default()
        };
        invalid_config.agents.insert(
            "other-agent".to_string(),
            AgentConfig {
                command: "other".to_string(),
                args: vec![],
                transport: Transport::Stdio,
                mcp_servers: vec![],
                capabilities: vec![],
            },
        );
        let result = invalid_config.validate();
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("default_agent 'nonexistent' not found"));
        assert!(err_msg.contains("Available agents: other-agent"));

        // Test 3: Empty command fails
        let mut config_empty_cmd = SurgeConfig {
            default_agent: "bad-agent".to_string(),
            ..Default::default()
        };
        config_empty_cmd.agents.insert(
            "bad-agent".to_string(),
            AgentConfig {
                command: "".to_string(),
                args: vec![],
                transport: Transport::Stdio,
                mcp_servers: vec![],
                capabilities: vec![],
            },
        );
        let result = config_empty_cmd.validate();
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Agent 'bad-agent' has empty command"));

        // Test 4: Empty TCP host fails
        let mut config_empty_host = SurgeConfig {
            default_agent: "tcp-agent".to_string(),
            ..Default::default()
        };
        config_empty_host.agents.insert(
            "tcp-agent".to_string(),
            AgentConfig {
                command: "test".to_string(),
                args: vec![],
                transport: Transport::Tcp {
                    host: "".to_string(),
                    port: 8080,
                },
                mcp_servers: vec![],
                capabilities: vec![],
            },
        );
        let result = config_empty_host.validate();
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Agent 'tcp-agent' TCP transport has empty host"));

        // Test 5: Invalid TCP port 0 fails
        let mut config_invalid_port = SurgeConfig {
            default_agent: "tcp-agent".to_string(),
            ..Default::default()
        };
        config_invalid_port.agents.insert(
            "tcp-agent".to_string(),
            AgentConfig {
                command: "test".to_string(),
                args: vec![],
                transport: Transport::Tcp {
                    host: "localhost".to_string(),
                    port: 0,
                },
                mcp_servers: vec![],
                capabilities: vec![],
            },
        );
        let result = config_invalid_port.validate();
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Agent 'tcp-agent' TCP transport has invalid port 0"));

        // Test 6: max_qa_iterations = 0 fails
        let mut config_zero_qa = SurgeConfig::default();
        config_zero_qa.pipeline.max_qa_iterations = 0;
        let result = config_zero_qa.validate();
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("pipeline.max_qa_iterations must be greater than 0"));

        // Test 7: max_parallel = 0 fails
        let mut config_zero_parallel = SurgeConfig::default();
        config_zero_parallel.pipeline.max_parallel = 0;
        let result = config_zero_parallel.validate();
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("pipeline.max_parallel must be greater than 0"));

        // Test 8: Valid TCP configuration passes
        let mut config_valid_tcp = SurgeConfig {
            default_agent: "tcp-agent".to_string(),
            ..Default::default()
        };
        config_valid_tcp.agents.insert(
            "tcp-agent".to_string(),
            AgentConfig {
                command: "test".to_string(),
                args: vec![],
                transport: Transport::Tcp {
                    host: "localhost".to_string(),
                    port: 8080,
                },
                mcp_servers: vec![],
                capabilities: vec![],
            },
        );
        assert!(config_valid_tcp.validate().is_ok());

        // Test 9: Empty agents map with default_agent is OK (default config scenario)
        let default_config = SurgeConfig::default();
        assert!(default_config.validate().is_ok());
    }

    #[test]
    fn test_env_overrides() {
        // Test environment variable overrides
        // Set environment variables
        unsafe {
            std::env::set_var("SURGE_DEFAULT_AGENT", "custom-agent");
            std::env::set_var("SURGE_MAX_QA_ITERATIONS", "20");
            std::env::set_var("SURGE_MAX_PARALLEL", "5");
            std::env::set_var("SURGE_GATE_AFTER_SPEC", "false");
            std::env::set_var("SURGE_GATE_AFTER_PLAN", "false");
            std::env::set_var("SURGE_GATE_AFTER_EACH_SUBTASK", "true");
            std::env::set_var("SURGE_GATE_AFTER_QA", "false");
        }

        // Create config with defaults
        let mut config = SurgeConfig::default();

        // Verify defaults before override
        assert_eq!(config.default_agent, "claude-acp");
        assert_eq!(config.pipeline.max_qa_iterations, 10);
        assert_eq!(config.pipeline.max_parallel, 3);
        assert!(config.pipeline.gates.after_spec);
        assert!(config.pipeline.gates.after_plan);
        assert!(!config.pipeline.gates.after_each_subtask);
        assert!(config.pipeline.gates.after_qa);

        // Apply environment overrides
        config.apply_env_overrides();

        // Verify overrides were applied
        assert_eq!(config.default_agent, "custom-agent");
        assert_eq!(config.pipeline.max_qa_iterations, 20);
        assert_eq!(config.pipeline.max_parallel, 5);
        assert!(!config.pipeline.gates.after_spec);
        assert!(!config.pipeline.gates.after_plan);
        assert!(config.pipeline.gates.after_each_subtask);
        assert!(!config.pipeline.gates.after_qa);

        // Clean up environment variables
        unsafe {
            std::env::remove_var("SURGE_DEFAULT_AGENT");
            std::env::remove_var("SURGE_MAX_QA_ITERATIONS");
            std::env::remove_var("SURGE_MAX_PARALLEL");
            std::env::remove_var("SURGE_GATE_AFTER_SPEC");
            std::env::remove_var("SURGE_GATE_AFTER_PLAN");
            std::env::remove_var("SURGE_GATE_AFTER_EACH_SUBTASK");
            std::env::remove_var("SURGE_GATE_AFTER_QA");
        }

        // Test that invalid values are ignored
        unsafe {
            std::env::set_var("SURGE_MAX_QA_ITERATIONS", "invalid");
            std::env::set_var("SURGE_MAX_PARALLEL", "not-a-number");
            std::env::set_var("SURGE_GATE_AFTER_SPEC", "not-a-bool");
        }

        let mut config2 = SurgeConfig::default();
        config2.apply_env_overrides();

        // Verify invalid values were ignored (defaults remain)
        assert_eq!(config2.pipeline.max_qa_iterations, 10);
        assert_eq!(config2.pipeline.max_parallel, 3);
        assert!(config2.pipeline.gates.after_spec);

        // Clean up
        unsafe {
            std::env::remove_var("SURGE_MAX_QA_ITERATIONS");
            std::env::remove_var("SURGE_MAX_PARALLEL");
            std::env::remove_var("SURGE_GATE_AFTER_SPEC");
        }
    }

    #[test]
    fn test_toml_serialization_minimal() {
        // Test minimal config serialization/deserialization
        let toml_str = r#"
default_agent = "test-agent"
"#;
        let config: SurgeConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.default_agent, "test-agent");
        assert!(config.agents.is_empty());
        assert_eq!(config.pipeline.max_qa_iterations, 10);
        assert_eq!(config.pipeline.max_parallel, 3);
    }

    #[test]
    fn test_toml_serialization_complete() {
        // Test complete config with all fields
        let toml_str = r#"
default_agent = "claude-acp"

[agents.claude-acp]
command = "claude"
args = ["--stdio"]
transport = "stdio"

[agents.copilot]
command = "gh"
args = ["copilot"]

[agents.remote]
command = "nc"
transport = { tcp = { host = "localhost", port = 9000 } }

[pipeline]
max_qa_iterations = 5
max_parallel = 2

[pipeline.gates]
after_spec = false
after_plan = true
after_each_subtask = true
after_qa = false
"#;
        let config: SurgeConfig = toml::from_str(toml_str).unwrap();

        assert_eq!(config.default_agent, "claude-acp");
        assert_eq!(config.agents.len(), 3);

        // Check claude-code agent
        let claude = config.agents.get("claude-acp").unwrap();
        assert_eq!(claude.command, "claude");
        assert_eq!(claude.args, vec!["--stdio"]);
        assert!(matches!(claude.transport, Transport::Stdio));

        // Check copilot agent
        let copilot = config.agents.get("copilot").unwrap();
        assert_eq!(copilot.command, "gh");
        assert_eq!(copilot.args, vec!["copilot"]);

        // Check remote agent with TCP transport
        let remote = config.agents.get("remote").unwrap();
        assert_eq!(remote.command, "nc");
        if let Transport::Tcp { host, port } = &remote.transport {
            assert_eq!(host, "localhost");
            assert_eq!(*port, 9000);
        } else {
            panic!("Expected TCP transport");
        }

        // Check pipeline config
        assert_eq!(config.pipeline.max_qa_iterations, 5);
        assert_eq!(config.pipeline.max_parallel, 2);

        // Check gates
        assert!(!config.pipeline.gates.after_spec);
        assert!(config.pipeline.gates.after_plan);
        assert!(config.pipeline.gates.after_each_subtask);
        assert!(!config.pipeline.gates.after_qa);
    }

    #[test]
    fn test_toml_deserialization_malformed() {
        // Test that malformed TOML returns error
        let bad_toml = r#"
default_agent =
command = "test"
"#;
        let result: Result<SurgeConfig, _> = toml::from_str(bad_toml);
        assert!(result.is_err());
    }

    #[test]
    fn test_load_config_file_success() {
        // Test loading a valid config file
        let temp_dir = std::env::temp_dir().join("surge_test_load_config");
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        let config_path = temp_dir.join("surge.toml");
        fs::write(
            &config_path,
            r#"
default_agent = "my-agent"

[agents.my-agent]
command = "agent-binary"
args = ["--verbose"]

[pipeline]
max_qa_iterations = 15
max_parallel = 4
"#,
        )
        .unwrap();

        let config = SurgeConfig::load(&config_path).unwrap();
        assert_eq!(config.default_agent, "my-agent");
        assert_eq!(config.pipeline.max_qa_iterations, 15);
        assert_eq!(config.pipeline.max_parallel, 4);

        let agent = config.agents.get("my-agent").unwrap();
        assert_eq!(agent.command, "agent-binary");
        assert_eq!(agent.args, vec!["--verbose"]);

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_load_config_file_not_found() {
        // Test loading a non-existent file
        let path = PathBuf::from("/nonexistent/path/surge.toml");
        let result = SurgeConfig::load(&path);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Failed to read"));
    }

    #[test]
    fn test_load_config_file_invalid_toml() {
        // Test loading a file with invalid TOML
        let temp_dir = std::env::temp_dir().join("surge_test_invalid_toml");
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        let config_path = temp_dir.join("surge.toml");
        fs::write(&config_path, "this is not valid TOML {{{").unwrap();

        let result = SurgeConfig::load(&config_path);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Failed to parse"));

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_load_config_file_invalid_config() {
        // Test loading a valid TOML but invalid config (validation fails)
        let temp_dir = std::env::temp_dir().join("surge_test_invalid_config");
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        let config_path = temp_dir.join("surge.toml");
        fs::write(
            &config_path,
            r#"
default_agent = "missing-agent"

[agents.other-agent]
command = "test"
"#,
        )
        .unwrap();

        let result = SurgeConfig::load(&config_path);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("default_agent 'missing-agent' not found"));

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_transport_stdio_default() {
        // Test that Transport::Stdio is the default
        let transport = Transport::default();
        assert!(matches!(transport, Transport::Stdio));
    }

    #[test]
    fn test_transport_tcp_serialization() {
        // Test TCP transport serialization via AgentConfig
        let toml_str = r#"
command = "test"
transport = { tcp = { host = "127.0.0.1", port = 8080 } }
"#;
        let agent: AgentConfig = toml::from_str(toml_str).unwrap();
        if let Transport::Tcp { host, port } = agent.transport {
            assert_eq!(host, "127.0.0.1");
            assert_eq!(port, 8080);
        } else {
            panic!("Expected TCP transport");
        }
    }

    #[test]
    fn test_transport_stdio_serialization() {
        // Test Stdio transport serialization via AgentConfig
        let toml_str = r#"
command = "test"
transport = "stdio"
"#;
        let agent: AgentConfig = toml::from_str(toml_str).unwrap();
        assert!(matches!(agent.transport, Transport::Stdio));
    }

    #[test]
    fn test_gate_config_defaults() {
        // Test GateConfig default values
        let gates = GateConfig::default();
        assert!(gates.after_spec);
        assert!(gates.after_plan);
        assert!(!gates.after_each_subtask);
        assert!(gates.after_qa);
    }

    #[test]
    fn test_pipeline_config_defaults() {
        // Test PipelineConfig default values
        let pipeline = PipelineConfig::default();
        assert_eq!(pipeline.max_qa_iterations, 10);
        assert_eq!(pipeline.max_parallel, 3);
        assert!(pipeline.gates.after_spec);
        assert!(pipeline.gates.after_plan);
        assert!(!pipeline.gates.after_each_subtask);
        assert!(pipeline.gates.after_qa);
    }

    #[test]
    fn test_agent_config_validation_whitespace_command() {
        // Test that whitespace-only command fails validation
        let agent = AgentConfig {
            command: "   ".to_string(),
            args: vec![],
            transport: Transport::Stdio,
            mcp_servers: vec![],
            capabilities: vec![],
        };
        let result = agent.validate("test-agent");
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("test-agent"));
        assert!(err_msg.contains("empty command"));
    }

    #[test]
    fn test_agent_config_validation_whitespace_tcp_host() {
        // Test that whitespace-only TCP host fails validation
        let agent = AgentConfig {
            command: "test".to_string(),
            args: vec![],
            transport: Transport::Tcp {
                host: "   ".to_string(),
                port: 8080,
            },
            mcp_servers: vec![],
            capabilities: vec![],
        };
        let result = agent.validate("test-agent");
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("test-agent"));
        assert!(err_msg.contains("empty host"));
    }

    #[test]
    fn test_pipeline_config_validation_success() {
        // Test that valid pipeline config passes validation
        let pipeline = PipelineConfig {
            max_qa_iterations: 5,
            max_parallel: 10,
            gates: GateConfig::default(),
            max_cost_usd: None,
            max_tokens: None,
        };
        assert!(pipeline.validate().is_ok());
    }

    #[test]
    fn test_agent_config_with_args() {
        // Test agent config with multiple arguments
        let toml_str = r#"
command = "gh"
args = ["copilot", "suggest", "--verbose"]
"#;
        let agent: AgentConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(agent.command, "gh");
        assert_eq!(agent.args, vec!["copilot", "suggest", "--verbose"]);
        assert!(matches!(agent.transport, Transport::Stdio));
    }

    #[test]
    fn test_config_clone() {
        // Test that SurgeConfig can be cloned
        let config = SurgeConfig {
            default_agent: "custom".to_string(),
            pipeline: crate::config::PipelineConfig {
                max_qa_iterations: 42,
                ..Default::default()
            },
            ..Default::default()
        };

        let cloned = config.clone();
        assert_eq!(cloned.default_agent, "custom");
        assert_eq!(cloned.pipeline.max_qa_iterations, 42);
    }

    #[test]
    fn test_transport_clone() {
        // Test that Transport can be cloned
        let tcp = Transport::Tcp {
            host: "localhost".to_string(),
            port: 9000,
        };
        let cloned = tcp.clone();
        if let Transport::Tcp { host, port } = cloned {
            assert_eq!(host, "localhost");
            assert_eq!(port, 9000);
        } else {
            panic!("Expected TCP transport");
        }
    }

    #[test]
    fn test_toml_partial_pipeline() {
        // Test TOML with partial pipeline config (uses defaults for missing fields)
        let toml_str = r#"
default_agent = "test"

[pipeline]
max_qa_iterations = 7

[pipeline.gates]
after_spec = false
"#;
        let config: SurgeConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.pipeline.max_qa_iterations, 7);
        assert_eq!(config.pipeline.max_parallel, 3); // default
        assert!(!config.pipeline.gates.after_spec);
        assert!(config.pipeline.gates.after_plan); // default
    }

    #[test]
    fn test_empty_agents_map_validation() {
        // Test that empty agents map with any default_agent is valid
        let config = SurgeConfig {
            default_agent: "nonexistent".to_string(),
            agents: HashMap::new(),
            pipeline: PipelineConfig::default(),
            routing: RoutingConfig::default(),
            cleanup: CleanupPolicy::default(),
            ide: IdeConfig::default(),
            resilience: ResilienceConfig::default(),
            log: LogConfig::default(),
        };
        // Should be valid because agents map is empty
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_tcp_transport_valid_port_range() {
        // Test valid TCP port ranges
        let agent_min = AgentConfig {
            command: "test".to_string(),
            args: vec![],
            transport: Transport::Tcp {
                host: "localhost".to_string(),
                port: 1,
            },
            mcp_servers: vec![],
            capabilities: vec![],
        };
        assert!(agent_min.validate("test").is_ok());

        let agent_max = AgentConfig {
            command: "test".to_string(),
            args: vec![],
            transport: Transport::Tcp {
                host: "localhost".to_string(),
                port: 65535,
            },
            mcp_servers: vec![],
            capabilities: vec![],
        };
        assert!(agent_max.validate("test").is_ok());
    }

    #[test]
    fn test_routing_config_defaults() {
        let config = RoutingConfig::default();
        assert_eq!(config.strategy, RoutingStrategy::Default);
        assert!(config.agent_preferences.is_empty());
    }

    #[test]
    fn test_cleanup_policy_defaults() {
        let policy = CleanupPolicy::default();
        assert!(policy.remove_worktrees_on_complete);
        assert_eq!(policy.keep_branches_days, 7);
    }

    #[test]
    fn test_extended_config_toml_roundtrip() {
        let toml_str = r#"
default_agent = "claude"

[agents.claude]
command = "claude"

[pipeline]
max_qa_iterations = 10
max_parallel = 3

[routing]
strategy = "default"

[cleanup]
remove_worktrees_on_complete = false
keep_branches_days = 14

[ide]
editor = "rustrover"
"#;
        let config: SurgeConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.routing.strategy, RoutingStrategy::Default);
        assert!(!config.cleanup.remove_worktrees_on_complete);
        assert_eq!(config.cleanup.keep_branches_days, 14);
        assert_eq!(config.ide.editor, Some("rustrover".to_string()));
    }

    #[test]
    fn test_extended_config_missing_sections_use_defaults() {
        let toml_str = r#"default_agent = "test""#;
        let config: SurgeConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.routing.strategy, RoutingStrategy::Default);
        assert!(config.cleanup.remove_worktrees_on_complete);
        assert_eq!(config.cleanup.keep_branches_days, 7);
        assert!(config.ide.editor.is_none());
    }

    #[test]
    fn test_websocket_transport_roundtrip() {
        let _toml_str = r#"
command = "agent"
[transport]
url = "ws://localhost:8080"
"#;
        // Deserialize using the "ws" tag
        let _toml_str2 = r#"transport = {ws = {url = "ws://localhost:8080"}}"#;
        // Use inline table format that matches serde rename
        let agent: AgentConfig =
            toml::from_str("command = \"agent\"\n[transport.ws]\nurl = \"ws://localhost:8080\"\n")
                .unwrap();
        assert!(matches!(agent.transport, Transport::WebSocket { .. }));

        let serialized = toml::to_string(&agent).unwrap();
        let roundtripped: AgentConfig = toml::from_str(&serialized).unwrap();
        assert!(
            matches!(roundtripped.transport, Transport::WebSocket { url } if url == "ws://localhost:8080")
        );
    }

    #[test]
    fn test_websocket_transport_validation_error() {
        let agent = AgentConfig {
            command: "agent".to_string(),
            args: vec![],
            transport: Transport::WebSocket {
                url: "ws://localhost:8080".to_string(),
            },
            mcp_servers: vec![],
            capabilities: vec![],
        };
        let err = agent.validate("test-agent").unwrap_err();
        assert!(
            err.to_string()
                .contains("WebSocket transport not yet supported")
        );
    }

    #[test]
    fn test_pipeline_token_budget_defaults_to_none() {
        let pipeline = PipelineConfig::default();
        assert!(pipeline.max_cost_usd.is_none());
        assert!(pipeline.max_tokens.is_none());
    }

    #[test]
    fn test_pipeline_token_budget_roundtrip() {
        let toml_str = r#"
default_agent = "test"

[pipeline]
max_cost_usd = 1.50
max_tokens = 100000
"#;
        let config: SurgeConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.pipeline.max_cost_usd, Some(1.50));
        assert_eq!(config.pipeline.max_tokens, Some(100_000));

        let serialized = toml::to_string(&config).unwrap();
        let roundtripped: SurgeConfig = toml::from_str(&serialized).unwrap();
        assert_eq!(roundtripped.pipeline.max_cost_usd, Some(1.50));
        assert_eq!(roundtripped.pipeline.max_tokens, Some(100_000));
    }

    #[test]
    fn test_pipeline_budget_omitted_when_none() {
        let config = SurgeConfig::default();
        let s = toml::to_string(&config).unwrap();
        assert!(!s.contains("max_cost_usd"));
        assert!(!s.contains("max_tokens"));
    }

    #[test]
    fn test_log_config_defaults() {
        let log = LogConfig::default();
        assert_eq!(log.level, "info");
        assert!(log.file.is_none());
        assert_eq!(log.max_size_mb, 50);
    }

    #[test]
    fn test_log_config_roundtrip() {
        let toml_str = r#"
default_agent = "test"

[log]
level = "debug"
file = "/var/log/surge.log"
max_size_mb = 100
"#;
        let config: SurgeConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.log.level, "debug");
        assert_eq!(
            config.log.file.as_ref().unwrap().to_str().unwrap(),
            "/var/log/surge.log"
        );
        assert_eq!(config.log.max_size_mb, 100);
    }

    #[test]
    fn test_ide_config_extension_defaults() {
        let ide = IdeConfig::default();
        assert!(ide.editor.is_none());
        assert!(ide.open_file_cmd.is_none());
        assert!(!ide.auto_open_worktree);
    }

    #[test]
    fn test_ide_config_extension_roundtrip() {
        let toml_str = r#"
default_agent = "test"

[ide]
editor = "vscode"
open_file_cmd = "code {path}:{line}"
auto_open_worktree = true
"#;
        let config: SurgeConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.ide.editor.as_deref(), Some("vscode"));
        assert_eq!(
            config.ide.open_file_cmd.as_deref(),
            Some("code {path}:{line}")
        );
        assert!(config.ide.auto_open_worktree);
    }

    #[test]
    fn test_log_level_validation_valid() {
        for level in ["error", "warn", "info", "debug", "trace"] {
            let mut config = SurgeConfig::default();
            config.log.level = level.to_string();
            assert!(config.validate().is_ok(), "level '{level}' should be valid");
        }
    }

    #[test]
    fn test_log_level_validation_invalid() {
        let mut config = SurgeConfig::default();
        config.log.level = "verbose".to_string();
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("verbose"));
    }

    #[test]
    fn test_example_toml_deserializes() {
        let content = include_str!("../../../surge.example.toml");
        let config: SurgeConfig = toml::from_str(content).unwrap();
        assert_eq!(config.default_agent, "claude");
        assert!(config.agents.contains_key("claude"));
        config.validate().unwrap();
    }
}
