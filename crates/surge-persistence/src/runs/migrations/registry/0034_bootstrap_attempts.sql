CREATE TABLE bootstrap_attempts (
    continuation_id TEXT PRIMARY KEY NOT NULL,
    operation_id TEXT NOT NULL REFERENCES bootstrap_operations(operation_id),
    source_run TEXT UNIQUE NOT NULL,
    run TEXT UNIQUE NOT NULL,
    boundary INTEGER NOT NULL CHECK(boundary > 0),
    runtime TEXT NOT NULL,
    state_json TEXT NOT NULL
);
CREATE TRIGGER bootstrap_attempt_identity_immutable BEFORE UPDATE OF
    continuation_id, operation_id, source_run, run, boundary, runtime ON bootstrap_attempts
BEGIN SELECT RAISE(ABORT, 'bootstrap continuation identity is immutable'); END;
