//! Agent display information — business logic for UI presentation.
//!
//! This module builds display-ready data from registry entries and health stats.
//! The UI should only render, never compute agent metadata.

use crate::health::AgentHealth;
use crate::metadata::{AgentMetadata, MetadataStore};
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

/// How an agent is available on the system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallStatus {
    /// Binary installed locally on PATH.
    Installed,
    /// Available via npx (downloaded on-demand).
    Npx,
    /// Available via uvx (downloaded on-demand).
    Uvx,
}

impl InstallStatus {
    /// Human-readable label.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Installed => "Installed",
            Self::Npx => "npx",
            Self::Uvx => "uvx",
        }
    }
}

/// Fully assembled display data for a runnable agent.
#[derive(Debug, Clone)]
pub struct ConfiguredAgent {
    pub name: String,
    pub display_name: String,
    pub description: String,
    pub model: Option<String>,
    pub binary: String,
    /// Version from ACP registry (may lag behind actual).
    pub registry_version: String,
    /// Actually installed version (detected via `--version`), None if not yet checked.
    pub installed_version: Option<String>,
    /// How this agent is available.
    pub install_status: InstallStatus,
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

/// Display data for an agent available for installation / on-demand use.
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
    /// Whether this agent can be launched right now (e.g. npx available).
    pub runnable: bool,
    /// How it would be launched (npx/uvx/binary).
    pub run_via: Option<InstallStatus>,
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

/// Build a `ConfiguredAgent` from a detected agent, health data, and metadata.
#[must_use]
pub fn build_configured_agent(
    detected: &DetectedAgent,
    health: Option<&AgentHealth>,
) -> ConfiguredAgent {
    build_configured_agent_with_metadata(detected, health, MetadataStore::global())
}

/// Build a `ConfiguredAgent` with explicit metadata store.
#[must_use]
pub fn build_configured_agent_with_metadata(
    detected: &DetectedAgent,
    health: Option<&AgentHealth>,
    metadata: &MetadataStore,
) -> ConfiguredAgent {
    let (requests, latency, failures) = match health {
        Some(h) => (h.total_requests, h.avg_latency_ms, h.total_failures),
        None => (0, 0, 0),
    };

    let install_status = if detected.entry.is_npx() {
        InstallStatus::Npx
    } else if detected.entry.is_uvx() {
        InstallStatus::Uvx
    } else {
        InstallStatus::Installed
    };

    let meta = metadata.get(&detected.entry.id);

    // Use metadata tagline if available, else registry description
    let description = meta
        .map(|m| m.tagline.clone())
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| detected.entry.description.clone());

    // Build models from metadata if available
    let model = meta
        .and_then(|m| m.models.first())
        .map(|m| m.name.clone())
        .or_else(|| detected.entry.models.first().cloned());

