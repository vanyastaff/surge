use anyhow::Result;
use clap::{Parser, Subcommand};

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
    }

    Ok(())
}
