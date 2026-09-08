//! `surge run` — inspect an existing run's worktree (Phase 2 B4).
//!
//! Read-only review affordances that complete the operator loop
//! (`surge inbox` → `surge resolve`/`surge steer` → **review** → merge):
//!
//! - `surge run diff <run>` — the unified diff of what the run's agents changed
//!   in its isolated worktree (committed and uncommitted), against the base it
//!   branched from.
//! - `surge run path <run>` — the worktree path, e.g. `cd "$(surge run path <run>)"`.
//! - `surge run report <run>` — the compiled Run Report (R28): one document
//!   a reviewer reads to accept or reject the run without opening the
//!   transcript. See `surge_core::run_report`.

use anyhow::{Context, Result, anyhow};
use clap::{Subcommand, ValueEnum};
use surge_core::RunId;
use surge_core::run_report::{RunReport, render_html, render_json, render_markdown};
use surge_git::GitManager;
use surge_persistence::runs::Storage;

use crate::commands::common;
use crate::commands::run_fold::read_run_events;

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
    /// Compile and print the Run Report (R27–R33): nodes, outcomes, verifier
    /// verdicts, evidence, cost, skills bound, memory receipts, steers, and
    /// approvals — everything reconstructed from the event log alone. A run
    /// that has not reached a terminal event still compiles, marked as such.
    Report {
        /// Run id (or its short suffix as shown by `surge inbox`).
        run: String,
        /// Output format.
        #[arg(long, value_enum, default_value = "md")]
        format: RunReportFormat,
    },
}

/// `surge run report --format` values (R28: `json|md|html`).
///
/// No `#[derive(Default)]` here: clap resolves `#[arg(default_value = "md")]`
/// by parsing that string through `ValueEnum`, never through
/// `Default::default()` — a `Default` impl alongside it would be a second,
/// dead source of the same default (nothing in this binary ever calls
/// `RunReportFormat::default()`; confirmed by grep before adding this note).
#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum RunReportFormat {
    /// Machine-readable JSON.
    Json,
    /// Markdown — readable in a terminal, a PR description, or archived as
    /// `.md`. The default (`#[arg(default_value = "md")]` on the flag
    /// below): a reviewer running the command with no flags gets something
    /// readable immediately.
    Md,
    /// One self-contained HTML file — inline styles, no external
    /// stylesheet/script/CDN reference of any kind (R29).
    Html,
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
        RunCommand::Report { run, format } => report(&run, format).await,
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

async fn report(run: &str, format: RunReportFormat) -> Result<()> {
    let storage = Storage::open(&common::surge_home_dir()?)
        .await
        .context("open storage")?;
    let run_id = common::resolve_run_id(&storage, run).await?;
    let reader = storage
        .open_run_reader(run_id)
        .await
        .with_context(|| format!("open event log for run {run_id}"))?;
    let events = read_run_events(&reader).await?;
    let compiled = RunReport::compile(run_id, &events);

    let rendered = match format {
        RunReportFormat::Json => render_json(&compiled).context("render report as JSON")?,
        RunReportFormat::Md => render_markdown(&compiled),
        RunReportFormat::Html => render_html(&compiled),
    };
    // `println!` panics on a write failure (including `BrokenPipe` from
    // e.g. `| head`), which would surface as "surge panicked — this is a
    // bug" for what is actually the ordinary, expected behavior of a
    // truncated pipe. An HTML/JSON report can run to megabytes, so this is
    // not a hypothetical: `inbox.rs::print_inbox_to` swallows the same
    // error the same way, one write, for the same reason.
    use std::io::Write as _;
    let _ = writeln!(std::io::stdout().lock(), "{rendered}");
    Ok(())
}

/// Resolve a run id (full ULID or unique short suffix) against the run store.
async fn resolve_run_id(value: &str) -> Result<RunId> {
    let storage = Storage::open(&common::surge_home_dir()?)
        .await
        .context("open storage")?;
    common::resolve_run_id(&storage, value).await
}
