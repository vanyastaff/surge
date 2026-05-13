// Pre-existing legacy code; M5 does not modify surge-cli.  Suppress pedantic
// lints that activate because -D clippy::pedantic is now applied to the engine
// module (surge-orchestrator), which Rust propagates workspace-wide with
// `cargo clippy --workspace -- -D warnings`.
#![allow(clippy::excessive_nesting)]
#![allow(clippy::identity_op)]

use std::io::{self, Write as _};

use anyhow::Result;
use clap::{Parser, Subcommand};
use surge_core::SurgeConfig;
use tokio::signal;

mod commands;
mod legacy_spec;

use commands::{
    agent::AgentCommands, analytics::AnalyticsCommands, bootstrap::BootstrapArgs,
    config::ConfigCommands, feature::FeatureCommands, init::InitArgs, insights::InsightsCommands,
    memory::MemoryCommands, migrate_spec::MigrateSpecArgs, project::ProjectCommands,
    registry::RegistryCommands, tracker::TrackerCommand,
};

#[derive(Parser)]
#[command(
    name = "surge",
    version,
    about = "⚡ Any Agent. One Protocol. Pure Rust."
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Check connection to an agent
    Ping {
        /// Agent name from config (default: uses default_agent)
        #[arg(short, long)]
        agent: Option<String>,
    },

    /// Send a one-shot prompt to an agent and stream the response
    Prompt {
        /// The prompt text
        message: String,

        /// Agent name
        #[arg(short, long)]
        agent: Option<String>,
    },

    /// Manage agents
    Agent {
        #[command(subcommand)]
        command: AgentCommands,
    },

    /// Validate and inspect Surge artifacts.
    Artifact {
        #[command(subcommand)]
        command: commands::artifact::ArtifactCommands,
    },

    /// Translate a legacy `.spec.toml` into a `flow.toml` document.
    MigrateSpec(MigrateSpecArgs),

    /// Clean up orphaned worktrees and merged branches
    Clean {
        /// Skip confirmation
        #[arg(short = 'y', long)]
        yes: bool,
    },

    /// List active worktrees
    Worktrees,

    /// Manage configuration
    Config {
        #[command(subcommand)]
        command: ConfigCommands,
    },

    /// Initialize surge.toml in current directory
    Init(InitArgs),

    /// Browse and add agents from the built-in registry
    Registry {
        #[command(subcommand)]
        command: RegistryCommands,
    },

    /// View insights and analytics
    Insights {
        #[command(subcommand)]
        command: InsightsCommands,
    },

    /// Manage project memory and knowledge base
    Memory {
        #[command(subcommand)]
        command: MemoryCommands,
    },

    /// View token and cost analytics
    Analytics {
        #[command(subcommand)]
        command: AnalyticsCommands,
    },

    /// Bootstrap an adaptive flow from a free-form prompt.
    Bootstrap(BootstrapArgs),

    /// Draft and apply roadmap amendments from follow-up feature requests.
    Feature {
        #[command(subcommand)]
        command: FeatureCommands,
    },

    /// New M6 engine commands — runs flow.toml graphs in-process.
    Engine {
        #[command(subcommand)]
        command: commands::engine::EngineCommands,
    },

    /// Manage issue-tracker integration (list sources, test connectivity).
    Tracker {
        #[command(subcommand)]
        cmd: TrackerCommand,
    },

    /// Manage the long-running surge-daemon process.
    Daemon {
        #[command(subcommand)]
        command: commands::daemon::DaemonCommands,
    },

    /// Diagnose ACP agents, runtime versions, and the sandbox-delegation matrix.
    Doctor {
        #[command(subcommand)]
        command: commands::doctor::DoctorCommands,
    },

    /// List, show, validate, or scaffold profiles.
    Profile {
        #[command(subcommand)]
        command: commands::profile::ProfileCommands,
    },

    /// Manage project-level context artifacts.
    Project {
        #[command(subcommand)]
        command: ProjectCommands,
    },
}

