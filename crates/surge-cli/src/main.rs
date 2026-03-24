use std::io::{self, Write as _};

use anyhow::Result;
use clap::{Parser, Subcommand};
use surge_core::SurgeConfig;
use tokio::signal;

mod commands;

use commands::{
    agent::AgentCommands, config::ConfigCommands, insights::InsightsCommands,
    registry::RegistryCommands, spec::SpecCommands,
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

    /// Manage specs
    Spec {
        #[command(subcommand)]
        command: SpecCommands,
    },

    /// Run a spec through the full pipeline
    Run {
        /// Spec ID or filename
        spec_id: String,
        /// Override max parallel subtasks
        #[arg(short = 'p', long)]
        parallel: Option<usize>,
        /// Override planner agent
        #[arg(long)]
        planner: Option<String>,
        /// Override coder agent
        #[arg(long)]
        coder: Option<String>,
        /// Resume from last checkpoint
        #[arg(long)]
        resume: bool,
    },

    /// Show pipeline status for a spec
    Status {
        /// Spec ID
        spec_id: String,
    },

    /// Show pipeline logs for a spec
    Logs {
        /// Spec ID
        spec_id: String,
        /// Follow log output in real time
        #[arg(short, long)]
        follow: bool,
    },

    /// Plan a spec (show execution order) without running it
    Plan {
        /// Spec ID
        spec_id: String,
        /// Agent to use for planning
        #[arg(long)]
        agent: Option<String>,
    },

    /// Show diff for a spec's worktree
    Diff {
        /// Spec ID
        spec_id: String,
    },

    /// Merge a spec's worktree into the current branch
    Merge {
        /// Spec ID
        spec_id: String,
        /// Skip confirmation
        #[arg(short = 'y', long)]
        yes: bool,
    },

    /// Discard a spec's worktree and branch
    Discard {
        /// Spec ID
        spec_id: String,
        /// Skip confirmation
        #[arg(short = 'y', long)]
        yes: bool,
    },

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
    Init,

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
}

/// Set up signal handlers for graceful shutdown.
///
/// Listens for SIGINT (Ctrl+C) and SIGTERM and triggers graceful shutdown.
async fn setup_signal_handler() {
    #[cfg(unix)]
    {
        let mut sigterm = signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler");
        let mut sigint = signal::unix::signal(signal::unix::SignalKind::interrupt())
            .expect("failed to install SIGINT handler");

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
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
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
        let lifecycle = surge_git::LifecycleManager::new(mgr);
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
        .init();

    let cli = Cli::parse();

    // Check for orphaned worktrees at startup (skip for certain commands)
    let should_check_orphans = !matches!(
        cli.command,
        Commands::Init | Commands::Clean { .. } | Commands::Config { .. }
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
                }
                Err(e) => {
                    println!("❌ Agent '{agent_name}' failed: {e}");
                    std::process::exit(2);
                }
            }
        }

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
        }

        Commands::Agent { command } => {
            commands::agent::run(command).await?;
        }

        Commands::Spec { command } => {
            commands::spec::run(command)?;
        }

        Commands::Run {
            spec_id,
            parallel,
            planner,
            coder,
            resume,
        } => {
            commands::pipeline::run(spec_id, parallel, planner, coder, resume).await?;
        }

        Commands::Status { spec_id } => {
            commands::pipeline::status(spec_id)?;
        }

        Commands::Logs { spec_id, follow } => {
            commands::pipeline::logs(spec_id, follow)?;
        }

        Commands::Plan { spec_id, agent } => {
            commands::pipeline::plan(spec_id, agent)?;
        }

        Commands::Diff { spec_id } => {
            commands::git::diff(spec_id)?;
        }

        Commands::Merge { spec_id, yes } => {
            commands::git::merge(spec_id, yes)?;
        }

        Commands::Discard { spec_id, yes } => {
            commands::git::discard(spec_id, yes)?;
        }

        Commands::Clean { yes } => {
            commands::git::clean(yes)?;
        }

        Commands::Worktrees => {
            commands::git::worktrees()?;
        }

        Commands::Config { command } => {
            commands::config::run(command)?;
        }

        Commands::Registry { command } => {
            commands::registry::run(command)?;
        }

        Commands::Insights { command } => {
            commands::insights::run(command)?;
        }

        Commands::Init => {
            let config_path = std::env::current_dir()?.join("surge.toml");

            if config_path.exists() {
                anyhow::bail!("surge.toml already exists in current directory");
            }

            let default_toml = r#"# Surge configuration
# See: https://github.com/vanyastaff/surge

default_agent = "claude"

[agents.claude]
command = "claude"
args = ["--print", "--output-format", "stream-json"]
transport = "stdio"

[pipeline]
max_qa_iterations = 10
max_parallel = 3

[pipeline.gates]
after_spec = true
after_plan = true
after_each_subtask = false
after_qa = true
"#;

            std::fs::write(&config_path, default_toml)?;
            println!("⚡ Created surge.toml");
            println!("   Edit agents section to configure your coding agents.");
        }
    }

    Ok(())
}
