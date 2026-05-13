//! `/feedback <run_id> <text>` — keyboard-less alternative to the
//! forced-reply edit flow.
//!
//! When the operator does not want to use the `✏ Edit` inline button
//! (e.g. they are on a desktop chat where the forced-reply UX is
//! awkward), `/feedback` lets them attach an edit response to a run by
//! id. The handler builds the same `{"outcome": "edit", "comment":
//! "..."}` JSON the callback router produces and forwards it through
//! the shared [`EngineResolver`] trait.

use async_trait::async_trait;
use serde_json::json;

use crate::cockpit::callback::EngineResolver;
use crate::commands::CommandReply;
use crate::error::Result;

/// Handle `/feedback <run_id> <text>`.
///
/// Splits the first whitespace token off as the run id; the remainder
/// is the feedback body. Empty arguments or a missing feedback body
/// returns a usage hint.
pub async fn handle_feedback<E: EngineResolver>(
    chat_id: i64,
    args: &str,
    engine: &E,
) -> Result<CommandReply> {
    let Some((run_id, comment)) = split_first_token(args.trim()) else {
        return Ok(CommandReply::new(
            "Usage: `/feedback <run_id> <text>` — attach edit feedback to a run.",
        ));
    };
    if comment.is_empty() {
        return Ok(CommandReply::new(
            "Usage: `/feedback <run_id> <text>` — attach edit feedback to a run.",
        ));
    }
    let response = json!({ "outcome": "edit", "comment": comment });
    match engine.resolve_human_input(run_id, None, response).await {
        Ok(()) => {
            tracing::info!(
                target: "telegram::cmd::feedback",
                %chat_id,
                %run_id,
                comment_len = comment.len(),
                "feedback delivered",
            );
            Ok(CommandReply::new(format!(
                "✅ Feedback attached to run `{run_id}`."
            )))
        },
        Err(err) => {
            tracing::warn!(
                target: "telegram::cmd::feedback",
                %chat_id,
                %run_id,
                error = %err,
                "feedback delivery failed",
            );
            Ok(CommandReply::new(format!(
                "❌ Could not deliver feedback to `{run_id}`: {err}"
            )))
        },
    }
}

fn split_first_token(s: &str) -> Option<(&str, &str)> {
    let s = s.trim_start();
    if s.is_empty() {
        return None;
    }
    let end = s
        .char_indices()
        .find(|(_, c)| c.is_whitespace())
        .map_or(s.len(), |(i, _)| i);
    if end == 0 {
        return None;
    }
    let (first, rest) = s.split_at(end);
    Some((first, rest.trim_start()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::TelegramCockpitError;
    use std::sync::Mutex;

    struct FakeEngine {
        result: Mutex<std::result::Result<(), TelegramCockpitError>>,
        calls: Mutex<Vec<(String, serde_json::Value)>>,
    }

    impl FakeEngine {
        fn ok() -> Self {
            Self {
                result: Mutex::new(Ok(())),
                calls: Mutex::new(Vec::new()),
            }
        }
        fn err() -> Self {
            Self {
                result: Mutex::new(Err(TelegramCockpitError::Persistence("nope".into()))),
                calls: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl EngineResolver for FakeEngine {
        async fn resolve_human_input(
            &self,
            run_id: &str,
            _call_id: Option<String>,
            response: serde_json::Value,
        ) -> Result<()> {
            self.calls
                .lock()
                .unwrap()
                .push((run_id.to_owned(), response));
            match &*self.result.lock().unwrap() {
                Ok(()) => Ok(()),
                Err(_) => Err(TelegramCockpitError::Persistence("test".into())),
            }
        }
    }

    #[tokio::test]
    async fn empty_args_returns_usage() {
        let engine = FakeEngine::ok();
        let reply = handle_feedback(7, "", &engine).await.unwrap();
        assert!(reply.text.contains("/feedback"));
        assert!(engine.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn missing_body_returns_usage() {
        let engine = FakeEngine::ok();
        let reply = handle_feedback(7, "01HK-RUN", &engine).await.unwrap();
        assert!(reply.text.contains("/feedback"));
        assert!(engine.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn happy_path_calls_engine_with_outcome_edit_and_comment() {
        let engine = FakeEngine::ok();
        let reply = handle_feedback(7, "01HK-RUN  please add error path", &engine)
            .await
            .unwrap();
        assert!(reply.text.contains("01HK-RUN"));
        let calls = engine.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "01HK-RUN");
        assert_eq!(calls[0].1["outcome"], "edit");
        assert_eq!(calls[0].1["comment"], "please add error path");
    }

    #[tokio::test]
    async fn engine_refusal_is_a_recoverable_reply() {
        let engine = FakeEngine::err();
        let reply = handle_feedback(7, "01HK comment body", &engine)
            .await
            .unwrap();
        assert!(reply.text.starts_with("❌"));
    }

    #[test]
    fn split_first_token_handles_corners() {
        assert_eq!(split_first_token("a b"), Some(("a", "b")));
        assert_eq!(split_first_token("  a   b c  "), Some(("a", "b c  ")));
        assert_eq!(split_first_token(""), None);
        assert_eq!(split_first_token("   "), None);
        assert_eq!(split_first_token("single"), Some(("single", "")));
    }
}