/// Set up signal handlers for graceful shutdown.
///
/// Listens for SIGINT (Ctrl+C) and SIGTERM and triggers graceful shutdown.
async fn setup_signal_handler() {
    #[cfg(unix)]
    {
        let mut sigterm = match signal::unix::signal(signal::unix::SignalKind::terminate()) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("failed to install SIGTERM handler: {e}");
                return;
            },
        };
        let mut sigint = match signal::unix::signal(signal::unix::SignalKind::interrupt()) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("failed to install SIGINT handler: {e}");
                return;
            },
        };

        tokio::select! {
            _ = sigterm.recv() => {
                eprintln!("\n⚡ Received SIGTERM. Shutting down gracefully...");
            }
            _ = sigint.recv() => {
                eprintln!("\n⚡ Received SIGINT. Shutting down gracefully...");
            }
        }
    }

    #[cfg(not(unix))]
    {
        if let Err(e) = signal::ctrl_c().await {
            tracing::error!("failed to install Ctrl+C handler: {e}");
            return;
        }
        eprintln!("\n⚡ Received Ctrl+C. Shutting down gracefully...");
    }
}

/// Check for orphaned worktrees at startup and prompt user for cleanup.
///
/// Returns `true` if cleanup was performed or if no orphans were found.
/// Returns `false` if user declined cleanup.
fn check_and_cleanup_orphans() -> Result<bool> {
    // Try to discover a git repo - if not found, skip orphan check
    let mgr = match surge_git::GitManager::discover() {
        Ok(m) => m,
        Err(_) => return Ok(true), // Not a git repo, skip check
    };

    let scanner = surge_git::OrphanScanner::new(mgr);
    let report = scanner.scan()?;

    if report.is_empty() {
        return Ok(true);
    }

    // Found orphans - prompt user
    let count = report.total_count();
    println!(
        "⚡ Found {} orphaned worktree{}. Clean up? [Y/n]",
        count,
        if count == 1 { "" } else { "s" }
    );

    // Read user input
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let input = input.trim().to_lowercase();

    // Default to yes if user just presses enter
    if input.is_empty() || input == "y" || input == "yes" {
        // Rediscover git manager for cleanup
        let mgr = surge_git::GitManager::discover()?;

        // Enable audit logging to .surge/cleanup.log
        let audit_path = mgr.repo_path().join(".surge").join("cleanup.log");
        let audit = surge_git::CleanupAudit::new(audit_path)?;
        let lifecycle = surge_git::LifecycleManager::with_audit(mgr, audit);

        let cleanup_report = lifecycle.full_cleanup()?;

        if cleanup_report.removed_worktrees.is_empty() && cleanup_report.removed_branches.is_empty()
        {
            println!("✅ Nothing to clean up");
        } else {
            for wt in &cleanup_report.removed_worktrees {
                println!("  Removed worktree: {wt}");
            }
            for br in &cleanup_report.removed_branches {
                println!("  Deleted branch: {br}");
            }
            println!("✅ Cleanup complete");
        }
        Ok(true)
    } else {
        println!("Skipping cleanup. Run 'surge clean -y' to clean up later.");
        Ok(false)
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "surge=info".into()),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();

    // Check for orphaned worktrees at startup (skip for certain commands)
    let should_check_orphans = !matches!(
        cli.command,
        Commands::Init(_)
            | Commands::Clean { .. }
            | Commands::Config { .. }
            | Commands::Bootstrap(_)
            | Commands::Feature { .. }
            | Commands::Engine { .. }
            | Commands::Artifact { .. }
            | Commands::MigrateSpec(_)
            | Commands::Tracker { .. }
            | Commands::Daemon { .. }
            | Commands::Doctor { .. }
            | Commands::Profile { .. }
            | Commands::Project { .. }
    );

    if should_check_orphans {
        // Run orphan check - if it fails, just log and continue
        let _ = check_and_cleanup_orphans();
    }

    // Run command with signal handling
    tokio::select! {
        result = run_command(cli.command) => {
            result
        }
        _ = setup_signal_handler() => {
            // Signal received, exit gracefully
            std::process::exit(130); // Standard exit code for SIGINT
        }
    }
}

