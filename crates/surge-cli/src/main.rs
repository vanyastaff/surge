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
            let agent_name = agent.as_deref().unwrap_or("default");
            println!("⚡ Pinging agent '{agent_name}'...");
            // TODO: Phase 0 — connect via ACP and ping
            println!("🚧 Not implemented yet. This is where ACP connection will go.");
        }
        Commands::Prompt { message, agent } => {
            let agent_name = agent.as_deref().unwrap_or("default");
            println!("⚡ Sending to '{agent_name}': {message}");
            // TODO: Phase 0 — send prompt via ACP
            println!("🚧 Not implemented yet.");
        }
        Commands::Agent { command } => match command {
            AgentCommands::List => {
                println!("⚡ Configured agents:");
                println!("🚧 Not implemented yet. Will read from surge.toml.");
            }
            AgentCommands::Test { name } => {
                println!("⚡ Testing agent '{name}'...");
                println!("🚧 Not implemented yet.");
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
