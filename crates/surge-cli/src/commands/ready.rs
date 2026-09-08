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
    /// Accepted for compatibility. Per-project scoping is currently disabled —
    /// all projects are always shown (see the module docs), so this flag is a
    /// no-op today.
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
        // Same rule the table applies (spec §10/R30): `verified` here means
        // evidence-backed, not the raw stored flag — see
        // `TaskLedgerIndexRecord::with_verified_normalized`'s own doc.
        let records: Vec<_> = records
            .into_iter()
            .map(TaskLedgerIndexRecord::with_verified_normalized)
            .collect();
        println!("{}", serde_json::to_string_pretty(&records)?);
    } else {
        print_ready_table(&mut std::io::stdout().lock(), &records);
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

/// Render the actionable-backlog table to `out`. The default view (no
/// explicit `--status`) filters out `Completed` rows before this ever runs,
/// so the VERIFIED column reads "no" for everything shown there today — it
/// only becomes meaningful under `--status completed`, an explicit override.
/// That column goes through
/// [`TaskLedgerIndexRecord::is_evidence_backed`] (spec §10/R30's single
/// predicate), the same rule `surge run report` and `surge ledger` apply —
/// `surge ledger.rs`'s own doc names this as a sibling reader of the same
/// `{status, verified}` shape found while auditing every existing caller for
/// this ticket.
fn print_ready_table(out: &mut impl std::io::Write, records: &[TaskLedgerIndexRecord]) {
    if records.is_empty() {
        let _ = writeln!(out, "No actionable tasks.");
        return;
    }
    let _ = writeln!(
        out,
        "{:<20} {:<22} {:<9} {:<16} RUN",
        "TASK", "STATUS", "VERIFIED", "DISCOVERED_FROM"
    );
    for r in records {
        let _ = writeln!(
            out,
            "{:<20} {:<22} {:<9} {:<16} {}",
            truncate(&r.task_id, 20),
            r.status.to_string(),
            if r.is_evidence_backed() { "yes" } else { "no" },
            r.discovered_from.as_deref().unwrap_or("-"),
            r.run_id,
        );
    }
    let _ = writeln!(out, "\n{} task(s).", records.len());
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

    use std::path::PathBuf;

    fn record(task_id: &str, status: RoadmapStatus, verified: bool) -> TaskLedgerIndexRecord {
        TaskLedgerIndexRecord {
            run_id: RunId::new(),
            task_id: task_id.to_owned(),
            project_path: PathBuf::from("/proj"),
            status,
            verified,
            discovered_from: None,
            last_authority_node: Some("verify_1".into()),
            updated_seq: 1,
            updated_at_ms: 0,
        }
    }

    fn rendered(records: &[TaskLedgerIndexRecord]) -> String {
        let mut buf = Vec::new();
        print_ready_table(&mut buf, records);
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn empty_backlog_prints_the_no_actionable_message() {
        assert!(rendered(&[]).contains("No actionable tasks."));
    }

    /// Spec §10/R30: same predicate as `surge ledger`, applied here too — a
    /// `--status completed` query (the one path where a `Completed` row ever
    /// reaches this table) must read the VERIFIED column through
    /// `is_evidence_backed`, not the raw field.
    #[test]
    fn evidence_backed_completed_row_reads_yes() {
        let out = rendered(&[record("t1", RoadmapStatus::Completed, true)]);
        assert!(
            out.lines()
                .any(|l| l.starts_with("t1") && l.contains("yes"))
        );
    }

    /// The boundary `is_evidence_backed` exists to guard: a `verified: true`
    /// row that never reached `Completed` must still read "no". No
    /// production writer builds this combination today, but the column must
    /// not take that on faith — mirrors `ledger::tests::
    /// verified_flag_without_completed_status_still_reads_no`.
    #[test]
    fn verified_flag_without_completed_status_still_reads_no() {
        let out = rendered(&[record("t2", RoadmapStatus::FailedVerification, true)]);
        assert!(out.lines().any(|l| l.starts_with("t2") && l.contains("no")));
    }
}
