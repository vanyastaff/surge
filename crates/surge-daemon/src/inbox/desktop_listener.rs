//! Desktop action listener.
//!
//! Spawns `notify-rust::Notification::show()` per pending desktop card and
//! waits on `wait_for_action` in a blocking task; the chosen action is
//! forwarded into `inbox_action_queue` via `tg_bot::handle_action`.
//!
//! `wait_for_action` is only available on Linux (dbus/zbus). On macOS and
//! Windows the notification is shown but action callbacks are not supported by
//! the underlying platform API; those platforms fall through to a no-op.

use std::sync::Arc;
use std::time::Duration;
use surge_persistence::inbox_queue;
use surge_persistence::runs::storage::Storage;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

/// Polls pending desktop deliveries and shows them via `notify-rust`,
/// forwarding action callbacks into the inbox-action queue.
pub struct DesktopActionListener {
    storage: Arc<Storage>,
    poll_interval: Duration,
}

impl DesktopActionListener {
    /// Construct with the storage handle. Default poll interval is 500ms.
    #[must_use]
    pub fn new(storage: Arc<Storage>) -> Self {
        Self {
            storage,
            poll_interval: Duration::from_millis(500),
        }
    }

    /// Drive the polling loop until cancellation.
    pub async fn run(self, shutdown: CancellationToken) {
        let mut interval = tokio::time::interval(self.poll_interval);
        loop {
            tokio::select! {
                () = shutdown.cancelled() => return,
                _ = interval.tick() => {}
            }
            if let Err(e) = self.tick().await {
                warn!(error = %e, "desktop listener tick failed");
            }
        }
    }

    #[allow(clippy::unused_async)]
    async fn tick(&self) -> Result<(), String> {
        let pending = {
            let conn = self
                .storage
                .acquire_registry_conn()
                .map_err(|e| e.to_string())?;
            inbox_queue::list_pending_desktop_deliveries(&conn).map_err(|e| e.to_string())?
        };
        for row in pending {
            // Mark as delivered immediately to avoid duplicates if notify-rust
            // takes a long time. wait_for_action runs in a blocking thread.
            {
                let conn = self
                    .storage
                    .acquire_registry_conn()
                    .map_err(|e| e.to_string())?;
                inbox_queue::record_desktop_delivered(&conn, row.seq).map_err(|e| e.to_string())?;
            }

            let payload: surge_notify::messages::InboxCardPayload =
                match serde_json::from_str(&row.payload_json) {
                    Ok(p) => p,
                    Err(e) => {
                        warn!(error = %e, seq = row.seq, "desktop payload parse failed; skipping");
                        continue;
                    },
                };
            let rendered = surge_notify::desktop::format_inbox_card_desktop(&payload);
            let token = payload.callback_token.clone();
            let storage = Arc::clone(&self.storage);
            let task_id_for_log = payload.task_id.as_str().to_string();
            tokio::task::spawn_blocking(move || {
                show_and_wait(rendered, token, storage, task_id_for_log);
            });
        }
        Ok(())
    }
}

/// Platform-specific: show the notification and, where supported, wait for
/// an action and bridge it back into the async runtime.
///
/// `wait_for_action` is only available on Linux (dbus/zbus, `unix` non-macOS).
/// On other platforms we show the notification but cannot receive click events.
#[allow(clippy::needless_pass_by_value)]
fn show_and_wait(
    rendered: surge_notify::desktop::InboxCardDesktopRendered,
    #[cfg(all(unix, not(target_os = "macos")))] token: String,
    #[cfg(not(all(unix, not(target_os = "macos"))))] _token: String,
    #[cfg(all(unix, not(target_os = "macos")))] storage: Arc<Storage>,
    #[cfg(not(all(unix, not(target_os = "macos"))))] _storage: Arc<Storage>,
    task_id_for_log: String,
) {
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        use crate::inbox::ActionChannel;
        use crate::inbox::tg_bot;
        use tracing::debug;

        let mut n = notify_rust::Notification::new();
        n.summary(&rendered.title).body(&rendered.body);
        for (action_id, label) in &rendered.actions {
            n.action(action_id, label);
        }
        let handle = match n.show() {
            Ok(h) => h,
            Err(e) => {
                warn!(error = %e, "notify-rust show failed");
                return;
            },
        };
        handle.wait_for_action(|action_id| {
            let action_kind = match parse_desktop_action_id(action_id) {
                Some(k) => k,
                None => {
                    debug!(action_id, "ignored desktop action (dismiss/expired)");
                    return;
                },
            };
            // Bridge into async via the current tokio runtime handle.
            let storage = Arc::clone(&storage);
            let token = token.clone();
            if let Ok(rt) = tokio::runtime::Handle::try_current() {
                rt.spawn(async move {
                    if let Err(e) =
                        tg_bot::handle_action(&storage, action_kind, &token, ActionChannel::Desktop)
                            .await
                    {
                        warn!(error = ?e, "desktop action enqueue failed");
                    }
                });
            } else {
                warn!("no tokio runtime available for desktop action; lost");
            }
        });
        info!(task_id = %task_id_for_log, "desktop card dismissed/answered");
    }

    #[cfg(not(all(unix, not(target_os = "macos"))))]
    {
        // On macOS and Windows notify-rust::show() returns () — no handle.
        // Show the notification; action callbacks are not supported here.
        let _ = notify_rust::Notification::new()
            .summary(&rendered.title)
            .body(&rendered.body)
            .show();
        info!(task_id = %task_id_for_log, "desktop card shown (no action callback on this platform)");
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn parse_desktop_action_id(s: &str) -> Option<surge_persistence::inbox_queue::InboxActionKind> {
    // Action IDs from desktop formatter are "inbox:start:<token>" etc.
    let mut parts = s.splitn(3, ':');
    if parts.next()? != "inbox" {
        return None;
    }
    surge_persistence::inbox_queue::InboxActionKind::parse(parts.next()?)
}

// On non-Linux platforms `parse_desktop_action_id` is unused at runtime but
// must still compile for tests. Expose it unconditionally in test builds.
#[cfg(not(all(unix, not(target_os = "macos"))))]
#[allow(dead_code)]
fn parse_desktop_action_id(s: &str) -> Option<surge_persistence::inbox_queue::InboxActionKind> {
    let mut parts = s.splitn(3, ':');
    if parts.next()? != "inbox" {
        return None;
    }
    surge_persistence::inbox_queue::InboxActionKind::parse(parts.next()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use surge_persistence::inbox_queue::InboxActionKind;

    #[test]
    fn desktop_action_id_parses_three_kinds() {
        assert_eq!(
            parse_desktop_action_id("inbox:start:tok").unwrap(),
            InboxActionKind::Start
        );
        assert_eq!(
            parse_desktop_action_id("inbox:snooze:tok").unwrap(),
            InboxActionKind::Snooze
        );
        assert_eq!(
            parse_desktop_action_id("inbox:skip:tok").unwrap(),
            InboxActionKind::Skip
        );
    }

    #[test]
    fn desktop_action_id_rejects_dismiss_and_garbage() {
        assert!(parse_desktop_action_id("__closed").is_none());
        assert!(parse_desktop_action_id("inbox:meow:tok").is_none());
        assert!(parse_desktop_action_id("approval:start:tok").is_none());
    }
}
