//! Telegram bot loop for inbox cards: outgoing delivery + incoming callbacks.

use crate::inbox::ActionChannel;
use chrono::{Duration as ChronoDuration, Utc};
use std::sync::Arc;
use surge_persistence::inbox_queue::{self, InboxActionKind};
use surge_persistence::runs::storage::Storage;
use teloxide::dispatching::{Dispatcher, UpdateFilterExt};
use teloxide::dptree;
use teloxide::prelude::*;
use teloxide::types::{CallbackQuery, ChatId, Update};
use teloxide::Bot;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

/// Telegram inbox bot — runs the long-poll dispatcher + outgoing-delivery loop.
pub struct TgInboxBot {
    bot: Bot,
    chat_id: ChatId,
    storage: Arc<Storage>,
}

impl TgInboxBot {
    /// Construct with the bot, target chat, and storage.
    #[must_use]
    pub fn new(bot: Bot, chat_id: ChatId, storage: Arc<Storage>) -> Self {
        Self { bot, chat_id, storage }
    }

    /// Drive both legs (outgoing + incoming) until cancellation.
    pub async fn run(self, shutdown: CancellationToken) {
        let outgoing = {
            let bot = self.bot.clone();
            let storage = Arc::clone(&self.storage);
            let shutdown = shutdown.clone();
            tokio::spawn(outgoing_loop(bot, self.chat_id, storage, shutdown))
        };
        let incoming = {
            let bot = self.bot.clone();
            let storage = Arc::clone(&self.storage);
            tokio::spawn(incoming_loop(bot, storage))
        };
        tokio::select! {
            _ = shutdown.cancelled() => {
                info!("TgInboxBot: shutdown signalled");
            }
            _ = outgoing => {}
            _ = incoming => {}
        }
    }
}

async fn outgoing_loop(
    _bot: Bot,
    _chat_id: ChatId,
    _storage: Arc<Storage>,
    _shutdown: CancellationToken,
) {
    // Implemented in Task 5.3.
}

async fn incoming_loop(bot: Bot, storage: Arc<Storage>) {
    let handler = Update::filter_callback_query().endpoint(on_callback);
    Dispatcher::builder(bot, handler)
        .dependencies(dptree::deps![storage])
        .enable_ctrlc_handler()
        .build()
        .dispatch()
        .await;
}

async fn on_callback(
    bot: Bot,
    q: CallbackQuery,
    storage: Arc<Storage>,
) -> ResponseResult<()> {
    let data = q.data.as_deref().unwrap_or("");
    match parse_callback_data(data) {
        Some((action, token)) => {
            match handle_action(&storage, action, token, ActionChannel::Telegram).await {
                Ok(()) => {
                    let _ = bot.answer_callback_query(q.id.clone()).text("Recorded").await;
                }
                Err(CallbackHandleError::TokenNotFound) => {
                    let _ = bot.answer_callback_query(q.id.clone()).text("Card expired").await;
                }
                Err(CallbackHandleError::Persistence(e)) => {
                    warn!(error = %e, "inbox callback persistence error");
                    let _ = bot
                        .answer_callback_query(q.id.clone())
                        .text("Internal error — see daemon logs")
                        .await;
                }
            }
        }
        None => {
            let _ = bot
                .answer_callback_query(q.id.clone())
                .text("Invalid action")
                .await;
        }
    }
    Ok(())
}

/// Parse `inbox:<action>:<token>` callback strings.
pub(crate) fn parse_callback_data(s: &str) -> Option<(InboxActionKind, &str)> {
    let mut parts = s.splitn(3, ':');
    if parts.next()? != "inbox" {
        return None;
    }
    let action = InboxActionKind::parse(parts.next()?)?;
    let token = parts.next()?;
    if token.is_empty() {
        return None;
    }
    Some((action, token))
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum CallbackHandleError {
    #[error("callback token not found")]
    TokenNotFound,
    #[error("persistence error: {0}")]
    Persistence(String),
}

/// Verify the token resolves to a ticket, then enqueue the action.
///
/// Shared by Telegram and Desktop receivers.
pub(crate) async fn handle_action(
    storage: &Storage,
    action: InboxActionKind,
    token: &str,
    via: ActionChannel,
) -> Result<(), CallbackHandleError> {
    // Resolve token → task_id.
    let task_id = {
        let conn = storage
            .acquire_registry_conn()
            .map_err(|e| CallbackHandleError::Persistence(e.to_string()))?;
        let repo = surge_persistence::intake::IntakeRepo::new(&conn);
        repo.fetch_by_callback_token(token)
            .map_err(|e| CallbackHandleError::Persistence(e.to_string()))?
            .map(|row| row.task_id)
    };
    let Some(task_id) = task_id else {
        return Err(CallbackHandleError::TokenNotFound);
    };

    // Enqueue.
    let snooze_until = match action {
        InboxActionKind::Snooze => Some(Utc::now() + ChronoDuration::hours(24)),
        _ => None,
    };
    let conn = storage
        .acquire_registry_conn()
        .map_err(|e| CallbackHandleError::Persistence(e.to_string()))?;
    inbox_queue::append_action(
        &conn,
        action,
        &task_id,
        token,
        via.as_str(),
        snooze_until,
    )
    .map_err(|e| CallbackHandleError::Persistence(e.to_string()))?;
    info!(task_id = %task_id, action = action.as_str(), via = via.as_str(), "inbox action enqueued");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_callback_data_valid_start() {
        let (kind, token) = parse_callback_data("inbox:start:01HKGZ").unwrap();
        assert_eq!(kind, InboxActionKind::Start);
        assert_eq!(token, "01HKGZ");
    }

    #[test]
    fn parse_callback_data_valid_snooze_skip() {
        assert_eq!(parse_callback_data("inbox:snooze:t").unwrap().0, InboxActionKind::Snooze);
        assert_eq!(parse_callback_data("inbox:skip:t").unwrap().0, InboxActionKind::Skip);
    }

    #[test]
    fn parse_callback_data_invalid_prefix() {
        assert!(parse_callback_data("approval:start:t").is_none());
    }

    #[test]
    fn parse_callback_data_invalid_action() {
        assert!(parse_callback_data("inbox:meow:t").is_none());
    }

    #[test]
    fn parse_callback_data_empty_token() {
        assert!(parse_callback_data("inbox:start:").is_none());
    }
}
