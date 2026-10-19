ALTER TABLE bootstrap_attempts ADD COLUMN binding_repair_json TEXT;
CREATE TRIGGER bootstrap_attempt_repair_immutable BEFORE UPDATE OF binding_repair_json ON bootstrap_attempts
BEGIN SELECT RAISE(ABORT, 'bootstrap continuation repair identity is immutable'); END;
