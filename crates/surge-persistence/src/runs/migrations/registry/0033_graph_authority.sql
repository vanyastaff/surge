-- A mandatory, non-defaulted column rejects queued legacy INSERT/UPSERT
-- before ON CONFLICT can restore an obsolete verified positive.
CREATE TABLE task_ledger_index_current (
    run_id              TEXT    NOT NULL,
    task_id             TEXT    NOT NULL,
    project_path        TEXT    NOT NULL,
    status              TEXT    NOT NULL,
    verified            INTEGER NOT NULL DEFAULT 0,
    discovered_from     TEXT,
    last_authority_node TEXT,
    updated_seq         INTEGER NOT NULL,
    updated_at          INTEGER NOT NULL,
    accepted_by_human   INTEGER NOT NULL DEFAULT 0,
    requirement_revised INTEGER NOT NULL DEFAULT 0,
    projection_semantics INTEGER NOT NULL CHECK(projection_semantics=1),
    PRIMARY KEY (run_id, task_id)
);
INSERT INTO task_ledger_index_current
    (run_id,task_id,project_path,status,verified,discovered_from,last_authority_node,
     updated_seq,updated_at,accepted_by_human,requirement_revised,projection_semantics)
SELECT run_id,task_id,project_path,status,0,discovered_from,last_authority_node,
       updated_seq,updated_at,accepted_by_human,requirement_revised,1
FROM task_ledger_index;
DROP TABLE task_ledger_index;
ALTER TABLE task_ledger_index_current RENAME TO task_ledger_index;
CREATE INDEX idx_task_ledger_index_status ON task_ledger_index(status);
CREATE INDEX idx_task_ledger_index_project_status ON task_ledger_index(project_path,status);
CREATE INDEX idx_task_ledger_index_run ON task_ledger_index(run_id);
CREATE INDEX idx_task_ledger_index_discovered ON task_ledger_index(discovered_from);
