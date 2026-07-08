//! `surge run` — inspect an existing run's worktree (Phase 2 B4).
//!
//! Read-only review affordances that complete the operator loop
//! (`surge inbox` → `surge resolve`/`surge steer` → **review** → merge):
//!
//! - `surge run diff <run>` — the unified diff of what the run's agents changed
//!   in its isolated worktree (committed and uncommitted), against the base it
//!   branched from.
//! - `surge run path <run>` — the worktree path, e.g. `cd "$(surge run path <run>)"`.

use std::path::PathBuf;

use anyhow::{Context, Result, anyhow};
use clap::Subcommand;
use surge_core::RunId;
use surge_git::GitManager;
use surge_persistence::runs::registry::RunFilter;
use surge_persistence::runs::Storage;

/// `surge run` subcommands.
#[derive(Subcommand, Debug)]
pub enum RunCommand {
    /// Show the diff of a run's worktree — what the agents changed.
    Diff {
        /// Run id (or its short suffix as shown by `surge inbox`).
        run: String,
    },
    /// Print the filesystem path of a run's worktree.
    Path {
        /// Run id (or its short suffix as shown by `surge inbox`).
        run: String,
    },
}

/// Run `surge run`.
///
/// # Errors
/// Returns an error if the current directory is not a git repo, the run cannot
/// be resolved, or its worktree/branch is absent.
pub async fn run(cmd: RunCommand) -> Result<()> {
    match cmd {
        RunCommand::Diff { run } => diff(&run).await,
        RunCommand::Path { run } => path(&run).await,
    }
}

async fn diff(run: &str) -> Result<()> {
    let run_id = resolve_run_id(run).await?;
    let git = GitManager::discover().context("not inside a git repository")?;
    let patch = git
        .run_diff(&run_id)
        .map_err(|e| anyhow!("could not diff run {run_id}: {e}"))?;
    if patch.trim().is_empty() {
        println!("run {run_id}: no changes in the worktree");
    } else {
        print!("{patch}");
    }
    Ok(())
}

async fn path(run: &str) -> Result<()> {
    let run_id = resolve_run_id(run).await?;
    let git = GitManager::discover().context("not inside a git repository")?;
    let wt = git
        .find_run_worktree_path(&run_id)
        .map_err(|e| anyhow!("no worktree on disk for run {run_id}: {e}"))?;
    println!("{}", wt.display());
    Ok(())
}

/// Resolve a run id, accepting the full ULID or a unique short suffix.
async fn resolve_run_id(value: &str) -> Result<RunId> {
    if let Ok(id) = value.parse::<RunId>() {
        return Ok(id);
    }
    let storage = Storage::open(&surge_home_dir()?)
        .await
        .context("open storage")?;
    let runs = storage
        .list_runs(RunFilter {
            status: None,
            project_path: None,
            limit: Some(500),
        })
        .await
        .context("list runs for id match")?;
    let matches: Vec<RunId> = runs
        .iter()
        .filter(|r| r.id.to_string().ends_with(value))
        .map(|r| r.id)
        .collect();
    match matches.as_slice() {
        [one] => Ok(*one),
        [] => Err(anyhow!("no run matching {value:?}")),
        many => Err(anyhow!(
            "{} runs match {value:?}; use the full run id",
            many.len()
        )),
    }
}

fn surge_home_dir() -> Result<PathBuf> {
    if let Ok(custom) = std::env::var("SURGE_HOME")
        && !custom.is_empty()
    {
        return Ok(PathBuf::from(custom));
    }
    let base = dirs::home_dir().ok_or_else(|| anyhow!("could not resolve home directory"))?;
    Ok(base.join(".surge"))
}
