//! Generic key-value secret store.
//!
//! Backed by migration `0011_secrets.sql`. Used by `surge telegram setup`
//! to persist the Bot API token; intended for other subsystems too.
//! Rows are namespaced by their `key` prefix (e.g.
//! `telegram.cockpit.bot_token`).
//!
//! **Security model:** values are stored unencrypted; protection comes from
//! the filesystem permissions on `~/.surge/db/registry.sqlite`. Deployments
//! that need stronger guarantees should layer an external secrets manager
//! and treat this table as a cache.

use rusqlite::{Connection, OptionalExtension, params};

/// Errors raised by the secrets helpers.
#[derive(Debug, thiserror::Error)]
pub enum SecretsError {
    /// Underlying SQLite error.
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

/// Telegram cockpit bot token key prefix. The cockpit reads this key on
/// startup; the CLI's `surge telegram setup` writes it.
pub const TELEGRAM_BOT_TOKEN_KEY: &str = "telegram.cockpit.bot_token";

/// Insert or update a secret. Returns the resulting `created_at`
/// timestamp (left unchanged on update).
///
/// # Errors
///
/// Returns [`SecretsError::Sqlite`] on storage failure.
pub fn set_secret(
    conn: &Connection,
    key: &str,
    value: &str,
    now_ms: i64,
) -> Result<(), SecretsError> {
    conn.execute(
        "INSERT INTO secrets (key, value, created_at, updated_at) \
         VALUES (?, ?, ?, ?) \
         ON CONFLICT(key) DO UPDATE SET \
            value = excluded.value, \
            updated_at = excluded.updated_at",
        params![key, value, now_ms, now_ms],
    )?;
    tracing::debug!(
        target: "persistence::secrets",
        key = %key,
        "secret stored",
    );
    Ok(())
}

/// Fetch a secret by key. `Ok(None)` when no row matches.
///
/// # Errors
///
/// Returns [`SecretsError::Sqlite`] on storage failure.
pub fn get_secret(conn: &Connection, key: &str) -> Result<Option<String>, SecretsError> {
    let value: Option<String> = conn
        .query_row(
            "SELECT value FROM secrets WHERE key = ?",
            params![key],
            |row| row.get(0),
        )
        .optional()?;
    Ok(value)
}

/// Delete a secret by key. Idempotent — no error if the row is missing.
///
/// # Errors
///
/// Returns [`SecretsError::Sqlite`] on storage failure.
pub fn delete_secret(conn: &Connection, key: &str) -> Result<(), SecretsError> {
    conn.execute("DELETE FROM secrets WHERE key = ?", params![key])?;
    tracing::debug!(
        target: "persistence::secrets",
        key = %key,
        "secret deleted",
    );
    Ok(())
}

/// Return `true` if a secret with the given key exists. Distinct from
/// [`get_secret`] because callers — e.g. doctor reports — should not pull
/// the value into memory just to check presence.
///
/// # Errors
///
/// Returns [`SecretsError::Sqlite`] on storage failure.
pub fn has_secret(conn: &Connection, key: &str) -> Result<bool, SecretsError> {
    let row: Option<i64> = conn
        .query_row("SELECT 1 FROM secrets WHERE key = ?", params![key], |row| {
            row.get(0)
        })
        .optional()?;
    Ok(row.is_some())
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
    fn round_trip_set_then_get() {
        let conn = fresh_db();
        set_secret(&conn, "k", "v", 1_000).unwrap();
        assert_eq!(get_secret(&conn, "k").unwrap().as_deref(), Some("v"));
    }

    #[test]
    fn get_missing_key_returns_none() {
        let conn = fresh_db();
        assert_eq!(get_secret(&conn, "missing").unwrap(), None);
    }

    #[test]
    fn set_overwrites_existing_value() {
        let conn = fresh_db();
        set_secret(&conn, "k", "first", 1_000).unwrap();
        set_secret(&conn, "k", "second", 2_000).unwrap();
        assert_eq!(get_secret(&conn, "k").unwrap().as_deref(), Some("second"));
    }

    #[test]
    fn delete_removes_secret() {
        let conn = fresh_db();
        set_secret(&conn, "k", "v", 1_000).unwrap();
        delete_secret(&conn, "k").unwrap();
        assert_eq!(get_secret(&conn, "k").unwrap(), None);
    }

    #[test]
    fn delete_missing_key_is_a_noop() {
        let conn = fresh_db();
        delete_secret(&conn, "missing").unwrap();
        // No error.
    }

    #[test]
    fn has_secret_reflects_presence() {
        let conn = fresh_db();
        assert!(!has_secret(&conn, "k").unwrap());
        set_secret(&conn, "k", "v", 1_000).unwrap();
        assert!(has_secret(&conn, "k").unwrap());
        delete_secret(&conn, "k").unwrap();
        assert!(!has_secret(&conn, "k").unwrap());
    }

    #[test]
    fn telegram_token_key_is_stable() {
        // Production cockpit reads this constant; tightening it here
        // catches accidental rename in review.
        assert_eq!(TELEGRAM_BOT_TOKEN_KEY, "telegram.cockpit.bot_token");
    }
}
