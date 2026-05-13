-- 0009_telegram_cards.sql
--
-- Telegram cockpit card store. One row per (run_id, node_key, attempt_index)
-- triple — the idempotency key locked in ADR 0011. INSERT OR IGNORE on this
-- triple guarantees no duplicate card per attempt; the bootstrap edit loop
-- (which re-enters the same gate up to edit_loop_cap times) advances
-- attempt_index and lands on a fresh row so the operator's history of the
-- run keeps every attempt visible.
--
-- Lifecycle:
--   1. upsert() inserts the row with message_id = NULL and a pre-computed
--      content_hash; returns the card_id (fresh or existing).
--   2. mark_message_sent() records the Telegram message_id and final
--      content_hash after the bot's sendMessage call.
--   3. update_content_hash() returns true when the value actually changed,
--      driving the editMessageText short-circuit (ADR 0011, Decision 8).
--   4. close() sets closed_at; subsequent callbacks for this card are
--      rejected with "card no longer active".
--
-- Recovery on daemon restart reads find_open() and reconciles each row
-- against the persisted run-event log. Never sends a new message on resume.

CREATE TABLE IF NOT EXISTS telegram_cards (
    card_id                         TEXT    PRIMARY KEY,        -- ULID
    run_id                          TEXT    NOT NULL,
    node_key                        TEXT    NOT NULL,
    attempt_index                   INTEGER NOT NULL,           -- RunMemory.node_visits
    kind                            TEXT    NOT NULL,           -- card kind discriminator
    chat_id                         INTEGER NOT NULL,
    message_id                      INTEGER,                    -- NULL until mark_message_sent
    content_hash                    TEXT    NOT NULL,
    pending_edit_prompt_message_id  INTEGER,                    -- ForceReply prompt id
    created_at                      INTEGER NOT NULL,           -- Unix epoch ms
    updated_at                      INTEGER NOT NULL,           -- Unix epoch ms
    closed_at                       INTEGER,                    -- NULL = open
    UNIQUE(run_id, node_key, attempt_index)
);

CREATE INDEX IF NOT EXISTS idx_telegram_cards_open
    ON telegram_cards(card_id)
    WHERE closed_at IS NULL;

CREATE INDEX IF NOT EXISTS idx_telegram_cards_by_run
    ON telegram_cards(run_id, node_key, attempt_index);
