-- Cross-run mirror of the per-run task-ledger human-override columns.

ALTER TABLE task_ledger_index ADD COLUMN accepted_by_human INTEGER NOT NULL DEFAULT 0;
ALTER TABLE task_ledger_index ADD COLUMN requirement_revised INTEGER NOT NULL DEFAULT 0;
