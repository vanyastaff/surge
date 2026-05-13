//! `/abort <run_id>` — request graceful cancellation of an in-flight run.
//!
//! The actual cancellation flows through `Engine::stop_run` (or its
//! daemon-facade equivalent); this handler is the thin wrapper that
//! parses the message, validates the id format, and asks the engine to
//! stop. Engine refusals (unknown run, terminal state) are surfaced as
//! recoverable replies — the cockpit must not crash on a stale id.

use async_trait::async_trait;

use crate::commands::CommandReply;
use crate::error::Result;

/// Engine surface for aborting an active run. Production wraps
/// `Engine::stop_run(run_id, reason)`.
#[async_trait]
pub trait RunAborter: Send + Sync {
    /// Stop the run with the given id (short or full form, the impl
    /// normalises).
    ///
    /// # Errors
    ///
    /// Returns [`TelegramCockpitError::EngineResolve`] when the engine
    /// rejects the request.
    async fn abort_run(&self, run_id: &str, reason: &str) -> Result<()>;
}

/// Handle `/abort <run_id>` from a paired chat.
pub async fn handle_abort<A: RunAborter>(
    chat_id: i64,
    args: &str,
    aborter: &A,
) -> Result<CommandReply> {
    let run_id = args.trim();
    if run_id.is_empty() {
        return Ok(CommandReply::new(
            "Usage: `/abort <run_id>` — request graceful cancellation.",
        ));
    }
    let reason = format!("aborted via Telegram by chat {chat_id}");
    match aborter.abort_run(run_id, &reason).await {
        Ok(()) => {
            tracing::info!(
                target: "telegram::cmd::abort",
                %chat_id,
                %run_id,
                "abort dispatched",
            );
            Ok(CommandReply::new(format!(
                "✅ Abort requested for `{run_id}`."
            )))
        },
        Err(err) => {
            tracing::warn!(
                target: "telegram::cmd::abort",
                %chat_id,
                %run_id,
                error = %err,
                "abort refused",
            );
            Ok(CommandReply::new(format!(
                "❌ Could not abort `{run_id}`: {err}"
            )))
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::TelegramCockpitError;
    use std::sync::Mutex;

    struct FakeAborter {
        result: Mutex<std::result::Result<(), TelegramCockpitError>>,
        calls: Mutex<Vec<(String, String)>>,
    }

    impl FakeAborter {
        fn ok() -> Self {
            Self {
                result: Mutex::new(Ok(())),
                calls: Mutex::new(Vec::new()),
            }
        }
        fn err() -> Self {
            Self {
                result: Mutex::new(Err(TelegramCockpitError::Persistence("gone".into()))),
                calls: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl RunAborter for FakeAborter {
        async fn abort_run(&self, run_id: &str, reason: &str) -> Result<()> {
            self.calls
                .lock()
                .unwrap()
                .push((run_id.to_owned(), reason.to_owned()));
            match &*self.result.lock().unwrap() {
                Ok(()) => Ok(()),
                Err(_) => Err(TelegramCockpitError::Persistence("test".into())),
            }
        }
    }

    #[tokio::test]
    async fn empty_args_returns_usage() {
        let aborter = FakeAborter::ok();
        let reply = handle_abort(7, "", &aborter).await.unwrap();
        assert!(reply.text.contains("/abort"));
        assert!(aborter.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn happy_path_reports_success_and_passes_reason_containing_chat_id() {
        let aborter = FakeAborter::ok();
        let reply = handle_abort(7, "01HK-RUN", &aborter).await.unwrap();
        assert!(reply.text.contains("01HK-RUN"));
        let calls = aborter.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "01HK-RUN");
        assert!(calls[0].1.contains("chat 7"));
    }

    #[tokio::test]
    async fn engine_refusal_is_a_recoverable_reply() {
        let aborter = FakeAborter::err();
        let reply = handle_abort(7, "01HK-RUN", &aborter).await.unwrap();
        assert!(reply.text.starts_with("❌"));
    }
}
