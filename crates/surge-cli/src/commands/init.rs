use std::collections::HashMap;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Args;
use surge_core::approvals::{ApprovalChannelKind, ApprovalPolicy};
use surge_core::config::{
    InitConfig, McpServerConfig, SurgeConfig, Transport, WorktreeLocationConfig,
};
use surge_core::sandbox::SandboxMode;
use tracing::{debug, info, warn};

/// Arguments for `surge init`.
#[derive(Debug, Clone, Args)]
pub struct InitArgs {
    /// Write a complete safe default config without interactive prompts.
    #[arg(long)]
    pub default: bool,
}

/// Run first-time Surge project initialization.
pub fn run(args: InitArgs) -> Result<()> {
    let cwd = std::env::current_dir().context("resolve current directory")?;
    let config_path = cwd.join("surge.toml");
    debug!(
        path = %config_path.display(),
        cwd = %cwd.display(),
        default = args.default,
        "starting surge init"
    );

    if config_path.exists() {
        return run_existing_config(args, &config_path);
    }

    let mut config = build_default_config();
    emit_prerequisite_diagnostics(&cwd, &config);

    if !args.default {
        run_wizard(&mut config).context("run init wizard")?;
    }

    debug!(
        path = %config_path.display(),
        default_agent = %config.default_agent,
        agents = config.agents.len(),
        sandbox_default = ?config.init.sandbox_default,
        worktree_location = ?config.init.worktree_location,
        project_context_path = %config.init.project_context_path.display(),
        "writing generated surge.toml"
    );
    config
        .save(&config_path)
        .with_context(|| format!("write {}", config_path.display()))?;

    info!(path = %config_path.display(), "created surge.toml");
    println!("⚡ Created surge.toml");
    println!("   Default agent: {}", config.default_agent);
    println!(
        "   Project context: {}",
        config.init.project_context_path.display()
    );
    Ok(())
}

fn run_existing_config(args: InitArgs, config_path: &Path) -> Result<()> {
    debug!(path = %config_path.display(), "surge.toml already exists");
    let mut config = SurgeConfig::load(config_path).map_err(|e| {
        warn!(
            category = "config",
            reason = "invalid_existing_config",
            error = %e,
            path = %config_path.display(),
            "existing surge.toml failed validation"
        );
        anyhow::anyhow!(
            "Existing surge.toml is invalid: {e}\nRun `surge config show` after fixing the reported section."
        )
    })?;
    emit_prerequisite_diagnostics(
        &config_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from(".")),
        &config,
    );

    if args.default {
        info!(path = %config_path.display(), "existing surge.toml left unchanged");
        println!("⚡ surge.toml already exists; --default leaves it unchanged.");
        println!("   Default agent: {}", config.default_agent);
        return Ok(());
    }

    println!("⚡ surge.toml already exists");
    println!("   Default agent: {}", config.default_agent);
    println!(
        "   Project context: {}",
        config.init.project_context_path.display()
    );

    if !confirm("Edit onboarding defaults now?", false)? {
        info!(path = %config_path.display(), "existing init edit skipped by user");
        println!("No changes made.");
        return Ok(());
    }

    edit_existing_sections(&mut config)?;
    config.validate()?;
    write_existing_sections_preserving_comments(config_path, &config)
        .with_context(|| format!("update {}", config_path.display()))?;
    info!(path = %config_path.display(), "updated onboarding sections in existing surge.toml");
    println!("✅ Updated onboarding defaults in surge.toml");
    Ok(())
}

fn build_default_config() -> SurgeConfig {
    let mut config = SurgeConfig::default();
    let registry = surge_acp::Registry::builtin();
    let detected = registry.detect_installed_with_paths();
    debug!(
        detected = detected.len(),
        registry_entries = registry.len(),
        "detected installed registry agents"
    );

    let selected = select_default_registry_id(&detected)
        .or_else(|| registry.find("claude-acp").map(|entry| entry.id.clone()));

    if let Some(agent_id) = selected {
        if let Some(entry) = registry.find(&agent_id) {
            config.default_agent = entry.id.clone();
            config
                .agents
                .insert(entry.id.clone(), entry.to_agent_config());
            if !detected.iter().any(|agent| agent.entry.id == entry.id) {
                warn!(
                    agent_id = %entry.id,
                    reason = "no_detected_agent",
                    "using installable registry fallback for default agent"
                );
            }
        }
    }

    config
}

