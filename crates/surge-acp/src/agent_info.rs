//! Agent display information — business logic for UI presentation.
//!
//! All agent metadata (colors, models, taglines, version commands) is
//! hardcoded here for the 4 supported agents. No external JSON files.

use crate::health::AgentHealth;
use crate::registry::{DetectedAgent, RegistryEntry};

// ── Display models ──────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ModelOption {
    pub name: String,
    pub price: String,
    pub context: String,
    pub note: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffortLevel { High, Medium, Low, Adaptive }

impl EffortLevel {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self { Self::High => "High", Self::Medium => "Medium", Self::Low => "Low", Self::Adaptive => "Adaptive" }
    }
}

#[derive(Debug, Clone)]
pub struct PermissionSetting {
    pub name: String,
    pub enabled: bool,
}

#[derive(Debug, Clone)]
pub struct AgentEffortConfig {
    pub default: EffortLevel,
    pub planning: EffortLevel,
    pub coding: EffortLevel,
    pub qa_review: EffortLevel,
}

#[derive(Debug, Clone)]
pub struct AgentCapabilities {
    pub models: Option<Vec<ModelOption>>,
    pub effort: Option<AgentEffortConfig>,
    pub permissions: Option<Vec<PermissionSetting>>,
    pub dangerous_ops: Option<String>,
}

#[derive(Debug, Clone)]
pub enum AgentUsage {
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
pub enum SessionStatus { Running, Completed, Failed }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallStatus { Installed, Npx, Uvx }

impl InstallStatus {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self { Self::Installed => "Installed", Self::Npx => "npx", Self::Uvx => "uvx" }
    }
}

#[derive(Debug, Clone)]
pub struct ConfiguredAgent {
    pub name: String,
    pub display_name: String,
    pub description: String,
    pub model: Option<String>,
    pub binary: String,
    pub registry_version: String,
    pub installed_version: Option<String>,
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
    pub runnable: bool,
    pub run_via: Option<InstallStatus>,
}

#[derive(Debug, Clone)]
pub struct AgentBadge {
    pub label: String,
    pub kind: BadgeKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BadgeKind { Popular, OpenSource, Free, New }

#[derive(Debug, Clone)]
pub struct VersionInfo {
    pub version: String,
    pub display: String,
    pub is_wrapper: bool,
}

// ── Builders ────────────────────────────────────────────────────────

#[must_use]
pub fn build_configured_agent(
    detected: &DetectedAgent,
    health: Option<&AgentHealth>,
) -> ConfiguredAgent {
    let (requests, latency, failures) = match health {
        Some(h) => (h.total_requests, h.avg_latency_ms, h.total_failures),
        None => (0, 0, 0),
    };

    let id = &detected.entry.id;
    let install_status = if detected.entry.is_npx() { InstallStatus::Npx }
        else if detected.entry.is_uvx() { InstallStatus::Uvx }
        else { InstallStatus::Installed };

    ConfiguredAgent {
        name: id.clone(),
        display_name: agent_display_name(id).unwrap_or(&detected.entry.display_name).to_string(),
        description: agent_tagline(id).unwrap_or(&detected.entry.description).to_string(),
        model: agent_default_model(id).map(String::from),
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
        capabilities: build_capabilities(id),
        usage: build_usage(id),
        subtasks_completed: 0,
        subtasks_failed: failures as u32,
        avg_subtask_secs: 0,
        qa_first_pass_rate: 0.0,
        uptime: "—".into(),
        last_seen: None,
        recent_sessions: vec![],
    }
}

#[must_use]
pub fn build_available_agent(entry: &RegistryEntry) -> AvailableAgent {
    let runnable = entry.is_runnable();
    let run_via = if entry.is_npx() { Some(InstallStatus::Npx) }
        else if entry.is_uvx() { Some(InstallStatus::Uvx) }
        else if runnable { Some(InstallStatus::Installed) }
        else { None };

    AvailableAgent {
        name: entry.id.clone(),
        display_name: agent_display_name(&entry.id).unwrap_or(&entry.display_name).to_string(),
        vendor: agent_vendor(&entry.id).unwrap_or_else(|| entry.vendor()).to_string(),
        description: agent_tagline(&entry.id).unwrap_or(&entry.description).to_string(),
        license: entry.license.clone(),
        install_command: entry.install_instructions.clone(),
        install_method: if entry.is_npx() { "npx" } else if entry.is_uvx() { "uvx" } else { "binary" }.into(),
        badges: build_badges(entry),
        runnable,
        run_via,
    }
}

// ── Agent metadata (hardcoded for 4 agents) ─────────────────────────

fn agent_display_name(id: &str) -> Option<&'static str> {
    match id {
        "claude-acp" => Some("Claude Agent"),
        "github-copilot-cli" => Some("GitHub Copilot"),
        "codex-acp" => Some("Codex CLI"),
        "gemini" => Some("Gemini CLI"),
        _ => None,
    }
}

