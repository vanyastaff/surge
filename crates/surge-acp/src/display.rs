//! Agent display data — UI presentation types and view models.
//!
//! View models that project [`RegistryEntry`]
//! data into structures suitable for UI rendering.

use crate::health::{AgentHealth, HealthStatus};
use crate::registry::{DetectedAgent, RegistryEntry};

// ── Display types ───────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Model {
    pub name: String,
    pub price: String,
    pub context: String,
    pub note: String,
    pub enabled: bool,
}

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

#[derive(Debug, Clone)]
pub struct Permission {
    pub name: String,
    pub enabled: bool,
}

#[derive(Debug, Clone)]
pub struct EffortConfig {
    pub default: EffortLevel,
    pub planning: EffortLevel,
    pub coding: EffortLevel,
    pub qa_review: EffortLevel,
}

#[derive(Debug, Clone)]
pub struct DisplayCapabilities {
    pub models: Option<Vec<Model>>,
    pub effort: Option<EffortConfig>,
    pub permissions: Option<Vec<Permission>>,
    pub dangerous_ops: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Usage {
    ClaudeCode {
        five_hour_pct: f32,
        five_hour_reset: String,
        weekly_pct: f32,
        weekly_reset: String,
        extra_usage_enabled: bool,
        extra_usage_cost: f64,
    },
    Estimated {
        provider: String,
        estimated_tokens: u64,
        estimated_cost: f64,
        is_local: bool,
    },
    Unknown,
}

#[derive(Debug, Clone)]
pub struct SessionEntry {
    pub label: String,
    pub status: SessionStatus,
    pub time_ago: String,
    pub tokens: Option<u64>,
    pub duration: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionStatus {
    Running,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallMethod {
    Installed,
    Npx,
    Uvx,
}

impl InstallMethod {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Installed => "Installed",
            Self::Npx => "npx",
            Self::Uvx => "uvx",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Badge {
    pub label: String,
    pub kind: BadgeKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BadgeKind {
    Popular,
    OpenSource,
    Free,
    New,
}

#[derive(Debug, Clone)]
pub struct VersionInfo {
    pub version: String,
    pub display: String,
    pub is_wrapper: bool,
}

// ── View models ─────────────────────────────────────────────────────

/// Detailed view of a configured and connected agent.
#[derive(Debug, Clone)]
pub struct AgentDetail {
    pub name: String,
    pub display_name: String,
    pub description: String,
    pub model: Option<String>,
    pub binary: String,
    pub registry_version: String,
    pub installed_version: Option<String>,
    pub install_method: InstallMethod,
    pub active_sessions: u32,
    pub requests_today: u32,
    pub tokens_today: u64,
    pub cost_today: f64,
    pub avg_latency_ms: u32,
    pub sessions_today: u32,
    pub capabilities: DisplayCapabilities,
    pub usage: Usage,
    pub subtasks_completed: u32,
    pub subtasks_failed: u32,
    pub avg_subtask_secs: u32,
    pub qa_first_pass_rate: f32,
    pub uptime: String,
    pub last_seen: Option<String>,
    pub recent_sessions: Vec<SessionEntry>,
    // Health metrics
    pub health_status: Option<HealthStatus>,
    pub total_failures: u32,
    pub error_rate: f32,
    pub rate_limited: bool,
    pub latency_p50_ms: u32,
    pub latency_p99_ms: u32,
    pub total_heartbeat_failures: u32,
    pub consecutive_heartbeat_failures: u32,
}

impl AgentDetail {
    /// Build from a detected agent and optional health stats.
    #[must_use]
    pub fn from_detected(detected: &DetectedAgent, health: Option<&AgentHealth>) -> Self {
        let agent_id = &detected.entry.id;
        let install_method = if detected.entry.is_npx() {
            InstallMethod::Npx
        } else if detected.entry.is_uvx() {
            InstallMethod::Uvx
        } else {
            InstallMethod::Installed
        };

        // Extract health metrics if available
        let (
            requests,
            latency,
            failures,
            health_status,
            error_rate,
            rate_limited,
            latency_p50,
            latency_p99,
            total_hb_failures,
            consecutive_hb_failures,
        ) = match health {
            Some(h) => (
                h.total_requests,
                h.avg_latency_ms,
                h.total_failures,
                Some(h.status()),
                h.error_rate() as f32,
                h.rate_limited,
                h.latency_p50_ms(),
                h.latency_p99_ms(),
                h.total_heartbeat_failures,
                h.consecutive_heartbeat_failures,
            ),
            None => (0, 0, 0, None, 0.0, false, 0, 0, 0, 0),
        };

        Self {
            name: detected.entry.id.clone(),
            display_name: detected.entry.display_name.clone(),
            description: detected.entry.description.clone(),
            model: detected.entry.models.first().cloned(),
            binary: detected.entry.command.clone(),
            registry_version: detected.entry.version.clone(),
            installed_version: None,
            install_method,
            active_sessions: 0,
            requests_today: requests as u32,
            tokens_today: 0,
            cost_today: 0.0,
            avg_latency_ms: latency as u32,
            sessions_today: 0,
            capabilities: capabilities(agent_id),
            usage: usage(agent_id),
            subtasks_completed: 0,
            subtasks_failed: 0,
            avg_subtask_secs: 0,
            qa_first_pass_rate: 0.0,
            uptime: "—".into(),
            last_seen: None,
            recent_sessions: vec![],
            health_status,
            total_failures: failures as u32,
            error_rate,
            rate_limited,
            latency_p50_ms: latency_p50 as u32,
            latency_p99_ms: latency_p99 as u32,
            total_heartbeat_failures: total_hb_failures as u32,
            consecutive_heartbeat_failures: consecutive_hb_failures as u32,
        }
    }
}

/// Summary view of an agent available in the registry.
#[derive(Debug, Clone)]
pub struct AgentSummary {
    pub name: String,
    pub display_name: String,
    pub vendor: String,
    pub description: String,
    pub license: String,
    pub install_command: String,
    pub install_method: String,
    pub badges: Vec<Badge>,
    pub runnable: bool,
    pub run_via: Option<InstallMethod>,
}

impl AgentSummary {
    /// Build from a registry entry.
    #[must_use]
    pub fn from_entry(entry: &RegistryEntry) -> Self {
        let runnable = entry.is_runnable();
        let run_via = if entry.is_npx() {
            Some(InstallMethod::Npx)
        } else if entry.is_uvx() {
            Some(InstallMethod::Uvx)
        } else if runnable {
            Some(InstallMethod::Installed)
        } else {
            None
        };

        Self {
            name: entry.id.clone(),
            display_name: entry.display_name.clone(),
            vendor: entry.vendor().to_string(),
            description: entry.description.clone(),
            license: entry.license.clone(),
            install_command: entry.install_instructions.clone(),
            install_method: if entry.is_npx() {
                "npx"
            } else if entry.is_uvx() {
                "uvx"
            } else {
                "binary"
            }
            .into(),
            badges: badges(entry),
            runnable,
            run_via,
        }
    }
}

// ── Per-agent UI data (keyed by agent ID) ──────────────────────────

fn capabilities(agent_id: &str) -> DisplayCapabilities {
    match agent_id {
        "claude-acp" => DisplayCapabilities {
            models: Some(vec![
                Model {
                    name: "Claude Opus 4.6".into(),
                    price: "$5/$25".into(),
                    context: "1M ctx".into(),
                    note: "Deep reasoning".into(),
                    enabled: true,
                },
                Model {
                    name: "Claude Sonnet 4.6".into(),
                    price: "$3/$15".into(),
                    context: "1M ctx".into(),
                    note: "Daily driver".into(),
                    enabled: true,
                },
                Model {
                    name: "Claude Haiku 4.5".into(),
                    price: "$0.80/$4".into(),
                    context: "200K".into(),
                    note: "Quick tasks".into(),
                    enabled: true,
                },
            ]),
            effort: Some(EffortConfig {
                default: EffortLevel::Adaptive,
                planning: EffortLevel::High,
                coding: EffortLevel::Adaptive,
                qa_review: EffortLevel::Low,
            }),
            permissions: Some(vec![
                Permission {
                    name: "File read".into(),
                    enabled: true,
                },
                Permission {
                    name: "File write".into(),
                    enabled: true,
                },
                Permission {
                    name: "Bash commands".into(),
                    enabled: true,
                },
                Permission {
                    name: "Network access".into(),
                    enabled: false,
                },
                Permission {
                    name: "Git push".into(),
                    enabled: false,
                },
            ]),
            dangerous_ops: Some("Ask permission".into()),
        },
        "github-copilot-cli" => DisplayCapabilities {
            models: Some(vec![
                Model {
                    name: "Claude Opus 4.6".into(),
                    price: "1x".into(),
                    context: "1M ctx".into(),
                    note: "Deep reasoning".into(),
                    enabled: true,
                },
                Model {
                    name: "Claude Sonnet 4.6".into(),
                    price: "1x".into(),
                    context: "1M ctx".into(),
                    note: "Fast coding".into(),
                    enabled: true,
                },
                Model {
                    name: "GPT-5.3-Codex".into(),
                    price: "1x".into(),
                    context: "—".into(),
                    note: "Terminal workflows".into(),
                    enabled: true,
                },
                Model {
                    name: "GPT-5 mini".into(),
                    price: "included".into(),
                    context: "—".into(),
                    note: "Free with subscription".into(),
                    enabled: true,
                },
                Model {
                    name: "GPT-4.1".into(),
                    price: "included".into(),
                    context: "—".into(),
                    note: "General coding".into(),
                    enabled: true,
                },
                Model {
                    name: "Gemini 3 Pro".into(),
                    price: "1x".into(),
                    context: "—".into(),
                    note: "Large context, multimodal".into(),
                    enabled: true,
                },
            ]),
            effort: None,
            permissions: None,
            dangerous_ops: Some("Ask permission".into()),
        },
        "codex-acp" => DisplayCapabilities {
            models: Some(vec![
                Model {
                    name: "GPT-5.3-Codex".into(),
                    price: "—".into(),
                    context: "200K".into(),
                    note: "Terminal workflows, polyglot".into(),
                    enabled: true,
                },
                Model {
                    name: "o4-mini".into(),
                    price: "—".into(),
                    context: "200K".into(),
                    note: "Deep reasoning".into(),
                    enabled: true,
                },
                Model {
                    name: "o3".into(),
                    price: "—".into(),
                    context: "200K".into(),
                    note: "Advanced reasoning".into(),
                    enabled: true,
                },
                Model {
                    name: "GPT-4.1".into(),
                    price: "—".into(),
                    context: "200K".into(),
                    note: "General coding".into(),
                    enabled: true,
                },
            ]),
            effort: None,
            permissions: None,
            dangerous_ops: Some("Sandboxed".into()),
        },
        "gemini" => DisplayCapabilities {
            models: Some(vec![
                Model {
                    name: "Gemini 2.5 Pro".into(),
                    price: "free".into(),
                    context: "1M ctx".into(),
                    note: "Flagship, reasoning".into(),
                    enabled: true,
                },
                Model {
                    name: "Gemini 2.5 Flash".into(),
                    price: "free".into(),
                    context: "1M ctx".into(),
                    note: "Fast, cost-effective".into(),
                    enabled: true,
                },
                Model {
                    name: "Gemini 3 Flash".into(),
                    price: "free".into(),
                    context: "1M ctx".into(),
                    note: "Latest generation".into(),
                    enabled: true,
                },
            ]),
            effort: None,
            permissions: None,
            dangerous_ops: Some("Ask permission".into()),
        },
        _ => DisplayCapabilities {
            models: None,
            effort: None,
            permissions: None,
            dangerous_ops: Some("Unknown".into()),
        },
    }
}

fn usage(agent_id: &str) -> Usage {
    match agent_id {
        "claude-acp" => Usage::ClaudeCode {
            five_hour_pct: 0.0,
            five_hour_reset: "—".into(),
            weekly_pct: 0.0,
            weekly_reset: "—".into(),
            extra_usage_enabled: false,
            extra_usage_cost: 0.0,
        },
        _ => Usage::Unknown,
    }
}

fn badges(entry: &RegistryEntry) -> Vec<Badge> {
    let mut result = Vec::new();
    if entry.tags.contains(&"popular".to_string()) {
        result.push(Badge {
            label: "Popular".into(),
            kind: BadgeKind::Popular,
        });
    }
    if entry.is_open_source() {
        result.push(Badge {
            label: "OSS".into(),
            kind: BadgeKind::OpenSource,
        });
    }
    result
}

// ── Version detection ───────────────────────────────────────────────

/// Detect the installed version of an agent by running its version command.
pub async fn detect_installed_version(entry: &RegistryEntry) -> Option<VersionInfo> {
    use tokio::process::Command;

    if entry.is_npx() || entry.is_uvx() {
        return None;
    }

    // Map agent ID to version command
    let (cmd_args, is_wrapper, cli_name): (&[&str], bool, &str) = match entry.id.as_str() {
        "claude-acp" => (&["claude", "--version"], true, "Claude Code"),
        "github-copilot-cli" => (&["gh", "copilot", "--version"], false, ""),
        "codex-acp" => (&["codex", "--version"], true, "Codex CLI"),
        "gemini" => (&["gemini", "--version"], false, ""),
        _ => return None,
    };
    let (cmd, args) = cmd_args.split_first()?;

    let output = Command::new(cmd)
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .await
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let ver = extract_version_string(&format!("{stdout} {stderr}"))?;

    let display = if is_wrapper {
        format!("{cli_name} {ver} (adapter v{})", entry.version)
    } else {
        ver.clone()
    };

    Some(VersionInfo {
        version: ver,
        display,
        is_wrapper,
    })
}

fn extract_version_string(text: &str) -> Option<String> {
    for word in text.split_whitespace() {
        let trimmed = word
            .trim_start_matches('v')
            .trim_matches(|c: char| !c.is_ascii_digit() && c != '.');
        let parts: Vec<&str> = trimmed.split('.').collect();
        if parts.len() >= 2
            && parts
                .iter()
                .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
        {
            return Some(trimmed.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_version() {
        assert_eq!(extract_version_string("v1.28.0"), Some("1.28.0".into()));
        assert_eq!(
            extract_version_string("Claude Code CLI v2.3.1"),
            Some("2.3.1".into()),
        );
        assert_eq!(extract_version_string(""), None);
    }

    #[test]
    fn test_all_agents_have_metadata() {
        use crate::registry::Registry;

        // Test that all builtin agents have required metadata
        let registry = Registry::builtin();
        for entry in registry.list() {
            assert!(!entry.id.is_empty());
            assert!(!entry.display_name.is_empty());
            assert!(!entry.description.is_empty());
            assert!(!entry.authors.is_empty());
        }
    }
}
