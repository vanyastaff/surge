//! Inbox action + delivery queues for the surge-daemon inbox subsystem.
//!
//! The receivers (`TgInboxBot`, `DesktopActionListener`) write to
//! `inbox_action_queue`; `InboxActionConsumer` polls and processes.
//!
//! The router writes to `inbox_delivery_queue` when a `RouterOutput::Triage`
//! event needs an inbox card; the bot's outgoing loop and the desktop
//! deliverer both read from it independently.

use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};

/// One row of the `inbox_action_queue` table — a pending request from a
/// receiver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboxActionRow {
    /// Auto-incremented FIFO sequence number (PK).
    pub seq: i64,
    /// Action kind.
    pub kind: InboxActionKind,
    /// External ticket id.
    pub task_id: String,
    /// Token from inbox-card callback_data.
    pub callback_token: String,
    /// Channel via which the action was decided ("telegram" | "desktop").
    pub decided_via: String,
    /// For `kind = Snooze`, the absolute time at which to re-emit the card.
    pub snooze_until: Option<DateTime<Utc>>,
    /// Time the row was inserted.
    pub enqueued_at: DateTime<Utc>,
}

/// Action kind on `inbox_action_queue.kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InboxActionKind {
    /// User tapped Start.
    Start,
    /// User tapped Snooze.
    Snooze,
    /// User tapped Skip.
    Skip,
}

impl InboxActionKind {
    /// Stable on-disk string form.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Snooze => "snooze",
            Self::Skip => "skip",
        }
    }

    /// Inverse of `as_str`. Returns `None` on unknown strings.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "start" => Some(Self::Start),
            "snooze" => Some(Self::Snooze),
            "skip" => Some(Self::Skip),
            _ => None,
        }
    }
}

/// One row of the `inbox_delivery_queue` table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboxDeliveryRow {
    /// Auto-incremented FIFO sequence number (PK).
    pub seq: i64,
    /// External ticket id.
    pub task_id: String,
    /// Callback token embedded in the delivered card.
    pub callback_token: String,
    /// Serialised `InboxCardPayload` JSON.
    pub payload_json: String,
    /// Time the row was inserted.
    pub enqueued_at: DateTime<Utc>,
    /// Time Telegram delivery was acked, NULL if not yet delivered.
    pub telegram_delivered_at: Option<DateTime<Utc>>,
    /// Telegram chat id used for delivery, NULL if not yet delivered.
    pub telegram_chat_id: Option<i64>,
    /// Telegram message id, NULL if not yet delivered.
    pub telegram_message_id: Option<i32>,
    /// Time Desktop delivery was attempted, NULL if not yet shown.
    pub desktop_delivered_at: Option<DateTime<Utc>>,
}

/// Append a new action row. Returns the assigned seq.
pub fn append_action(
    conn: &Connection,
    kind: InboxActionKind,
    task_id: &str,
    callback_token: &str,
    decided_via: &str,
    snooze_until: Option<DateTime<Utc>>,
) -> rusqlite::Result<i64> {
    let now = Utc::now();
    conn.execute(
        "INSERT INTO inbox_action_queue \
            (kind, task_id, callback_token, decided_via, snooze_until, enqueued_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            kind.as_str(),
            task_id,
            callback_token,
            decided_via,
            snooze_until.map(|d| d.to_rfc3339()),
            now.to_rfc3339(),
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Read pending action rows (`processed_at IS NULL`), ordered by seq.
pub fn list_pending_actions(conn: &Connection) -> rusqlite::Result<Vec<InboxActionRow>> {
    let mut stmt = conn.prepare(
        "SELECT seq, kind, task_id, callback_token, decided_via, snooze_until, enqueued_at \
         FROM inbox_action_queue WHERE processed_at IS NULL ORDER BY seq ASC",
    )?;
    let mut out = Vec::new();
    let mut rows = stmt.query([])?;
    while let Some(r) = rows.next()? {
        let kind_str: String = r.get(1)?;
        let kind = InboxActionKind::parse(&kind_str).ok_or_else(|| {
            rusqlite::Error::FromSqlConversionFailure(
                1,
                rusqlite::types::Type::Text,
                format!("unknown inbox action kind: {kind_str}").into(),
            )
        })?;
        let snooze_until_str: Option<String> = r.get(5)?;
        let enqueued_at_str: String = r.get(6)?;
        out.push(InboxActionRow {
            seq: r.get(0)?,
            kind,
            task_id: r.get(2)?,
            callback_token: r.get(3)?,
            decided_via: r.get(4)?,
            snooze_until: snooze_until_str
                .map(|s| DateTime::parse_from_rfc3339(&s).map(|d| d.with_timezone(&Utc)))
                .transpose()
                .map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        5,
                        rusqlite::types::Type::Text,
                        e.to_string().into(),
                    )
                })?,
            enqueued_at: DateTime::parse_from_rfc3339(&enqueued_at_str)
                .map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        6,
                        rusqlite::types::Type::Text,
                        e.to_string().into(),
                    )
                })?
                .with_timezone(&Utc),
        });
    }
    Ok(out)
}

