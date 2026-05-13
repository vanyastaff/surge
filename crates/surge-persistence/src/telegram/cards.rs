//! Telegram cockpit card store — see migration `0009_telegram_cards.sql`
//! and [ADR 0011](../../../docs/adr/0011-telegram-card-lifecycle.md).
//!
//! The store owns the cockpit's card lifecycle: idempotent creation keyed by
//! `(run_id, node_key, attempt_index)`, post-send `message_id` capture,
//! content-hash short-circuit for the `editMessageText`-only update path,
//! and soft-close for the stale-tap responder.

use rusqlite::{Connection, OptionalExtension, params};

/// One row of the `telegram_cards` table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Card {
    /// ULID primary key.
    pub card_id: String,
    /// The run this card mirrors a gate or status event for.
    pub run_id: String,
    /// The originating node in the run's graph.
    pub node_key: String,
    /// `RunMemory.node_visits[node_key]` at the time the card was created.
    pub attempt_index: i64,
    /// Card-kind discriminator (e.g. `"human_gate"`, `"bootstrap_description"`).
    pub kind: String,
    /// Telegram chat the card was sent to.
    pub chat_id: i64,
    /// Telegram message id; `None` until [`mark_message_sent`] records it.
    pub message_id: Option<i64>,
    /// Hash over `(body_md, keyboard_serialized)` — used by
    /// [`update_content_hash`] to short-circuit no-op `editMessageText` calls.
    pub content_hash: String,
    /// Message id of the bot's `ForceReply` prompt asking for edit feedback,
    /// or `None` when no edit reply is currently pending.
    pub pending_edit_prompt_message_id: Option<i64>,
    /// Unix epoch ms at which the card row was created.
    pub created_at: i64,
    /// Unix epoch ms of the most recent mutation.
    pub updated_at: i64,
    /// Unix epoch ms when the card was closed; `None` while open.
    pub closed_at: Option<i64>,
}

/// Errors raised by the cards repository helpers.
#[derive(Debug, thiserror::Error)]
pub enum CardsError {
    /// A repository call referenced a `card_id` that does not exist in the
    /// table.
    #[error("card not found: {0}")]
    NotFound(String),
    /// Underlying SQLite error.
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

/// Upsert a card row keyed by `(run_id, node_key, attempt_index)`.
///
/// On a fresh triple this inserts a row with `message_id = NULL`. On a
/// re-emit for the same triple this leaves the existing row untouched. In
/// both cases the canonical `card_id` (newly generated or pre-existing) is
/// returned so the caller can address the row going forward.
///
/// `content_hash` is the pre-rendered hash for the card's body+keyboard.
/// The hash is stored verbatim and is the value [`update_content_hash`]
/// will compare against on later updates.
///
/// # Errors
///
/// Returns [`CardsError::Sqlite`] on storage failure.
pub fn upsert(
    conn: &Connection,
    run_id: &str,
    node_key: &str,
    attempt_index: i64,
    kind: &str,
    chat_id: i64,
    content_hash: &str,
    now_ms: i64,
) -> Result<String, CardsError> {
    let fresh_card_id = ulid::Ulid::new().to_string();
    conn.execute(
        "INSERT OR IGNORE INTO telegram_cards \
         (card_id, run_id, node_key, attempt_index, kind, chat_id, \
          content_hash, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        params![
            &fresh_card_id,
            run_id,
            node_key,
            attempt_index,
            kind,
            chat_id,
            content_hash,
            now_ms,
            now_ms,
        ],
    )?;
    let existing: String = conn.query_row(
        "SELECT card_id FROM telegram_cards \
         WHERE run_id = ? AND node_key = ? AND attempt_index = ?",
        params![run_id, node_key, attempt_index],
        |row| row.get(0),
    )?;
    tracing::debug!(
        target: "persistence::telegram::cards",
        card_id = %existing,
        run_id = %run_id,
        node_key = %node_key,
        attempt_index = %attempt_index,
        fresh = existing == fresh_card_id,
        "card upserted",
    );
    Ok(existing)
}

