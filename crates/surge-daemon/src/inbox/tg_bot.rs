//! Telegram bot loop for inbox cards: outgoing delivery + incoming callbacks.

use crate::inbox::ActionChannel;
use chrono::{Duration as ChronoDuration, Utc};
use std::sync::Arc;
use surge_persistence::inbox_queue::{self, InboxActionKind};
use surge_persistence::runs::storage::Storage;
use teloxide::Bot;
use teloxide::dispatching::{Dispatcher, UpdateFilterExt};
use teloxide::dptree;
use teloxide::prelude::*;
use teloxide::types::{CallbackQuery, ChatId, Update};
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
        Self {
            bot,
            chat_id,
            storage,
        }
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
            () = shutdown.cancelled() => {
                info!("TgInboxBot: shutdown signalled");
            }
            _ = outgoing => {}
            _ = incoming => {}
        }
    }
}

async fn outgoing_loop(
    bot: Bot,
    chat_id: ChatId,
    storage: Arc<Storage>,
    shutdown: CancellationToken,
) {
    let mut interval = tokio::time::interval(std::time::Duration::from_millis(500));
    loop {
        tokio::select! {
            () = shutdown.cancelled() => return,
            _ = interval.tick() => {}
        }
        if let Err(e) = tick_outgoing(&bot, chat_id, &storage).await {
            warn!(error = %e, "TgInboxBot outgoing tick failed");
        }
    }
}

/// Convert a single `surge_notify` keyboard button descriptor into a teloxide
/// `InlineKeyboardButton`. URL buttons whose `data` field fails to parse fall
/// back to a callback button so the outgoing loop never crashes.
fn build_keyboard_button(
    btn: &surge_notify::telegram::InboxKeyboardButton,
) -> teloxide::types::InlineKeyboardButton {
    use teloxide::types::InlineKeyboardButton;
    if btn.is_url {
        if let Ok(url) = btn.data.parse::<reqwest::Url>() {
            return InlineKeyboardButton::url(btn.label.clone(), url);
        }
        warn!(label = %btn.label, data = %btn.data, "URL button has invalid URL; rendering as text");
        InlineKeyboardButton::callback(btn.label.clone(), format!("invalid_url:{}", btn.data))
    } else {
        InlineKeyboardButton::callback(btn.label.clone(), btn.data.clone())
    }
}

async fn tick_outgoing(bot: &Bot, chat_id: ChatId, storage: &Storage) -> Result<(), String> {
    use surge_notify::messages::InboxCardPayload;
    use surge_notify::telegram::format_inbox_card;
    use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup};

    let pending = {
        let conn = storage.acquire_registry_conn().map_err(|e| e.to_string())?;
        inbox_queue::list_pending_telegram_deliveries(&conn).map_err(|e| e.to_string())?
    };
    for row in pending {
        let payload: InboxCardPayload = match serde_json::from_str(&row.payload_json) {
            Ok(p) => p,
            Err(e) => {
                warn!(
                    error = %e,
                    seq = row.seq,
                    "failed to parse delivery payload; marking as delivered (sentinel chat_id=0) to break retry loop"
                );
                // Without this, the row would stay pending and the 500ms
                // outgoing tick would hot-loop forever on the same parse
                // error. Sentinel chat_id=0 + msg_id=0 marks the row out
                // of the pending pool. A future migration may add a
                // dedicated `delivery_failed_reason` column.
                let conn = storage.acquire_registry_conn().map_err(|e| e.to_string())?;
                inbox_queue::record_telegram_delivered(&conn, row.seq, 0, 0)
                    .map_err(|e| e.to_string())?;
                continue;
            },
        };
        let rendered = format_inbox_card(&payload);
        let kb_rows: Vec<Vec<InlineKeyboardButton>> = rendered
            .keyboard
            .iter()
            .map(|row| row.iter().map(build_keyboard_button).collect())
            .collect();
        let kb = InlineKeyboardMarkup::new(kb_rows);
        match bot
            .send_message(chat_id, rendered.body)
            .reply_markup(kb)
            .await
        {
            Ok(msg) => {
                let conn = storage.acquire_registry_conn().map_err(|e| e.to_string())?;
                inbox_queue::record_telegram_delivered(&conn, row.seq, chat_id.0, msg.id.0)
                    .map_err(|e| e.to_string())?;
                let repo = surge_persistence::intake::IntakeRepo::new(&conn);
                if let Err(e) = repo.set_tg_message_ref(&row.task_id, chat_id.0, msg.id.0) {
                    // The row may not exist yet (if enqueue_inbox_card was skipped or
                    // the row is in another state); log but don't fail the loop.
                    warn!(error = %e, task_id = %row.task_id, "failed to persist tg message ref");
                }
                info!(
                    task_id = %row.task_id,
                    seq = row.seq,
                    "InboxCard delivered to Telegram"
                );
            },
            Err(e) => {
                warn!(error = %e, task_id = %row.task_id, "Telegram send failed; will retry");
                // Don't mark as delivered — next tick retries.
            },
        }
    }
    Ok(())
}

async fn incoming_loop(bot: Bot, storage: Arc<Storage>) {
    let handler = Update::filter_callback_query().endpoint(on_callback);
    Box::pin(
        Dispatcher::builder(bot, handler)
            .dependencies(dptree::deps![storage])
            .enable_ctrlc_handler()
            .build()
            .dispatch(),
    )
    .await;
}

async fn on_callback(bot: Bot, q: CallbackQuery, storage: Arc<Storage>) -> ResponseResult<()> {
    let data = q.data.as_deref().unwrap_or("");
    match parse_callback_data(data) {
        Some((action, token)) => {
            match handle_action(&storage, action, token, ActionChannel::Telegram).await {
                Ok(()) => {
                    let _ = bot
                        .answer_callback_query(q.id.clone())
                        .text("Recorded")
                        .await;
                },
                Err(CallbackHandleError::TokenNotFound) => {
                    let _ = bot
                        .answer_callback_query(q.id.clone())
                        .text("Card expired")
                        .await;
                },
                Err(CallbackHandleError::Persistence(e)) => {
                    warn!(error = %e, "inbox callback persistence error");
                    let _ = bot
                        .answer_callback_query(q.id.clone())
                        .text("Internal error — see daemon logs")
                        .await;
                },
            }
        },
        None => {
            let _ = bot
                .answer_callback_query(q.id.clone())
                .text("Invalid action")
                .await;
        },
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
#[allow(clippy::unused_async)]
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
    inbox_queue::append_action(&conn, action, &task_id, token, via.as_str(), snooze_until)
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
        assert_eq!(
            parse_callback_data("inbox:snooze:t").unwrap().0,
            InboxActionKind::Snooze
        );
        assert_eq!(
            parse_callback_data("inbox:skip:t").unwrap().0,
            InboxActionKind::Skip
        );
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
