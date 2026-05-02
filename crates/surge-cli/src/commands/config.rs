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
                        },
                        surge_core::config::Transport::Tcp { host, port } => {
                            println!("    transport: tcp");
                            println!("      host: {}", host);
                            println!("      port: {}", port);
                        },
                        surge_core::config::Transport::WebSocket { url } => {
                            println!("    transport: ws");
                            println!("      url: {}", url);
                        },
                    }
                }
            }

            println!("\nPipeline:");
            println!("  max_qa_iterations: {}", config.pipeline.max_qa_iterations);
            println!("  max_parallel: {}", config.pipeline.max_parallel);

            println!("\n  Gates:");
            println!("    after_spec: {}", config.pipeline.gates.after_spec);
            println!("    after_plan: {}", config.pipeline.gates.after_plan);
            println!(
                "    after_each_subtask: {}",
                config.pipeline.gates.after_each_subtask
            );
            println!("    after_qa: {}", config.pipeline.gates.after_qa);

            println!("\nAnalytics:");
            if let Some(budget) = config.analytics.budget_usd {
                println!("  budget_usd: ${:.2}", budget);
            } else {
                println!("  budget_usd: unlimited");
            }
            if let Some(tokens) = config.analytics.budget_tokens {
                println!("  budget_tokens: {}", tokens);
            } else {
                println!("  budget_tokens: unlimited");
            }
            println!(
                "  budget_warn_threshold: {}%",
                config.analytics.budget_warn_threshold
            );

            println!("\n  Default Pricing:");
            if let Some(input_cost) = config
                .analytics
                .default_pricing
                .input_cost_per_million_tokens
            {
                println!("    input_cost_per_million_tokens: ${:.2}", input_cost);
            } else {
                println!("    input_cost_per_million_tokens: not set");
            }
            if let Some(output_cost) = config
                .analytics
                .default_pricing
                .output_cost_per_million_tokens
            {
                println!("    output_cost_per_million_tokens: ${:.2}", output_cost);
            } else {
                println!("    output_cost_per_million_tokens: not set");
            }
            println!(
                "    currency: {}",
                config.analytics.default_pricing.currency
            );
        },
    }
    Ok(())
}
