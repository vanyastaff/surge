//! `NotifyDeliverer` trait + `NotifyError`.

use async_trait::async_trait;
use std::path::PathBuf;
use surge_core::id::RunId;
use surge_core::keys::NodeKey;
use surge_core::notify_config::{NotifyChannel, NotifySeverity};
use thiserror::Error;

/// Pluggable channel delivery.
#[async_trait]
pub trait NotifyDeliverer: Send + Sync {
    /// Deliver `rendered` over `channel`. Returns `Ok(())` on success
    /// or one of the [`NotifyError`] variants on failure.
    async fn deliver(
        &self,
        ctx: &NotifyDeliveryContext<'_>,
        channel: &NotifyChannel,
        rendered: &RenderedNotification,
    ) -> Result<(), NotifyError>;
}

/// Per-call delivery context — read-only metadata about the run + node.
pub struct NotifyDeliveryContext<'a> {
    /// Run identifier for cross-referencing with the event log.
    pub run_id: RunId,
    /// `NodeKey` of the Notify node emitting the notification.
    pub node: &'a NodeKey,
}

/// Pre-rendered notification ready for delivery.
#[derive(Debug, Clone)]
pub struct RenderedNotification {
    /// Severity tier (Info/Warn/Error/Success).
    pub severity: NotifySeverity,
    /// Rendered title.
    pub title: String,
    /// Rendered body.
    pub body: String,
    /// Resolved artifact paths the recipient may want to inline / link.
    pub artifact_paths: Vec<PathBuf>,
}

/// Errors a `NotifyDeliverer` can produce.
#[derive(Debug, Error)]
pub enum NotifyError {
    /// Required secret reference (chat id, bot token, SMTP creds) missing.
    #[error("missing secret reference {0}")]
    MissingSecret(String),
    /// Network / SMTP / IPC transport error.
    #[error("transport error: {0}")]
    Transport(String),
    /// Template rendering failure (unclosed placeholder, etc).
    #[error("template render error: {0}")]
    Render(String),
    /// Channel was constructed but never configured (default deliverer).
    #[error("channel not configured")]
    ChannelNotConfigured,
}
