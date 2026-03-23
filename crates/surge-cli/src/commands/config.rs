use anyhow::Result;
use clap::Subcommand;
use surge_core::SurgeConfig;

#[derive(Subcommand)]
pub enum ConfigCommands {
    /// Display effective configuration
    Show,
}

pub fn run(command: ConfigCommands) -> Result<()> {
    match command {
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
    }
    Ok(())
}