/// Execute the CLI command.
async fn run_command(command: Commands) -> Result<()> {
    match command {
        Commands::Ping { agent } => {
            let mut config = SurgeConfig::load_or_default()?;
            config.apply_env_overrides();

            let agent_name = agent
                .as_deref()
                .unwrap_or(&config.default_agent)
                .to_string();

            if !config.agents.contains_key(&agent_name) {
                anyhow::bail!("Agent '{}' not found in configuration", agent_name);
            }

            println!("⚡ Pinging agent '{agent_name}'...");

            let cwd = std::env::current_dir()?;
            let pool = surge_acp::AgentPool::new(
                config.agents.clone(),
                config.default_agent.clone(),
                cwd,
                surge_acp::PermissionPolicy::default(),
                config.resilience.clone(),
            )?;

            let result = pool.ping(&agent_name).await;
            pool.shutdown().await;

            match result {
                Ok(()) => {
                    println!("✅ Agent '{agent_name}' is responsive");
                },
                Err(e) => {
                    println!("❌ Agent '{agent_name}' failed: {e}");
                    std::process::exit(2);
                },
            }
        },

        Commands::Prompt { message, agent } => {
            let mut config = SurgeConfig::load_or_default()?;
            config.apply_env_overrides();

            let agent_name = agent
                .as_deref()
                .unwrap_or(&config.default_agent)
                .to_string();

            if !config.agents.contains_key(&agent_name) {
                anyhow::bail!("Agent '{}' not found in configuration", agent_name);
            }

            println!("⚡ Sending to '{agent_name}'...\n");

            let cwd = std::env::current_dir()?;
            let pool = surge_acp::AgentPool::new(
                config.agents.clone(),
                config.default_agent.clone(),
                cwd.clone(),
                surge_acp::PermissionPolicy::default(),
                config.resilience.clone(),
            )?;

            // Subscribe to events before creating the session so we don't miss chunks
            let mut events = pool.subscribe();
            let print_task = tokio::spawn(async move {
                while let Ok(event) = events.recv().await {
                    if let surge_core::SurgeEvent::AgentMessageChunk { text, .. } = event {
                        print!("{text}");
                        let _ = std::io::stdout().flush();
                    }
                }
            });

            let session = pool.create_session(Some(&agent_name), None, &cwd).await?;

            let content = vec![agent_client_protocol::ContentBlock::Text(
                agent_client_protocol::TextContent::new(message),
            )];

            let response = pool.prompt(&session, content).await?;

            pool.shutdown().await;
            // Give the print task a moment to flush remaining chunks before exiting
            let _ = tokio::time::timeout(std::time::Duration::from_millis(100), print_task).await;

            println!("\n\n✅ Done (stop_reason: {:?})", response.stop_reason);
        },

        Commands::Agent { command } => {
            commands::agent::run(command).await?;
        },

        Commands::Artifact { command } => {
            commands::artifact::run(command)?;
        },

        Commands::MigrateSpec(args) => {
            commands::migrate_spec::run(args)?;
        },

        Commands::Clean { yes } => {
            commands::git::clean(yes)?;
        },

        Commands::Worktrees => {
            commands::git::worktrees()?;
        },

        Commands::Config { command } => {
            commands::config::run(command)?;
        },

        Commands::Registry { command } => {
            commands::registry::run(command).await?;
        },

        Commands::Insights { command } => {
            commands::insights::run(command)?;
        },

        Commands::Memory { command } => {
            commands::memory::run(command)?;
        },

        Commands::Analytics { command } => {
            commands::analytics::run(command)?;
        },

        Commands::Bootstrap(args) => {
            commands::bootstrap::run(args).await?;
        },

        Commands::Feature { command } => {
            commands::feature::run(command).await?;
        },

        Commands::Engine { command } => {
            commands::engine::run(command).await?;
        },

        Commands::Tracker { cmd } => {
            let config = SurgeConfig::load_or_default()?;
            commands::tracker::run(cmd, config).await?;
        },

        Commands::Daemon { command } => {
            commands::daemon::run(command).await?;
        },

        Commands::Doctor { command } => {
            commands::doctor::run(command).await?;
        },

        Commands::Profile { command } => {
            commands::profile::run(command).await?;
        },

        Commands::Project { command } => {
            commands::project::run(command).await?;
        },

        Commands::Init(args) => {
            commands::init::run(args)?;
        },
    }

    Ok(())
}
