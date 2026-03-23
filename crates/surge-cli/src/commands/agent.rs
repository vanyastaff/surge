use anyhow::Result;
use clap::Subcommand;
use surge_core::SurgeConfig;

#[derive(Subcommand)]
pub enum AgentCommands {
    /// List configured agents
    List,
    /// Test connection to an agent
    Test {
        /// Agent name
        name: String,
    },
    /// Show agent health status by pinging all configured agents
    Status,
}

pub async fn run(command: AgentCommands) -> Result<()> {
    match command {
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
                        surge_core::config::Transport::WebSocket { url } => {
                            println!("    transport: ws ({})", url);
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

            let agent_config = &config.agents[&name];
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
                config.resilience.clone(),
            )?;

            let result = pool.ping(&name).await;
            pool.shutdown().await;

            match result {
                Ok(()) => {
                    println!("✅ Agent '{name}' — connection OK");
                }
                Err(e) => {
                    println!("❌ Agent '{name}' — failed: {e}");
                    std::process::exit(2);
                }
            }
        }
        AgentCommands::Status => {
            let mut config = SurgeConfig::load_or_default()?;
            config.apply_env_overrides();

            if config.agents.is_empty() {
                println!("No agents configured. Run 'surge init' to get started.");
                return Ok(());
            }

            println!("⚡ Agent status:\n");

            let cwd = std::env::current_dir()?;
            let pool = surge_acp::AgentPool::new(
                config.agents.clone(),
                config.default_agent.clone(),
                cwd,
                surge_acp::PermissionPolicy::default(),
                config.resilience.clone(),
            )?;

            let mut any_offline = false;
            // Collect names first to avoid borrow issues
            let agent_names: Vec<String> = config.agents.keys().cloned().collect();
            for name in &agent_names {
                let marker = if name == &config.default_agent { " (default)" } else { "" };
                match pool.ping(name).await {
                    Ok(()) => println!("  ✅ {name}{marker} — online"),
                    Err(e) => {
                        println!("  ❌ {name}{marker} — {e}");
                        any_offline = true;
                    }
                }
            }

            pool.shutdown().await;

            if any_offline {
                std::process::exit(2);
            }
        }
    }
    Ok(())
}
