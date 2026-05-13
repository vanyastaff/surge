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

/// Consumes a previously-minted pairing token. Production wraps
/// `surge_persistence::telegram::pairing::consume_pairing_token`.
#[async_trait]
pub trait PairingTokenConsumer: Send + Sync {
    /// Consume `token`. Returns the label that was attached at mint time
    /// (used to label the resulting allowlist row).
    ///
    /// # Errors
    ///
    /// Returns [`TelegramCockpitError::PairingTokenInvalid`] for unknown
    /// tokens and [`TelegramCockpitError::PairingTokenExpired`] for stale
    /// or already-consumed ones.
    async fn consume(&self, token: &str, now_ms: i64) -> Result<String>;
}

/// Inserts an allowlist row. Production wraps
/// `surge_persistence::telegram::pairings::pair`.
#[async_trait]
pub trait PairingWriter: Send + Sync {
    /// Pair (or re-pair) `chat_id` with the given label.
    async fn pair(&self, chat_id: i64, user_label: &str, now_ms: i64) -> Result<()>;
}

/// Handle `/pair <token>` from an unpaired chat.
///
/// `args` is the rest of the message after `/pair`. Tokens are
/// case-insensitive on the wire (the database stores Crockford
/// uppercase); this handler normalises trim+uppercase before consume.
///
/// # Errors
///
/// Returns [`TelegramCockpitError`] when token consume or pairings write
/// fails for a reason other than a recoverable "invalid token" message —
/// those are reported back in [`CommandReply`] instead.
pub async fn handle_pair<C, W>(
    chat_id: i64,
    args: &str,
    consumer: &C,
    writer: &W,
    now_ms: i64,
) -> Result<CommandReply>
where
    C: PairingTokenConsumer,
    W: PairingWriter,
{
    let token = args.trim().to_ascii_uppercase();
    if token.is_empty() {
        return Ok(CommandReply::new(
            "Usage: `/pair <token>` — get a token from `surge telegram setup`.",
        ));
    }

    let consume = consumer.consume(&token, now_ms).await;
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
        Err(other) => return Err(other),
    };

    writer.pair(chat_id, &label, now_ms).await?;

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
        async fn consume(&self, token: &str, now_ms: i64) -> Result<String> {
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

    #[derive(Default)]
    struct FakeWriter {
        calls: Mutex<Vec<(i64, String, i64)>>,
    }

    #[async_trait]
    impl PairingWriter for FakeWriter {
        async fn pair(&self, chat_id: i64, user_label: &str, now_ms: i64) -> Result<()> {
            self.calls
                .lock()
                .unwrap()
                .push((chat_id, user_label.to_owned(), now_ms));
            Ok(())
        }
    }

    #[tokio::test]
    async fn empty_token_returns_usage_message() {
        let consumer = FakeConsumer::allowing("");
        let writer = FakeWriter::default();
        let reply = handle_pair(42, "", &consumer, &writer, 1_000)
            .await
            .unwrap();
        assert!(reply.text.contains("/pair"));
        assert!(consumer.calls.lock().unwrap().is_empty());
        assert!(writer.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn valid_token_pairs_chat_and_returns_success() {
        let consumer = FakeConsumer::allowing("phone");
        let writer = FakeWriter::default();
        let reply = handle_pair(42, "  abcdef  ", &consumer, &writer, 1_500)
            .await
            .unwrap();
        assert!(reply.text.contains("Paired"));
        assert!(reply.text.contains("phone"));

        // Token was normalised (trim + upper).
        let consume_calls = consumer.calls.lock().unwrap();
        assert_eq!(consume_calls.len(), 1);
        assert_eq!(consume_calls[0].0, "ABCDEF");
        assert_eq!(consume_calls[0].1, 1_500);

        // Pairing write happened with the right label.
        let pair_calls = writer.calls.lock().unwrap();
        assert_eq!(pair_calls.len(), 1);
        assert_eq!(pair_calls[0].0, 42);
        assert_eq!(pair_calls[0].1, "phone");
    }

    #[tokio::test]
    async fn unknown_token_returns_recoverable_error_message() {
        let consumer = FakeConsumer::rejecting(TelegramCockpitError::PairingTokenInvalid);
        let writer = FakeWriter::default();
        let reply = handle_pair(42, "BADTOK", &consumer, &writer, 1_000)
            .await
            .unwrap();
        assert!(reply.text.contains("not recognised"));
        // No pairings write must have happened.
        assert!(writer.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn expired_token_returns_recoverable_error_message() {
        let consumer = FakeConsumer::rejecting(TelegramCockpitError::PairingTokenExpired);
        let writer = FakeWriter::default();
        let reply = handle_pair(42, "OLDTOK", &consumer, &writer, 1_000)
            .await
            .unwrap();
        assert!(reply.text.contains("expired"));
        assert!(writer.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn token_unrelated_persistence_error_bubbles_up() {
        let consumer = FakeConsumer::rejecting(TelegramCockpitError::Persistence("DB down".into()));
        let writer = FakeWriter::default();
        let err = handle_pair(42, "ANY", &consumer, &writer, 1_000)
            .await
            .unwrap_err();
        assert!(matches!(err, TelegramCockpitError::Persistence(_)));
    }
}
