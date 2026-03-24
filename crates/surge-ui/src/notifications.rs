use gpui::SharedString;
use gpui_component::notification::Notification;

/// Surge notification builders — convenience wrappers around gpui-component Notification.
///
/// Usage: call `window.push_notification(SurgeNotification::task_completed("my-task"), cx);`
/// from any context with access to `&mut Window` and `&mut App`.
pub struct SurgeNotification;

impl SurgeNotification {
    pub fn task_completed(task_name: &str) -> Notification {
        Notification::success(SharedString::from(format!(
            "{task_name} finished successfully"
        )))
        .title("Task Completed")
    }

    pub fn task_failed(task_name: &str, reason: &str) -> Notification {
        Notification::error(SharedString::from(format!("{task_name}: {reason}")))
            .title("Task Failed")
            .autohide(false)
    }

    pub fn agent_connected(agent: &str) -> Notification {
        Notification::info(SharedString::from(format!("{agent} is ready"))).title("Agent Connected")
    }

    pub fn agent_disconnected(agent: &str) -> Notification {
        Notification::warning(SharedString::from(format!("{agent} connection lost")))
            .title("Agent Disconnected")
            .autohide(false)
    }

    pub fn review_needed(task_name: &str) -> Notification {
        Notification::warning(SharedString::from(format!("{task_name} needs your review")))
            .title("Review Required")
            .autohide(false)
    }

    pub fn rate_limit_warning(agent: &str, reset_secs: u64) -> Notification {
        Notification::warning(SharedString::from(format!(
            "{agent} rate limited — resets in {reset_secs}s"
        )))
        .title("Rate Limit")
        .autohide(false)
    }
}
