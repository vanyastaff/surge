//! Slack notification via Web API `chat.postMessage`.
//!
//! Requires a bot token resolved from the channel's `channel_ref`
//! secret reference.

use crate::deliverer::{NotifyDeliverer, NotifyDeliveryContext, NotifyError, RenderedNotification};
use async_trait::async_trait;
use std::sync::Arc;
use surge_core::notify_config::NotifyChannel;

/// Resolves a secret reference to Slack bot token + channel id.
/// Caller-supplied — `surge-notify` doesn't own a secret store.
#[async_trait]
pub trait SlackSecretResolver: Send + Sync {
    /// Resolve the channel reference to credentials.
    async fn resolve(&self, channel_ref: &str) -> Result<SlackCredentials, NotifyError>;
}

/// Resolved Slack credentials: bot token + channel id.
pub struct SlackCredentials {
    /// Slack bot token (e.g., `xoxb-...`).
    pub bot_token: String,
    /// Slack channel id (e.g., `C01ABCDEF`).
    pub channel_id: String,
}

/// Slack deliverer using `chat.postMessage`.
pub struct SlackDeliverer {
    client: reqwest::Client,
    resolver: Arc<dyn SlackSecretResolver>,
}

impl SlackDeliverer {
    /// Construct with a caller-supplied resolver.
    #[must_use]
    pub fn new(resolver: Arc<dyn SlackSecretResolver>) -> Self {
        Self {
            client: reqwest::Client::new(),
            resolver,
        }
    }
}

#[async_trait]
impl NotifyDeliverer for SlackDeliverer {
    async fn deliver(
        &self,
        _ctx: &NotifyDeliveryContext<'_>,
        channel: &NotifyChannel,
        rendered: &RenderedNotification,
    ) -> Result<(), NotifyError> {
        let NotifyChannel::Slack { channel_ref } = channel else {
            return Err(NotifyError::Transport(
                "SlackDeliverer received non-Slack channel".into(),
            ));
        };

        let creds = self.resolver.resolve(channel_ref).await?;

        let payload = serde_json::json!({
            "channel": creds.channel_id,
            "text": format!("*{}*\n{}", rendered.title, rendered.body),
        });

        let response = self
            .client
            .post("https://slack.com/api/chat.postMessage")
            .bearer_auth(&creds.bot_token)
            .json(&payload)
            .send()
            .await
            .map_err(|e| NotifyError::Transport(format!("Slack POST: {e}")))?;

        let status = response.status();
        let body: serde_json::Value = response
            .json()
            .await
            .map_err(|e| NotifyError::Transport(format!("Slack response parse: {e}")))?;

        if !status.is_success() || body.get("ok") != Some(&serde_json::Value::Bool(true)) {
            return Err(NotifyError::Transport(format!(
                "Slack chat.postMessage failed: status={status}, body={body}"
            )));
        }
        Ok(())
    }
}
