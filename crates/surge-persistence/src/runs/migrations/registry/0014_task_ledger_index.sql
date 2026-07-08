-- Registry-level task-ledger index for cross-run CLI queries (surge ready /
-- surge ledger). Per-run event logs remain the source of truth; this mirrors
-- each run's ledger so the CLI can answer "what is unblocked" without scanning
-- every run database.

CREATE TABLE task_ledger_index (
    run_id              TEXT    NOT NULL,
    task_id             TEXT    NOT NULL,
    project_path        TEXT    NOT NULL,
    status              TEXT    NOT NULL,
    verified            INTEGER NOT NULL DEFAULT 0,
    discovered_from     TEXT,
    last_authority_node TEXT,
    updated_seq         INTEGER NOT NULL,
    updated_at          INTEGER NOT NULL,
    PRIMARY KEY (run_id, task_id)
);

CREATE INDEX idx_task_ledger_index_status
    ON task_ledger_index(status);

CREATE INDEX idx_task_ledger_index_project_status
    ON task_ledger_index(project_path, status);

CREATE INDEX idx_task_ledger_index_run
    ON task_ledger_index(run_id);

CREATE INDEX idx_task_ledger_index_discovered
    ON task_ledger_index(discovered_from);
