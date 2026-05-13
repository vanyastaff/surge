//! Error types for the Telegram cockpit.
//!
//! All cockpit operations return [`Result<T>`]. The outer event loop logs and
//! continues — errors never abort the bot. Errors are surfaced to the operator
//! only for explicit user actions (a failed `/pair` reports its reason);
//! transient transport errors stay in the structured log and the user sees a
//! generic "try again" via `answerCallbackQuery`.

use std::result;

use surge_orchestrator::engine::EngineError;
use teloxide::types::ChatId;
use thiserror::Error;

/// Result alias for cockpit operations.
pub type Result<T, E = TelegramCockpitError> = result::Result<T, E>;

/// Errors produced by the Telegram cockpit.
///
/// The variants split along the seam the cockpit's outer loop cares about:
/// admission failures, lifecycle-token problems, card lookups, rate limiting,
/// transport failures, and the two adjacent subsystems (`surge-orchestrator`
/// for engine resolution, `surge-persistence` for the cards / pairings store).
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum TelegramCockpitError {
    /// A chat that is not on the pairing allowlist tried to interact with the
    /// cockpit.
    #[error("chat {0} is not paired with this cockpit")]
    Auth(ChatId),

    /// The `/pair` command was invoked with a token that no row claims.
    #[error("pairing token is invalid")]
    PairingTokenInvalid,

    /// The `/pair` token exists but is past its TTL or already consumed.
    #[error("pairing token has expired or has already been consumed")]
    PairingTokenExpired,

    /// A callback referenced a card the cards table no longer has.
    #[error("card not found")]
    CardNotFound,

    /// A callback referenced a card whose `closed_at` is set.
    #[error("card is closed and no longer accepts actions")]
    CardClosed,

    /// The cockpit's rate-limit budget for the target chat (or global ceiling)
    /// has been hit and the operation was deferred.
    #[error("rate-limited")]
    RateLimited,

    /// A transport-layer failure talking to the Telegram Bot API.
    #[error("telegram transport failure: {0}")]
    Transport(String),

    /// The engine refused to resolve a human-input event the cockpit was
    /// dispatching on the operator's behalf.
    #[error("engine resolve failed: {0}")]
    EngineResolve(#[from] EngineError),

    /// A persistence-layer failure (cards table, pairings table, snooze table).
    #[error("persistence failure: {0}")]
    Persistence(String),
}

impl From<teloxide::RequestError> for TelegramCockpitError {
    fn from(value: teloxide::RequestError) -> Self {
        Self::Transport(value.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_variant_renders_chat_id() {
        let err = TelegramCockpitError::Auth(ChatId(42));
        assert_eq!(err.to_string(), "chat 42 is not paired with this cockpit");
    }

    #[test]
    fn pairing_token_variants_have_distinct_messages() {
        let invalid = TelegramCockpitError::PairingTokenInvalid.to_string();
        let expired = TelegramCockpitError::PairingTokenExpired.to_string();
        assert_ne!(invalid, expired);
        assert!(invalid.contains("invalid"));
        assert!(expired.contains("expired"));
    }

    #[test]
    fn card_lifecycle_variants_have_distinct_messages() {
        let not_found = TelegramCockpitError::CardNotFound.to_string();
        let closed = TelegramCockpitError::CardClosed.to_string();
        assert_ne!(not_found, closed);
    }

    #[test]
    fn rate_limited_renders_known_short_message() {
        let err = TelegramCockpitError::RateLimited;
        assert_eq!(err.to_string(), "rate-limited");
    }

    #[test]
    fn transport_variant_carries_underlying_message() {
        let err = TelegramCockpitError::Transport("network down".into());
        assert!(err.to_string().contains("network down"));
    }

    #[test]
    fn persistence_variant_carries_underlying_message() {
        let err = TelegramCockpitError::Persistence("table missing".into());
        assert!(err.to_string().contains("table missing"));
    }
}