fn select_default_registry_id(detected: &[surge_acp::DetectedAgent]) -> Option<String> {
    const PREFERENCE: &[&str] = &["claude-acp", "codex-acp", "gemini", "github-copilot-cli"];
    for preferred in PREFERENCE {
        if detected.iter().any(|agent| agent.entry.id == *preferred) {
            debug!(agent_id = %preferred, "selected preferred detected agent");
            return Some((*preferred).to_string());
        }
    }
    detected.first().map(|agent| {
        debug!(agent_id = %agent.entry.id, "selected first detected agent");
        agent.entry.id.clone()
    })
}

fn emit_prerequisite_diagnostics(cwd: &Path, config: &SurgeConfig) {
    if surge_git::GitManager::discover().is_err() {
        warn!(
            category = "git",
            reason = "not_a_git_repository",
            cwd = %cwd.display(),
            "surge init is running outside a git repository"
        );
        println!("⚠️  Git repository not detected; run `git init` before starting AFK runs.");
    }

    if config.agents.is_empty() {
        warn!(
            category = "agents",
            reason = "no_agents_configured",
            "no ACP agents are configured"
        );
        println!("⚠️  No ACP agents configured. Use `surge registry detect` for install options.");
    } else if let Some(default_agent) = config.agents.get(&config.default_agent) {
        debug!(
            default_agent = %config.default_agent,
            command = %default_agent.command,
            args_len = default_agent.args.len(),
            "default agent configured"
        );
    }

    diagnose_default_agent_transport(config);
    diagnose_worktree_root(config);
    diagnose_telegram_placeholders(config);
}

fn diagnose_default_agent_transport(config: &SurgeConfig) {
    let Some(default_agent) = config.agents.get(&config.default_agent) else {
        return;
    };
    if matches!(default_agent.transport, Transport::WebSocket { .. }) {
        warn!(
            category = "agents",
            reason = "unsupported_transport",
            agent = %config.default_agent,
            "default agent uses unsupported websocket transport"
        );
        println!(
            "⚠️  Default agent '{}' uses ws transport; stdio or tcp is required today.",
            config.default_agent
        );
    }
}

fn diagnose_worktree_root(config: &SurgeConfig) {
    if config.init.worktree_location != WorktreeLocationConfig::Custom {
        return;
    }
    let root = &config.init.worktree_root;
    if root.as_os_str().is_empty() {
        warn!(
            category = "worktrees",
            reason = "empty_worktree_root",
            "custom worktree root is empty"
        );
        println!("⚠️  Custom worktree root is empty; choose a writable directory.");
        return;
    }
    let parent = root.parent().unwrap_or_else(|| Path::new("."));
    if !parent.exists() {
        warn!(
            category = "worktrees",
            reason = "missing_worktree_parent",
            parent = %parent.display(),
            "custom worktree parent does not exist"
        );
        println!(
            "⚠️  Custom worktree parent '{}' does not exist yet.",
            parent.display()
        );
    } else {
        debug!(worktree_root = %root.display(), "custom worktree root looks usable");
    }
}

fn diagnose_telegram_placeholders(config: &SurgeConfig) {
    let Some(telegram) = &config.telegram else {
        return;
    };
    debug!("Telegram env-var placeholders configured");
    for env_name in [&telegram.chat_id_env, &telegram.bot_token_env]
        .into_iter()
        .flatten()
    {
        if std::env::var_os(env_name).is_none() {
            warn!(
                category = "telegram",
                reason = "missing_env_var",
                env = %env_name,
                "Telegram env-var placeholder is not set"
            );
            println!("⚠️  Set {env_name} before using Telegram approvals.");
        }
    }
}

fn run_wizard(config: &mut SurgeConfig) -> Result<()> {
    debug!("entering interactive init wizard");
    println!("⚡ Surge project setup");
    prompt_agent(config)?;
    prompt_sandbox(&mut config.init)?;
    prompt_worktree(&mut config.init)?;
    prompt_approvals(config)?;
    prompt_mcp_server(config)?;
    info!("completed interactive init wizard");
    Ok(())
}

fn edit_existing_sections(config: &mut SurgeConfig) -> Result<()> {
    debug!("editing existing init sections");
    prompt_sandbox(&mut config.init)?;
    prompt_worktree(&mut config.init)?;
    prompt_approvals(config)?;
    Ok(())
}

