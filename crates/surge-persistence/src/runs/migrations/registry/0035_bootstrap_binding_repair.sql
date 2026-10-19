-- Retained migration history: this ID was applied before the unused generated-graph
-- repair feature was removed. Keep the issued additive schema stable and do not
-- reuse its ID. Runtime attempt reads/writes ignore this nullable column; inserts
-- name the original seven columns so both schema shapes remain compatible.
ALTER TABLE bootstrap_attempts ADD COLUMN binding_repair_json TEXT;
CREATE TRIGGER bootstrap_attempt_repair_immutable BEFORE UPDATE OF binding_repair_json ON bootstrap_attempts
BEGIN SELECT RAISE(ABORT, 'bootstrap continuation repair identity is immutable'); END;
