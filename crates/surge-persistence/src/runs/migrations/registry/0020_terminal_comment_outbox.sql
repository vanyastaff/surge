-- Immutable terminal comment intent, mutable delivery/lease metadata.
CREATE TABLE terminal_comment_outbox (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    source_id TEXT NOT NULL,
    task_id TEXT NOT NULL,
    run_id TEXT NOT NULL,
    event_kind TEXT NOT NULL CHECK(event_kind IN ('run_completed', 'run_failed', 'run_aborted')),
    body TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    next_attempt_at INTEGER NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 0,
    lease_token TEXT,
    lease_until INTEGER,
    delivered_at INTEGER,
    last_error TEXT,
    UNIQUE(source_id, task_id, event_kind, run_id),
    CHECK((lease_token IS NULL) = (lease_until IS NULL))
);
CREATE INDEX terminal_comment_outbox_due ON terminal_comment_outbox(next_attempt_at, id)
    WHERE delivered_at IS NULL;
CREATE TRIGGER terminal_comment_outbox_immutable
BEFORE UPDATE OF source_id, task_id, run_id, event_kind, body, created_at ON terminal_comment_outbox
BEGIN
    SELECT RAISE(ABORT, 'terminal comment identity is immutable');
END;