fn agent_tagline(id: &str) -> Option<&'static str> {
    match id {
        "claude-acp" => Some("Anthropic's autonomous coding agent — deepest reasoning, largest context"),
        "github-copilot-cli" => Some("GitHub's multi-model terminal agent with native repo integration"),
        "codex-acp" => Some("OpenAI's cloud-native coding agent with sandboxed parallel execution"),
        "gemini" => Some("Google's CLI with the most generous free tier — 1M context on all models"),
        _ => None,
    }
}

fn agent_vendor(id: &str) -> Option<&'static str> {
    match id {
        "claude-acp" => Some("Anthropic"),
        "github-copilot-cli" => Some("GitHub"),
        "codex-acp" => Some("OpenAI"),
        "gemini" => Some("Google"),
        _ => None,
    }
}

fn agent_default_model(id: &str) -> Option<&'static str> {
    match id {
        "claude-acp" => Some("Claude Sonnet 4.6"),
        "github-copilot-cli" => Some("GPT-5 mini"),
        "codex-acp" => Some("GPT-5.3-Codex"),
        "gemini" => Some("Gemini 2.5 Pro"),
        _ => None,
    }
}

/// Vendor brand color as (r, g, b) in 0.0–1.0.
#[must_use]
pub fn vendor_color(id: &str) -> Option<(f32, f32, f32)> {
    match id {
        "claude-acp"        => Some((0.851, 0.467, 0.341)),  // #D97757
        "github-copilot-cli"=> Some((0.431, 0.251, 0.788)),  // #6E40C9
        "codex-acp"         => Some((0.063, 0.639, 0.498)),  // #10A37F
        "gemini"            => Some((0.259, 0.522, 0.957)),  // #4285F4
        _ => None,
    }
}

/// Version detection command for each agent.
fn version_command(id: &str) -> Option<(&'static [&'static str], bool, &'static str)> {
    // Returns (command+args, is_wrapper, wrapped_cli_name)
    match id {
        "claude-acp"         => Some((&["claude", "--version"], true, "Claude Code")),
        "github-copilot-cli" => Some((&["gh", "copilot", "--version"], false, "")),
        "codex-acp"          => Some((&["codex", "--version"], true, "Codex CLI")),
        "gemini"             => Some((&["gemini", "--version"], false, "")),
        _ => None,
    }
}

