CREATE TABLE IF NOT EXISTS work_item_projects (
 id TEXT PRIMARY KEY, repository TEXT NOT NULL UNIQUE, checkout TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS work_items (
 id TEXT PRIMARY KEY, project_id TEXT NOT NULL REFERENCES work_item_projects(id),
 title TEXT NOT NULL, accepted_revision INTEGER NOT NULL CHECK(accepted_revision>0),
 version INTEGER NOT NULL CHECK(version>0), archived_at_ms INTEGER,
 workspace TEXT NOT NULL, workspace_prepared INTEGER NOT NULL DEFAULT 0, active_run TEXT UNIQUE, generation INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS work_item_revisions (
 item TEXT NOT NULL REFERENCES work_items(id), revision INTEGER NOT NULL CHECK(revision>0),
 payload TEXT NOT NULL, PRIMARY KEY(item,revision)
);
CREATE TRIGGER IF NOT EXISTS work_item_revision_immutable BEFORE UPDATE ON work_item_revisions
 BEGIN SELECT RAISE(ABORT,'accepted requirement revisions are immutable'); END;
CREATE TABLE IF NOT EXISTS work_item_discussion (
 item TEXT NOT NULL REFERENCES work_items(id), sequence INTEGER NOT NULL, payload TEXT NOT NULL,
 PRIMARY KEY(item,sequence)
);
CREATE TABLE IF NOT EXISTS work_item_pr (
 item TEXT PRIMARY KEY REFERENCES work_items(id), payload TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS work_item_attempts (
 run TEXT PRIMARY KEY, item TEXT NOT NULL REFERENCES work_items(id), ordinal INTEGER NOT NULL,
 generation INTEGER NOT NULL, state TEXT NOT NULL CHECK(state IN ('reserved','launched','attention','completed','failed','aborted','rejected')),
 payload TEXT NOT NULL, claim_token TEXT,
 usage_seq INTEGER NOT NULL DEFAULT 0, input_tokens INTEGER NOT NULL DEFAULT 0,
 output_tokens INTEGER NOT NULL DEFAULT 0, known_cost_usd REAL NOT NULL DEFAULT 0,
 usage_unpriced INTEGER NOT NULL DEFAULT 0, usage_unknown INTEGER NOT NULL DEFAULT 1, UNIQUE(item,ordinal), UNIQUE(item,generation)
);
CREATE UNIQUE INDEX IF NOT EXISTS work_item_one_active_attempt ON work_item_attempts(item)
 WHERE state IN ('reserved','launched','attention');
CREATE INDEX IF NOT EXISTS work_item_attempt_cursor ON work_item_attempts(item,ordinal);
CREATE TABLE IF NOT EXISTS work_item_operations (
 operation TEXT PRIMARY KEY, body_hash TEXT NOT NULL, result TEXT NOT NULL
);