    ConfiguredAgent {
        name: detected.entry.id.clone(),
        display_name: meta
            .map(|m| m.display_name.clone())
            .unwrap_or_else(|| detected.entry.display_name.clone()),
        description,
        model,
        binary: detected.entry.command.clone(),
        registry_version: detected.entry.version.clone(),
        installed_version: None,
        install_status,
        active_sessions: 0,
        requests_today: requests as u32,
        tokens_today: 0,
        cost_today: 0.0,
        avg_latency_ms: latency as u32,
        sessions_today: 0,
        capabilities: build_capabilities_from_metadata(&detected.entry.id, meta),
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

/// Build an `AvailableAgent` from a registry entry (uses embedded metadata).
#[must_use]
pub fn build_available_agent(entry: &RegistryEntry) -> AvailableAgent {
    let store = MetadataStore::global();
    build_available_agent_with_metadata(entry, &store)
}

/// Build an `AvailableAgent` with explicit metadata store.
#[must_use]
pub fn build_available_agent_with_metadata(
    entry: &RegistryEntry,
    metadata: &MetadataStore,
) -> AvailableAgent {
    let runnable = entry.is_runnable();
    let run_via = if entry.is_npx() {
        Some(InstallStatus::Npx)
    } else if entry.is_uvx() {
        Some(InstallStatus::Uvx)
    } else if runnable {
        Some(InstallStatus::Installed)
    } else {
        None
    };

    let meta = metadata.get(&entry.id);

    AvailableAgent {
        name: entry.id.clone(),
        display_name: meta
            .map(|m| m.display_name.clone())
            .unwrap_or_else(|| entry.display_name.clone()),
        vendor: meta
            .map(|m| m.vendor.clone())
            .unwrap_or_else(|| entry.vendor().to_string()),
        description: meta
            .map(|m| m.tagline.clone())
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| entry.description.clone()),
        license: entry.license.clone(),
        install_command: entry.install_instructions.clone(),
        install_method: extract_install_method(&entry.install_instructions),
        badges: build_badges(entry),
        runnable,
        run_via,
    }
}

// ── Internal helpers ────────────────────────────────────────────────

/// Build capabilities from metadata if available, else fallback to hardcoded.
fn build_capabilities_from_metadata(
    agent_id: &str,
    meta: Option<&AgentMetadata>,
) -> AgentCapabilities {
    let models = meta.and_then(|m| {
        if m.models.is_empty() {
            return None;
        }
        Some(
            m.models
                .iter()
                .map(|model| {
                    let context = if model.context > 0 {
                        format_context(model.context)
                    } else {
                        "—".into()
                    };
                    ModelOption {
                        name: model.name.clone(),
                        price: model
                            .premium_multiplier
                            .map(|pm| {
                                if pm == 0.0 {
                                    "included".into()
                                } else {
                                    format!("{pm}x")
                                }
                            })
                            .unwrap_or_else(|| "—".into()),
                        context,
                        note: if !model.note.is_empty() {
                            model.note.clone()
                        } else {
                            model.strengths.join(", ")
                        },
                        enabled: true,
                    }
                })
                .collect(),
        )
    });

    // Effort control only for agents that support it
    let effort = meta.and_then(|m| {
        if m.features.get("effort_control").and_then(|v| v.as_bool()) == Some(true) {
            Some(AgentEffortConfig {
                default: EffortLevel::Adaptive,
                planning: EffortLevel::High,
                coding: EffortLevel::Adaptive,
                qa_review: EffortLevel::Low,
            })
        } else {
            None
        }
    });

    AgentCapabilities {
        models,
        effort,
        permissions: if agent_id == "claude-acp" {
            Some(vec![
                PermissionSetting { name: "File read".into(), enabled: true },
                PermissionSetting { name: "File write".into(), enabled: true },
                PermissionSetting { name: "Bash commands".into(), enabled: true },
                PermissionSetting { name: "Network access".into(), enabled: false },
                PermissionSetting { name: "Git push".into(), enabled: false },
            ])
        } else {
            None
        },
        dangerous_ops: Some("Ask permission".into()),
    }
}

fn format_context(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{}M ctx", tokens / 1_000_000)
    } else if tokens >= 1_000 {
        format!("{}K ctx", tokens / 1_000)
    } else {
        format!("{tokens} ctx")
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

/// Vendor color as (r, g, b) floats for an agent ID.
///
/// Uses metadata hex color if available, else returns None.
/// UI maps this to its own color type (Hsla, rgb, etc.).
#[must_use]
pub fn vendor_color(agent_id: &str) -> Option<(f32, f32, f32)> {
    let store = MetadataStore::global();
    if let Some(meta) = store.get(agent_id) {
        if !meta.color.is_empty() {
            return MetadataStore::parse_color(&meta.color);
        }
    }
    None
}

/// Vendor color hue (0.0–1.0) for an agent ID. Returns None for unknown agents.
///
/// Derives hue from metadata hex color. Fallback for agents without metadata.
#[must_use]
pub fn vendor_hue(agent_id: &str) -> Option<f32> {
    // Try metadata color first
    if let Some((r, g, b)) = vendor_color(agent_id) {
        return Some(rgb_to_hue(r, g, b));
    }
    // Fallback for agents without metadata
    None
}

/// Convert RGB (0.0-1.0) to hue (0.0-1.0).
fn rgb_to_hue(r: f32, g: f32, b: f32) -> f32 {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;

    if delta < f32::EPSILON {
        return 0.0;
    }

    let hue = if (max - r).abs() < f32::EPSILON {
        ((g - b) / delta) % 6.0
    } else if (max - g).abs() < f32::EPSILON {
        (b - r) / delta + 2.0
    } else {
        (r - g) / delta + 4.0
    };

    let hue = hue / 6.0;
    if hue < 0.0 { hue + 1.0 } else { hue }
}

/// Detected version info for display.
#[derive(Debug, Clone)]
pub struct VersionInfo {
    /// Parsed version number (e.g. "2.1.81").
    pub version: String,
    /// Full display string (e.g. "Claude Code 2.1.81 (adapter v0.22.2)").
    pub display: String,
    /// Whether this is a wrapper adapter.
    pub is_wrapper: bool,
}

/// Detect the actually installed version of an agent.
///
/// Uses `version_command` from metadata if available, otherwise tries
/// common flags. Returns structured version info for display.
pub async fn detect_installed_version(entry: &RegistryEntry) -> Option<VersionInfo> {
    use tokio::process::Command;

    let store = MetadataStore::global();
    let meta = store.get(&entry.id);

    // For npx/uvx agents, skip detection (would trigger download)
    if entry.is_npx() || entry.is_uvx() {
        return None;
    }

    // Try metadata version_command first
    if let Some(m) = meta {
        if !m.version_command.is_empty() {
            let cmd = &m.version_command[0];
            let args: Vec<&str> = m.version_command[1..].iter().map(String::as_str).collect();

            if let Ok(output) = Command::new(cmd)
                .args(&args)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .output()
                .await
            {
                if output.status.success() {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    let combined = format!("{stdout} {stderr}");

                    if let Some(ver) = extract_version_string(&combined) {
                        let is_wrapper = m.acp_type == "wrapper";
                        let display = if is_wrapper {
                            let cli_name = if m.wrapped_cli_name.is_empty() {
                                &m.display_name
                            } else {
                                &m.wrapped_cli_name
                            };
                            format!("{cli_name} {ver} (adapter v{})", entry.version)
                        } else {
                            format!("{} {ver}", m.display_name)
                        };

                        return Some(VersionInfo {
                            version: ver,
                            display,
                            is_wrapper,
                        });
                    }
                }
            }
        }
    }

    // Fallback: try common version flags on the registry command
    let cmd = &entry.command;
    for flag in &["--version", "-v", "-V"] {
        if let Ok(output) = Command::new(cmd)
            .arg(flag)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .output()
            .await
        {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);
                let combined = format!("{stdout} {stderr}");

                if let Some(ver) = extract_version_string(&combined) {
                    return Some(VersionInfo {
                        display: format!("{ver}"),
                        version: ver,
                        is_wrapper: false,
                    });
                }
            }
        }
    }

