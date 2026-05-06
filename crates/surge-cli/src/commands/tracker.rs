//! `surge tracker` subcommand: list configured sources, test connectivity.

use anyhow::{Context, Result};
use clap::Subcommand;
use std::env;
use std::time::Duration;
use surge_core::config::{SurgeConfig, TaskSourceConfig};
use surge_intake::TaskSource;
use surge_intake::github::source::{GitHubConfig, GitHubIssuesTaskSource};
use surge_intake::linear::source::{LinearConfig, LinearTaskSource};

/// Subcommands for `surge tracker`.
#[derive(Subcommand, Debug)]
pub enum TrackerCommand {
    /// List task sources configured in `surge.toml`.
    List,
    /// Test connectivity to a configured source by id.
    Test {
        /// Source id (e.g. `"linear-acme"`).
        id: String,
    },
}

/// Entry point for the `surge tracker` subcommand.
///
/// Dispatches to list or test based on the command variant.
pub async fn run(cmd: TrackerCommand, config: SurgeConfig) -> Result<()> {
    match cmd {
        TrackerCommand::List => list(config),
        TrackerCommand::Test { id } => test(config, &id).await,
    }
}

fn list(config: SurgeConfig) -> Result<()> {
    if config.task_sources.is_empty() {
        println!("No task sources configured.");
        return Ok(());
    }
    println!("Configured task sources:");
    for s in &config.task_sources {
        match s {
            TaskSourceConfig::Linear(l) => {
                println!(
                    "  · linear · id={} workspace={} env={} interval={}s",
                    l.id,
                    l.workspace_id,
                    l.api_token_env,
                    l.poll_interval.as_secs()
                );
            },
            TaskSourceConfig::GithubIssues(g) => {
                println!(
                    "  · github_issues · id={} repo={} env={} interval={}s",
                    g.id,
                    g.repo,
                    g.api_token_env,
                    g.poll_interval.as_secs()
                );
            },
        }
    }
    Ok(())
}

async fn test(config: SurgeConfig, target_id: &str) -> Result<()> {
    let s = config
        .task_sources
        .iter()
        .find(|s| match s {
            TaskSourceConfig::Linear(l) => l.id == target_id,
            TaskSourceConfig::GithubIssues(g) => g.id == target_id,
        })
        .with_context(|| format!("source not found: {target_id}"))?;

    match s {
        TaskSourceConfig::Linear(l) => {
            let token = env::var(&l.api_token_env)
                .with_context(|| format!("env var {} not set", l.api_token_env))?;
            let cfg = LinearConfig {
                id: l.id.clone(),
                display_name: format!("Linear · {}", l.workspace_id),
                workspace_id: l.workspace_id.clone(),
                api_token: token,
                poll_interval: Duration::from_secs(60),
                label_filters: l.label_filters.clone(),
            };
            let src = LinearTaskSource::new(cfg)?;
            let summaries = src.list_open_tasks().await?;
            println!(
                "✓ Linear source {} reachable: {} open tasks",
                l.id,
                summaries.len()
            );
        },
        TaskSourceConfig::GithubIssues(g) => {
            let token = env::var(&g.api_token_env)
                .with_context(|| format!("env var {} not set", g.api_token_env))?;
            let (owner, repo) = g
                .repo
                .split_once('/')
                .with_context(|| format!("invalid repo format: {}", g.repo))?;
            let cfg = GitHubConfig {
                id: g.id.clone(),
                display_name: format!("GitHub · {}", g.repo),
                owner: owner.into(),
                repo: repo.into(),
                api_token: token,
                poll_interval: Duration::from_secs(60),
                label_filters: g.label_filters.clone(),
            };
            let src = GitHubIssuesTaskSource::new(cfg)?;
            let summaries = src.list_open_tasks().await?;
            println!(
                "✓ GitHub source {} reachable: {} open issues",
                g.id,
                summaries.len()
            );
        },
    }
    Ok(())
}
