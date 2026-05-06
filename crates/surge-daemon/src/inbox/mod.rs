//! Inbox-action subsystem.
//!
//! Receivers (Telegram bot loop, Desktop action listener) write requests to
//! `inbox_action_queue`; `InboxActionConsumer` polls and dispatches.
//! Outgoing inbox cards are rendered by `TgInboxBot::outgoing_loop` from
//! `inbox_delivery_queue`. `TicketStateSync` follows engine events for
//! inbox-initiated runs. `SnoozeScheduler` re-emits cards when their
//! snooze_until expires.

pub mod consumer;
pub mod desktop_listener;
pub mod snooze_scheduler;
pub mod state_sync;
pub mod tg_bot;

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
