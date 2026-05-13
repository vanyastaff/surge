//! Inbox-action subsystem.
//!
//! Receivers (Telegram bot loop, Desktop action listener) write requests to
//! `inbox_action_queue`; `InboxActionConsumer` polls and dispatches.
//! Outgoing inbox cards are rendered by `TgInboxBot::outgoing_loop` from
//! `inbox_delivery_queue`. `TicketStateSync` follows engine events for
//! inbox-initiated runs. `SnoozeScheduler` re-emits cards when their
//! `snooze_until` expires.

pub mod consumer;
pub mod desktop_listener;
pub mod snooze_scheduler;
pub mod state_sync;
pub mod tg_bot;
pub mod ticket_run_launcher;

/// Channel through which an inbox decision was received.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionChannel {
    /// User tapped a Telegram inline keyboard button.
    Telegram,
    /// User tapped a desktop notification action.
    Desktop,
}

impl ActionChannel {
    /// Stable on-disk string form.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Telegram => "telegram",
            Self::Desktop => "desktop",
        }
    }

    /// Inverse of `as_str`. Returns `None` on unknown strings.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "telegram" => Some(Self::Telegram),
            "desktop" => Some(Self::Desktop),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_channel_round_trip() {
        for kind in [ActionChannel::Telegram, ActionChannel::Desktop] {
            let s = kind.as_str();
            let back = ActionChannel::parse(s).unwrap();
            assert_eq!(kind, back);
        }
    }

    #[test]
    fn action_channel_parse_unknown() {
        assert!(ActionChannel::parse("slack").is_none());
        assert!(ActionChannel::parse("").is_none());
    }
}

use std::sync::Arc;
use surge_notify::messages::InboxCardPayload;
use surge_persistence::inbox_queue;
use surge_persistence::intake::{IntakeRepo, IntakeRow, TicketState};
use surge_persistence::runs::storage::Storage;

pub(crate) mod consumer_helpers {
    use surge_intake::types::Priority;

    pub fn parse_priority_str(s: &str) -> Option<Priority> {
        match s {
            "urgent" => Some(Priority::Urgent),
            "high" => Some(Priority::High),
            "medium" => Some(Priority::Medium),
            "low" => Some(Priority::Low),
            _ => None,
        }
    }
}

/// Enqueue an inbox card for delivery and persist the `callback_token` on
/// the existing `ticket_index` row. If the row doesn't exist yet, it's
/// inserted with state=`InboxNotified`. If it does exist, the row's state
/// must permit a transition to `InboxNotified` (`Triaged`, `InboxNotified`
/// self-transition, `Snoozed`); otherwise the enqueue is refused so users
/// don't see actionable cards on terminal-state tickets that
/// `InboxActionConsumer` would later reject.
pub async fn enqueue_inbox_card(
    storage: &Arc<Storage>,
    payload: &InboxCardPayload,
) -> Result<(), String> {
    let conn = storage.acquire_registry_conn().map_err(|e| e.to_string())?;
    let repo = IntakeRepo::new(&conn);

    let existing = repo
        .fetch(payload.task_id.as_str())
        .map_err(|e| e.to_string())?;
    let now = chrono::Utc::now();
    if existing.is_none() {
        let row = IntakeRow {
            task_id: payload.task_id.as_str().into(),
            source_id: payload.source_id.clone(),
            provider: payload.provider.clone(),
            run_id: None,
            triage_decision: None,
            duplicate_of: None,
            priority: Some(payload.priority.label().into()),
            state: TicketState::InboxNotified,
            first_seen: now,
            last_seen: now,
            snooze_until: None,
            callback_token: Some(payload.callback_token.clone()),
            tg_chat_id: None,
            tg_message_id: None,
        };
        repo.insert(&row).map_err(|e| e.to_string())?;
    } else {
        // FSM-enforced state guard: only enqueue if the existing row can
        // transition to InboxNotified. Terminal/in-flight states (Active,
        // RunStarted, Completed, Failed, Aborted, Skipped, Stale) reject —
        // the user should not see actionable buttons that the consumer
        // will silently ignore. Self-transition (already InboxNotified)
        // is valid; Snoozed→InboxNotified is valid for re-emission;
        // Triaged→InboxNotified is the natural Triage→inbox path.
        match repo.update_state_validated(
            payload.task_id.as_str(),
            surge_persistence::intake::TicketState::InboxNotified,
        ) {
            Ok(()) => {},
            Err(surge_persistence::intake::IntakeError::InvalidTransition { from, .. }) => {
                return Err(format!(
                    "ticket {} is in state {from:?}; not eligible for an actionable inbox card",
                    payload.task_id.as_str()
                ));
            },
            Err(e) => return Err(e.to_string()),
        }
        repo.set_callback_token(payload.task_id.as_str(), &payload.callback_token)
            .map_err(|e| e.to_string())?;
    }

    let payload_json = serde_json::to_string(payload).map_err(|e| e.to_string())?;
    inbox_queue::append_delivery(
        &conn,
        payload.task_id.as_str(),
        &payload.callback_token,
        &payload_json,
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}