/// Record the Telegram `message_id` and the final `content_hash` after the
/// bot's initial `sendMessage` call succeeded.
///
/// # Errors
///
/// Returns [`CardsError::NotFound`] if no row matches `card_id`; otherwise
/// surfaces [`CardsError::Sqlite`].
pub fn mark_message_sent(
    conn: &Connection,
    card_id: &str,
    message_id: i64,
    content_hash: &str,
    now_ms: i64,
) -> Result<(), CardsError> {
    let rows = conn.execute(
        "UPDATE telegram_cards \
         SET message_id = ?, content_hash = ?, updated_at = ? \
         WHERE card_id = ?",
        params![message_id, content_hash, now_ms, card_id],
    )?;
    if rows == 0 {
        return Err(CardsError::NotFound(card_id.to_owned()));
    }
    tracing::debug!(
        target: "persistence::telegram::cards",
        card_id = %card_id,
        message_id = %message_id,
        "card message sent",
    );
    Ok(())
}

/// Conditional-update of `content_hash`. Returns `true` when the column
/// actually changed, `false` when the new hash matches the stored one.
///
/// The cockpit emitter uses this to short-circuit `editMessageText` calls
/// for no-op updates (ADR 0011, Decision 8).
///
/// # Errors
///
/// Returns [`CardsError::Sqlite`] on storage failure.
pub fn update_content_hash(
    conn: &Connection,
    card_id: &str,
    new_hash: &str,
    now_ms: i64,
) -> Result<bool, CardsError> {
    let rows = conn.execute(
        "UPDATE telegram_cards SET content_hash = ?, updated_at = ? \
         WHERE card_id = ? AND content_hash != ?",
        params![new_hash, now_ms, card_id, new_hash],
    )?;
    Ok(rows > 0)
}

/// Soft-close a card. Subsequent callbacks for this card are rejected by
/// the cockpit's `answerCallbackQuery` "card no longer active" handler.
///
/// Idempotent: calling `close` on an already-closed card is a no-op.
///
/// # Errors
///
/// Returns [`CardsError::Sqlite`] on storage failure.
pub fn close(conn: &Connection, card_id: &str, now_ms: i64) -> Result<(), CardsError> {
    conn.execute(
        "UPDATE telegram_cards SET closed_at = ?, updated_at = ? \
         WHERE card_id = ? AND closed_at IS NULL",
        params![now_ms, now_ms, card_id],
    )?;
    tracing::debug!(
        target: "persistence::telegram::cards",
        card_id = %card_id,
        "card closed",
    );
    Ok(())
}

/// Fetch a card by its primary key.
///
/// Returns `Ok(None)` when no row matches; `Err` only on storage failure.
///
/// # Errors
///
/// Returns [`CardsError::Sqlite`] on storage failure.
pub fn find_by_id(conn: &Connection, card_id: &str) -> Result<Option<Card>, CardsError> {
    let card = conn
        .query_row(
            "SELECT card_id, run_id, node_key, attempt_index, kind, chat_id, \
             message_id, content_hash, pending_edit_prompt_message_id, \
             created_at, updated_at, closed_at \
             FROM telegram_cards WHERE card_id = ?",
            params![card_id],
            row_to_card,
        )
        .optional()?;
    Ok(card)
}

/// Count currently open cards. Used by `surge doctor` — cheaper than
/// [`find_open`] when only the count is needed.
///
/// # Errors
///
/// Returns [`CardsError::Sqlite`] on storage failure.
pub fn count_open(conn: &Connection) -> Result<i64, CardsError> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM telegram_cards WHERE closed_at IS NULL",
        [],
        |row| row.get(0),
    )?;
    Ok(n)
}

/// Latest `updated_at` across all cards, or `None` if the table is empty.
/// Approximates "last successful Bot API call timestamp" for
/// `surge doctor`.
///
/// # Errors
///
/// Returns [`CardsError::Sqlite`] on storage failure.
pub fn latest_updated_at_ms(conn: &Connection) -> Result<Option<i64>, CardsError> {
    let row: Option<i64> = conn
        .query_row(
            "SELECT MAX(updated_at) FROM telegram_cards",
            [],
            |row| row.get::<_, Option<i64>>(0),
        )
        .optional()?
        .flatten();
    Ok(row)
}

