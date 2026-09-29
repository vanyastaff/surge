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

use anyhow::{Context, Result};
use clap::Args;
use surge_core::{RoadmapStatus, RunId};
use surge_orchestrator::operator::{ReadyQuery, query_ready};
use surge_persistence::runs::Storage;

use crate::commands::common::{operator_failure, surge_home_dir};
use surge_persistence::task_ledger::TaskLedgerIndexRecord;

/// Arguments for `surge ready`.
#[derive(Args, Debug)]
pub struct ReadyArgs {
    /// Only tasks with this exact status (e.g. `pending`,
    /// `ready_for_verification`, `failed_verification`).
    #[arg(long)]
    pub status: Option<RoadmapStatus>,
    /// Only tasks discovered mid-run (with a `discovered_from` edge).
    #[arg(long)]
    pub discovered: bool,
    /// Only tasks belonging to this run id.
    #[arg(long = "run")]
    pub run_id: Option<RunId>,
    /// Accepted for compatibility. Per-project scoping is currently disabled —
    /// all projects are always shown (see the module docs), so this flag is a
    /// no-op today.
    #[arg(long)]
    pub all_projects: bool,
    /// Maximum rows to return.
    #[arg(long, default_value_t = ReadyQuery::DEFAULT_LIMIT)]
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
    let records = query_records(&storage, &args)?;

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

/// The rows `surge ready` shows for `args`, via the shared backlog service.
fn query_records(storage: &Storage, args: &ReadyArgs) -> Result<Vec<TaskLedgerIndexRecord>> {
    // Per-project scoping is disabled: `--all-projects` is an accepted no-op.
    let _ = args.all_projects;
    query_ready(
        storage,
        &ReadyQuery {
            status: args.status,
            discovered_only: args.discovered,
            run_id: args.run_id,
            limit: args.limit,
        },
    )
    .map_err(operator_failure)
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

#[cfg(test)]
mod tests {
    use super::*;
    use surge_core::{RoadmapStatus, RunId};

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
