//! External integrations: MCP servers, tracker sources, Telegram, inbox.

use super::*;

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

/// Configuration for a task source (Linear, GitHub Issues, ...).
///
/// Serialised in `surge.toml` as `[[task_sources]]` entries with a `type`
/// discriminator. The provider implementation is selected by `type`; each
/// variant carries provider-specific fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TaskSourceConfig {
    /// Linear (linear.app) GraphQL workspace.
    Linear(LinearSourceConfig),
    /// GitHub Issues for a single repository.
    GithubIssues(GitHubIssuesSourceConfig),
}

/// Linear-specific task source configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinearSourceConfig {
    /// Stable identifier (e.g. `"linear-acme"`).
    pub id: String,
    /// Linear workspace ID this source targets.
    pub workspace_id: String,
    /// Name of the env var holding the Linear API token (e.g. `"LINEAR_API_TOKEN"`).
    pub api_token_env: String,
    /// Polling interval in seconds.
    #[serde(
        rename = "poll_interval_seconds",
        with = "duration_seconds",
        default = "default_poll_interval"
    )]
    pub poll_interval: std::time::Duration,
    /// Labels that gate which Linear issues this source ingests.
    #[serde(default)]
    pub label_filters: Vec<String>,
}

/// How the L3 auto-merge gate merges a ready PR.
///
/// Mirrors GitHub's three merge methods. Squash is the default because it
/// keeps `main` history linear and matches the most common repo convention;
/// override per source in `surge.toml` to match a repo that uses merge
/// commits or rebases.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum MergeMethod {
    /// Squash all commits into one before merging (default).
    #[default]
    Squash,
    /// Create a merge commit.
    Merge,
    /// Rebase the PR commits onto the base branch.
    Rebase,
}

/// GitHub Issues-specific task source configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitHubIssuesSourceConfig {
    /// Stable identifier (e.g. `"github-myapp"`).
    pub id: String,
    /// Repository in `owner/repo` form (e.g. `"user/myapp"`).
    pub repo: String,
    /// Name of the env var holding the GitHub PAT (e.g. `"GITHUB_TOKEN"`).
    pub api_token_env: String,
    /// Polling interval in seconds.
    #[serde(
        rename = "poll_interval_seconds",
        with = "duration_seconds",
        default = "default_poll_interval"
    )]
    pub poll_interval: std::time::Duration,
    /// Labels that gate which GitHub issues this source ingests.
    #[serde(default)]
    pub label_filters: Vec<String>,
    /// Merge method the L3 auto-merge gate uses for this repository.
    /// Defaults to [`MergeMethod::Squash`].
    #[serde(default)]
    pub merge_method: MergeMethod,
}

fn default_poll_interval() -> std::time::Duration {
    std::time::Duration::from_secs(60)
}

/// Optional Telegram bot configuration.
///
/// `chat_id_env` and `bot_token_env` are the names of the environment
/// variables to read for the chat ID and bot token respectively. Direct
/// `chat_id` is an explicit nonsecret delivery target — secrets never go in
/// `surge.toml` (per RFC-0010 pattern).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TelegramConfig {
    /// Name of the env var that holds the numeric chat id.
    #[serde(default)]
    pub chat_id_env: Option<String>,
    /// Name of the env var that holds the bot token.
    #[serde(default)]
    pub bot_token_env: Option<String>,
    /// Explicit delivery chat id (overrides `chat_id_env` if set).
    #[serde(default)]
    pub chat_id: Option<i64>,
}

/// Inbox subsystem configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InboxConfig {
    /// How often the snooze scheduler polls for due cards.
    #[serde(
        rename = "snooze_poll_interval_seconds",
        default = "default_snooze_poll_interval_secs",
        with = "duration_seconds"
    )]
    pub snooze_poll_interval: std::time::Duration,
    /// Which channels deliver inbox cards. Empty == "all configured".
    #[serde(default)]
    pub delivery_channels: Vec<String>,
}

impl Default for InboxConfig {
    fn default() -> Self {
        Self {
            snooze_poll_interval: std::time::Duration::from_secs(300),
            delivery_channels: Vec::new(),
        }
    }
}

fn default_snooze_poll_interval_secs() -> std::time::Duration {
    std::time::Duration::from_secs(300)
}

mod duration_seconds {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(d: &std::time::Duration, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u64(d.as_secs())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<std::time::Duration, D::Error> {
        let secs = u64::deserialize(d)?;
        Ok(std::time::Duration::from_secs(secs))
    }
}
