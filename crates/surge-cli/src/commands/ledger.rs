//! `surge ledger` — the full task ledger (every task, including settled ones)
//! for a run or project, from the cross-run task-ledger index (Phase 1 M5).
//!
//! Where `surge ready` shows only the actionable backlog, `surge ledger` shows
//! the complete picture: verified completions, failures, and the
//! `discovered_from` edges of mid-run discoveries.

use anyhow::{Context, Result, anyhow};
use clap::Args;
use surge_core::RunId;
use surge_persistence::runs::Storage;

use crate::commands::common::surge_home_dir;
use surge_persistence::task_ledger::{TaskLedgerIndexFilter, TaskLedgerIndexRecord};

/// Arguments for `surge ledger`.
#[derive(Args, Debug)]
pub struct LedgerArgs {
    /// Scope to one run id.
    #[arg(long = "run")]
    pub run_id: Option<String>,
    /// Include tasks from every project, not just the current repo.
    #[arg(long)]
    pub all_projects: bool,
    /// Maximum rows to return.
    #[arg(long, default_value_t = 500)]
    pub limit: usize,
    /// Emit JSON instead of a table.
    #[arg(long)]
    pub json: bool,
}

/// Run `surge ledger`.
///
/// # Errors
/// Returns an error if storage cannot be opened or the query fails.
pub async fn run(args: LedgerArgs) -> Result<()> {
    let storage = Storage::open(&surge_home_dir()?)
        .await
        .context("open storage")?;
    let run_id = args
        .run_id
        .as_deref()
        .map(parse_run_id)
        .transpose()
        .context("parse --run")?;
    let project_path = if args.all_projects {
        None
    } else {
        std::env::current_dir().ok()
    };

    let records = storage.task_ledger_store().list(&TaskLedgerIndexFilter {
        status: None,
        project_path,
        run_id,
        discovered_only: false,
        limit: Some(args.limit),
    })?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&records)?);
    } else {
        print_ledger_table(&records);
    }
    Ok(())
}

fn print_ledger_table(records: &[TaskLedgerIndexRecord]) {
    if records.is_empty() {
        println!("Ledger is empty for this scope.");
        return;
    }
    println!(
        "{:<20} {:<22} {:<9} {:<16} {}",
        "TASK", "STATUS", "VERIFIED", "DISCOVERED_FROM", "RUN"
    );
    for r in records {
        println!(
            "{:<20} {:<22} {:<9} {:<16} {}",
            truncate(&r.task_id, 20),
            r.status.to_string(),
            if r.verified { "yes" } else { "no" },
            r.discovered_from.as_deref().unwrap_or("-"),
            r.run_id,
        );
    }
    let verified = records.iter().filter(|r| r.verified).count();
    println!("\n{} task(s), {verified} verified.", records.len());
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_owned()
    } else {
        format!(
            "{}…",
            s.chars().take(max.saturating_sub(1)).collect::<String>()
        )
    }
}

fn parse_run_id(value: &str) -> Result<RunId> {
    value
        .parse()
        .map_err(|error| anyhow!("invalid run id {value:?}: {error}"))
}