/// Return every open (non-closed) card. Used by the cockpit's startup
/// reconcile pass to refresh content against the current run-event state.
///
/// # Errors
///
/// Returns [`CardsError::Sqlite`] on storage failure.
pub fn find_open(conn: &Connection) -> Result<Vec<Card>, CardsError> {
    let mut stmt = conn.prepare(
        "SELECT card_id, run_id, node_key, attempt_index, kind, chat_id, \
         message_id, content_hash, pending_edit_prompt_message_id, \
         created_at, updated_at, closed_at \
         FROM telegram_cards WHERE closed_at IS NULL \
         ORDER BY created_at ASC",
    )?;
    let cards = stmt
        .query_map([], row_to_card)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(cards)
}

/// Shared row mapper used by [`find_by_id`] and [`find_open`].
fn row_to_card(row: &rusqlite::Row<'_>) -> rusqlite::Result<Card> {
    Ok(Card {
        card_id: row.get(0)?,
        run_id: row.get(1)?,
        node_key: row.get(2)?,
        attempt_index: row.get(3)?,
        kind: row.get(4)?,
        chat_id: row.get(5)?,
        message_id: row.get(6)?,
        content_hash: row.get(7)?,
        pending_edit_prompt_message_id: row.get(8)?,
        created_at: row.get(9)?,
        updated_at: row.get(10)?,
        closed_at: row.get(11)?,
    })
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
    fn upsert_creates_row_with_null_message_id() {
        let conn = fresh_db();
        let card_id = upsert(
            &conn,
            "run-1",
            "approve_plan",
            0,
            "human_gate",
            42,
            "hash-a",
            1_000,
        )
        .unwrap();
        assert_eq!(card_id.len(), 26, "card_id must be a 26-char ULID");

        let card = find_by_id(&conn, &card_id).unwrap().unwrap();
        assert!(card.message_id.is_none());
        assert_eq!(card.run_id, "run-1");
        assert_eq!(card.kind, "human_gate");
        assert_eq!(card.attempt_index, 0);
    }

    #[test]
    fn upsert_is_idempotent_on_same_triple() {
        let conn = fresh_db();
        let first = upsert(
            &conn,
            "run-1",
            "approve_plan",
            0,
            "human_gate",
            42,
            "hash-a",
            1_000,
        )
        .unwrap();
        let second = upsert(
            &conn,
            "run-1",
            "approve_plan",
            0,
            "human_gate",
            42,
            "hash-b",
            2_000,
        )
        .unwrap();
        assert_eq!(first, second, "same triple must produce same card_id");

        // The original content_hash must NOT be overwritten by a re-upsert —
        // updates happen exclusively via update_content_hash.
        let card = find_by_id(&conn, &first).unwrap().unwrap();
        assert_eq!(card.content_hash, "hash-a");
    }

    #[test]
    fn upsert_separates_attempts_with_distinct_card_ids() {
        let conn = fresh_db();
        let attempt_0 = upsert(
            &conn,
            "run-1",
            "approve_plan",
            0,
            "human_gate",
            42,
            "hash-a",
            1_000,
        )
        .unwrap();
        let attempt_1 = upsert(
            &conn,
            "run-1",
            "approve_plan",
            1,
            "human_gate",
            42,
            "hash-b",
            2_000,
        )
        .unwrap();
        assert_ne!(attempt_0, attempt_1, "attempts must produce distinct cards");
    }

    #[test]
    fn mark_message_sent_records_message_id() {
        let conn = fresh_db();
        let card_id = upsert(
            &conn,
            "run-1",
            "approve_plan",
            0,
            "human_gate",
            42,
            "hash-a",
            1_000,
        )
        .unwrap();
        mark_message_sent(&conn, &card_id, 9876, "hash-a-final", 1_500).unwrap();

        let card = find_by_id(&conn, &card_id).unwrap().unwrap();
        assert_eq!(card.message_id, Some(9876));
        assert_eq!(card.content_hash, "hash-a-final");
        assert_eq!(card.updated_at, 1_500);
    }

    #[test]
    fn mark_message_sent_unknown_card_returns_not_found() {
        let conn = fresh_db();
        let err = mark_message_sent(&conn, "missing", 1, "h", 1_000).unwrap_err();
        assert!(matches!(err, CardsError::NotFound(_)));
    }

    #[test]
    fn update_content_hash_returns_true_on_change() {
        let conn = fresh_db();
        let card_id = upsert(
            &conn,
            "run-1",
            "approve_plan",
            0,
            "human_gate",
            42,
            "hash-a",
            1_000,
        )
        .unwrap();
        let changed = update_content_hash(&conn, &card_id, "hash-b", 1_100).unwrap();
        assert!(changed);

        let card = find_by_id(&conn, &card_id).unwrap().unwrap();
        assert_eq!(card.content_hash, "hash-b");
    }

    #[test]
    fn update_content_hash_returns_false_on_no_op() {
        let conn = fresh_db();
        let card_id = upsert(
            &conn,
            "run-1",
            "approve_plan",
            0,
            "human_gate",
            42,
            "hash-a",
            1_000,
        )
        .unwrap();
        let changed = update_content_hash(&conn, &card_id, "hash-a", 1_200).unwrap();
        assert!(!changed, "identical hash must not write");
    }

    #[test]
    fn close_marks_closed_at_and_excludes_from_find_open() {
        let conn = fresh_db();
        let open = upsert(
            &conn,
            "run-1",
            "approve_plan",
            0,
            "human_gate",
            42,
            "hash-a",
            1_000,
        )
        .unwrap();
        let closed = upsert(
            &conn,
            "run-1",
            "approve_plan",
            1,
            "human_gate",
            42,
            "hash-b",
            1_100,
        )
        .unwrap();
        close(&conn, &closed, 1_200).unwrap();

        let still_open = find_open(&conn).unwrap();
        let open_ids: Vec<String> = still_open.iter().map(|c| c.card_id.clone()).collect();
        assert_eq!(open_ids, vec![open]);
        assert!(!open_ids.contains(&closed));

        let closed_card = find_by_id(&conn, &closed).unwrap().unwrap();
        assert_eq!(closed_card.closed_at, Some(1_200));
    }

    #[test]
    fn close_is_idempotent() {
        let conn = fresh_db();
        let card_id = upsert(
            &conn,
            "run-1",
            "approve_plan",
            0,
            "human_gate",
            42,
            "hash-a",
            1_000,
        )
        .unwrap();
        close(&conn, &card_id, 1_200).unwrap();
        let before = find_by_id(&conn, &card_id).unwrap().unwrap();
        close(&conn, &card_id, 1_300).unwrap();
        let after = find_by_id(&conn, &card_id).unwrap().unwrap();
        assert_eq!(before.closed_at, after.closed_at);
    }

    #[test]
    fn find_open_returns_oldest_first() {
        let conn = fresh_db();
        let _newer = upsert(
            &conn,
            "run-2",
            "approve",
            0,
            "human_gate",
            42,
            "hash",
            2_000,
        )
        .unwrap();
        let older = upsert(
            &conn,
            "run-1",
            "approve",
            0,
            "human_gate",
            42,
            "hash",
            1_000,
        )
        .unwrap();
        let _middle = upsert(
            &conn,
            "run-3",
            "approve",
            0,
            "human_gate",
            42,
            "hash",
            1_500,
        )
        .unwrap();

        let open = find_open(&conn).unwrap();
        assert_eq!(open.first().unwrap().card_id, older);
        assert_eq!(open.len(), 3);
        let ordered: Vec<i64> = open.iter().map(|c| c.created_at).collect();
        assert_eq!(ordered, vec![1_000, 1_500, 2_000]);
    }
}
