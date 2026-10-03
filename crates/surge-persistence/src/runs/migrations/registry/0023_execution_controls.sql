-- Extend attempt states without losing payload, claims, usage cursors or uniqueness.
DROP INDEX work_item_one_active_attempt;
DROP INDEX work_item_attempt_cursor;
CREATE TABLE work_item_attempts_next (
 run TEXT PRIMARY KEY, item TEXT NOT NULL REFERENCES work_items(id), ordinal INTEGER NOT NULL,
 generation INTEGER NOT NULL, state TEXT NOT NULL CHECK(state IN ('reserved','launched','attention','suspended','completed','failed','aborted','rejected')),
 payload TEXT NOT NULL, claim_token TEXT,
 usage_seq INTEGER NOT NULL DEFAULT 0, input_tokens INTEGER NOT NULL DEFAULT 0,
 output_tokens INTEGER NOT NULL DEFAULT 0, known_cost_usd REAL NOT NULL DEFAULT 0,
 usage_unpriced INTEGER NOT NULL DEFAULT 0, usage_unknown INTEGER NOT NULL DEFAULT 1, UNIQUE(item,ordinal), UNIQUE(item,generation)
);
INSERT INTO work_item_attempts_next SELECT * FROM work_item_attempts;
DROP TABLE work_item_attempts;
ALTER TABLE work_item_attempts_next RENAME TO work_item_attempts;
CREATE UNIQUE INDEX work_item_one_active_attempt ON work_item_attempts(item)
 WHERE state IN ('reserved','launched','attention','suspended');
CREATE INDEX work_item_attempt_cursor ON work_item_attempts(item,ordinal);
CREATE TABLE work_item_execution_controls (
 run TEXT NOT NULL REFERENCES work_item_attempts(run), generation INTEGER NOT NULL CHECK(generation>0),
 item TEXT NOT NULL REFERENCES work_items(id), attempt_generation INTEGER NOT NULL CHECK(attempt_generation>0),
 operation TEXT NOT NULL UNIQUE REFERENCES work_item_operations(operation) DEFERRABLE INITIALLY DEFERRED,
 state TEXT NOT NULL CHECK(state IN ('suspend_requested','suspended','continue_reserved','executing','attention','terminal')),
 payload TEXT NOT NULL, PRIMARY KEY(run,generation)
);
CREATE INDEX work_item_control_latest ON work_item_execution_controls(run,generation DESC);
CREATE TABLE work_item_recovery_cycles (
 run TEXT NOT NULL REFERENCES work_item_attempts(run), invocation TEXT NOT NULL,
 control_generation INTEGER NOT NULL, cycle_generation INTEGER NOT NULL CHECK(cycle_generation>0),
 attempted_candidates TEXT NOT NULL, selected_runtime TEXT, wake_at_ms INTEGER,
 PRIMARY KEY(run,invocation,cycle_generation)
);
