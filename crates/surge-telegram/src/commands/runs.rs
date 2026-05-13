//! `/runs` — list the most recent runs the cockpit knows about.
//!
//! Requires the chat to be on the pairings allowlist (Decision 6); the
//! admission check lives in the bot loop's command router.

use async_trait::async_trait;

use crate::commands::CommandReply;
use crate::error::Result;

/// One row in the `/runs` listing. Production wraps the
/// `surge_persistence::runs::registry::RunSummary` type, projecting just
/// the fields the bot renders so the trait does not need to expose the
/// full schema.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunRow {
    /// Run id rendered to the operator.
    pub run_id: String,
    /// Coarse lifecycle status (e.g. `Active`, `Completed`, `Failed`).
    pub status: String,
    /// Unix epoch ms of run creation.
    pub started_at_ms: i64,
}

/// Reads the recent-runs view. Production wraps
/// `surge_persistence::runs::registry::list_runs`.
#[async_trait]
pub trait RunListProvider: Send + Sync {
    /// Return up to `limit` recent runs, newest first.
    async fn list_recent(&self, limit: u32) -> Result<Vec<RunRow>>;
}

/// Default page size for `/runs`. Telegram allows ~4096 bytes per body,
/// which is comfortably above 20 rows.
pub const DEFAULT_LIMIT: u32 = 20;

/// Handle `/runs`.
///
/// Ignores any args today — pagination / filtering will be wired with the
/// rest of Task 17 once the bot loop lands.
///
/// # Errors
///
/// Returns whatever the underlying [`RunListProvider`] returns.
pub async fn handle_runs<P>(chat_id: i64, _args: &str, provider: &P) -> Result<CommandReply>
where
    P: RunListProvider,
{
    let rows = provider.list_recent(DEFAULT_LIMIT).await?;
    let body = if rows.is_empty() {
        "📋 *Runs*\n\nNo runs yet.".to_owned()
    } else {
        let mut buf = String::from("📋 *Recent runs*\n\n");
        for row in &rows {
            // 4096 bytes / 20 rows ≈ 204 bytes per row; we render compact
            // single-line entries to stay well below the limit.
            buf.push_str(&format!(
                "`{run_id}` — {status}\n",
                run_id = row.run_id,
                status = row.status
            ));
        }
        buf
    };

    tracing::info!(
        target: "telegram::cmd::runs",
        chat_id = %chat_id,
        count = rows.len(),
        "runs listed",
    );

    Ok(CommandReply::new(body))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeList {
        rows: Mutex<Vec<RunRow>>,
        calls: Mutex<Vec<u32>>,
    }

    impl FakeList {
        fn with_rows(rows: Vec<RunRow>) -> Self {
            Self {
                rows: Mutex::new(rows),
                calls: Mutex::default(),
            }
        }
    }

    #[async_trait]
    impl RunListProvider for FakeList {
        async fn list_recent(&self, limit: u32) -> Result<Vec<RunRow>> {
            self.calls.lock().unwrap().push(limit);
            Ok(self.rows.lock().unwrap().clone())
        }
    }

    #[tokio::test]
    async fn empty_list_yields_no_runs_yet_text() {
        let provider = FakeList::default();
        let reply = handle_runs(42, "", &provider).await.unwrap();
        assert!(reply.text.contains("No runs yet"));
        assert_eq!(provider.calls.lock().unwrap()[0], DEFAULT_LIMIT);
    }

    #[tokio::test]
    async fn non_empty_list_renders_one_line_per_row() {
        let rows = vec![
            RunRow {
                run_id: "run-1".into(),
                status: "Active".into(),
                started_at_ms: 0,
            },
            RunRow {
                run_id: "run-2".into(),
                status: "Completed".into(),
                started_at_ms: 1_000,
            },
        ];
        let provider = FakeList::with_rows(rows);
        let reply = handle_runs(42, "", &provider).await.unwrap();
        assert!(reply.text.contains("run-1"));
        assert!(reply.text.contains("Active"));
        assert!(reply.text.contains("run-2"));
        assert!(reply.text.contains("Completed"));
        assert!(reply.text.contains("Recent runs"));
    }

    #[tokio::test]
    async fn list_uses_default_limit() {
        let provider = FakeList::default();
        let _ = handle_runs(42, "anything", &provider).await.unwrap();
        assert_eq!(provider.calls.lock().unwrap()[0], DEFAULT_LIMIT);
    }
}
