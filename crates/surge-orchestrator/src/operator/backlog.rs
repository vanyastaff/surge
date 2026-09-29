//! The task backlog and ledger read models: the actionable backlog (`ready`)
//! and the complete task ledger, both read from the cross-run task-ledger
//! index.

use surge_core::{RoadmapStatus, RunId};
use surge_persistence::runs::Storage;
use surge_persistence::task_ledger::{TaskLedgerIndexFilter, TaskLedgerIndexRecord};

use crate::operator::error::OperatorError;

/// Which tasks of the actionable backlog to list.
#[derive(Debug, Clone)]
pub struct ReadyQuery {
    /// Only tasks with this exact status. `None` lists every unsettled task.
    pub status: Option<RoadmapStatus>,
    /// Only tasks discovered mid-run (with a `discovered_from` edge).
    pub discovered_only: bool,
    /// Only tasks belonging to this run.
    pub run_id: Option<RunId>,
    /// Maximum rows to return.
    pub limit: usize,
}

impl ReadyQuery {
    /// Row cap the `surge ready` command applies unless told otherwise.
    pub const DEFAULT_LIMIT: usize = 200;
}

impl Default for ReadyQuery {
    /// Every unsettled task of every run, capped at [`Self::DEFAULT_LIMIT`].
    fn default() -> Self {
        Self {
            status: None,
            discovered_only: false,
            run_id: None,
            limit: Self::DEFAULT_LIMIT,
        }
    }
}

/// Which tasks of the full ledger to list.
#[derive(Debug, Clone)]
pub struct LedgerQuery {
    /// Scope to this run.
    pub run_id: Option<RunId>,
    /// Maximum rows to return.
    pub limit: usize,
}

impl LedgerQuery {
    /// Row cap the `surge ledger` command applies unless told otherwise.
    pub const DEFAULT_LIMIT: usize = 500;
}

impl Default for LedgerQuery {
    /// The ledger of every run, capped at [`Self::DEFAULT_LIMIT`].
    fn default() -> Self {
        Self {
            run_id: None,
            limit: Self::DEFAULT_LIMIT,
        }
    }
}

/// The rows of the actionable backlog for `query` (raw, un-normalized —
/// callers that emit JSON apply
/// [`TaskLedgerIndexRecord::with_verified_normalized`]).
///
/// Without an explicit `status`, settled tasks (completed, failed, skipped) are
/// dropped and the result is cut to `limit`; with one, the index applies the
/// status and `limit` itself.
///
/// Per-project scoping is disabled: a run records its isolated worktree path
/// as its project path, which never equals the invoking repo, so every project
/// is always listed.
///
/// # Errors
/// Returns [`OperatorError::TaskLedger`] if the index query fails.
pub fn query_ready(
    storage: &Storage,
    query: &ReadyQuery,
) -> Result<Vec<TaskLedgerIndexRecord>, OperatorError> {
    let query_limit = query.status.map(|_| query.limit);
    let mut records = storage
        .task_ledger_store()
        .list(&TaskLedgerIndexFilter {
            status: query.status,
            project_path: None,
            run_id: query.run_id,
            discovered_only: query.discovered_only,
            limit: query_limit,
        })
        .map_err(OperatorError::TaskLedger)?;

    if query.status.is_none() {
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
/// Returns [`OperatorError::TaskLedger`] if the index query fails.
pub fn query_ledger(
    storage: &Storage,
    query: &LedgerQuery,
) -> Result<Vec<TaskLedgerIndexRecord>, OperatorError> {
    storage
        .task_ledger_store()
        .list(&TaskLedgerIndexFilter {
            status: None,
            project_path: None,
            run_id: query.run_id,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settled_states_are_dropped_from_default_view() {
        assert!(is_settled(RoadmapStatus::Completed));
        assert!(is_settled(RoadmapStatus::Failed));
        assert!(is_settled(RoadmapStatus::Skipped));
        assert!(!is_settled(RoadmapStatus::Pending));
        assert!(!is_settled(RoadmapStatus::ReadyForVerification));
    }
}
