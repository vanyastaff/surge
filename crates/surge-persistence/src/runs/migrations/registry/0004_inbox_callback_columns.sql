-- 0004_inbox_callback_columns.sql
-- Adds the per-card callback_token plus Telegram message references to
-- ticket_index. callback_token is generated each time an inbox card is
-- emitted (including on snooze re-emission); cleared on Start. The
-- partial UNIQUE index allows multiple post-decision rows with NULL
-- token while preventing two open cards from colliding.

ALTER TABLE ticket_index ADD COLUMN callback_token TEXT;
ALTER TABLE ticket_index ADD COLUMN tg_chat_id INTEGER;
ALTER TABLE ticket_index ADD COLUMN tg_message_id INTEGER;

CREATE UNIQUE INDEX IF NOT EXISTS idx_ticket_index_callback_token
    ON ticket_index(callback_token)
    WHERE callback_token IS NOT NULL;
