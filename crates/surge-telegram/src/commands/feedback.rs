//! Unbound run-id feedback cannot identify an approval request safely.
use crate::commands::CommandReply;

/// Direct operators to the card-bound reply workflow.
#[must_use]
pub fn handle_feedback() -> CommandReply {
    CommandReply::new(
        "Use Edit on the current approval card, then reply to the bot prompt. A run ID alone cannot identify an approval request safely.",
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn unbound_feedback_directs_to_card_reply() {
        assert!(super::handle_feedback().text.contains("Edit"));
    }
}
