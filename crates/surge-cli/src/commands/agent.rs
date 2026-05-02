use anyhow::Result;
use clap::Subcommand;
use surge_acp::Registry;
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
    /// Refresh agent discovery cache
    Refresh,
    /// Add a custom agent to surge.toml
    Add {
        /// Agent name
        name: String,
        /// Path to the agent command
        #[arg(short, long)]
        command: String,
        /// Optional arguments for the agent
        #[arg(short, long)]
        args: Vec<String>,
    },
}

pub async fn run(command: AgentCommands) -> Result<()> {
    match command {
        AgentCommands::List => {
            let mut config = SurgeConfig::load_or_default()?;
            config.apply_env_overrides();

            let registry = Registry::builtin();
            let discovered = registry.detect_runnable_with_paths();

            println!("⚡ Agent Discovery Report\n");
            println!("Default: {}\n", config.default_agent);

            // Section 1: Discovered agents (available on system)
            println!("📦 Available agents ({} discovered):", discovered.len());
            if discovered.is_empty() {
                println!("   (no agents discovered on system)");
            } else {
                for detected in &discovered {
                    let version = detected
                        .detected_version
                        .as_ref()
                        .or(Some(&detected.entry.version))
                        .map_or(String::new(), |v| format!(" v{v}"));
                    let path = detected
                        .command_path
                        .as_ref()
                        .map_or(String::new(), |p| format!(" ({})", p));
                    println!("   ✓ {}{}{}", detected.entry.display_name, version, path);

                    // Show capabilities if available
                    if !detected.entry.capabilities.is_empty() {
                        let caps: Vec<String> = detected
                            .entry
                            .capabilities
                            .iter()
                            .map(|c| c.to_string())
                            .collect();
                        println!("       capabilities: {}", caps.join(", "));
                    }

                    // Show models if available
                    if !detected.entry.models.is_empty() {
                        println!("       models: {}", detected.entry.models.join(", "));
                    }
                }
            }

            println!();

            // Section 2: Configured agents
            println!("⚙️  Configured agents ({}):", config.agents.len());
            if config.agents.is_empty() {
                println!("   (no agents configured)");
            } else {
                for (name, agent_config) in &config.agents {
                    let marker = if name == &config.default_agent {
                        "*"
                    } else {
                        " "
                    };

                    // Check if this configured agent is available
                    let available = discovered.iter().any(|d| {
                        // Match by command + args, or by agent kind ID
                        (d.entry.command == agent_config.command
                            && d.entry.default_args == agent_config.args)
                            || d.entry.id == *name
                    });

                    let status = if available { "✓" } else { "✗" };

                    println!("{} {} {}", marker, status, name);
                    println!("       command: {}", agent_config.command);
                    if !agent_config.args.is_empty() {
                        println!("       args: {:?}", agent_config.args);
                    }
                    match &agent_config.transport {
                        surge_core::config::Transport::Stdio => {
                            println!("       transport: stdio");
                        },
                        surge_core::config::Transport::Tcp { host, port } => {
                            println!("       transport: tcp ({}:{})", host, port);
                        },
                        surge_core::config::Transport::WebSocket { url } => {
                            println!("       transport: ws ({})", url);
                        },
                    }
                }
            }

            println!();

            // Section 3: Missing agents (configured but not available)
            let missing: Vec<_> = config
                .agents
                .iter()
                .filter(|(name, agent_config)| {
                    !discovered.iter().any(|d| {
                        (d.entry.command == agent_config.command
                            && d.entry.default_args == agent_config.args)
                            || d.entry.id == **name
                    })
                })
                .collect();

            if !missing.is_empty() {
                println!(
                    "⚠️  Missing agents ({} configured but not available):",
                    missing.len()
                );
                for (name, _) in missing {
                    println!("   ✗ {}", name);
                }
                println!();
            }

            // Legend
            println!("Legend:");
            println!("  * = default agent");
            println!("  ✓ = available");
            println!("  ✗ = missing");
        },
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
                },
                Err(e) => {
                    println!("❌ Agent '{name}' — failed: {e}");
                    std::process::exit(2);
                },
            }
        },
        AgentCommands::Status => {
            let mut config = SurgeConfig::load_or_default()?;
            config.apply_env_overrides();

            if config.agents.is_empty() {
                println!("No agents configured. Run 'surge init' to get started.");
                return Ok(());
            }

            println!("⚡ Agent Health Dashboard\n");

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
                let marker = if name == &config.default_agent {
                    " (default)"
                } else {
                    ""
                };

                // Ping the agent to check connectivity
                let ping_result = pool.ping(name).await;

                // Access health metrics and extract needed data
                let health_lock = pool.health().lock().await;
                let health_data = health_lock.get_health(name).map(|h| {
                    (
                        h.status(),
                        h.latency_p50_ms(),
                        h.latency_p99_ms(),
                        h.total_requests,
                        h.error_rate(),
                        h.uptime(),
                        h.rate_limited,
                        h.rate_limit_reset,
                        h.last_error.clone(),
                    )
                });
                drop(health_lock); // Release lock early

                // Determine status display
                let (status_icon, status_text) = if let Some((status, ..)) = health_data {
                    match status {
                        surge_acp::HealthStatus::Healthy => ("✅", "online"),
                        surge_acp::HealthStatus::Degraded => {
                            any_offline = true;
                            ("⚠️ ", "degraded")
                        },
                        surge_acp::HealthStatus::Offline => {
                            any_offline = true;
                            ("❌", "offline")
                        },
                    }
                } else if ping_result.is_ok() {
                    ("✅", "online")
                } else {
                    any_offline = true;
                    ("❌", "offline")
                };

                println!("  {} {}{} — {}", status_icon, name, marker, status_text);

                // Display detailed health metrics if available
                if let Some((
                    status,
                    p50,
                    p99,
                    total_requests,
                    error_rate,
                    uptime,
                    rate_limited,
                    rate_limit_reset,
                    last_error,
                )) = health_data
                {
                    // Latency percentiles
                    if p50 > 0 || p99 > 0 {
                        println!("       latency: p50={}ms, p99={}ms", p50, p99);
                    }

                    // Error rate
                    if total_requests > 0 {
                        println!(
                            "       requests: {} total, {:.1}% errors",
                            total_requests, error_rate
                        );
                    }

                    // Uptime
                    let uptime_secs = uptime.as_secs();
                    if uptime_secs > 0 {
                        if uptime_secs < 60 {
                            println!("       uptime: {}s", uptime_secs);
                        } else if uptime_secs < 3600 {
                            println!("       uptime: {}m {}s", uptime_secs / 60, uptime_secs % 60);
                        } else {
                            println!(
                                "       uptime: {}h {}m",
                                uptime_secs / 3600,
                                (uptime_secs % 3600) / 60
                            );
                        }
                    }

                    // Rate limit cooldown
                    if rate_limited {
                        if let Some(reset_time) = rate_limit_reset {
                            let now = std::time::Instant::now();
                            if reset_time > now {
                                let cooldown = reset_time.duration_since(now);
                                println!(
                                    "       ⏳ rate-limited: cooldown {}s remaining",
                                    cooldown.as_secs()
                                );
                            } else {
                                println!("       ⏳ rate-limited: cooldown expired");
                            }
                        } else {
                            println!("       ⏳ rate-limited");
                        }
                    }

                    // Last error (if any and agent is not healthy)
                    if status != surge_acp::HealthStatus::Healthy
                        && let Some(err) = last_error
                    {
                        let truncated = if err.len() > 60 { &err[..60] } else { &err };
                        println!("       last error: {}", truncated);
                    }
                }

                // Show error if ping failed
                if let Err(e) = ping_result {
                    println!("       error: {}", e);
                }

                println!();
            }

            pool.shutdown().await;

            if any_offline {
                std::process::exit(2);
            }
        },
        AgentCommands::Refresh => {
            println!("⚡ Refreshing agent discovery cache...");
            Registry::refresh_discovery();
            println!("✅ Agent discovery cache cleared. Next 'surge agent list' will re-scan.");
        },
        AgentCommands::Add {
            name,
            command,
            args,
        } => {
            let config_path = std::env::current_dir()?.join("surge.toml");
            if !config_path.exists() {
                anyhow::bail!("No surge.toml found. Run 'surge init' first.");
            }

            let contents = std::fs::read_to_string(&config_path)?;
            let mut doc: toml::Table = contents.parse()?;

            let agents = doc
                .entry("agents")
                .or_insert_with(|| toml::Value::Table(toml::Table::new()))
                .as_table_mut()
                .ok_or_else(|| anyhow::anyhow!("'agents' is not a table in surge.toml"))?;

            if agents.contains_key(&name) {
                anyhow::bail!("Agent '{}' already exists in surge.toml", name);
            }

            let mut agent_table = toml::Table::new();
            agent_table.insert("command".into(), toml::Value::String(command.clone()));

            let args_values: Vec<toml::Value> = args
                .iter()
                .map(|a| toml::Value::String(a.clone()))
                .collect();
            agent_table.insert("args".into(), toml::Value::Array(args_values));
            agent_table.insert("transport".into(), toml::Value::String("stdio".into()));

            agents.insert(name.clone(), toml::Value::Table(agent_table));

            std::fs::write(&config_path, doc.to_string())?;
            println!("✅ Added agent '{}' to surge.toml", name);
        },
    }
    Ok(())
}