/// Mark a row processed (idempotent — safe to call twice).
pub fn mark_action_processed(conn: &Connection, seq: i64) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE inbox_action_queue SET processed_at = ?1 WHERE seq = ?2 AND processed_at IS NULL",
        params![Utc::now().to_rfc3339(), seq],
    )?;
    Ok(())
}

/// Append a delivery row for an outgoing inbox card.
pub fn append_delivery(
    conn: &Connection,
    task_id: &str,
    callback_token: &str,
    payload_json: &str,
) -> rusqlite::Result<i64> {
    let now = Utc::now();
    conn.execute(
        "INSERT INTO inbox_delivery_queue \
            (task_id, callback_token, payload_json, enqueued_at) \
         VALUES (?1, ?2, ?3, ?4)",
        params![task_id, callback_token, payload_json, now.to_rfc3339()],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Pending Telegram deliveries (`telegram_delivered_at IS NULL`).
pub fn list_pending_telegram_deliveries(
    conn: &Connection,
) -> rusqlite::Result<Vec<InboxDeliveryRow>> {
    list_deliveries(conn, "telegram_delivered_at IS NULL")
}

/// Pending Desktop deliveries (`desktop_delivered_at IS NULL`).
pub fn list_pending_desktop_deliveries(
    conn: &Connection,
) -> rusqlite::Result<Vec<InboxDeliveryRow>> {
    list_deliveries(conn, "desktop_delivered_at IS NULL")
}

fn list_deliveries(
    conn: &Connection,
    where_clause: &str,
) -> rusqlite::Result<Vec<InboxDeliveryRow>> {
    let sql = format!(
        "SELECT seq, task_id, callback_token, payload_json, enqueued_at, \
                telegram_delivered_at, telegram_chat_id, telegram_message_id, \
                desktop_delivered_at \
         FROM inbox_delivery_queue WHERE {where_clause} ORDER BY seq ASC"
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut out = Vec::new();
    let mut rows = stmt.query([])?;
    while let Some(r) = rows.next()? {
        let enqueued_at_str: String = r.get(4)?;
        let tg_at_str: Option<String> = r.get(5)?;
        let dt_at_str: Option<String> = r.get(8)?;
        out.push(InboxDeliveryRow {
            seq: r.get(0)?,
            task_id: r.get(1)?,
            callback_token: r.get(2)?,
            payload_json: r.get(3)?,
            enqueued_at: DateTime::parse_from_rfc3339(&enqueued_at_str)
                .map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        4,
                        rusqlite::types::Type::Text,
                        e.to_string().into(),
                    )
                })?
                .with_timezone(&Utc),
            telegram_delivered_at: tg_at_str
                .map(|s| DateTime::parse_from_rfc3339(&s).map(|d| d.with_timezone(&Utc)))
                .transpose()
                .map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        5,
                        rusqlite::types::Type::Text,
                        e.to_string().into(),
                    )
                })?,
            telegram_chat_id: r.get(6)?,
            telegram_message_id: r.get(7)?,
            desktop_delivered_at: dt_at_str
                .map(|s| DateTime::parse_from_rfc3339(&s).map(|d| d.with_timezone(&Utc)))
                .transpose()
                .map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        8,
                        rusqlite::types::Type::Text,
                        e.to_string().into(),
                    )
                })?,
        });
    }
    Ok(out)
}