fn build_capabilities(id: &str) -> AgentCapabilities {
    match id {
        "claude-acp" => AgentCapabilities {
            models: Some(vec![
                ModelOption { name: "Claude Opus 4.6".into(), price: "$5/$25".into(), context: "1M ctx".into(), note: "Deep reasoning".into(), enabled: true },
                ModelOption { name: "Claude Sonnet 4.6".into(), price: "$3/$15".into(), context: "1M ctx".into(), note: "Daily driver".into(), enabled: true },
                ModelOption { name: "Claude Haiku 4.5".into(), price: "$0.80/$4".into(), context: "200K".into(), note: "Quick tasks".into(), enabled: true },
            ]),
            effort: Some(AgentEffortConfig {
                default: EffortLevel::Adaptive, planning: EffortLevel::High,
                coding: EffortLevel::Adaptive, qa_review: EffortLevel::Low,
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
        "github-copilot-cli" => AgentCapabilities {
            models: Some(vec![
                ModelOption { name: "Claude Opus 4.6".into(), price: "1x".into(), context: "1M ctx".into(), note: "Deep reasoning".into(), enabled: true },
                ModelOption { name: "Claude Sonnet 4.6".into(), price: "1x".into(), context: "1M ctx".into(), note: "Fast coding".into(), enabled: true },
                ModelOption { name: "GPT-5.3-Codex".into(), price: "1x".into(), context: "—".into(), note: "Terminal workflows".into(), enabled: true },
                ModelOption { name: "GPT-5 mini".into(), price: "included".into(), context: "—".into(), note: "Free with subscription".into(), enabled: true },
                ModelOption { name: "GPT-4.1".into(), price: "included".into(), context: "—".into(), note: "General coding".into(), enabled: true },
                ModelOption { name: "Gemini 3 Pro".into(), price: "1x".into(), context: "—".into(), note: "Large context, multimodal".into(), enabled: true },
            ]),
            effort: None,
            permissions: None,
            dangerous_ops: Some("Ask permission".into()),
        },
        "codex-acp" => AgentCapabilities {
            models: Some(vec![
                ModelOption { name: "GPT-5.3-Codex".into(), price: "—".into(), context: "200K".into(), note: "Terminal workflows, polyglot".into(), enabled: true },
                ModelOption { name: "o4-mini".into(), price: "—".into(), context: "200K".into(), note: "Deep reasoning".into(), enabled: true },
                ModelOption { name: "o3".into(), price: "—".into(), context: "200K".into(), note: "Advanced reasoning".into(), enabled: true },
                ModelOption { name: "GPT-4.1".into(), price: "—".into(), context: "200K".into(), note: "General coding".into(), enabled: true },
            ]),
            effort: None,
            permissions: None,
            dangerous_ops: Some("Sandboxed".into()),
        },
        "gemini" => AgentCapabilities {
            models: Some(vec![
                ModelOption { name: "Gemini 2.5 Pro".into(), price: "free".into(), context: "1M ctx".into(), note: "Flagship, reasoning".into(), enabled: true },
                ModelOption { name: "Gemini 2.5 Flash".into(), price: "free".into(), context: "1M ctx".into(), note: "Fast, cost-effective".into(), enabled: true },
                ModelOption { name: "Gemini 3 Flash".into(), price: "free".into(), context: "1M ctx".into(), note: "Latest generation".into(), enabled: true },
            ]),
            effort: None,
            permissions: None,
            dangerous_ops: Some("Ask permission".into()),
        },
        _ => AgentCapabilities { models: None, effort: None, permissions: None, dangerous_ops: None },
    }
}

fn build_usage(id: &str) -> AgentUsage {
    match id {
        "claude-acp" => AgentUsage::ClaudeCode {
            five_hour_pct: 0.0, five_hour_reset: "—".into(),
            weekly_pct: 0.0, weekly_reset: "—".into(),
            extra_usage_enabled: false, extra_usage_cost: 0.0,
        },
        _ => AgentUsage::Unknown,
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
    badges
}

// ── Version detection ───────────────────────────────────────────────

/// Detect the installed version of an agent by running its version command.
pub async fn detect_installed_version(entry: &RegistryEntry) -> Option<VersionInfo> {
    use tokio::process::Command;

    if entry.is_npx() || entry.is_uvx() {
        return None;
    }

    let (cmd_args, is_wrapper, cli_name) = version_command(&entry.id)?;
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

    Some(VersionInfo { version: ver, display, is_wrapper })
}

fn extract_version_string(text: &str) -> Option<String> {
    for word in text.split_whitespace() {
        let trimmed = word.trim_start_matches('v')
            .trim_matches(|c: char| !c.is_ascii_digit() && c != '.');
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
    fn test_vendor_colors() {
        assert!(vendor_color("claude-acp").is_some());
        assert!(vendor_color("gemini").is_some());
        assert!(vendor_color("unknown").is_none());
    }

    #[test]
    fn test_extract_version() {
        assert_eq!(extract_version_string("v1.28.0"), Some("1.28.0".into()));
        assert_eq!(extract_version_string("Claude Code CLI v2.3.1"), Some("2.3.1".into()));
        assert_eq!(extract_version_string(""), None);
    }

    #[test]
    fn test_all_agents_have_metadata() {
        for id in &["claude-acp", "github-copilot-cli", "codex-acp", "gemini"] {
            assert!(agent_display_name(id).is_some(), "{id} missing display_name");
            assert!(agent_tagline(id).is_some(), "{id} missing tagline");
            assert!(agent_vendor(id).is_some(), "{id} missing vendor");
            assert!(agent_default_model(id).is_some(), "{id} missing default model");
            assert!(vendor_color(id).is_some(), "{id} missing color");
            assert!(version_command(id).is_some(), "{id} missing version_command");
        }
    }
}
