-- 0005_inbox_queues.sql
-- Two queues that decouple the inbox-action receivers (Telegram bot,
-- Desktop listener) from the consumer (InboxActionConsumer) and the
-- delivery loop (TgInboxBot.outgoing_loop) from the router (which
-- enqueues fresh cards). Both are FIFO with monotonic seq.
--
-- inbox_action_queue: incoming requests from receivers. processed_at
-- becomes non-NULL once InboxActionConsumer commits the dispatch.
--
-- inbox_delivery_queue: outgoing card payloads. Each transport leg
-- (telegram, desktop) records its own delivery timestamp + IDs so the
-- legs run independently and can both deliver the same card.

CREATE TABLE IF NOT EXISTS inbox_action_queue (
    seq             INTEGER PRIMARY KEY AUTOINCREMENT,
    kind            TEXT    NOT NULL,    -- "start" | "snooze" | "skip"
    task_id         TEXT    NOT NULL,
    callback_token  TEXT    NOT NULL,
    decided_via     TEXT    NOT NULL,    -- "telegram" | "desktop"
    snooze_until    TEXT,                -- ISO-8601, only for kind="snooze"
    enqueued_at     TEXT    NOT NULL,
    processed_at    TEXT
);

CREATE INDEX IF NOT EXISTS idx_inbox_action_queue_pending
    ON inbox_action_queue(seq)
    WHERE processed_at IS NULL;

CREATE TABLE IF NOT EXISTS inbox_delivery_queue (
    seq                       INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id                   TEXT    NOT NULL,
    callback_token            TEXT    NOT NULL,
    payload_json              TEXT    NOT NULL,
    enqueued_at               TEXT    NOT NULL,
    telegram_delivered_at     TEXT,
    telegram_chat_id          INTEGER,
    telegram_message_id       INTEGER,
    desktop_delivered_at      TEXT
);

CREATE INDEX IF NOT EXISTS idx_inbox_delivery_queue_tg_pending
    ON inbox_delivery_queue(seq)
    WHERE telegram_delivered_at IS NULL;

CREATE INDEX IF NOT EXISTS idx_inbox_delivery_queue_desktop_pending
    ON inbox_delivery_queue(seq)
    WHERE desktop_delivered_at IS NULL;