/// Record successful Telegram delivery.
pub fn record_telegram_delivered(
    conn: &Connection,
    seq: i64,
    chat_id: i64,
    message_id: i32,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE inbox_delivery_queue SET telegram_delivered_at = ?1, \
            telegram_chat_id = ?2, telegram_message_id = ?3 \
         WHERE seq = ?4 AND telegram_delivered_at IS NULL",
        params![Utc::now().to_rfc3339(), chat_id, message_id, seq],
    )?;
    Ok(())
}

/// Record successful Desktop delivery.
pub fn record_desktop_delivered(conn: &Connection, seq: i64) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE inbox_delivery_queue SET desktop_delivered_at = ?1 \
         WHERE seq = ?2 AND desktop_delivered_at IS NULL",
        params![Utc::now().to_rfc3339(), seq],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE runs (id TEXT PRIMARY KEY);")
            .unwrap();
        let m1 = include_str!("migrations/registry/0002_ticket_index.sql");
        conn.execute_batch(m1).unwrap();
        let m2 = include_str!("migrations/registry/0004_inbox_callback_columns.sql");
        conn.execute_batch(m2).unwrap();
        let m3 = include_str!("migrations/registry/0005_inbox_queues.sql");
        conn.execute_batch(m3).unwrap();
        conn
    }

    #[test]
    fn append_then_list_then_mark_processed() {
        let conn = db();
        let seq = append_action(&conn, InboxActionKind::Start, "linear:t/T-1", "tok1", "telegram", None).unwrap();
        assert!(seq > 0);
        let pending = list_pending_actions(&conn).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].kind, InboxActionKind::Start);
        assert_eq!(pending[0].callback_token, "tok1");
        mark_action_processed(&conn, seq).unwrap();
        assert_eq!(list_pending_actions(&conn).unwrap().len(), 0);
    }

    #[test]
    fn snooze_action_carries_until_timestamp() {
        let conn = db();
        let until = DateTime::parse_from_rfc3339("2030-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        append_action(&conn, InboxActionKind::Snooze, "linear:t/T-2", "tok2", "desktop", Some(until)).unwrap();
        let pending = list_pending_actions(&conn).unwrap();
        assert_eq!(pending[0].snooze_until, Some(until));
    }

    #[test]
    fn delivery_legs_independent() {
        let conn = db();
        let seq = append_delivery(&conn, "linear:t/D-1", "tok-d", r#"{"x":1}"#).unwrap();
        assert_eq!(list_pending_telegram_deliveries(&conn).unwrap().len(), 1);
        assert_eq!(list_pending_desktop_deliveries(&conn).unwrap().len(), 1);

        record_telegram_delivered(&conn, seq, 12345, 6789).unwrap();
        assert_eq!(list_pending_telegram_deliveries(&conn).unwrap().len(), 0);
        assert_eq!(list_pending_desktop_deliveries(&conn).unwrap().len(), 1);

        record_desktop_delivered(&conn, seq).unwrap();
        assert_eq!(list_pending_desktop_deliveries(&conn).unwrap().len(), 0);
    }

    #[test]
    fn idempotent_marking() {
        let conn = db();
        let seq = append_action(&conn, InboxActionKind::Skip, "linear:t/T-3", "tok3", "telegram", None).unwrap();
        mark_action_processed(&conn, seq).unwrap();
        // Second call is a no-op (the WHERE clause filters it out).
        mark_action_processed(&conn, seq).unwrap();
    }
}
