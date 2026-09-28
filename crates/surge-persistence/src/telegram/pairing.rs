//! Pairing-token mint and consume operations.
//!
//! See migration `0007_telegram_pairing_tokens.sql` for the table contract.
//! Tokens are short (6-char Crockford base32), one-shot, TTL-bounded; the
//! cockpit mints one for each `surge telegram setup` invocation and consumes
//! it when an unpaired chat sends `/pair <token>`.

use std::time::Duration;

use rand::Rng;
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

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
    /// Legacy codes have no target and cannot authorize a chat.
    #[error(
        "legacy unbound pairing code; run surge telegram setup --token-env NAME --chat-id ID again"
    )]
    LegacyUnbound,
    /// Only the configured target may consume this code.
    #[error("pairing code is for a different chat")]
    WrongChat,
    /// Zero is not a Telegram chat target.
    #[error("pairing requires a nonzero target chat id")]
    InvalidTarget,
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
    target_chat_id: i64,
    ttl: Duration,
    now_ms: i64,
) -> Result<String, PairingError> {
    if target_chat_id == 0 {
        return Err(PairingError::InvalidTarget);
    }
    let expires_at = now_ms.saturating_add(i64::try_from(ttl.as_millis()).unwrap_or(i64::MAX));
    for _ in 0..MAX_MINT_ATTEMPTS {
        let token = random_token();
        let rows = conn.execute(
            "INSERT OR IGNORE INTO telegram_pairing_tokens \
             (token, created_at, expires_at, label, target_chat_id) \
             VALUES (?, ?, ?, ?, ?)",
            params![&token, now_ms, expires_at, label, target_chat_id],
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

struct PairingCode {
    expires_at: i64,
    consumed_at: Option<i64>,
    label: Option<String>,
    target: Option<i64>,
}

/// Consume a target-bound code and admit that chat in one IMMEDIATE transaction.
/// Failed admission writes roll back consumption. Legacy unbound codes fail closed.
///
/// # Errors
/// Rejects unknown, expired, consumed, unbound or wrong-chat codes and propagates SQLite failures.
pub fn pair_with_token(
    conn: &Connection,
    token: &str,
    chat_id: i64,
    now_ms: i64,
) -> Result<String, PairingError> {
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    let row: Option<PairingCode> = tx.query_row(
        "SELECT expires_at, consumed_at, label, target_chat_id FROM telegram_pairing_tokens WHERE token = ?",
        params![token], |row| Ok(PairingCode { expires_at: row.get(0)?, consumed_at: row.get(1)?, label: row.get(2)?, target: row.get(3)? }),
    ).optional()?;
    let Some(PairingCode {
        expires_at,
        consumed_at,
        label,
        target,
    }) = row
    else {
        return Err(PairingError::NotFound);
    };
    let target = target.ok_or(PairingError::LegacyUnbound)?;
    if target != chat_id || chat_id == 0 {
        return Err(PairingError::WrongChat);
    }
    if consumed_at.is_some() {
        return Err(PairingError::AlreadyConsumed);
    }
    if now_ms >= expires_at {
        return Err(PairingError::Expired);
    }
    let changed = tx.execute("UPDATE telegram_pairing_tokens SET consumed_at = ? WHERE token = ? AND consumed_at IS NULL AND target_chat_id = ? AND expires_at > ?", params![now_ms, token, chat_id, now_ms])?;
    if changed != 1 {
        return Err(PairingError::AlreadyConsumed);
    }
    let label = label.unwrap_or_default();
    super::pairings::write_pairing(&tx, chat_id, &label, now_ms)?;
    tx.commit()?;
    tracing::info!(target: "persistence::telegram", chat_id, "paired chat with one-shot code");
    Ok(label)
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
    fn concurrent_pair_code_consumption_has_exactly_one_winner() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static WAITING: AtomicUsize = AtomicUsize::new(0);
        fn busy(attempt: i32) -> bool {
            if attempt == 0 {
                WAITING.fetch_add(1, Ordering::SeqCst);
            }
            std::thread::sleep(Duration::from_millis(1));
            attempt < 5000
        }
        WAITING.store(0, Ordering::SeqCst);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pairing.sqlite");
        let mut holder = Connection::open(&path).unwrap();
        apply(&mut holder, REGISTRY_MIGRATIONS, &MockClock::new(1000)).unwrap();
        holder.execute_batch("PRAGMA journal_mode=WAL;").unwrap();
        let code =
            mint_pairing_token(&holder, "operator", 42, Duration::from_secs(60), 1000).unwrap();
        holder.execute_batch("BEGIN IMMEDIATE").unwrap();
        let workers: Vec<_> = (0..8)
            .map(|_| {
                let path = path.clone();
                let code = code.clone();
                std::thread::spawn(move || {
                    let conn = Connection::open(path).unwrap();
                    conn.busy_handler(Some(busy)).unwrap();
                    pair_with_token(&conn, &code, 42, 1001)?;
                    Ok::<_, PairingError>(())
                })
            })
            .collect();
        let until = std::time::Instant::now() + Duration::from_secs(5);
        while WAITING.load(Ordering::SeqCst) < 8 && std::time::Instant::now() < until {
            std::thread::sleep(Duration::from_millis(1));
        }
        let all_waiting = WAITING.load(Ordering::SeqCst) >= 8;
        holder.execute_batch("COMMIT").unwrap();
        let winners = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .filter(std::result::Result::is_ok)
            .count();
        // Joining every contender also proves no blocked writer remains.
        assert_eq!(winners, 1, "one code was accepted by multiple callers");
        assert!(
            all_waiting,
            "contenders did not reach SQLite write contention"
        );
        assert_eq!(
            super::super::pairings::count_active(&holder).unwrap(),
            1,
            "one code bound multiple chats"
        );
    }

    #[test]
    fn mint_creates_unique_token() {
        let conn = fresh_db();
        let token =
            mint_pairing_token(&conn, "phone", 42, Duration::from_secs(600), 1_000).unwrap();
        assert_eq!(token.len(), TOKEN_LEN);
        assert!(
            token.chars().all(|c| c.is_ascii_alphanumeric()),
            "token {token} is not Crockford-printable"
        );
    }

    #[test]
    fn consume_returns_label_and_marks_consumed() {
        let conn = fresh_db();
        let token =
            mint_pairing_token(&conn, "phone", 42, Duration::from_secs(600), 1_000).unwrap();

        let label = pair_with_token(&conn, &token, 42, 1_500).unwrap();
        assert_eq!(label, "phone");

        // Second consume must fail.
        let err = pair_with_token(&conn, &token, 42, 2_000).unwrap_err();
        assert!(matches!(err, PairingError::AlreadyConsumed));
    }

    #[test]
    fn consume_rejects_unknown_token() {
        let conn = fresh_db();
        let err = pair_with_token(&conn, "ZZZZZZ", 42, 1_000).unwrap_err();
        assert!(matches!(err, PairingError::NotFound));
    }

    #[test]
    fn consume_rejects_expired_token() {
        let conn = fresh_db();
        let token = mint_pairing_token(&conn, "phone", 42, Duration::from_secs(60), 1_000).unwrap();
        let err = pair_with_token(&conn, &token, 42, 1_000 + 60_001).unwrap_err();
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
        let t1 = mint_pairing_token(&conn, "phone", 42, Duration::from_secs(60), 1_000).unwrap();
        let t2 = mint_pairing_token(&conn, "phone", 42, Duration::from_secs(60), 1_001).unwrap();
        assert_ne!(t1, t2, "consecutive mints must be unique");
    }
    #[test]
    fn wrong_chat_does_not_consume_code_and_expiry_is_inclusive() {
        let conn = fresh_db();
        let code = mint_pairing_token(&conn, "phone", 42, Duration::from_secs(1), 1000).unwrap();
        assert!(matches!(
            pair_with_token(&conn, &code, 43, 1001),
            Err(PairingError::WrongChat)
        ));
        assert_eq!(pair_with_token(&conn, &code, 42, 1001).unwrap(), "phone");
        assert!(!super::super::pairings::is_admitted(&conn, 43).unwrap());
        let code = mint_pairing_token(&conn, "phone", 42, Duration::from_secs(1), 1000).unwrap();
        assert!(matches!(
            pair_with_token(&conn, &code, 42, 2000),
            Err(PairingError::Expired)
        ));
    }

    #[test]
    fn legacy_unbound_code_cannot_admit_a_chat() {
        let conn = fresh_db();
        conn.execute("INSERT INTO telegram_pairing_tokens(token, created_at, expires_at) VALUES ('LEGACY', 0, 9000)", []).unwrap();
        assert!(matches!(
            pair_with_token(&conn, "LEGACY", 42, 1),
            Err(PairingError::LegacyUnbound)
        ));
        assert_eq!(super::super::pairings::count_active(&conn).unwrap(), 0);
    }

    #[test]
    fn failed_allowlist_insert_rolls_back_consumption() {
        let conn = fresh_db();
        let code = mint_pairing_token(&conn, "phone", 42, Duration::from_secs(60), 1000).unwrap();
        conn.execute_batch("CREATE TRIGGER fail_pair BEFORE INSERT ON telegram_pairings BEGIN SELECT RAISE(ABORT, 'injected'); END;").unwrap();
        assert!(matches!(
            pair_with_token(&conn, &code, 42, 1001),
            Err(PairingError::Sqlite(_))
        ));
        assert_eq!(super::super::pairings::count_active(&conn).unwrap(), 0);
        conn.execute_batch("DROP TRIGGER fail_pair").unwrap();
        assert!(pair_with_token(&conn, &code, 42, 1002).is_ok());
    }
}
