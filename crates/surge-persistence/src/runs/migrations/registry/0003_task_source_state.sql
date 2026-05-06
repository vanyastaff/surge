-- 0003_task_source_state.sql
-- Per-source polling cursor + failure counter used by surge-intake's TaskRouter.

CREATE TABLE IF NOT EXISTS task_source_state (
    source_id            TEXT PRIMARY KEY,
    last_seen_cursor     TEXT,
    last_poll_at         TEXT,
    consecutive_failures INTEGER NOT NULL DEFAULT 0
);
