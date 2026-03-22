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

    /// Initialize surge.toml in current directory
    Init,
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

            if !config.agents.contains_key(agent_name) {
                anyhow::bail!("Agent '{}' not found in configuration", agent_name);
            }

            println!("⚡ Sending to '{agent_name}': {message}");

            let cwd = std::env::current_dir()?;
            let pool = surge_acp::AgentPool::new(
                config.agents.clone(),
                config.default_agent.clone(),
                cwd.clone(),
                surge_acp::PermissionPolicy::default(),
            )?;

            let session = pool.create_session(Some(agent_name), None, &cwd).await?;

            let content = vec![agent_client_protocol::ContentBlock::Text(
                agent_client_protocol::TextContent {
                    text: message,
                    annotations: None,
                    meta: None,
                },
            )];

            let response = pool.prompt(&session, content).await?;

            println!("✅ Agent responded (stop_reason: {:?})", response.stop_reason);

            pool.shutdown().await;
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

                if !config.agents.contains_key(&name) {
                    anyhow::bail!("Agent '{}' not found in configuration", name);
                }

                println!("⚡ Testing agent '{name}'...");

                let agent_config = config.agents.get(&name).unwrap();
                println!("   Command: {}", agent_config.command);
                if !agent_config.args.is_empty() {
                    println!("   Args: {:?}", agent_config.args);
                }

                let cwd = std::env::current_dir()?;
                let pool = surge_acp::AgentPool::new(
                    config.agents.clone(),
                    config.default_agent.clone(),
                    cwd,
                    surge_acp::PermissionPolicy::default(),
                )?;

                match pool.ping(&name).await {
                    Ok(()) => {
                        println!("✅ Agent '{name}' — connection OK");
                    }
                    Err(e) => {
                        println!("❌ Agent '{name}' — failed: {e}");
                        std::process::exit(1);
                    }
                }

                pool.shutdown().await;
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
