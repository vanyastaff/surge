use anyhow::Result;
use clap::Subcommand;

#[derive(Subcommand)]
pub enum RegistryCommands {
    /// List all known agents in the registry
    List,
    /// Search the registry by query
    Search {
        /// Search query (matches id, name, description, tags)
        query: String,
    },
    /// Show detailed info about a registry agent
    Info {
        /// Agent id (e.g. claude-code)
        id: String,
    },
    /// Detect agents installed on this system
    Detect,
    /// Add a registry agent to surge.toml
    Add {
        /// Agent id from the registry
        id: String,
        /// Optional custom name for the agent in config
        #[arg(short, long)]
        name: Option<String>,
    },
}

pub fn run(command: RegistryCommands) -> Result<()> {
    match command {
        RegistryCommands::List => {
            let registry = surge_acp::Registry::builtin();
            println!("⚡ Known agents:\n");
            for entry in registry.list() {
                let caps: Vec<String> =
                    entry.capabilities.iter().map(|c| c.to_string()).collect();
                println!("  {} — {}", entry.id, entry.display_name);
                println!("    {}", entry.description);
                println!("    capabilities: {}", caps.join(", "));
                println!();
            }
        }
        RegistryCommands::Search { query } => {
            let registry = surge_acp::Registry::builtin();
            let results = registry.search(&query);
            if results.is_empty() {
                println!("No agents matching '{query}'.");
            } else {
                println!("⚡ Agents matching '{query}':\n");
                for entry in results {
                    let caps: Vec<String> =
                        entry.capabilities.iter().map(|c| c.to_string()).collect();
                    println!("  {} — {}", entry.id, entry.display_name);
                    println!("    {}", entry.description);
                    println!("    capabilities: {}", caps.join(", "));
                    println!();
                }
            }
        }
        RegistryCommands::Detect => {
            let registry = surge_acp::Registry::builtin();
            let detected = registry.detect_installed_with_paths();

            if detected.is_empty() {
                println!("No known agents detected on this system.\n");
                println!("Install an agent:");
                for entry in registry.list() {
                    println!("  {} — {}", entry.id, entry.install_instructions);
                }
            } else {
                println!("⚡ Detected agents:\n");
                for agent in &detected {
                    let caps: Vec<String> =
                        agent.entry.capabilities.iter().map(|c| c.to_string()).collect();
                    println!("  ✅ {} ({})", agent.entry.display_name, agent.entry.id);
                    if let Some(path) = &agent.command_path {
                        println!("     Path: {path}");
                    }
                    println!("     Capabilities: {}", caps.join(", "));
                    println!();
                }

                let not_installed: Vec<_> =
                    registry.list().iter().filter(|e| !e.is_installed()).collect();
                if !not_installed.is_empty() {
                    println!("  Not installed:");
                    for entry in not_installed {
                        println!("    ❌ {} — {}", entry.id, entry.install_instructions);
                    }
                }

                println!("\nUse 'surge registry add <id>' to add a detected agent to surge.toml.");
            }
        }
        RegistryCommands::Info { id } => {
            let registry = surge_acp::Registry::builtin();
            match registry.find(&id) {
                Some(entry) => {
                    let caps: Vec<String> =
                        entry.capabilities.iter().map(|c| c.to_string()).collect();
                    println!("⚡ {}\n", entry.display_name);
                    println!("  ID:           {}", entry.id);
                    println!("  Command:      {}", entry.command);
                    if !entry.default_args.is_empty() {
                        println!("  Args:         {:?}", entry.default_args);
                    }
                    match &entry.transport {
                        surge_core::config::Transport::Stdio => {
                            println!("  Transport:    stdio");
                        }
                        surge_core::config::Transport::Tcp { host, port } => {
                            println!("  Transport:    tcp ({}:{})", host, port);
                        }
                        surge_core::config::Transport::WebSocket { url } => {
                            println!("  Transport:    ws ({})", url);
                        }
                    }
                    println!("  Capabilities: {}", caps.join(", "));
                    println!("  Install:      {}", entry.install_instructions);
                    if let Some(website) = &entry.website {
                        println!("  Website:      {}", website);
                    }
                }
                None => {
                    anyhow::bail!(
                        "Agent '{}' not found in registry. Try: surge registry list",
                        id
                    );
                }
            }
        }
        RegistryCommands::Add { id, name } => {
            let registry = surge_acp::Registry::builtin();
            match registry.find(&id) {
                Some(entry) => {
                    let config_path = std::env::current_dir()?.join("surge.toml");
                    if !config_path.exists() {
                        anyhow::bail!("No surge.toml found. Run 'surge init' first.");
                    }

                    let agent_name = name.as_deref().unwrap_or(&entry.id);
                    let contents = std::fs::read_to_string(&config_path)?;
                    let mut doc: toml::Table = contents.parse()?;

                    let agents = doc
                        .entry("agents")
                        .or_insert_with(|| toml::Value::Table(toml::Table::new()))
                        .as_table_mut()
                        .ok_or_else(|| {
                            anyhow::anyhow!("'agents' is not a table in surge.toml")
                        })?;

                    if agents.contains_key(agent_name) {
                        anyhow::bail!(
                            "Agent '{}' already exists in surge.toml",
                            agent_name
                        );
                    }

                    let mut agent_table = toml::Table::new();
                    agent_table.insert(
                        "command".into(),
                        toml::Value::String(entry.command.clone()),
                    );
                    let args: Vec<toml::Value> = entry
                        .default_args
                        .iter()
                        .map(|a| toml::Value::String(a.clone()))
                        .collect();
                    agent_table.insert("args".into(), toml::Value::Array(args));
                    agent_table
                        .insert("transport".into(), toml::Value::String("stdio".into()));

                    agents.insert(agent_name.to_string(), toml::Value::Table(agent_table));

                    std::fs::write(&config_path, doc.to_string())?;
                    println!("✅ Added agent '{}' to surge.toml", agent_name);
                }
                None => {
                    anyhow::bail!(
                        "Agent '{}' not found in registry. Try: surge registry list",
                        id
                    );
                }
            }
        }
    }
    Ok(())
}
