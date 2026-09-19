use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::common::project_root;
use anyhow::{Context, Result};
use clap::{Args, Subcommand, ValueEnum};
use surge_acp::bridge::facade::BridgeFacade;
use surge_core::SurgeConfig;
use surge_orchestrator::project_context::{
    ProjectContextOptions, ProjectContextStatus, describe_project, describe_project_with_bridge,
};
use tracing::{debug, info, warn};

#[derive(Debug, Clone, Subcommand)]
pub enum ProjectCommands {
    /// Generate or refresh stable project.md context for agent runs.
    Describe(ProjectDescribeArgs),
    /// Register this repository's `.surge/roadmap.toml` with the task queue.
    Start,
    /// Pause the project queue: no new task dispatches until resumed.
    Pause,
    /// Resume a paused project queue.
    Resume,
    /// Show the queue's pause state.
    Status,
}

#[derive(Debug, Clone, Args)]
pub struct ProjectDescribeArgs {
    /// Output markdown path. Defaults to init.project_context_path.
    #[arg(long)]
    pub output: Option<PathBuf>,
    /// Refresh even when project.md already exists.
    #[arg(long)]
    pub refresh: bool,
    /// Print whether project.md would change without writing.
    #[arg(long)]
    pub dry_run: bool,
    /// Choose how project.md is authored.
    #[arg(long = "author-mode", value_enum, default_value_t = ProjectDescribeAuthorMode::Auto)]
    pub author_mode: ProjectDescribeAuthorMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ProjectDescribeAuthorMode {
    /// Use the Project Context Author ACP profile when its runtime is installed; otherwise fallback.
    Auto,
    /// Require Project Context Author ACP execution.
    Agent,
    /// Use the deterministic local renderer.
    Deterministic,
}

pub async fn run(command: ProjectCommands) -> Result<()> {
    match command {
        ProjectCommands::Describe(args) => describe(args).await,
        ProjectCommands::Start => start().await,
        ProjectCommands::Pause => set_paused(true).await,
        ProjectCommands::Resume => set_paused(false).await,
        ProjectCommands::Status => status().await,
    }
}

/// Relative path of the project roadmap inside the repository.
const ROADMAP_RELPATH: &str = ".surge/roadmap.toml";

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn open_storage()
-> impl std::future::Future<Output = Result<std::sync::Arc<surge_persistence::runs::Storage>>> {
    let home = super::common::surge_home_dir();
    async move {
        let home = home?;
        surge_persistence::runs::Storage::open(&home)
            .await
            .context("open storage")
    }
}

/// Register the repository with the queue and mirror its roadmap.
async fn start() -> Result<()> {
    let cwd = std::env::current_dir().context("resolve current directory")?;
    let root = project_root(&cwd);
    let roadmap_path = root.join(ROADMAP_RELPATH);
    let text = std::fs::read_to_string(&roadmap_path).with_context(|| {
        format!(
            "read {} — run `surge bootstrap` first, or write the roadmap by hand",
            roadmap_path.display()
        )
    })?;
    let roadmap: surge_core::RoadmapArtifact =
        toml::from_str(&text).with_context(|| format!("parse {}", roadmap_path.display()))?;
    let issues = roadmap.validate_ledger();
    if !issues.is_empty() {
        let rendered = issues
            .iter()
            .map(|i| format!("  - {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        anyhow::bail!("roadmap ledger is invalid:\n{rendered}");
    }
    let hash = surge_core::ContentHash::compute(text.as_bytes());
    let storage = open_storage().await?;
    let queue = storage.task_queue_store();
    queue
        .register_project(&root, now_ms())
        .context("register")?;
    let count = queue
        .mirror(&root, &hash, &roadmap.to_queue_entries(), now_ms())
        .context("mirror roadmap")?;
    println!(
        "registered {} with the task queue ({} task(s))",
        root.display(),
        count
    );
    println!("run `surge task list` to see the queue, `surge ready` for unblocked work");
    Ok(())
}

async fn set_paused(paused: bool) -> Result<()> {
    let cwd = std::env::current_dir().context("resolve current directory")?;
    let root = project_root(&cwd);
    let storage = open_storage().await?;
    storage
        .task_queue_store()
        .set_project_paused(&root, paused, now_ms())
        .context("set project pause")?;
    println!(
        "{}",
        if paused {
            "project queue paused"
        } else {
            "project queue resumed"
        }
    );
    Ok(())
}

async fn status() -> Result<()> {
    let cwd = std::env::current_dir().context("resolve current directory")?;
    let root = project_root(&cwd);
    let storage = open_storage().await?;
    let paused = storage
        .task_queue_store()
        .is_project_paused(&root)
        .context("read project pause")?;
    println!(
        "{}: {}",
        root.display(),
        if paused { "paused" } else { "running" }
    );
    Ok(())
}

async fn describe(args: ProjectDescribeArgs) -> Result<()> {
    let cwd = std::env::current_dir().context("resolve current directory")?;
    let project_root = project_root(&cwd);
    let config = load_config_for_root(&project_root).context("load surge config")?;
    let output_path = args
        .output
        .unwrap_or_else(|| config.init.project_context_path.clone());
    let output_path = if output_path.is_absolute() {
        output_path
    } else {
        project_root.join(output_path)
    };
    debug!(
        cwd = %cwd.display(),
        project_root = %project_root.display(),
        output = %output_path.display(),
        refresh = args.refresh,
        dry_run = args.dry_run,
        author_mode = ?args.author_mode,
        "running project describe"
    );

    let mut options = ProjectContextOptions::new(project_root, output_path);
    options.refresh = args.refresh;
    options.dry_run = args.dry_run;
    let outcome = describe_with_mode(options, args.author_mode)
        .await
        .context("describe project context")?;

    match outcome.status {
        ProjectContextStatus::Drafted => println!("✅ Wrote {}", outcome.output_path.display()),
        ProjectContextStatus::NoChange => println!("✅ project.md is already up to date"),
        ProjectContextStatus::WouldDraft => {
            println!("Would update {}", outcome.output_path.display());
        },
        ProjectContextStatus::WouldNoChange => {
            println!("No changes needed for {}", outcome.output_path.display());
        },
    }
    println!("   Outcome: {}", outcome.status.as_str());
    println!("   Scan hash: {}", outcome.scan_hash);
    println!("   Output hash: {}", outcome.output_hash);
    println!("   Profile: {}", outcome.profile_id);
    println!("   Agent runtime: {}", outcome.normalized_agent_id);
    if !outcome.skipped_files.is_empty() {
        println!(
            "   Skipped: {} files/directories",
            outcome.skipped_files.len()
        );
    }
    info!(
        status = outcome.status.as_str(),
        output = %outcome.output_path.display(),
        "project describe command completed"
    );
    Ok(())
}

fn load_config_for_root(project_root: &Path) -> Result<SurgeConfig> {
    let config_path = project_root.join("surge.toml");
    if config_path.exists() {
        return SurgeConfig::load(&config_path).map_err(Into::into);
    }
    SurgeConfig::load_or_default().map_err(Into::into)
}

async fn describe_with_mode(
    options: ProjectContextOptions,
    mode: ProjectDescribeAuthorMode,
) -> Result<surge_orchestrator::project_context::ProjectContextOutcome> {
    match mode {
        ProjectDescribeAuthorMode::Deterministic => describe_project(options).map_err(Into::into),
        ProjectDescribeAuthorMode::Agent => describe_with_agent(options).await,
        ProjectDescribeAuthorMode::Auto if project_context_author_runtime_available() => {
            match describe_with_agent(options.clone()).await {
                Ok(outcome) => Ok(outcome),
                Err(e) => {
                    warn!(
                        error = %e,
                        "Project Context Author failed; falling back to deterministic renderer"
                    );
                    eprintln!(
                        "⚠️  Project Context Author failed; falling back to deterministic renderer: {e}"
                    );
                    describe_project(options).map_err(Into::into)
                },
            }
        },
        ProjectDescribeAuthorMode::Auto => describe_project(options).map_err(Into::into),
    }
}

async fn describe_with_agent(
    options: ProjectContextOptions,
) -> Result<surge_orchestrator::project_context::ProjectContextOutcome> {
    let bridge =
        Arc::new(surge_acp::bridge::AcpBridge::with_defaults().context("start ACP bridge")?);
    let facade: Arc<dyn BridgeFacade> = bridge.clone();
    let result = describe_project_with_bridge(options, facade).await;
    if let Ok(bridge) = Arc::try_unwrap(bridge)
        && let Err(e) = bridge.shutdown().await
    {
        warn!(error = %e, "ACP bridge shutdown after project describe failed");
    }
    result.map_err(Into::into)
}

fn project_context_author_runtime_available() -> bool {
    surge_acp::Registry::builtin()
        .find_normalized("claude-code")
        .is_some_and(surge_acp::RegistryEntry::is_installed)
}
