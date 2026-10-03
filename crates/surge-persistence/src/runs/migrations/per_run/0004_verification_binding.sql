CREATE TABLE verification_context (
    singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
    subject_json TEXT,
    graph_json TEXT,
    updated_seq INTEGER NOT NULL DEFAULT 0
);
INSERT INTO verification_context(singleton) VALUES (1);
CREATE TABLE verification_criteria (
    task_id TEXT PRIMARY KEY,
    criteria_json TEXT NOT NULL,
    updated_seq INTEGER NOT NULL
);
CREATE TABLE verification_proofs (
    task_id TEXT PRIMARY KEY,
    evidence TEXT NOT NULL,
    report_path TEXT NOT NULL,
    binding_json TEXT NOT NULL
);
-- Earlier journals did not bind verification to a revision or criteria epoch.
UPDATE task_ledger SET verified=0;
