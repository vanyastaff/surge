use std::path::{Path, PathBuf};
use std::sync::Arc;

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
    }
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

fn project_root(cwd: &Path) -> PathBuf {
    surge_git::GitManager::discover()
        .map(|manager| manager.repo_path().to_path_buf())
        .unwrap_or_else(|_| cwd.to_path_buf())
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
