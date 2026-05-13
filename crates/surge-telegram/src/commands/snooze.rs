//! `/snooze <duration>` — defer a cockpit card to a later wake-up time.
//!
//! Routed by [`crate::cockpit::run::UpdateRoutes::handle_reply`] when
//! the operator replies to one of our cards with `/snooze 1h` (or
//! similar). The handler:
//!
//! 1. Parses the duration string (`30m`, `2h`, `1d`, `90s`, …).
//! 2. Asks [`CockpitSnoozeWriter`] to record the request keyed by
//!    `(card_id, wake_at_ms)`. Production wires this to the
//!    `inbox_action_queue` table with `subject_kind = "cockpit_card"`
//!    (snooze schema extension landed in registry migration 0010).
//! 3. Returns a reply confirming the wake-up time.
//!
//! Re-emission once the wake-up time elapses is the snooze
//! consumer's job (T19).

use async_trait::async_trait;

use crate::commands::CommandReply;
use crate::error::Result;

/// Persistence surface for recording a cockpit-card snooze request.
///
/// Production wraps a `surge_persistence::inbox_queue::append_action`
/// call with `kind = Snooze`, `subject_kind = "cockpit_card"`,
/// `subject_ref = card_id`, and `snooze_until = wake_at_ms`.
#[async_trait]
pub trait CockpitSnoozeWriter: Send + Sync {
    /// Record a snooze for `card_id` to wake at `wake_at_ms`.
    async fn snooze(&self, card_id: &str, wake_at_ms: i64) -> Result<()>;
}

/// Handle `/snooze <duration>` issued as a reply to a cockpit card.
///
/// `card_id` is the ULID resolved from `reply_to_message.message_id`
/// by the upstream routing (see [`crate::cockpit::run::UpdateRoutes`]).
/// `now_ms` is supplied by the caller for deterministic tests.
pub async fn handle_snooze<W: CockpitSnoozeWriter>(
    chat_id: i64,
    args: &str,
    card_id: &str,
    writer: &W,
    now_ms: i64,
) -> Result<CommandReply> {
    let trimmed = args.trim();
    if trimmed.is_empty() {
        return Ok(CommandReply::new(
            "Usage: `/snooze <duration>` — e.g. `/snooze 30m`, `/snooze 2h`, `/snooze 1d`.",
        ));
    }
    let secs = match parse_duration_secs(trimmed) {
        Ok(s) => s,
        Err(reason) => {
            return Ok(CommandReply::new(format!(
                "❌ Could not parse duration `{trimmed}`: {reason}. Try `/snooze 30m`."
            )));
        },
    };
    let wake_at_ms = now_ms.saturating_add(secs.saturating_mul(1000));

    writer.snooze(card_id, wake_at_ms).await?;

    tracing::info!(
        target: "telegram::cmd::snooze",
        %chat_id,
        %card_id,
        wake_at_ms,
        duration_s = secs,
        "snooze recorded",
    );

    Ok(CommandReply::new(format!(
        "🛏 Snoozed for {trimmed}. I'll bring this card back when the timer elapses."
    )))
}

/// Parse a duration string like `30m` / `2h` / `1d` / `90s` into
/// seconds.
///
/// Supported suffixes: `s` (seconds), `m` (minutes), `h` (hours), `d`
/// (days). No-suffix is rejected — operators must pass a unit.
///
/// # Errors
///
/// Returns a human-readable reason string suitable for direct chat
/// reply.
fn parse_duration_secs(input: &str) -> std::result::Result<i64, String> {
    if input.is_empty() {
        return Err("empty string".into());
    }
    let (digits, suffix) = match input.chars().last() {
        Some(c) if c.is_ascii_alphabetic() => {
            let split_at = input.len() - c.len_utf8();
            (&input[..split_at], c)
        },
        Some(_) => return Err("missing unit suffix (use s / m / h / d)".into()),
        None => return Err("empty string".into()),
    };
    if digits.is_empty() {
        return Err("missing numeric part".into());
    }
    let count: i64 = digits
        .parse()
        .map_err(|_| format!("`{digits}` is not an integer"))?;
    if count < 0 {
        return Err("negative duration".into());
    }
    let multiplier = match suffix.to_ascii_lowercase() {
        's' => 1,
        'm' => 60,
        'h' => 60 * 60,
        'd' => 24 * 60 * 60,
        other => return Err(format!("unknown unit `{other}` (use s / m / h / d)")),
    };
    Ok(count.saturating_mul(multiplier))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeWriter {
        calls: Mutex<Vec<(String, i64)>>,
    }

    #[async_trait]
    impl CockpitSnoozeWriter for FakeWriter {
        async fn snooze(&self, card_id: &str, wake_at_ms: i64) -> Result<()> {
            self.calls
                .lock()
                .unwrap()
                .push((card_id.to_owned(), wake_at_ms));
            Ok(())
        }
    }

    #[tokio::test]
    async fn empty_duration_returns_usage_and_does_not_write() {
        let writer = FakeWriter::default();
        let reply = handle_snooze(7, "", "CARD-1", &writer, 1_000)
            .await
            .unwrap();
        assert!(reply.text.contains("Usage"));
        assert!(writer.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn happy_path_records_snooze_with_correct_wake_at() {
        let writer = FakeWriter::default();
        let reply = handle_snooze(7, "30m", "CARD-1", &writer, 1_000)
            .await
            .unwrap();
        assert!(reply.text.contains("Snoozed"));
        let calls = writer.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "CARD-1");
        assert_eq!(calls[0].1, 1_000 + 30 * 60 * 1_000);
    }

    #[tokio::test]
    async fn invalid_duration_returns_recoverable_reply() {
        let writer = FakeWriter::default();
        let reply = handle_snooze(7, "tomorrow", "CARD-1", &writer, 0)
            .await
            .unwrap();
        assert!(reply.text.starts_with("❌"));
        assert!(writer.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn parse_duration_secs_supports_every_unit() {
        assert_eq!(parse_duration_secs("90s"), Ok(90));
        assert_eq!(parse_duration_secs("30m"), Ok(30 * 60));
        assert_eq!(parse_duration_secs("2h"), Ok(2 * 60 * 60));
        assert_eq!(parse_duration_secs("1d"), Ok(24 * 60 * 60));
    }

    #[test]
    fn parse_duration_secs_rejects_invalid_inputs() {
        assert!(parse_duration_secs("").is_err());
        assert!(parse_duration_secs("30").is_err()); // missing unit
        assert!(parse_duration_secs("m").is_err()); // missing digits
        assert!(parse_duration_secs("-5m").is_err());
        assert!(parse_duration_secs("5z").is_err());
        assert!(parse_duration_secs("abc").is_err());
    }
}
