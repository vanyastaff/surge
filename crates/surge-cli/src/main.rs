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
    },

    /// Show pipeline status for a spec
    Status {
        /// Spec ID
        spec_id: String,
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
    /// Show agent health status
    Status,
}

#[derive(Subcommand)]
enum SpecCommands {
    /// Create a new spec from a template
    Create {
        /// Description of the spec
        description: String,
        /// Template to use (feature, bugfix, refactor)
        #[arg(short, long)]
        template: Option<String>,
    },
    /// List all specs
    List,
    /// Show spec details
    Show {
        /// Spec ID or filename
        id: String,
    },
    /// Validate a spec
    Validate {
        /// Spec ID or filename
        id: String,
    },
}

#[derive(Subcommand)]
enum ConfigCommands {
    /// Display effective configuration
    Show,
}

/// Load a spec by ID or filename.
fn load_spec_by_id(id: &str) -> anyhow::Result<surge_spec::SpecFile> {
    let path = std::path::Path::new(id);
    if path.exists() {
        return Ok(surge_spec::SpecFile::load(path)?);
    }

    let specs_dir = surge_spec::SpecFile::specs_dir()?;
    let with_ext = specs_dir.join(format!("{id}.toml"));
    if with_ext.exists() {
        return Ok(surge_spec::SpecFile::load(&with_ext)?);
    }

    let specs = surge_spec::SpecFile::list_all()?;
    for (spec_path, spec_file) in specs {
        if spec_file.spec.id.to_string().contains(id) {
            return Ok(surge_spec::SpecFile::load(&spec_path)?);
        }
    }

    anyhow::bail!("Spec '{}' not found. Check surge spec list.", id)
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
            AgentCommands::Status => {
                println!("⚡ Agent health monitoring is active during pipeline execution.");
                println!("   Use 'surge run' to see live agent status.");
            }
        },
        Commands::Spec { command } => match command {
            SpecCommands::Create { description, template } => {
                let kind = template.as_deref().unwrap_or("feature");
                let template_kind = surge_spec::TemplateKind::parse(kind)?;
                let spec_file = surge_spec::generate_template(template_kind, &description)?;

                let path = spec_file.save_to_specs_dir()?;
                println!("⚡ Created spec: {}", spec_file.spec.title);
                println!("   ID: {}", spec_file.spec.id);
                println!("   File: {}", path.display());
                println!("   Subtasks: {}", spec_file.spec.subtasks.len());
            }
            SpecCommands::List => {
                let specs = surge_spec::SpecFile::list_all()?;
                if specs.is_empty() {
                    println!("No specs found. Create one with: surge spec create \"description\"");
                } else {
                    println!("⚡ Specs:\n");
                    for (path, sf) in &specs {
                        let filename = path.file_name()
                            .map(|f| f.to_string_lossy().to_string())
                            .unwrap_or_default();
                        println!("  {} — {} ({} subtasks)",
                            filename, sf.spec.title, sf.spec.subtasks.len());
                    }
                }
            }
            SpecCommands::Show { id } => {
                let spec_file = load_spec_by_id(&id)?;
                let spec = &spec_file.spec;

                println!("⚡ Spec: {}\n", spec.title);
                println!("ID: {}", spec.id);
                println!("Complexity: {:?}", spec.complexity);
                println!("Description: {}", spec.description);
                println!("\nSubtasks ({}):", spec.subtasks.len());

                for (i, sub) in spec.subtasks.iter().enumerate() {
                    println!("  {}. {} [{:?}]", i + 1, sub.title, sub.complexity);
                    if !sub.acceptance_criteria.is_empty() {
                        for ac in &sub.acceptance_criteria {
                            let mark = if ac.met { "✅" } else { "⬜" };
                            println!("     {mark} {}", ac.description);
                        }
                    }
                }

                if !spec.subtasks.is_empty() {
                    match surge_spec::DependencyGraph::from_spec(spec) {
                        Ok(graph) => {
                            println!("\nDependency Graph:");
                            println!("{}", graph.to_ascii(spec));
                        }
                        Err(e) => println!("\nGraph error: {e}"),
                    }
                }
            }
            SpecCommands::Validate { id } => {
                let spec_file = load_spec_by_id(&id)?;
                let result = surge_spec::validate_spec(&spec_file.spec);

                if result.is_ok() {
                    println!("✅ Spec '{}' is valid", spec_file.spec.title);
                    for w in &result.warnings {
                        println!("   ⚠️  {w}");
                    }
                } else {
                    println!("❌ Spec '{}' has errors:", spec_file.spec.title);
                    for e in &result.errors {
                        println!("   ❌ {e}");
                    }
                    for w in &result.warnings {
                        println!("   ⚠️  {w}");
                    }
                    std::process::exit(1);
                }
            }
        },
        Commands::Run { spec_id, parallel, planner: _, coder: _ } => {
            let mut config = SurgeConfig::load_or_default()?;
            config.apply_env_overrides();

            if let Some(p) = parallel {
                config.pipeline.max_parallel = p;
            }

            let spec_file = load_spec_by_id(&spec_id)?;

            println!("⚡ Running spec: {}", spec_file.spec.title);
            println!("   Subtasks: {}", spec_file.spec.subtasks.len());

            let cwd = std::env::current_dir()?;
            let orch_config = surge_orchestrator::OrchestratorConfig {
                surge_config: config,
                working_dir: cwd,
            };
            let orchestrator = surge_orchestrator::Orchestrator::new(orch_config);

            let mut events = orchestrator.subscribe();
            tokio::spawn(async move {
                while let Ok(event) = events.recv().await {
                    match event {
                        surge_core::SurgeEvent::SubtaskStarted { subtask_id, .. } => {
                            println!("  ▶ Starting subtask {subtask_id}");
                        }
                        surge_core::SurgeEvent::SubtaskCompleted { subtask_id, success, .. } => {
                            let mark = if success { "✅" } else { "❌" };
                            println!("  {mark} Subtask {subtask_id}");
                        }
                        surge_core::SurgeEvent::TaskStateChanged { new_state, .. } => {
                            println!("  📊 State: {new_state}");
                        }
                        _ => {}
                    }
                }
            });

            let result = orchestrator.execute(&spec_file).await;

            match result {
                surge_orchestrator::PipelineResult::Completed => {
                    println!("\n✅ Pipeline completed successfully!");
                }
                surge_orchestrator::PipelineResult::Paused { phase, reason } => {
                    println!("\n⏸️  Pipeline paused at {phase}: {reason}");
                }
                surge_orchestrator::PipelineResult::Failed { reason } => {
                    println!("\n❌ Pipeline failed: {reason}");
                    std::process::exit(1);
                }
            }
        }
        Commands::Status { spec_id } => {
            let git = surge_git::GitManager::discover()?;
            let worktrees = git.list_worktrees()?;
            let wt = worktrees.iter().find(|w| w.spec_id.contains(&spec_id));

            if let Some(wt) = wt {
                println!("⚡ Status for '{}':", spec_id);
                println!("   Worktree: {} {}", if wt.exists_on_disk { "✅" } else { "❌" }, wt.path.display());
                println!("   Branch: {}", wt.branch);
            } else {
                println!("No active worktree for '{spec_id}'");
            }
        }
        Commands::Diff { spec_id } => {
            let mgr = surge_git::GitManager::discover()?;
            let diff = mgr.diff(&spec_id)?;
            if diff.is_empty() {
                println!("No changes in worktree for '{spec_id}'");
            } else {
                println!("{diff}");
            }
        }
        Commands::Merge { spec_id, yes } => {
            if !yes {
                println!("⚡ Merge worktree for '{spec_id}' into current branch?");
                println!("   Run with -y to skip confirmation.");
                return Ok(());
            }

            let mgr = surge_git::GitManager::discover()?;
            mgr.merge(&spec_id, None)?;
            println!("✅ Merged '{spec_id}' into current branch");
        }
        Commands::Discard { spec_id, yes } => {
            if !yes {
                println!("⚡ Discard worktree and branch for '{spec_id}'?");
                println!("   This is irreversible. Run with -y to confirm.");
                return Ok(());
            }

            let mgr = surge_git::GitManager::discover()?;
            mgr.discard(&spec_id)?;
            println!("✅ Discarded worktree for '{spec_id}'");
        }
        Commands::Clean { yes } => {
            let mgr = surge_git::GitManager::discover()?;
            let lifecycle = surge_git::LifecycleManager::new(mgr);

            if !yes {
                println!("⚡ Cleanup preview (run with -y to execute):");
                return Ok(());
            }

            let report = lifecycle.full_cleanup()?;

            if report.removed_worktrees.is_empty() && report.removed_branches.is_empty() {
                println!("✅ Nothing to clean up");
            } else {
                for wt in &report.removed_worktrees {
                    println!("  Removed worktree: {wt}");
                }
                for br in &report.removed_branches {
                    println!("  Deleted branch: {br}");
                }
                println!("\n✅ Cleanup complete");
            }
        }
        Commands::Worktrees => {
            let mgr = surge_git::GitManager::discover()?;
            let worktrees = mgr.list_worktrees()?;

            if worktrees.is_empty() {
                println!("No active worktrees.");
            } else {
                println!("⚡ Active worktrees:\n");
                for wt in &worktrees {
                    let status = if wt.exists_on_disk { "✅" } else { "❌ (missing)" };
                    println!("  {status} {} — {}", wt.spec_id, wt.branch);
                    println!("       {}", wt.path.display());
                }
            }
        }
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
