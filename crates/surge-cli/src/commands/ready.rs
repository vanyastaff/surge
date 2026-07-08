//! `surge ready` — the actionable task backlog from the cross-run task-ledger
//! index (Phase 1 M5).
//!
//! Lists ledger tasks that still need attention (not completed/failed/skipped)
//! across all projects. The registry index is mirrored from each run's folded
//! ledger at completion; a run that has not completed since the index was
//! introduced will not appear until it next syncs.
//!
//! Per-project scoping is disabled until runs record their origin repo: a run's
//! stored `project_path` is its isolated worktree, which never matches the
//! invoking repo (a documented follow-up).
//!
//! Note: dependency-aware unblocking (only tasks whose `depends_on` are all
//! satisfied) is not yet applied — the index does not carry `depends_on`, which
//! lives in the roadmap artifact. This is a documented follow-up.

use anyhow::{Context, Result, anyhow};
use clap::Args;
use surge_core::{RoadmapStatus, RunId};
use surge_persistence::runs::Storage;

use crate::commands::common::surge_home_dir;
use surge_persistence::task_ledger::{TaskLedgerIndexFilter, TaskLedgerIndexRecord};

/// Arguments for `surge ready`.
#[derive(Args, Debug)]
pub struct ReadyArgs {
    /// Only tasks with this exact status (e.g. `pending`,
    /// `ready_for_verification`, `failed_verification`).
    #[arg(long)]
    pub status: Option<String>,
    /// Only tasks discovered mid-run (with a `discovered_from` edge).
    #[arg(long)]
    pub discovered: bool,
    /// Only tasks belonging to this run id.
    #[arg(long = "run")]
    pub run_id: Option<String>,
    /// Accepted for compatibility; project scoping is currently always on
    /// (see the module docs), so this is a no-op today.
    #[arg(long)]
    pub all_projects: bool,
    /// Maximum rows to return.
    #[arg(long, default_value_t = 200)]
    pub limit: usize,
    /// Emit JSON instead of a table.
    #[arg(long)]
    pub json: bool,
}

/// Run `surge ready`.
///
/// # Errors
/// Returns an error if storage cannot be opened or the query fails.
pub async fn run(args: ReadyArgs) -> Result<()> {
    let storage = Storage::open(&surge_home_dir()?)
        .await
        .context("open storage")?;

    let status = args
        .status
        .as_deref()
        .map(parse_status)
        .transpose()
        .context("parse --status")?;
    let run_id = args
        .run_id
        .as_deref()
        .map(parse_run_id)
        .transpose()
        .context("parse --run")?;
    // Per-project scoping is disabled for now: a run records its isolated
    // worktree path as project_path, which never equals the invoking repo, so a
    // current-dir filter silently matched nothing. Show all projects until runs
    // record their origin repo (tracked follow-up). `--all-projects` is kept as
    // an accepted no-op so scripts don't break.
    let _ = args.all_projects;
    let project_path = None;

    // For the default actionable view (no explicit --status) we filter out
    // settled tasks in Rust, so the SQL LIMIT must NOT be applied first — a
    // backlog of newer settled tasks would otherwise fill the window and hide
    // real work. Fetch unbounded, filter, then truncate to the display limit.
    let query_limit = if status.is_none() {
        None
    } else {
        Some(args.limit)
    };
    let mut records = storage.task_ledger_store().list(&TaskLedgerIndexFilter {
        status,
        project_path,
        run_id,
        discovered_only: args.discovered,
        limit: query_limit,
    })?;

    // Default view (no explicit --status): actionable backlog only — drop
    // tasks that have reached a terminal state, then apply the display limit.
    if status.is_none() {
        records.retain(|r| !is_settled(r.status));
        records.truncate(args.limit);
    }

    if args.json {
        println!("{}", serde_json::to_string_pretty(&records)?);
    } else {
        print_ready_table(&records);
    }
    Ok(())
}

/// A task is settled when no further work is expected on it.
fn is_settled(status: RoadmapStatus) -> bool {
    matches!(
        status,
        RoadmapStatus::Completed | RoadmapStatus::Failed | RoadmapStatus::Skipped
    )
}

fn print_ready_table(records: &[TaskLedgerIndexRecord]) {
    if records.is_empty() {
        println!("No actionable tasks.");
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
    println!("\n{} task(s).", records.len());
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

fn parse_status(value: &str) -> Result<RoadmapStatus> {
    Ok(match value.trim().to_ascii_lowercase().as_str() {
        "pending" => RoadmapStatus::Pending,
        "running" => RoadmapStatus::Running,
        "paused" => RoadmapStatus::Paused,
        "ready_for_verification" | "ready-for-verification" => RoadmapStatus::ReadyForVerification,
        "failed_verification" | "failed-verification" => RoadmapStatus::FailedVerification,
        "completed" => RoadmapStatus::Completed,
        "failed" => RoadmapStatus::Failed,
        "skipped" => RoadmapStatus::Skipped,
        other => return Err(anyhow!("unknown status {other:?}")),
    })
}

fn parse_run_id(value: &str) -> Result<RunId> {
    value
        .parse()
        .map_err(|error| anyhow!("invalid run id {value:?}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_status_accepts_ledger_statuses() {
        assert_eq!(parse_status("pending").unwrap(), RoadmapStatus::Pending);
        assert_eq!(
            parse_status("ready_for_verification").unwrap(),
            RoadmapStatus::ReadyForVerification
        );
        assert_eq!(
            parse_status("failed-verification").unwrap(),
            RoadmapStatus::FailedVerification
        );
        assert!(parse_status("bogus").is_err());
    }

    #[test]
    fn settled_states_are_dropped_from_default_view() {
        assert!(is_settled(RoadmapStatus::Completed));
        assert!(is_settled(RoadmapStatus::Failed));
        assert!(is_settled(RoadmapStatus::Skipped));
        assert!(!is_settled(RoadmapStatus::Pending));
        assert!(!is_settled(RoadmapStatus::ReadyForVerification));
    }
}
