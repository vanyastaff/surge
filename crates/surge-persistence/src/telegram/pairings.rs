//! Paired-chat allowlist — admission control for the Telegram cockpit.
//!
//! Every incoming callback and command is checked against this table before
//! a handler runs (see [ADR 0011](../../../docs/adr/0011-telegram-card-lifecycle.md)
//! and the Telegram cockpit milestone plan, Decision 6).

use rusqlite::{Connection, OptionalExtension, params};

/// One row of the `telegram_pairings` table.
#[derive(Debug, Clone)]
pub struct Pairing {
    /// Telegram chat id of the paired user.
    pub chat_id: i64,
    /// Operator-supplied label (carried over from the pairing-token mint).
    pub user_label: String,
    /// Unix epoch ms at which the chat was paired.
    pub paired_at: i64,
}

/// Errors raised by the pairings repository helpers.
#[derive(Debug, thiserror::Error)]
pub enum PairingsError {
    /// Underlying SQLite error.
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

/// Return `true` if `chat_id` is currently paired and not revoked.
///
/// # Errors
///
/// Returns [`PairingsError::Sqlite`] if the underlying query fails.
pub fn is_admitted(conn: &Connection, chat_id: i64) -> Result<bool, PairingsError> {
    let row: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM telegram_pairings \
             WHERE chat_id = ? AND revoked_at IS NULL",
            params![chat_id],
            |r| r.get(0),
        )
        .optional()?;
    Ok(row.is_some())
}

/// Insert or refresh a pairing row for `chat_id`.
///
/// On a fresh `chat_id` this inserts. On a re-pair of an existing (possibly
/// revoked) chat this overwrites `user_label`, `paired_at` and clears
/// `revoked_at`. This makes re-pairing after revocation transparent for the
/// caller — the `/pair <token>` handler does not need to branch on whether
/// the chat was previously known.
///
/// # Errors
///
/// Returns [`PairingsError::Sqlite`] if the upsert fails.
pub fn pair(
    conn: &Connection,
    chat_id: i64,
    user_label: &str,
    now_ms: i64,
) -> Result<(), PairingsError> {
    conn.execute(
        "INSERT INTO telegram_pairings (chat_id, user_label, paired_at, revoked_at) \
         VALUES (?, ?, ?, NULL) \
         ON CONFLICT(chat_id) DO UPDATE SET \
            user_label = excluded.user_label, \
            paired_at = excluded.paired_at, \
            revoked_at = NULL",
        params![chat_id, user_label, now_ms],
    )?;
    tracing::info!(
        target: "persistence::telegram",
        chat_id = %chat_id,
        label = %user_label,
        "paired chat",
    );
    Ok(())
}

/// Revoke the pairing for `chat_id`.
///
/// Soft-deletes by setting `revoked_at = now_ms`. Subsequent
/// [`is_admitted`] calls return `false`. A revoked chat can re-pair via
/// [`pair`] and the same row is reused.
///
/// # Errors
///
/// Returns [`PairingsError::Sqlite`] if the update fails.
pub fn revoke(conn: &Connection, chat_id: i64, now_ms: i64) -> Result<(), PairingsError> {
    conn.execute(
        "UPDATE telegram_pairings SET revoked_at = ? WHERE chat_id = ? AND revoked_at IS NULL",
        params![now_ms, chat_id],
    )?;
    tracing::info!(
        target: "persistence::telegram",
        chat_id = %chat_id,
        "revoked chat",
    );
    Ok(())
}

/// List all currently active (non-revoked) pairings, ordered by `paired_at`
/// ascending (oldest first).
///
/// # Errors
///
/// Returns [`PairingsError::Sqlite`] if the query fails.
pub fn list_active(conn: &Connection) -> Result<Vec<Pairing>, PairingsError> {
    let mut stmt = conn.prepare(
        "SELECT chat_id, user_label, paired_at FROM telegram_pairings \
         WHERE revoked_at IS NULL ORDER BY paired_at ASC",
    )?;
    let rows = stmt
        .query_map([], |row| {
            Ok(Pairing {
                chat_id: row.get(0)?,
                user_label: row.get(1)?,
                paired_at: row.get(2)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
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
    fn unpaired_chat_is_not_admitted() {
        let conn = fresh_db();
        assert!(!is_admitted(&conn, 42).unwrap());
    }

    #[test]
    fn paired_chat_is_admitted() {
        let conn = fresh_db();
        pair(&conn, 42, "phone", 1_000).unwrap();
        assert!(is_admitted(&conn, 42).unwrap());
    }

    #[test]
    fn revoked_chat_is_not_admitted() {
        let conn = fresh_db();
        pair(&conn, 42, "phone", 1_000).unwrap();
        revoke(&conn, 42, 2_000).unwrap();
        assert!(!is_admitted(&conn, 42).unwrap());
    }

    #[test]
    fn re_pair_replaces_label_and_clears_revoked_at() {
        let conn = fresh_db();
        pair(&conn, 42, "phone", 1_000).unwrap();
        revoke(&conn, 42, 2_000).unwrap();
        pair(&conn, 42, "tablet", 3_000).unwrap();

        assert!(is_admitted(&conn, 42).unwrap());

        let active = list_active(&conn).unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].user_label, "tablet");
        assert_eq!(active[0].paired_at, 3_000);
    }

    #[test]
    fn double_pair_same_chat_updates_label() {
        let conn = fresh_db();
        pair(&conn, 42, "phone", 1_000).unwrap();
        pair(&conn, 42, "phone-renamed", 2_000).unwrap();

        let active = list_active(&conn).unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].user_label, "phone-renamed");
        assert_eq!(active[0].paired_at, 2_000);
    }

    #[test]
    fn list_active_orders_by_paired_at_ascending() {
        let conn = fresh_db();
        pair(&conn, 100, "second", 2_000).unwrap();
        pair(&conn, 50, "first", 1_000).unwrap();
        pair(&conn, 200, "third", 3_000).unwrap();
        revoke(&conn, 50, 1_500).unwrap();

        let active = list_active(&conn).unwrap();
        let chat_ids: Vec<i64> = active.iter().map(|p| p.chat_id).collect();
        assert_eq!(chat_ids, vec![100, 200]);
    }
}
