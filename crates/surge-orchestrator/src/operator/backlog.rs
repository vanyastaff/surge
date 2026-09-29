//! The task backlog and ledger read models: the actionable backlog (`ready`)
//! and the complete task ledger, both read from the cross-run task-ledger
//! index.

use surge_core::{RoadmapStatus, RunId};
use surge_persistence::runs::Storage;
use surge_persistence::task_ledger::{TaskLedgerIndexFilter, TaskLedgerIndexRecord};

use crate::operator::error::{InvalidFilter, OperatorError};

/// Which tasks of the actionable backlog to list.
#[derive(Debug, Clone, Default)]
pub struct ReadyQuery {
    /// Only tasks with this exact status (`pending`, `ready_for_verification`,
    /// `failed_verification`, ...). `None` lists every unsettled task.
    pub status: Option<String>,
    /// Only tasks discovered mid-run (with a `discovered_from` edge).
    pub discovered_only: bool,
    /// Only tasks belonging to this full run id.
    pub run_id: Option<String>,
    /// Maximum rows to return.
    pub limit: usize,
}

/// Which tasks of the full ledger to list.
#[derive(Debug, Clone, Default)]
pub struct LedgerQuery {
    /// Scope to this full run id.
    pub run_id: Option<String>,
    /// Maximum rows to return.
    pub limit: usize,
}

/// The rows of the actionable backlog for `query` (raw, un-normalized —
/// callers that emit JSON apply
/// [`TaskLedgerIndexRecord::with_verified_normalized`]).
///
/// Per-project scoping is disabled: a run records its isolated worktree path
/// as its project path, which never equals the invoking repo, so every project
/// is always listed.
///
/// # Errors
/// Returns [`OperatorError::InvalidStatus`] or
/// [`OperatorError::InvalidRunFilter`] for a malformed filter, and
/// [`OperatorError::TaskLedger`] if the index query fails.
pub fn query_ready(
    storage: &Storage,
    query: &ReadyQuery,
) -> Result<Vec<TaskLedgerIndexRecord>, OperatorError> {
    let status = query
        .status
        .as_deref()
        .map(parse_status)
        .transpose()
        .map_err(OperatorError::InvalidStatus)?;
    let run_id = query
        .run_id
        .as_deref()
        .map(parse_run_id)
        .transpose()
        .map_err(OperatorError::InvalidRunFilter)?;

    let query_limit = if status.is_none() {
        None
    } else {
        Some(query.limit)
    };
    let mut records = storage
        .task_ledger_store()
        .list(&TaskLedgerIndexFilter {
            status,
            project_path: None,
            run_id,
            discovered_only: query.discovered_only,
            limit: query_limit,
        })
        .map_err(OperatorError::TaskLedger)?;

    if status.is_none() {
        records.retain(|r| !is_settled(r.status));
        records.truncate(query.limit);
    }
    Ok(records)
}

/// The ledger rows for `query` (raw, un-normalized — callers that emit JSON
/// apply [`TaskLedgerIndexRecord::with_verified_normalized`]). Per-project
/// scoping is disabled for the reason given on [`query_ready`].
///
/// # Errors
/// Returns [`OperatorError::InvalidRunFilter`] if the run id is malformed and
/// [`OperatorError::TaskLedger`] if the index query fails.
pub fn query_ledger(
    storage: &Storage,
    query: &LedgerQuery,
) -> Result<Vec<TaskLedgerIndexRecord>, OperatorError> {
    let run_id = query
        .run_id
        .as_deref()
        .map(parse_run_id)
        .transpose()
        .map_err(OperatorError::InvalidRunFilter)?;
    storage
        .task_ledger_store()
        .list(&TaskLedgerIndexFilter {
            status: None,
            project_path: None,
            run_id,
            discovered_only: false,
            limit: Some(query.limit),
        })
        .map_err(OperatorError::TaskLedger)
}

/// A task is settled when no further work is expected on it.
fn is_settled(status: RoadmapStatus) -> bool {
    matches!(
        status,
        RoadmapStatus::Completed | RoadmapStatus::Failed | RoadmapStatus::Skipped
    )
}

fn parse_status(value: &str) -> Result<RoadmapStatus, InvalidFilter> {
    Ok(match value.trim().to_ascii_lowercase().as_str() {
        "pending" => RoadmapStatus::Pending,
        "running" => RoadmapStatus::Running,
        "paused" => RoadmapStatus::Paused,
        "ready_for_verification" | "ready-for-verification" => RoadmapStatus::ReadyForVerification,
        "failed_verification" | "failed-verification" => RoadmapStatus::FailedVerification,
        "completed" => RoadmapStatus::Completed,
        "failed" => RoadmapStatus::Failed,
        "skipped" => RoadmapStatus::Skipped,
        other => return Err(InvalidFilter::new(format!("unknown status {other:?}"))),
    })
}

fn parse_run_id(value: &str) -> Result<RunId, InvalidFilter> {
    value
        .parse()
        .map_err(|error| InvalidFilter::new(format!("invalid run id {value:?}: {error}")))
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
