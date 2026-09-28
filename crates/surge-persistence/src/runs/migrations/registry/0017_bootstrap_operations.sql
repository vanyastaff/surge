-- Bootstrap-specific intent journal. Legacy StartRun admission is unchanged.
CREATE TABLE bootstrap_operations (
    queue_sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    operation_id TEXT NOT NULL UNIQUE,
    planning_run TEXT NOT NULL UNIQUE,
    implementation_run TEXT NOT NULL UNIQUE,
    payload_version INTEGER NOT NULL CHECK(payload_version > 0),
    intent_fingerprint TEXT NOT NULL,
    capture_fingerprint TEXT NOT NULL,
    intent_json TEXT NOT NULL,
    capture_json TEXT NOT NULL,
    state_json TEXT NOT NULL,
    revision INTEGER NOT NULL DEFAULT 0 CHECK(revision >= 0),
    cancel_requested INTEGER NOT NULL DEFAULT 0 CHECK(cancel_requested IN (0, 1)),
    CHECK(planning_run != implementation_run)
);
