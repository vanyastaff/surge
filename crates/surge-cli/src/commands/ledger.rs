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
    /// Accepted for compatibility. Per-project scoping is currently disabled —
    /// all projects are always shown (runs store their worktree path, not the
    /// origin repo), so this flag is a no-op today.
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
    // Per-project scoping is disabled for now: a run records its isolated
    // worktree path as project_path, which never equals the invoking repo, so a
    // current-dir filter silently matched nothing (same reason inbox/ready pass
    // None). Show all until runs record their origin repo. `--all-projects` is a
    // no-op kept for compatibility.
    let _ = args.all_projects;

    let records = storage.task_ledger_store().list(&TaskLedgerIndexFilter {
        status: None,
        project_path: None,
        run_id,
        discovered_only: false,
        limit: Some(args.limit),
    })?;

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
        print_ledger_table(&mut std::io::stdout().lock(), &records);
    }
    Ok(())
}

/// Render the ledger table to `out`. Split from [`run`] so a test can assert
/// on the exact printed bytes — in particular that the VERIFIED column and
/// summary count are read through
/// [`TaskLedgerIndexRecord::is_evidence_backed`] (spec §10/R30's single
/// predicate) rather than the raw `verified` field, the same rule `surge run
/// report` and `surge inbox` apply.
fn print_ledger_table(out: &mut impl std::io::Write, records: &[TaskLedgerIndexRecord]) {
    if records.is_empty() {
        let _ = writeln!(out, "Ledger is empty for this scope.");
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
    let verified = records.iter().filter(|r| r.is_evidence_backed()).count();
    let _ = writeln!(out, "\n{} task(s), {verified} verified.", records.len());
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use surge_core::RoadmapStatus;

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
        print_ledger_table(&mut buf, records);
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn empty_scope_prints_the_empty_message() {
        assert!(rendered(&[]).contains("Ledger is empty"));
    }

    /// A completed, verified task reads "yes" in the VERIFIED column and
    /// counts toward the summary line — pinned so the column can never
    /// silently flip to reading the raw `verified` field again without a
    /// failing test.
    #[test]
    fn evidence_backed_task_reads_yes_and_is_counted() {
        let out = rendered(&[record("t1", RoadmapStatus::Completed, true)]);
        assert!(
            out.lines()
                .any(|l| l.starts_with("t1") && l.contains("yes"))
        );
        assert!(out.contains("1 task(s), 1 verified."));
    }

    /// The mutation this pins: `is_evidence_backed()` is the source of the
    /// column, not the raw `verified` field alone — a task stuck at
    /// `ReadyForVerification` (never reached `Completed`) reads "no" and is
    /// excluded from the verified count even though nothing here ever set
    /// `verified: true` for it either, so a regression that only dropped the
    /// `status == Completed` half of the predicate would not be caught by
    /// this row alone. See the next test for that half specifically.
    #[test]
    fn unverified_task_reads_no_and_is_excluded_from_the_count() {
        let out = rendered(&[record("t2", RoadmapStatus::ReadyForVerification, false)]);
        assert!(out.lines().any(|l| l.starts_with("t2") && l.contains("no")));
        assert!(out.contains("1 task(s), 0 verified."));
    }

    /// The half the previous test cannot pin: a `verified: true` row that has
    /// not (or no longer) reached `Completed` must still read "no". No
    /// production writer can build this combination today (`verified` is
    /// only ever set alongside `status: Completed` — see `evidence.rs`'s
    /// module doc) but the table's own column must not take that on faith;
    /// it goes through `is_evidence_backed`, which checks both fields.
    /// Deleting the `status == Completed` half of that predicate turns this
    /// row's column back to "yes" and this assertion red.
    #[test]
    fn verified_flag_without_completed_status_still_reads_no() {
        let out = rendered(&[record("t3", RoadmapStatus::FailedVerification, true)]);
        assert!(out.lines().any(|l| l.starts_with("t3") && l.contains("no")));
        assert!(out.contains("1 task(s), 0 verified."));
    }
}
