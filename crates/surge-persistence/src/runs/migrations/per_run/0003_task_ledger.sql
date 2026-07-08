-- Per-run task-ledger materialized view.
--
-- Maintained inside the same transaction as the event append (see
-- runs::views::maintain). Rebuildable from the event log if dropped: it is a
-- projection of TaskStatusChanged / TaskDiscovered / TaskVerified events, the
-- same shape as run_state::LedgerState.

CREATE TABLE task_ledger (
    task_id            TEXT    PRIMARY KEY,
    status             TEXT    NOT NULL,
    verified           INTEGER NOT NULL DEFAULT 0,
    discovered_from    TEXT,
    last_authority_node TEXT,
    updated_seq        INTEGER NOT NULL
);

CREATE INDEX idx_task_ledger_status ON task_ledger(status);
CREATE INDEX idx_task_ledger_verified ON task_ledger(verified);
CREATE INDEX idx_task_ledger_discovered_from ON task_ledger(discovered_from);