fn prompt_agent(config: &mut SurgeConfig) -> Result<()> {
    debug!("wizard step: agent");
    let registry = surge_acp::Registry::builtin();
    let detected = registry.detect_installed_with_paths();

    if detected.is_empty() {
        warn!(
            category = "agents",
            reason = "none_detected",
            "no installed ACP-compatible agents detected"
        );
        println!("No installed ACP agents detected; using claude-acp as installable fallback.");
        return Ok(());
    }

    println!("\nDetected agents:");
    for (idx, agent) in detected.iter().enumerate() {
        println!(
            "  {}. {} ({})",
            idx + 1,
            agent.entry.display_name,
            agent.entry.id
        );
    }
    let choice = prompt("Choose default agent", "1")?;
    let index = choice.parse::<usize>().unwrap_or(1).saturating_sub(1);
    let selected = detected.get(index).unwrap_or(&detected[0]);
    debug!(agent_id = %selected.entry.id, "wizard selected default agent");
    config.default_agent = selected.entry.id.clone();
    config
        .agents
        .insert(selected.entry.id.clone(), selected.entry.to_agent_config());
    Ok(())
}

fn prompt_sandbox(init: &mut InitConfig) -> Result<()> {
    debug!("wizard step: sandbox");
    println!("\nSandbox default:");
    println!("  1. workspace-write");
    println!("  2. read-only");
    println!("  3. workspace-network");
    println!("  4. full-access");
    let choice = prompt("Choose sandbox", "1")?;
    init.sandbox_default = match choice.as_str() {
        "2" | "read-only" => SandboxMode::ReadOnly,
        "3" | "workspace-network" => SandboxMode::WorkspaceNetwork,
        "4" | "full-access" => SandboxMode::FullAccess,
        _ => SandboxMode::WorkspaceWrite,
    };
    debug!(sandbox_default = ?init.sandbox_default, "wizard selected sandbox");
    Ok(())
}

fn prompt_worktree(init: &mut InitConfig) -> Result<()> {
    debug!("wizard step: worktree");
    println!("\nManaged worktree location:");
    println!("  1. sibling (.surge-worktrees next to repo)");
    println!("  2. central (~/.surge/runs)");
    println!("  3. custom root");
    let choice = prompt("Choose worktree location", "1")?;
    init.worktree_location = match choice.as_str() {
        "2" | "central" => WorktreeLocationConfig::Central,
        "3" | "custom" => {
            let root = prompt("Custom worktree root", ".surge-worktrees")?;
            init.worktree_root = PathBuf::from(root);
            WorktreeLocationConfig::Custom
        },
        _ => WorktreeLocationConfig::Sibling,
    };
    debug!(
        worktree_location = ?init.worktree_location,
        worktree_root = %init.worktree_root.display(),
        "wizard selected worktree defaults"
    );
    Ok(())
}

fn prompt_approvals(config: &mut SurgeConfig) -> Result<()> {
    debug!("wizard step: approvals");
    let mut channels = vec![ApprovalChannelKind::Desktop];
    if confirm("Enable Telegram approval placeholders?", false)? {
        channels.push(ApprovalChannelKind::Telegram);
        config.telegram = Some(surge_core::config::TelegramConfig {
            chat_id_env: Some("SURGE_TELEGRAM_CHAT_ID".to_string()),
            bot_token_env: Some("SURGE_TELEGRAM_BOT_TOKEN".to_string()),
            chat_id: None,
        });
        config.inbox.delivery_channels = vec!["telegram".to_string(), "desktop".to_string()];
        debug!("Telegram approval placeholders enabled");
    }
    config.init.approval_channels = channels;
    config.init.approval_policy = ApprovalPolicy::OnRequest;
    debug!(
        approval_channels = ?config.init.approval_channels,
        approval_policy = ?config.init.approval_policy,
        "wizard selected approval defaults"
    );
    Ok(())
}

fn prompt_mcp_server(config: &mut SurgeConfig) -> Result<()> {
    debug!("wizard step: mcp");
    if !confirm("Add one MCP server to the default agent?", false)? {
        debug!("wizard skipped MCP server setup");
        return Ok(());
    }
    let name = prompt("MCP server name", "filesystem")?;
    let command = prompt("MCP server command", "npx")?;
    let args = prompt(
        "MCP server args",
        "@modelcontextprotocol/server-filesystem .",
    )?
    .split_whitespace()
    .map(str::to_string)
    .collect();
    let server = McpServerConfig {
        name,
        command,
        args,
        env: HashMap::new(),
    };
    if let Some(agent) = config.agents.get_mut(&config.default_agent) {
        agent.mcp_servers.push(server);
        debug!(agent = %config.default_agent, "added MCP server to default agent");
    }
    Ok(())
}

