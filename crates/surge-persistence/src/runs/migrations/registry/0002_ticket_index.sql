-- 0002_ticket_index.sql
-- Tracks lifecycle of external tickets ingested via surge-intake.
-- See docs/revision/rfcs/0010-issue-tracker-integration.md, decision #22.

CREATE TABLE IF NOT EXISTS ticket_index (
    task_id          TEXT PRIMARY KEY,
    source_id        TEXT NOT NULL,
    provider         TEXT NOT NULL,
    run_id           TEXT,
    triage_decision  TEXT,
    duplicate_of     TEXT,
    priority         TEXT,
    state            TEXT NOT NULL,
    first_seen       TEXT NOT NULL,
    last_seen        TEXT NOT NULL,
    snooze_until     TEXT,

    FOREIGN KEY (run_id) REFERENCES runs(id) ON DELETE SET NULL,
    FOREIGN KEY (duplicate_of) REFERENCES ticket_index(task_id) ON DELETE SET NULL
);

CREATE INDEX IF NOT EXISTS idx_ticket_index_source ON ticket_index(source_id);
CREATE INDEX IF NOT EXISTS idx_ticket_index_run    ON ticket_index(run_id);
CREATE INDEX IF NOT EXISTS idx_ticket_index_state  ON ticket_index(state);
