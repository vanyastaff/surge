use anyhow::Result;
use clap::{Parser, Subcommand};
use surge_core::SurgeConfig;

#[derive(Parser)]
#[command(name = "surge", version, about = "⚡ Any Agent. One Protocol. Pure Rust.")]
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

    /// Send a one-shot prompt to an agent
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

    /// Manage configuration
    Config {
        #[command(subcommand)]
        command: ConfigCommands,
    },
}

#[derive(Subcommand)]
enum AgentCommands {
    /// List configured agents
    List,
    /// Test connection to an agent
    Test {
        /// Agent name
        name: String,
    },
}

#[derive(Subcommand)]
enum ConfigCommands {
    /// Display effective configuration
    Show,
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

    match cli.command {
        Commands::Ping { agent } => {
            let mut config = SurgeConfig::load_or_default()?;
            config.apply_env_overrides();

            let agent_name = agent.as_deref().unwrap_or(&config.default_agent);

            if !config.agents.contains_key(agent_name) {
                anyhow::bail!("Agent '{}' not found in configuration", agent_name);
            }

            println!("⚡ Pinging agent '{agent_name}'...");

            let cwd = std::env::current_dir()?;
            let pool = surge_acp::AgentPool::new(
                config.agents.clone(),
                config.default_agent.clone(),
                cwd,
                surge_acp::PermissionPolicy::default(),
            )?;

            match pool.ping(agent_name).await {
                Ok(()) => {
                    println!("✅ Agent '{agent_name}' is responsive");
                }
                Err(e) => {
                    println!("❌ Agent '{agent_name}' failed: {e}");
                    std::process::exit(1);
                }
            }

            pool.shutdown().await;
        }
        Commands::Prompt { message, agent } => {
            let mut config = SurgeConfig::load_or_default()?;
            config.apply_env_overrides();

            let agent_name = agent.as_deref().unwrap_or(&config.default_agent);

            if let Some(agent_config) = config.agents.get(agent_name) {
                println!("⚡ Sending to '{agent_name}': {message}");
                println!("   Command: {}", agent_config.command);
                // TODO: Phase 0 — send prompt via ACP
                println!("🚧 Not implemented yet.");
            } else {
                anyhow::bail!("Agent '{}' not found in configuration", agent_name);
            }
        }
        Commands::Agent { command } => match command {
            AgentCommands::List => {
                let mut config = SurgeConfig::load_or_default()?;
                config.apply_env_overrides();

                println!("⚡ Configured agents:");
                println!("\nDefault: {}", config.default_agent);

                if config.agents.is_empty() {
                    println!("\n(no agents configured)");
                } else {
                    println!();
                    for (name, agent_config) in &config.agents {
                        let marker = if name == &config.default_agent { "*" } else { " " };
                        println!("{} {}", marker, name);
                        println!("    command: {}", agent_config.command);
                        if !agent_config.args.is_empty() {
                            println!("    args: {:?}", agent_config.args);
                        }
                        match &agent_config.transport {
                            surge_core::config::Transport::Stdio => {
                                println!("    transport: stdio");
                            }
                            surge_core::config::Transport::Tcp { host, port } => {
                                println!("    transport: tcp ({}:{})", host, port);
                            }
                        }
                    }
                }
            }
            AgentCommands::Test { name } => {
                let mut config = SurgeConfig::load_or_default()?;
                config.apply_env_overrides();

                if let Some(agent_config) = config.agents.get(&name) {
                    println!("⚡ Testing agent '{name}'...");
                    println!("   Command: {}", agent_config.command);
                    println!("🚧 Not implemented yet.");
                } else {
                    anyhow::bail!("Agent '{}' not found in configuration", name);
                }
            }
        },
        Commands::Config { command } => match command {
            ConfigCommands::Show => {
                let mut config = SurgeConfig::load_or_default()?;
                config.apply_env_overrides();

                println!("⚡ Surge Configuration\n");
                println!("Default Agent: {}", config.default_agent);

                if config.agents.is_empty() {
                    println!("\nAgents: (none configured)");
                } else {
                    println!("\nAgents:");
                    for (name, agent_config) in &config.agents {
                        println!("  {}:", name);
                        println!("    command: {}", agent_config.command);
                        if !agent_config.args.is_empty() {
                            println!("    args: {:?}", agent_config.args);
                        }
                        match &agent_config.transport {
                            surge_core::config::Transport::Stdio => {
                                println!("    transport: stdio");
                            }
                            surge_core::config::Transport::Tcp { host, port } => {
                                println!("    transport: tcp");
                                println!("      host: {}", host);
                                println!("      port: {}", port);
                            }
                        }
                    }
                }

                println!("\nPipeline:");
                println!("  max_qa_iterations: {}", config.pipeline.max_qa_iterations);
                println!("  max_parallel: {}", config.pipeline.max_parallel);

                println!("\n  Gates:");
                println!("    after_spec: {}", config.pipeline.gates.after_spec);
                println!("    after_plan: {}", config.pipeline.gates.after_plan);
                println!("    after_each_subtask: {}", config.pipeline.gates.after_each_subtask);
                println!("    after_qa: {}", config.pipeline.gates.after_qa);
            }
        },
    }

    Ok(())
}
