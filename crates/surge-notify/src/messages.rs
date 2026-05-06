//! Message types for notification dispatching.
//!
//! Defines `NotifyMessage` enum and payload structs that channel
//! formatters (Telegram, Desktop, etc.) pattern-match and render.

use serde::{Deserialize, Serialize};
use surge_intake::types::{Priority, TaskId};

/// Payload for the `InboxCard` notification variant.
///
/// Surfaces a freshly-detected task from a `TaskSource` to the user via
/// notification channels (Telegram, Desktop). The user can then choose
/// to start the run, snooze, or skip via channel-specific UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InboxCardPayload {
    /// Identifier of the originating ticket (e.g. `"linear:wsp_acme/ABC-42"`).
    pub task_id: TaskId,
    /// Source instance id (e.g. `"linear:wsp_acme"`).
    pub source_id: String,
    /// Provider tag (`"linear"`, `"github_issues"`, ...).
    pub provider: String,
    /// Ticket title.
    pub title: String,
    /// Short summary (typically Triage Author's `inbox_summary.md`, 3-5 lines).
    pub summary: String,
    /// Priority assigned by Triage Author.
    pub priority: Priority,
    /// Tracker URL of the originating ticket (deep-link).
    pub task_url: String,
    /// Short ULID — embedded in `callback_data` of inline buttons. The
    /// daemon resolves this to a `task_id` via
    /// `IntakeRepo::fetch_by_callback_token`. Replaces the prior `run_id`
    /// field (the actual `RunId` is generated only when `Engine::start_run`
    /// runs; pre-creating one violated the FK on `ticket_index.run_id`).
    pub callback_token: String,
}

/// Notification message type.
///
/// Used by channel-specific formatters to render payloads for delivery
/// (Telegram, Desktop, Email, Slack, Webhook).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NotifyMessage {
    /// An inbox card — a freshly-detected task waiting for user action.
    InboxCard(InboxCardPayload),
}

#[cfg(test)]
mod inbox_card_tests {
    use super::*;

    #[test]
    fn round_trip_inbox_card() {
        let payload = InboxCardPayload {
            task_id: TaskId::try_new("github_issues:user/repo#1").unwrap(),
            source_id: "github_issues:user/repo".into(),
            provider: "github_issues".into(),
            title: "Fix parser".into(),
            summary: "panic on nested".into(),
            priority: Priority::High,
            task_url: "https://github.com/user/repo/issues/1".into(),
            callback_token: "01HKGZTOKABC".into(),
        };
        let msg = NotifyMessage::InboxCard(payload.clone());
        let s = serde_json::to_string(&msg).unwrap();
        let back: NotifyMessage = serde_json::from_str(&s).unwrap();
        match back {
            NotifyMessage::InboxCard(p) => {
                assert_eq!(p.task_id, payload.task_id);
                assert_eq!(p.callback_token, "01HKGZTOKABC");
                assert_eq!(p.priority, Priority::High);
            },
        }
    }
}
