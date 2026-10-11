-- Human overrides of an exhausted retry ladder (v1 task 1.2): a task accepted
-- by a human is completed but never verified; a revised requirement is kept
-- visible. Projected from TaskAcceptedByHuman / RequirementRevised events.

ALTER TABLE task_ledger ADD COLUMN accepted_by_human INTEGER NOT NULL DEFAULT 0;
ALTER TABLE task_ledger ADD COLUMN requirement_revised INTEGER NOT NULL DEFAULT 0;
