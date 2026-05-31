//! `TaskSource` trait — the contract every task-source adapter implements.
//!
//! Implementations live in this crate (Linear, GitHub Issues) or in
//! downstream adapter crates (`surge-intake-discord`, etc.).

use crate::Result;
use crate::types::{TaskDetails, TaskEvent, TaskId, TaskSummary};
use async_trait::async_trait;
use futures::stream::BoxStream;

/// Result of a merge-readiness check against an external PR.
///
/// Returned by [`TaskSource::check_merge_readiness`] and consumed by the
/// L3 auto-merge gate (`surge-daemon::automation_merge_gate`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeReadiness {
    /// All required checks are green and the PR has an approved review.
    Ready,
    /// One or more requirements failed; the human-readable reason is
    /// posted to the tracker as part of the `merge-blocked` comment.
    Blocked(String),
}

/// Outcome of an attempted PR merge.
///
/// Returned by [`TaskSource::merge_pr`] and consumed by the L3 auto-merge
/// gate. `Merged` and `AlreadyMerged` are both success terminals (the gate
/// records a durable dedup row so a re-fired completion never double-merges);
/// `Conflict` is the expected "could not proceed" failure mode that the gate
/// surfaces as a tracker comment plus an operator escalation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeOutcome {
    /// The merge succeeded.
    Merged,
    /// The PR was already merged before this attempt — a clean, idempotent
    /// no-op (e.g. a manual merge raced the gate, or a recovery re-fire).
    AlreadyMerged,
    /// The merge could not proceed: conflict, head moved, or the readiness
    /// the gate observed went stale before the merge call landed. Carries a
    /// human-readable reason for the tracker comment + escalation.
    Conflict(String),
}

/// Adapter to an external task tracker (Linear, GitHub Issues, future Discord/Jira/...).
///
/// `TaskSource` exposes only the operations that respect the
/// **tracker is master** authority model: read tickets, write comments,
/// set labels. Status changes / assignments are intentionally absent.
#[async_trait]
pub trait TaskSource: Send + Sync {
    /// Stable identifier (e.g. `"linear:wsp_acme"`). Used as foreign key in storage.
    fn id(&self) -> &str;

    /// Human-readable name (shown in inbox cards, logs).
    fn display_name(&self) -> &str;

    /// Provider type tag (`"linear"`, `"github_issues"`, ...).
    fn provider(&self) -> &'static str;

    /// Stream of incoming task events. Implementations may use polling,
    /// long-poll, or webhook delivery — the consumer doesn't care.
    fn watch_for_tasks<'a>(&'a self) -> BoxStream<'a, Result<TaskEvent>>;

    /// Fetch full details of a single task on demand (used by Triage Author).
    async fn fetch_task(&self, id: &TaskId) -> Result<TaskDetails>;

    /// List currently open tasks (bounded — provider-specific cap).
    /// Used to assemble Triage Author's candidate set.
    async fn list_open_tasks(&self) -> Result<Vec<TaskSummary>>;

    /// Mark that we've seen and started processing a task. Idempotent.
    /// (Used to gate retries; storage-side only, no provider call required.)
    async fn acknowledge_task(&self, id: &TaskId) -> Result<()>;

    /// Post a comment on the task. Idempotency is the implementation's
    /// responsibility (Linear has idempotency keys; GitHub does not — use
    /// telltale-prefix detection there).
    async fn post_comment(&self, id: &TaskId, body: &str) -> Result<()>;

    /// Set or remove a label on the task. Natively idempotent.
    async fn set_label(&self, id: &TaskId, label: &str, present: bool) -> Result<()>;

    /// Read the current labels on the task.
    async fn read_labels(&self, id: &TaskId) -> Result<Vec<String>>;

    /// Evaluate whether the PR linked to this task is ready to auto-merge.
    ///
    /// Default implementation returns `MergeReadiness::Blocked` —
    /// providers that have a PR concept (e.g. GitHub) override this to
    /// inspect required checks and review approvals. Linear and other
    /// PR-less trackers keep the default; the L3 gate then surfaces an
    /// explicit "provider does not support merge readiness" comment on
    /// the ticket rather than silently no-op.
    ///
    /// # Errors
    ///
    /// Returns a transport error if the underlying API call fails.
    /// Logical "not ready" outcomes are returned as
    /// [`MergeReadiness::Blocked`], not errors.
    async fn check_merge_readiness(&self, _id: &TaskId) -> Result<MergeReadiness> {
        Ok(MergeReadiness::Blocked(
            "provider does not implement merge readiness checks".into(),
        ))
    }

    /// Merge the PR linked to this task.
    ///
    /// The L3 auto-merge gate calls this only after
    /// [`TaskSource::check_merge_readiness`] returned [`MergeReadiness::Ready`].
    /// The merge method (squash / merge / rebase) is the implementation's own
    /// concern — it is a property of the repository convention, configured on
    /// the source, not chosen by the gate.
    ///
    /// Default implementation returns an error — providers without a PR
    /// concept (Linear, etc.) never reach this path because their
    /// [`TaskSource::check_merge_readiness`] stays `Blocked`. The gate treats
    /// an error here as an escalation, never a silent success.
    ///
    /// # Errors
    ///
    /// Returns a transport error if the merge API call fails for a reason
    /// other than "not mergeable" (those are reported as
    /// [`MergeOutcome::Conflict`], not errors).
    async fn merge_pr(&self, _id: &TaskId) -> Result<MergeOutcome> {
        Err(crate::Error::Internal(
            "provider does not implement PR merge".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Compile-only check: trait is object-safe (`dyn TaskSource`).
    #[allow(dead_code)]
    fn assert_object_safe(_: Box<dyn TaskSource>) {}
}
