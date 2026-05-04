//! `MultiplexingNotifier` — dispatches on `NotifyChannel` variant
//! to one of five built-in deliverers (each behind its own builder
//! method). Default state: all channels return `ChannelNotConfigured`.

use crate::deliverer::{NotifyDeliverer, NotifyDeliveryContext, NotifyError, RenderedNotification};
use async_trait::async_trait;
use std::sync::Arc;
use surge_core::notify_config::NotifyChannel;

/// Multiplexer that dispatches by channel kind to a per-channel `NotifyDeliverer`.
#[derive(Default, Clone)]
pub struct MultiplexingNotifier {
    desktop: Option<Arc<dyn NotifyDeliverer>>,
    webhook: Option<Arc<dyn NotifyDeliverer>>,
    slack: Option<Arc<dyn NotifyDeliverer>>,
    email: Option<Arc<dyn NotifyDeliverer>>,
    telegram: Option<Arc<dyn NotifyDeliverer>>,
}

impl MultiplexingNotifier {
    /// Construct a notifier with all channels unconfigured.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Install the Desktop channel deliverer.
    #[must_use]
    pub fn with_desktop(mut self, d: Arc<dyn NotifyDeliverer>) -> Self {
        self.desktop = Some(d);
        self
    }

    /// Install the Webhook channel deliverer.
    #[must_use]
    pub fn with_webhook(mut self, d: Arc<dyn NotifyDeliverer>) -> Self {
        self.webhook = Some(d);
        self
    }

    /// Install the Slack channel deliverer.
    #[must_use]
    pub fn with_slack(mut self, d: Arc<dyn NotifyDeliverer>) -> Self {
        self.slack = Some(d);
        self
    }

    /// Install the Email channel deliverer.
    #[must_use]
    pub fn with_email(mut self, d: Arc<dyn NotifyDeliverer>) -> Self {
        self.email = Some(d);
        self
    }

    /// Install the Telegram channel deliverer.
    #[must_use]
    pub fn with_telegram(mut self, d: Arc<dyn NotifyDeliverer>) -> Self {
        self.telegram = Some(d);
        self
    }
}

#[async_trait]
impl NotifyDeliverer for MultiplexingNotifier {
    async fn deliver(
        &self,
        ctx: &NotifyDeliveryContext<'_>,
        channel: &NotifyChannel,
        rendered: &RenderedNotification,
    ) -> Result<(), NotifyError> {
        let inner = match channel {
            NotifyChannel::Desktop => &self.desktop,
            NotifyChannel::Webhook { .. } => &self.webhook,
            NotifyChannel::Slack { .. } => &self.slack,
            NotifyChannel::Email { .. } => &self.email,
            NotifyChannel::Telegram { .. } => &self.telegram,
        };
        match inner {
            Some(d) => d.deliver(ctx, channel, rendered).await,
            None => Err(NotifyError::ChannelNotConfigured),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use surge_core::id::RunId;
    use surge_core::keys::NodeKey;
    use surge_core::notify_config::NotifySeverity;

    struct Recorder {
        calls: Mutex<u32>,
    }

    #[async_trait]
    impl NotifyDeliverer for Recorder {
        async fn deliver(
            &self,
            _ctx: &NotifyDeliveryContext<'_>,
            _ch: &NotifyChannel,
            _r: &RenderedNotification,
        ) -> Result<(), NotifyError> {
            *self.calls.lock().unwrap() += 1;
            Ok(())
        }
    }

    fn rendered() -> RenderedNotification {
        RenderedNotification {
            severity: NotifySeverity::Info,
            title: "t".into(),
            body: "b".into(),
            artifact_paths: vec![],
        }
    }

    #[tokio::test]
    async fn default_returns_channel_not_configured() {
        let mux = MultiplexingNotifier::new();
        let node = NodeKey::try_from("n").unwrap();
        let ctx = NotifyDeliveryContext {
            run_id: RunId::new(),
            node: &node,
        };
        let result = mux
            .deliver(&ctx, &NotifyChannel::Desktop, &rendered())
            .await;
        assert!(matches!(result, Err(NotifyError::ChannelNotConfigured)));
    }

    #[tokio::test]
    async fn dispatches_to_configured_channel() {
        let rec = Arc::new(Recorder {
            calls: Mutex::new(0),
        });
        let mux = MultiplexingNotifier::new().with_desktop(rec.clone());
        let node = NodeKey::try_from("n").unwrap();
        let ctx = NotifyDeliveryContext {
            run_id: RunId::new(),
            node: &node,
        };
        mux.deliver(&ctx, &NotifyChannel::Desktop, &rendered())
            .await
            .unwrap();
        assert_eq!(*rec.calls.lock().unwrap(), 1);
    }
}
