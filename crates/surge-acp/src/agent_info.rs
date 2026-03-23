//! Agent display information — business logic for UI presentation.
//!
//! This module builds display-ready data from registry entries and health stats.
//! The UI should only render, never compute agent metadata.

use crate::health::AgentHealth;
use crate::registry::{DetectedAgent, RegistryEntry};

// ── Display models ──────────────────────────────────────────────────

/// Model option for agent configuration panel.
#[derive(Debug, Clone)]
pub struct ModelOption {
    pub name: String,
    pub price: String,
    pub context: String,
    pub note: String,
    pub enabled: bool,
}

/// Effort/thinking level for agent operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffortLevel {
    High,
    Medium,
    Low,
    Adaptive,
}

impl EffortLevel {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::High => "High",
            Self::Medium => "Medium",
            Self::Low => "Low",
            Self::Adaptive => "Adaptive",
        }
    }
}

/// Permission toggle for agent configuration panel.
#[derive(Debug, Clone)]
pub struct PermissionSetting {
    pub name: String,
    pub enabled: bool,
}

/// Agent-specific effort configuration.
#[derive(Debug, Clone)]
pub struct AgentEffortConfig {
    pub default: EffortLevel,
    pub planning: EffortLevel,
    pub coding: EffortLevel,
    pub qa_review: EffortLevel,
}

/// Agent capabilities for the configuration panel.
#[derive(Debug, Clone)]
pub struct AgentCapabilities {
    pub models: Option<Vec<ModelOption>>,
    pub effort: Option<AgentEffortConfig>,
    pub permissions: Option<Vec<PermissionSetting>>,
    pub dangerous_ops: Option<String>,
}

/// Usage data — varies by agent type.
#[derive(Debug, Clone)]
pub enum AgentUsage {
    /// Claude Code: native statusline data.
    ClaudeCode {
        five_hour_pct: f32,
        five_hour_reset: String,
        weekly_pct: f32,
        weekly_reset: String,
        extra_usage_enabled: bool,
        extra_usage_cost: f64,
    },
    /// Estimated from ACP response tokens.
    Estimated {
        provider: String,
        estimated_tokens: u64,
        estimated_cost: f64,
        is_local: bool,
    },
    /// No data yet.
    Unknown,
}

/// Session entry for agent detail panel.
#[derive(Debug, Clone)]
pub struct SessionEntry {
    pub label: String,
    pub status: SessionStatus,
    pub time_ago: String,
    pub tokens: Option<u64>,
    pub duration: Option<String>,
}

/// Session status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionStatus {
    Running,
    Completed,
    Failed,
}

/// Fully assembled display data for an installed (configured) agent.
#[derive(Debug, Clone)]
pub struct ConfiguredAgent {
    pub name: String,
    pub display_name: String,
    pub description: String,
    pub model: Option<String>,
    pub binary: String,
    pub version: Option<String>,
    pub active_sessions: u32,
    pub requests_today: u32,
    pub tokens_today: u64,
    pub cost_today: f64,
    pub avg_latency_ms: u32,
    pub sessions_today: u32,
    pub capabilities: AgentCapabilities,
    pub usage: AgentUsage,
    pub subtasks_completed: u32,
    pub subtasks_failed: u32,
    pub avg_subtask_secs: u32,
    pub qa_first_pass_rate: f32,
    pub uptime: String,
    pub last_seen: Option<String>,
    pub recent_sessions: Vec<SessionEntry>,
}

/// Display data for an agent available for installation.
#[derive(Debug, Clone)]
pub struct AvailableAgent {
    pub name: String,
    pub display_name: String,
    pub vendor: String,
    pub description: String,
    pub license: String,
    pub install_command: String,
    pub install_method: String,
    pub badges: Vec<AgentBadge>,
}

/// A badge to display on an agent card.
#[derive(Debug, Clone)]
pub struct AgentBadge {
    pub label: String,
    pub kind: BadgeKind,
}

/// Badge visual category (UI maps these to colors).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BadgeKind {
    Popular,
    OpenSource,
    Free,
    New,
}

// ── Builder functions ───────────────────────────────────────────────

/// Build a `ConfiguredAgent` from a detected agent and optional health data.
#[must_use]
pub fn build_configured_agent(
    detected: &DetectedAgent,
    health: Option<&AgentHealth>,
) -> ConfiguredAgent {
    let (requests, latency, failures) = match health {
        Some(h) => (h.total_requests, h.avg_latency_ms, h.total_failures),
        None => (0, 0, 0),
    };

    ConfiguredAgent {
        name: detected.entry.id.clone(),
        display_name: detected.entry.display_name.clone(),
        description: detected.entry.description.clone(),
        model: detected.entry.models.first().cloned(),
        binary: detected.entry.command.clone(),
        version: Some(detected.entry.version.clone()),
        active_sessions: 0,
        requests_today: requests as u32,
        tokens_today: 0,
        cost_today: 0.0,
        avg_latency_ms: latency as u32,
        sessions_today: 0,
        capabilities: build_capabilities(&detected.entry.id),
        usage: build_usage(&detected.entry.id),
        subtasks_completed: 0,
        subtasks_failed: failures as u32,
        avg_subtask_secs: 0,
        qa_first_pass_rate: 0.0,
        uptime: "—".into(),
        last_seen: None,
        recent_sessions: vec![],
    }
}

