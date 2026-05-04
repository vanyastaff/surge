//! Telegram notification via Bot API `sendMessage`.

use crate::deliverer::{NotifyDeliverer, NotifyDeliveryContext, NotifyError, RenderedNotification};
use async_trait::async_trait;
use std::sync::Arc;
use surge_core::notify_config::NotifyChannel;

/// Resolves a secret reference to Telegram bot token + chat id.
#[async_trait]
pub trait TelegramSecretResolver: Send + Sync {
    /// Resolve the chat-id reference to credentials.
    async fn resolve(&self, chat_id_ref: &str) -> Result<TelegramCredentials, NotifyError>;
}

/// Resolved Telegram credentials.
pub struct TelegramCredentials {
    /// Bot token from `@BotFather`.
    pub bot_token: String,
    /// Numeric chat id (or `@channelname`).
    pub chat_id: String,
}

/// Telegram deliverer using Bot API `sendMessage`.
pub struct TelegramDeliverer {
    client: reqwest::Client,
    resolver: Arc<dyn TelegramSecretResolver>,
}

impl TelegramDeliverer {
    /// Construct with a caller-supplied resolver.
    #[must_use]
    pub fn new(resolver: Arc<dyn TelegramSecretResolver>) -> Self {
        Self {
            client: reqwest::Client::new(),
            resolver,
        }
    }
}

#[async_trait]
impl NotifyDeliverer for TelegramDeliverer {
    async fn deliver(
        &self,
        _ctx: &NotifyDeliveryContext<'_>,
        channel: &NotifyChannel,
        rendered: &RenderedNotification,
    ) -> Result<(), NotifyError> {
        let NotifyChannel::Telegram { chat_id_ref } = channel else {
            return Err(NotifyError::Transport(
                "TelegramDeliverer received non-Telegram channel".into(),
            ));
        };

        let creds = self.resolver.resolve(chat_id_ref).await?;
        let url = format!(
            "https://api.telegram.org/bot{}/sendMessage",
            creds.bot_token
        );
        let payload = serde_json::json!({
            "chat_id": creds.chat_id,
            "text": format!("{}\n\n{}", rendered.title, rendered.body),
        });
        let response = self
            .client
            .post(&url)
            .json(&payload)
            .send()
            .await
            .map_err(|e| NotifyError::Transport(format!("Telegram POST: {e}")))?;
        if !response.status().is_success() {
            return Err(NotifyError::Transport(format!(
                "Telegram sendMessage status: {}",
                response.status()
            )));
        }
        Ok(())
    }
}
