//! `surge run` — inspect an existing run's worktree (Phase 2 B4).
//!
//! Read-only review affordances that complete the operator loop
//! (`surge inbox` → `surge resolve`/`surge steer` → **review** → merge):
//!
//! - `surge run diff <run>` — the unified diff of what the run's agents changed
//!   in its isolated worktree (committed and uncommitted), against the base it
//!   branched from.
//! - `surge run path <run>` — the worktree path, e.g. `cd "$(surge run path <run>)"`.

use anyhow::{Context, Result, anyhow};
use clap::Subcommand;
use surge_core::RunId;
use surge_git::GitManager;
use surge_persistence::runs::Storage;

use crate::commands::common;

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

/// Resolve a run id (full ULID or unique short suffix) against the run store.
async fn resolve_run_id(value: &str) -> Result<RunId> {
    let storage = Storage::open(&common::surge_home_dir()?)
        .await
        .context("open storage")?;
    common::resolve_run_id(&storage, value).await
}