/// Build an `AvailableAgent` from a registry entry.
#[must_use]
pub fn build_available_agent(entry: &RegistryEntry) -> AvailableAgent {
    AvailableAgent {
        name: entry.id.clone(),
        display_name: entry.display_name.clone(),
        vendor: entry.vendor().to_string(),
        description: entry.description.clone(),
        license: entry.license.clone(),
        install_command: entry.install_instructions.clone(),
        install_method: extract_install_method(&entry.install_instructions),
        badges: build_badges(entry),
    }
}

// ── Internal helpers ────────────────────────────────────────────────

fn build_capabilities(agent_id: &str) -> AgentCapabilities {
    match agent_id {
        "claude-acp" => AgentCapabilities {
            models: Some(vec![
                ModelOption {
                    name: "Opus 4.6".into(),
                    price: "$5/$25".into(),
                    context: "1M ctx".into(),
                    note: "Heavy reasoning".into(),
                    enabled: true,
                },
                ModelOption {
                    name: "Sonnet 4.6".into(),
                    price: "$3/$15".into(),
                    context: "1M ctx".into(),
                    note: "Daily driver".into(),
                    enabled: true,
                },
                ModelOption {
                    name: "Haiku 4.5".into(),
                    price: "$0.80/$4".into(),
                    context: "200K".into(),
                    note: "Quick tasks".into(),
                    enabled: true,
                },
            ]),
            effort: Some(AgentEffortConfig {
                default: EffortLevel::Adaptive,
                planning: EffortLevel::High,
                coding: EffortLevel::Adaptive,
                qa_review: EffortLevel::Low,
            }),
            permissions: Some(vec![
                PermissionSetting { name: "File read".into(), enabled: true },
                PermissionSetting { name: "File write".into(), enabled: true },
                PermissionSetting { name: "Bash commands".into(), enabled: true },
                PermissionSetting { name: "Network access".into(), enabled: false },
                PermissionSetting { name: "Git push".into(), enabled: false },
            ]),
            dangerous_ops: Some("Ask permission".into()),
        },
        _ => AgentCapabilities {
            models: None,
            effort: None,
            permissions: None,
            dangerous_ops: None,
        },
    }
}

fn build_usage(agent_id: &str) -> AgentUsage {
    match agent_id {
        "claude-acp" => AgentUsage::ClaudeCode {
            five_hour_pct: 0.0,
            five_hour_reset: "—".into(),
            weekly_pct: 0.0,
            weekly_reset: "—".into(),
            extra_usage_enabled: false,
            extra_usage_cost: 0.0,
        },
        _ => AgentUsage::Estimated {
            provider: "Unknown".into(),
            estimated_tokens: 0,
            estimated_cost: 0.0,
            is_local: false,
        },
    }
}

fn build_badges(entry: &RegistryEntry) -> Vec<AgentBadge> {
    let mut badges = Vec::new();

    if entry.tags.contains(&"popular".to_string()) {
        badges.push(AgentBadge { label: "Popular".into(), kind: BadgeKind::Popular });
    }
    if entry.is_open_source() {
        badges.push(AgentBadge { label: "OSS".into(), kind: BadgeKind::OpenSource });
    }
    if entry.license.to_lowercase().contains("free")
        || entry.tags.contains(&"free".to_string())
    {
        badges.push(AgentBadge { label: "Free".into(), kind: BadgeKind::Free });
    }

    badges
}

fn extract_install_method(instructions: &str) -> String {
    let lower = instructions.to_lowercase();
    if lower.starts_with("npx ") {
        "npx".into()
    } else if lower.starts_with("uvx ") {
        "uvx".into()
    } else if lower.contains("npm") {
        "npm".into()
    } else if lower.contains("brew") {
        "brew".into()
    } else if lower.contains("pip") {
        "pip".into()
    } else if lower.contains("download") {
        "download".into()
    } else {
        "binary".into()
    }
}

/// Vendor color hue (0.0–1.0) for an agent ID. Returns None for unknown agents.
///
/// UI maps this to its own color type (Hsla, etc.).
#[must_use]
pub fn vendor_hue(agent_id: &str) -> Option<f32> {
    let hue = match agent_id {
        "claude-acp" => 263.0,
        "github-copilot-cli" => 210.0,
        "gemini" => 217.0,
        "codex-acp" => 150.0,
        "goose" => 25.0,
        "cline" => 340.0,
        "amp-acp" => 280.0,
        "mistral-vibe" => 35.0,
        "cursor" => 50.0,
        "junie" => 310.0,
        "kimi" => 190.0,
        "qwen-code" => 200.0,
        "kilo" => 160.0,
        "opencode" => 120.0,
        "factory-droid" => 0.0,
        "auggie" => 270.0,
        "codebuddy-code" => 200.0,
        "stakpak" => 140.0,
        "corust-agent" => 30.0,
        "nova" => 220.0,
        "dimcode" => 180.0,
        "autohand" => 90.0,
        "pi-acp" => 60.0,
        "qoder" => 250.0,
        "crow-cli" => 100.0,
        "deepagents" => 170.0,
        "fast-agent" => 130.0,
        "minion-code" => 300.0,
        _ => return None,
    };
    Some(hue / 360.0)
}
