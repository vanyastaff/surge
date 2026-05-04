//! Desktop notification via `notify-rust`.

use crate::deliverer::{NotifyDeliverer, NotifyDeliveryContext, NotifyError, RenderedNotification};
use async_trait::async_trait;
use surge_core::notify_config::NotifyChannel;

/// Desktop deliverer using the system tray notification API.
#[derive(Default)]
pub struct DesktopDeliverer;

impl DesktopDeliverer {
    /// Construct a new deliverer.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl NotifyDeliverer for DesktopDeliverer {
    async fn deliver(
        &self,
        _ctx: &NotifyDeliveryContext<'_>,
        channel: &NotifyChannel,
        rendered: &RenderedNotification,
    ) -> Result<(), NotifyError> {
        let NotifyChannel::Desktop = channel else {
            return Err(NotifyError::Transport(
                "DesktopDeliverer received non-Desktop channel".into(),
            ));
        };

        // notify-rust is sync; offload to blocking task.
        let title = rendered.title.clone();
        let body = rendered.body.clone();
        tokio::task::spawn_blocking(move || -> Result<(), NotifyError> {
            notify_rust::Notification::new()
                .summary(&title)
                .body(&body)
                .show()
                .map_err(|e| NotifyError::Transport(e.to_string()))?;
            Ok(())
        })
        .await
        .map_err(|e| NotifyError::Transport(format!("blocking task: {e}")))??;

        Ok(())
    }
}
