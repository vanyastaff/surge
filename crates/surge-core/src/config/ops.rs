//! Analytics/pricing, cleanup, IDE and logging settings.

use super::*;

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

/// Analytics configuration for cost tracking and budgets.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalyticsConfig {
    /// Default pricing information for agents that don't specify their own.
    #[serde(default)]
    pub default_pricing: PricingInfo,
    /// Global budget limit in USD. None = unlimited.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_usd: Option<f64>,
    /// Global token budget limit. None = unlimited.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_tokens: Option<u64>,
    /// Warn when cost reaches this percentage of budget (0-100).
    #[serde(default = "default_budget_warn_threshold")]
    pub budget_warn_threshold: u8,
}

impl Default for AnalyticsConfig {
    fn default() -> Self {
        Self {
            default_pricing: PricingInfo::default(),
            budget_usd: None,
            budget_tokens: None,
            budget_warn_threshold: default_budget_warn_threshold(),
        }
    }
}

impl AnalyticsConfig {
    /// Resolve the live-enforcement budget guard from the configured limits.
    ///
    /// The breach policy defaults to [`crate::budget::BudgetPolicy::Abort`]
    /// (the safe AFK posture). When neither `budget_usd` nor `budget_tokens`
    /// is set the guard is unlimited and the engine skips enforcement.
    #[must_use]
    pub fn budget_guard(&self) -> crate::budget::BudgetGuard {
        crate::budget::BudgetGuard {
            limits: crate::budget::BudgetLimits {
                usd: self.budget_usd,
                tokens: self.budget_tokens,
                warn_threshold_pct: self.budget_warn_threshold,
            },
            policy: crate::budget::BudgetPolicy::default(),
        }
    }
}

fn default_budget_warn_threshold() -> u8 {
    80
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
