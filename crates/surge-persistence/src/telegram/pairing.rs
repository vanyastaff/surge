//! Pairing-token mint and consume operations.
//!
//! See migration `0007_telegram_pairing_tokens.sql` for the table contract.
//! Tokens are short (6-char Crockford base32), one-shot, TTL-bounded; the
//! cockpit mints one for each `surge telegram setup` invocation and consumes
//! it when an unpaired chat sends `/pair <token>`.

use std::time::Duration;

use rand::Rng;
use rusqlite::{Connection, OptionalExtension, params};

/// Crockford base32 alphabet (omits I, L, O, U).
const CROCKFORD_ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// Length of every minted token, in characters.
pub const TOKEN_LEN: usize = 6;

/// Maximum attempts the minter will retry on a collision before giving up.
///
/// With `TOKEN_LEN = 6` Crockford characters the keyspace is 32^6 ≈ 10^9, so
/// collisions are essentially impossible at the scale Surge operates at. A
/// small retry budget exists only to convert any unexpected duplicate into a
/// retry rather than a panic.
const MAX_MINT_ATTEMPTS: u32 = 3;

/// Errors raised by the pairing-token helpers.
#[derive(Debug, thiserror::Error)]
pub enum PairingError {
    /// The token is already consumed and cannot be re-used.
    #[error("pairing token already consumed")]
    AlreadyConsumed,
    /// The token's `expires_at` is before `now_ms`.
    #[error("pairing token has expired")]
    Expired,
    /// No row matches the given token.
    #[error("pairing token not found")]
    NotFound,
    /// `INSERT OR IGNORE` collided for [`MAX_MINT_ATTEMPTS`] in a row.
    #[error("could not mint unique pairing token after {0} attempts")]
    MintCollisionExhausted(u32),
    /// Underlying SQLite error.
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

/// Generate a fresh random token string of [`TOKEN_LEN`] Crockford characters.
///
/// Uses the thread-local Rng; the caller does not seed.
fn random_token() -> String {
    let mut rng = rand::rng();
    (0..TOKEN_LEN)
        .map(|_| {
            let idx = rng.random_range(0..CROCKFORD_ALPHABET.len());
            CROCKFORD_ALPHABET[idx] as char
        })
        .collect()
}

/// Mint a new pairing token with the given TTL and operator-supplied label.
///
/// Returns the freshly-minted token string on success. Inserts directly into
/// `telegram_pairing_tokens`; on collision (`INSERT OR IGNORE` returning 0
/// rows) retries up to [`MAX_MINT_ATTEMPTS`] before failing.
///
/// `now_ms` is supplied by the caller (instead of `Clock::now_ms()`) so unit
/// tests can fake time and so the surrounding transaction context controls
/// timestamp consistency.
///
/// # Errors
///
/// Returns [`PairingError::MintCollisionExhausted`] if no unique token can be
/// committed within the retry budget. Surfaces [`PairingError::Sqlite`] for
/// underlying storage failures.
pub fn mint_pairing_token(
    conn: &Connection,
    label: &str,
    ttl: Duration,
    now_ms: i64,
) -> Result<String, PairingError> {
    let expires_at = now_ms.saturating_add(i64::try_from(ttl.as_millis()).unwrap_or(i64::MAX));
    for _ in 0..MAX_MINT_ATTEMPTS {
        let token = random_token();
        let rows = conn.execute(
            "INSERT OR IGNORE INTO telegram_pairing_tokens \
             (token, created_at, expires_at, label) \
             VALUES (?, ?, ?, ?)",
            params![&token, now_ms, expires_at, label],
        )?;
        if rows == 1 {
            tracing::debug!(
                target: "persistence::telegram",
                label = %label,
                ttl_ms = %i64::try_from(ttl.as_millis()).unwrap_or(i64::MAX),
                "minted pairing token"
            );
            return Ok(token);
        }
    }
    Err(PairingError::MintCollisionExhausted(MAX_MINT_ATTEMPTS))
}

/// Consume a previously-minted token and return the label that was attached
/// at mint time.
///
/// `now_ms` is compared against `expires_at`; expired tokens are rejected
/// **without** being marked consumed so a maintenance pass can later garbage
/// collect them. Already-consumed tokens are rejected with
/// [`PairingError::AlreadyConsumed`].
///
/// On success the row's `consumed_at` is updated to `now_ms`. The label is
/// returned to the caller (typically the bot's `/pair` handler) so it can be
/// recorded on the resulting allowlist row.
///
/// # Errors
///
/// Returns one of [`PairingError::NotFound`], [`PairingError::Expired`],
/// [`PairingError::AlreadyConsumed`], or [`PairingError::Sqlite`].
pub fn consume_pairing_token(
    conn: &Connection,
    token: &str,
    now_ms: i64,
) -> Result<String, PairingError> {
    let row: Option<(i64, Option<i64>, Option<String>)> = conn
        .query_row(
            "SELECT expires_at, consumed_at, label \
             FROM telegram_pairing_tokens \
             WHERE token = ?",
            params![token],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    let Some((expires_at, consumed_at, label)) = row else {
        return Err(PairingError::NotFound);
    };
    if consumed_at.is_some() {
        return Err(PairingError::AlreadyConsumed);
    }
    if now_ms > expires_at {
        return Err(PairingError::Expired);
    }
    conn.execute(
        "UPDATE telegram_pairing_tokens SET consumed_at = ? WHERE token = ?",
        params![now_ms, token],
    )?;
    tracing::debug!(
        target: "persistence::telegram",
        token = %token,
        "consumed pairing token"
    );
    Ok(label.unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runs::clock::MockClock;
    use crate::runs::migrations::{REGISTRY_MIGRATIONS, apply};

    fn fresh_db() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        let clock = MockClock::new(1_700_000_000_000);
        apply(&mut conn, REGISTRY_MIGRATIONS, &clock).unwrap();
        conn
    }

    #[test]
    fn mint_creates_unique_token() {
        let conn = fresh_db();
        let token = mint_pairing_token(&conn, "phone", Duration::from_secs(600), 1_000).unwrap();
        assert_eq!(token.len(), TOKEN_LEN);
        assert!(
            token.chars().all(|c| c.is_ascii_alphanumeric()),
            "token {token} is not Crockford-printable"
        );
    }

    #[test]
    fn consume_returns_label_and_marks_consumed() {
        let conn = fresh_db();
        let token = mint_pairing_token(&conn, "phone", Duration::from_secs(600), 1_000).unwrap();

        let label = consume_pairing_token(&conn, &token, 1_500).unwrap();
        assert_eq!(label, "phone");

        // Second consume must fail.
        let err = consume_pairing_token(&conn, &token, 2_000).unwrap_err();
        assert!(matches!(err, PairingError::AlreadyConsumed));
    }

    #[test]
    fn consume_rejects_unknown_token() {
        let conn = fresh_db();
        let err = consume_pairing_token(&conn, "ZZZZZZ", 1_000).unwrap_err();
        assert!(matches!(err, PairingError::NotFound));
    }

    #[test]
    fn consume_rejects_expired_token() {
        let conn = fresh_db();
        let token = mint_pairing_token(&conn, "phone", Duration::from_secs(60), 1_000).unwrap();
        let err = consume_pairing_token(&conn, &token, 1_000 + 60_001).unwrap_err();
        assert!(matches!(err, PairingError::Expired));

        // Expired token must NOT be marked consumed — a later GC pass can
        // remove it. We re-check the row directly.
        let consumed_at: Option<i64> = conn
            .query_row(
                "SELECT consumed_at FROM telegram_pairing_tokens WHERE token = ?",
                params![&token],
                |row| row.get(0),
            )
            .unwrap();
        assert!(consumed_at.is_none(), "expired-rejection must not consume");
    }

    #[test]
    fn double_mint_with_same_label_produces_distinct_tokens() {
        let conn = fresh_db();
        let t1 = mint_pairing_token(&conn, "phone", Duration::from_secs(60), 1_000).unwrap();
        let t2 = mint_pairing_token(&conn, "phone", Duration::from_secs(60), 1_001).unwrap();
        assert_ne!(t1, t2, "consecutive mints must be unique");
    }
}
