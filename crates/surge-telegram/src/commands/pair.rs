//! `/pair <token>` — the only handler that runs without prior admission.
//!
//! Decision 6 (Telegram cockpit milestone plan): unpaired chats can only
//! invoke `/pair`. The handler consumes a previously-minted pairing token
//! and, on success, inserts the calling chat into the
//! `telegram_pairings` allowlist.
//!
//! Token mint (`surge telegram setup`) is owned by the CLI; this module
//! only consumes.

use async_trait::async_trait;

use crate::commands::CommandReply;
use crate::error::{Result, TelegramCockpitError};

/// Atomically consumes a target-bound code and admits the matching chat.
#[async_trait]
pub trait PairingTokenConsumer: Send + Sync {
    /// # Errors
    /// Rejects invalid, expired, consumed, unbound or wrong-chat codes; propagates storage failures.
    async fn pair_with_token(&self, token: &str, chat_id: i64, now_ms: i64) -> Result<String>;
}

/// Handle `/pair <token>`; consumption and allowlist insertion share one transaction.
///
/// # Errors
/// Propagates storage errors. Invalid codes return actionable replies.
pub async fn handle_pair<C: PairingTokenConsumer>(
    chat_id: i64,
    args: &str,
    consumer: &C,
    now_ms: i64,
) -> Result<CommandReply> {
    let token = args.trim().to_ascii_uppercase();
    if token.is_empty() {
        return Ok(CommandReply::new(
            "Usage: `/pair <token>` — get a token from `surge telegram setup`.",
        ));
    }

    let consume = consumer.pair_with_token(&token, chat_id, now_ms).await;
    let label = match consume {
        Ok(label) => label,
        Err(TelegramCockpitError::PairingTokenInvalid) => {
            tracing::info!(
                target: "telegram::cmd::pair",
                chat_id = %chat_id,
                "pair rejected — token unknown",
            );
            return Ok(CommandReply::new(
                "❌ Pair failed: token not recognised. Generate a fresh one with `surge telegram setup`.",
            ));
        },
        Err(TelegramCockpitError::PairingTokenExpired) => {
            tracing::info!(
                target: "telegram::cmd::pair",
                chat_id = %chat_id,
                "pair rejected — token expired or consumed",
            );
            return Ok(CommandReply::new(
                "❌ Pair failed: token has expired or was already used. Generate a fresh one with `surge telegram setup`.",
            ));
        },
        Err(TelegramCockpitError::PairingTargetMismatch) => {
            return Ok(CommandReply::new(
                "Pair failed: this code is for a different chat.",
            ));
        },
        Err(TelegramCockpitError::PairingLegacyCode) => {
            return Ok(CommandReply::new(
                "Legacy unbound code rejected. Run surge telegram setup --token-env NAME --chat-id ID again.",
            ));
        },
        Err(other) => return Err(other),
    };

    tracing::info!(
        target: "telegram::cmd::pair",
        chat_id = %chat_id,
        label = %label,
        "chat paired",
    );

    Ok(CommandReply::new(format!(
        "✅ Paired this chat as `{label}`. You can now use `/status`, `/runs`, and approve cards."
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct FakeConsumer {
        result: Mutex<Result<String>>,
        calls: Mutex<Vec<(String, i64)>>,
    }

    impl FakeConsumer {
        fn allowing(label: &str) -> Self {
            Self {
                result: Mutex::new(Ok(label.to_owned())),
                calls: Mutex::new(Vec::new()),
            }
        }
        fn rejecting(err: TelegramCockpitError) -> Self {
            Self {
                result: Mutex::new(Err(err)),
                calls: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl PairingTokenConsumer for FakeConsumer {
        async fn pair_with_token(&self, token: &str, chat_id: i64, now_ms: i64) -> Result<String> {
            assert_eq!(chat_id, 42);
            self.calls.lock().unwrap().push((token.to_owned(), now_ms));
            match &*self.result.lock().unwrap() {
                Ok(label) => Ok(label.clone()),
                Err(TelegramCockpitError::PairingTokenInvalid) => {
                    Err(TelegramCockpitError::PairingTokenInvalid)
                },
                Err(TelegramCockpitError::PairingTokenExpired) => {
                    Err(TelegramCockpitError::PairingTokenExpired)
                },
                Err(_) => Err(TelegramCockpitError::Persistence("test".into())),
            }
        }
    }

    #[tokio::test]
    async fn empty_token_returns_usage_message() {
        let consumer = FakeConsumer::allowing("");
        let reply = handle_pair(42, "", &consumer, 1_000).await.unwrap();
        assert!(reply.text.contains("/pair"));
        assert!(consumer.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn valid_token_pairs_chat_and_returns_success() {
        let consumer = FakeConsumer::allowing("phone");
        let reply = handle_pair(42, "  abcdef  ", &consumer, 1_500)
            .await
            .unwrap();
        assert!(reply.text.contains("Paired"));
        assert!(reply.text.contains("phone"));

        // Token was normalised (trim + upper).
        let consume_calls = consumer.calls.lock().unwrap();
        assert_eq!(consume_calls.len(), 1);
        assert_eq!(consume_calls[0].0, "ABCDEF");
        assert_eq!(consume_calls[0].1, 1_500);
    }

    #[tokio::test]
    async fn unknown_token_returns_recoverable_error_message() {
        let consumer = FakeConsumer::rejecting(TelegramCockpitError::PairingTokenInvalid);
        let reply = handle_pair(42, "BADTOK", &consumer, 1_000).await.unwrap();
        assert!(reply.text.contains("not recognised"));
        // No pairings write must have happened.
    }

    #[tokio::test]
    async fn expired_token_returns_recoverable_error_message() {
        let consumer = FakeConsumer::rejecting(TelegramCockpitError::PairingTokenExpired);
        let reply = handle_pair(42, "OLDTOK", &consumer, 1_000).await.unwrap();
        assert!(reply.text.contains("expired"));
    }

    #[tokio::test]
    async fn token_unrelated_persistence_error_bubbles_up() {
        let consumer = FakeConsumer::rejecting(TelegramCockpitError::Persistence("DB down".into()));
        let err = handle_pair(42, "ANY", &consumer, 1_000).await.unwrap_err();
        assert!(matches!(err, TelegramCockpitError::Persistence(_)));
    }
}
