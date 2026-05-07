//! Desktop notification via `notify-rust`.

use crate::deliverer::{NotifyDeliverer, NotifyDeliveryContext, NotifyError, RenderedNotification};
use crate::messages::InboxCardPayload;
use async_trait::async_trait;
use surge_core::notify_config::NotifyChannel;

/// Rendered desktop inbox card ready for delivery.
///
/// Desktop notification systems (`notify-rust`, dbus, `NSUserNotification`)
/// don't support inline keyboards. We expose three actions as named labels
/// the channel can map to its native UI (Linux Action button, macOS reply
/// dropdown, Windows toast button).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboxCardDesktopRendered {
    /// Notification title (short, single-line).
    pub title: String,
    /// Notification body (1-3 lines).
    pub body: String,
    /// Action labels in display order. Each is a tuple of `(action_id, label)`.
    /// `action_id` is the short string the channel emits when the user clicks
    /// the action — surge-daemon maps it to a decision (start / snooze / skip).
    pub actions: Vec<(String, String)>,
}

/// Format an [`InboxCardPayload`] as a Desktop notification.
///
/// Returns title + body + 3 actions (start, snooze, skip). Wiring into the
/// `notify-rust` (or platform-specific) send path is a follow-up.
#[must_use]
pub fn format_inbox_card_desktop(payload: &InboxCardPayload) -> InboxCardDesktopRendered {
    let title = "📋 New Surge task".to_string();
    let body = format!(
        "{title}\n\
         priority: {prio} ({provider})",
        title = payload.title,
        prio = payload.priority.label(),
        provider = payload.provider,
    );
    let token = payload.callback_token.clone();
    let actions = vec![
        (format!("inbox:start:{token}"), "Start".to_string()),
        (format!("inbox:snooze:{token}"), "Snooze 24h".to_string()),
        (format!("inbox:skip:{token}"), "Skip".to_string()),
    ];
    InboxCardDesktopRendered {
        title,
        body,
        actions,
    }
}

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

#[cfg(test)]
mod desktop_inbox_format_tests {
    use super::*;
    use surge_intake::types::{Priority, TaskId};

    fn sample_payload() -> InboxCardPayload {
        InboxCardPayload {
            task_id: TaskId::try_new("linear:wsp/A-1").unwrap(),
            source_id: "linear:wsp".into(),
            provider: "linear".into(),
            title: "Add tracing to auth".into(),
            summary: "ad-hoc".into(),
            priority: Priority::Medium,
            task_url: "https://linear.app/wsp/issue/A-1".into(),
            callback_token: "tok_x".into(),
        }
    }

    #[test]
    fn title_is_static() {
        let r = format_inbox_card_desktop(&sample_payload());
        assert_eq!(r.title, "📋 New Surge task");
    }

    #[test]
    fn body_contains_title_priority_provider() {
        let r = format_inbox_card_desktop(&sample_payload());
        assert!(r.body.contains("Add tracing to auth"));
        assert!(r.body.contains("priority: medium"));
        assert!(r.body.contains("linear"));
    }

    #[test]
    fn three_actions_in_correct_order() {
        let r = format_inbox_card_desktop(&sample_payload());
        assert_eq!(r.actions.len(), 3);
        assert_eq!(r.actions[0].0, "inbox:start:tok_x");
        assert_eq!(r.actions[0].1, "Start");
        assert_eq!(r.actions[1].0, "inbox:snooze:tok_x");
        assert_eq!(r.actions[1].1, "Snooze 24h");
        assert_eq!(r.actions[2].0, "inbox:skip:tok_x");
        assert_eq!(r.actions[2].1, "Skip");
    }

    #[test]
    fn priority_label_renders() {
        let mut p = sample_payload();
        p.priority = Priority::Urgent;
        let r = format_inbox_card_desktop(&p);
        assert!(r.body.contains("priority: urgent"));
    }
}
