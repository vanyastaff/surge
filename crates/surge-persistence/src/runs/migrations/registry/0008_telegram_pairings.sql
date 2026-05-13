-- 0008_telegram_pairings.sql
--
-- Paired-chat allowlist that gates the Telegram cockpit. Every incoming
-- callback and every command message has its chat_id checked against this
-- table before any handler runs (see ADR 0011 and the cockpit milestone
-- plan, Decision 6).
--
-- Rows are inserted by the `/pair <token>` command handler after a successful
-- consume_pairing_token call (see 0007 / pairing.rs). Revocation is
-- soft — revoked_at is set on the row instead of deleting, so audit trails
-- and any in-flight messages can still be correlated to a known chat.
--
-- A chat that was previously revoked and re-pairs replaces its label and
-- clears revoked_at via the upsert semantics in TelegramPairingsRepo::pair.

CREATE TABLE IF NOT EXISTS telegram_pairings (
    chat_id     INTEGER PRIMARY KEY,
    user_label  TEXT    NOT NULL,
    paired_at   INTEGER NOT NULL,    -- Unix epoch ms
    revoked_at  INTEGER              -- Unix epoch ms; NULL = active
);

CREATE INDEX IF NOT EXISTS idx_telegram_pairings_active
    ON telegram_pairings(chat_id)
    WHERE revoked_at IS NULL;