fn prompt(label: &str, default: &str) -> Result<String> {
    print!("{label} [{default}]: ");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let trimmed = input.trim();
    if trimmed.is_empty() {
        Ok(default.to_string())
    } else {
        Ok(trimmed.to_string())
    }
}

fn confirm(label: &str, default_yes: bool) -> Result<bool> {
    let default = if default_yes { "Y/n" } else { "y/N" };
    let answer = prompt(label, default)?;
    if answer == default {
        return Ok(default_yes);
    }
    Ok(matches!(answer.to_ascii_lowercase().as_str(), "y" | "yes"))
}

fn write_existing_sections_preserving_comments(path: &Path, config: &SurgeConfig) -> Result<()> {
    debug!(path = %path.display(), "updating existing TOML document with toml_edit");
    let contents = std::fs::read_to_string(path)?;
    let mut doc = contents.parse::<toml_edit::DocumentMut>()?;
    upsert_init_table(&mut doc, &config.init);
    upsert_telegram_table(&mut doc, config);
    upsert_inbox_table(&mut doc, config);
    std::fs::write(path, doc.to_string())?;
    Ok(())
}

fn upsert_init_table(doc: &mut toml_edit::DocumentMut, init: &InitConfig) {
    let mut table = toml_edit::Table::new();
    table["sandbox_default"] = toml_edit::value(sandbox_mode_value(init.sandbox_default));
    table["worktree_location"] = toml_edit::value(worktree_location_value(init.worktree_location));
    table["worktree_root"] = toml_edit::value(init.worktree_root.to_string_lossy().to_string());
    table["approval_policy"] = toml_edit::value(approval_policy_value(init.approval_policy));
    table["approval_channels"] = toml_edit::value(channel_array(&init.approval_channels));
    table["project_context_path"] =
        toml_edit::value(init.project_context_path.to_string_lossy().to_string());
    table["project_context_auto_seed"] = toml_edit::value(init.project_context_auto_seed);
    table["created_by"] = toml_edit::value(init.created_by.clone());
    doc["init"] = toml_edit::Item::Table(table);
}

fn upsert_telegram_table(doc: &mut toml_edit::DocumentMut, config: &SurgeConfig) {
    let Some(telegram) = &config.telegram else {
        return;
    };
    let mut table = toml_edit::Table::new();
    if let Some(chat_id_env) = &telegram.chat_id_env {
        table["chat_id_env"] = toml_edit::value(chat_id_env.clone());
    }
    if let Some(bot_token_env) = &telegram.bot_token_env {
        table["bot_token_env"] = toml_edit::value(bot_token_env.clone());
    }
    doc["telegram"] = toml_edit::Item::Table(table);
}

fn upsert_inbox_table(doc: &mut toml_edit::DocumentMut, config: &SurgeConfig) {
    let mut table = toml_edit::Table::new();
    table["snooze_poll_interval_seconds"] =
        toml_edit::value(i64::try_from(config.inbox.snooze_poll_interval.as_secs()).unwrap_or(300));
    if !config.inbox.delivery_channels.is_empty() {
        let mut array = toml_edit::Array::default();
        for channel in &config.inbox.delivery_channels {
            array.push(channel.as_str());
        }
        table["delivery_channels"] = toml_edit::value(array);
    }
    doc["inbox"] = toml_edit::Item::Table(table);
}

fn channel_array(channels: &[ApprovalChannelKind]) -> toml_edit::Array {
    let mut array = toml_edit::Array::default();
    for channel in channels {
        array.push(channel.as_str());
    }
    array
}

fn sandbox_mode_value(mode: SandboxMode) -> &'static str {
    match mode {
        SandboxMode::ReadOnly => "read-only",
        SandboxMode::WorkspaceWrite => "workspace-write",
        SandboxMode::WorkspaceNetwork => "workspace-network",
        SandboxMode::FullAccess => "full-access",
        SandboxMode::Custom => "custom",
        // SandboxMode is `#[non_exhaustive]`; surface unknown variants as a
        // safe default rather than panicking. New variants need an explicit
        // arm before they can be written to surge.toml.
        _ => "workspace-write",
    }
}

fn worktree_location_value(location: WorktreeLocationConfig) -> &'static str {
    match location {
        WorktreeLocationConfig::Sibling => "sibling",
        WorktreeLocationConfig::Central => "central",
        WorktreeLocationConfig::Custom => "custom",
    }
}

fn approval_policy_value(policy: ApprovalPolicy) -> &'static str {
    match policy {
        ApprovalPolicy::Untrusted => "untrusted",
        ApprovalPolicy::OnRequest => "on-request",
        ApprovalPolicy::Never => "never",
    }
}
