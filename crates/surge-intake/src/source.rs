//! `TaskSource` trait — the contract every task-source adapter implements.
//!
//! Implementations live in this crate (Linear, GitHub Issues) or in
//! downstream adapter crates (`surge-intake-discord`, etc.).

use crate::Result;
use crate::types::{TaskDetails, TaskEvent, TaskId, TaskSummary};
use async_trait::async_trait;
use futures::stream::BoxStream;

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
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Compile-only check: trait is object-safe (`dyn TaskSource`).
    #[allow(dead_code)]
    fn assert_object_safe(_: Box<dyn TaskSource>) {}
}
