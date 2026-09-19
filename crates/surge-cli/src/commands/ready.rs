//! `surge ready` — the actionable task backlog.
//!
//! Two modes:
//!
//! 1. **Project queue mode** (ADR-0020), when the current repository has a
//!    `.surge/roadmap.toml`: the project's queue is mirrored and evaluated
//!    with [`surge_orchestrator::scheduler::QueuePolicy`]. The output is
//!    dependency-aware — only tasks whose `depends_on` are all completed are
//!    listed as ready, and tasks blocked by a failed dependency are reported
//!    as `blocked_by_failed` with the offending id. This is the mode that
//!    answers "what can Surge start now".
//! 2. **Ledger index mode** (the Phase-1 fallback), when there is no project
//!    roadmap in the current directory: the cross-run task-ledger index is
//!    listed as before, and its rows cannot be dependency-aware because the
//!    index does not carry `depends_on` (the roadmap artifact does).

use anyhow::{Context, Result, anyhow};
use clap::Args;
use surge_core::{RoadmapArtifact, RoadmapStatus, RunId};
use surge_orchestrator::scheduler::{QueueDecision, QueueEntry, QueuePolicy};
use surge_persistence::runs::Storage;
use surge_persistence::task_queue::{DispatchState, TaskQueueFilter};

use crate::commands::common::{project_root, surge_home_dir};
use surge_persistence::task_ledger::{TaskLedgerIndexFilter, TaskLedgerIndexRecord};

/// Relative path of the project roadmap inside the repository.
const ROADMAP_RELPATH: &str = ".surge/roadmap.toml";

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
    let cwd = std::env::current_dir().context("resolve current directory")?;
    let root = project_root(&cwd);
    if root.join(ROADMAP_RELPATH).exists() {
        return run_project_queue(&root, args).await;
    }
    run_ledger_index(args).await
}

/// Dependency-aware view over the current project's task queue.
async fn run_project_queue(root: &std::path::Path, args: ReadyArgs) -> Result<()> {
    if args.run_id.is_some() || args.status.is_some() || args.discovered {
        // Those filters are ledger-index concepts; the project view answers
        // "what is unblocked", so mixing them silently would be a lie.
        anyhow::bail!(
            "`--status`, `--run` and `--discovered` apply to the cross-run ledger view; \
             this repository has a .surge/roadmap.toml, so `surge ready` is showing its \
             queue instead. Run from a directory without a project roadmap to use them."
        );
    }
    let roadmap_path = root.join(ROADMAP_RELPATH);
    let text = std::fs::read_to_string(&roadmap_path)
        .with_context(|| format!("read {}", roadmap_path.display()))?;
    let roadmap: RoadmapArtifact =
        toml::from_str(&text).with_context(|| format!("parse {}", roadmap_path.display()))?;
    let hash = surge_core::ContentHash::compute(text.as_bytes());

    let storage = Storage::open(&surge_home_dir()?)
        .await
        .context("open storage")?;
    let queue = storage.task_queue_store();
    let now_ms = chrono::Utc::now().timestamp_millis();
    queue
        .register_project(root, now_ms)
        .context("register project")?;
    queue
        .mirror(root, &hash, &roadmap.to_queue_entries(), now_ms)
        .context("mirror roadmap")?;

    let rows = queue
        .list(&TaskQueueFilter {
            project_root: Some(root.to_path_buf()),
            dispatch_state: Some(DispatchState::Queued),
            ..Default::default()
        })
        .context("list queue")?;
    let dep_states = queue.dependency_states(root).context("dependency states")?;
    let config = surge_core::SurgeConfig::load_or_default().unwrap_or_default();
    let entries: Vec<QueueEntry> = rows
        .iter()
        .map(|row| QueueEntry {
            task_id: row.task_id.clone(),
            priority: row.priority,
            depends_on: row.depends_on.clone(),
            dep_states: dep_states.get(&row.task_id).cloned().unwrap_or_default(),
            size: row.size,
            enqueued_at: row.enqueued_at,
            skipped_dispatches: row.skipped_dispatches,
        })
        .collect();
    let decision = QueuePolicy::next(&entries, &config.queue);

    if args.json {
        println!("{}", serde_json::to_string_pretty(&decision)?);
    } else {
        print_project_ready_table(&mut std::io::stdout().lock(), &decision, args.limit);
    }
    Ok(())
}

fn print_project_ready_table(
    out: &mut impl std::io::Write,
    decision: &QueueDecision,
    limit: usize,
) {
    let ready: Vec<&String> = decision.ready.iter().take(limit).collect();
    if ready.is_empty() {
        let _ = writeln!(out, "No actionable tasks.");
    } else {
        let _ = writeln!(out, "{:<24} ORDER", "TASK");
        for (index, task) in ready.iter().enumerate() {
            let _ = writeln!(out, "{:<24} {}", task, index + 1);
        }
        let _ = writeln!(out, "\n{} ready task(s).", ready.len());
    }
    for blocked in &decision.blocked_by_failed {
        let by = blocked
            .by
            .iter()
            .map(|(id, state)| format!("{id} ({state})"))
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(
            out,
            "blocked_by_failed: {} — unblock with `surge task requeue <dependency>` or \
             `surge task skip <dependency>` (blocked by {by})",
            blocked.task_id
        );
    }
}

/// Cross-run ledger view (no project roadmap in the current directory).
async fn run_ledger_index(args: ReadyArgs) -> Result<()> {
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