    None
}

/// Extract a semver-like version string from text.
/// Finds patterns like "1.2.3", "v0.10.0", "888.212.0".
fn extract_version_string(text: &str) -> Option<String> {
    for word in text.split_whitespace() {
        let trimmed = word.trim_start_matches('v').trim_matches(|c: char| !c.is_ascii_digit() && c != '.');
        let parts: Vec<&str> = trimmed.split('.').collect();
        if parts.len() >= 2 && parts.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit())) {
            return Some(trimmed.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_version_string() {
        assert_eq!(extract_version_string("goose v1.28.0"), Some("1.28.0".into()));
        assert_eq!(extract_version_string("version 0.10.0"), Some("0.10.0".into()));
        assert_eq!(extract_version_string("888.212.0"), Some("888.212.0".into()));
        assert_eq!(extract_version_string("Claude Code CLI v2.3.1"), Some("2.3.1".into()));
        assert_eq!(extract_version_string("no version here"), None);
        assert_eq!(extract_version_string(""), None);
    }

    #[test]
    fn test_install_status_label() {
        assert_eq!(InstallStatus::Installed.label(), "Installed");
        assert_eq!(InstallStatus::Npx.label(), "npx");
        assert_eq!(InstallStatus::Uvx.label(), "uvx");
    }
}
