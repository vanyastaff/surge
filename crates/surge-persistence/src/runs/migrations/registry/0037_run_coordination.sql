-- Immutable coordination originates before the first new run namespace.
CREATE TABLE run_coordination_origins (
    run_id TEXT PRIMARY KEY,
    kind TEXT NOT NULL CHECK(kind IN ('Legacy','NewControlled')),
    creation_nonce TEXT,
    CHECK((kind='Legacy' AND creation_nonce IS NULL) OR (kind='NewControlled' AND length(creation_nonce)>0))
);
INSERT INTO run_coordination_origins(run_id,kind)
SELECT id,'Legacy' FROM runs UNION SELECT run_id,'Legacy' FROM run_host_openings;
CREATE TRIGGER run_coordination_no_update BEFORE UPDATE ON run_coordination_origins BEGIN SELECT RAISE(ABORT,'immutable coordination origin'); END;
CREATE TRIGGER run_coordination_no_delete BEFORE DELETE ON run_coordination_origins BEGIN SELECT RAISE(ABORT,'permanent coordination origin'); END;
CREATE TABLE run_host_controls (
    run_id TEXT PRIMARY KEY REFERENCES run_coordination_origins(run_id),
    creation_nonce TEXT NOT NULL,
    identity_version INTEGER NOT NULL CHECK(identity_version=1),
    home_identity BLOB NOT NULL CHECK(length(home_identity) IN (16,24)),
    parent_identity BLOB NOT NULL CHECK(length(parent_identity) IN (16,24)),
    file_identity BLOB NOT NULL CHECK(length(file_identity) IN (16,24))
);
CREATE TRIGGER run_controls_origin BEFORE INSERT ON run_host_controls
WHEN NOT EXISTS(SELECT 1 FROM run_coordination_origins WHERE run_id=NEW.run_id AND kind='NewControlled' AND creation_nonce=NEW.creation_nonce)
BEGIN SELECT RAISE(ABORT,'control origin mismatch'); END;
CREATE TRIGGER run_controls_no_update BEFORE UPDATE ON run_host_controls BEGIN SELECT RAISE(ABORT,'immutable run control'); END;
CREATE TRIGGER run_controls_no_delete BEFORE DELETE ON run_host_controls BEGIN SELECT RAISE(ABORT,'permanent run control'); END;
ALTER TABLE run_host_openings ADD COLUMN coordination_origin TEXT REFERENCES run_coordination_origins(run_id);
UPDATE run_host_openings SET coordination_origin=run_id;
CREATE TRIGGER run_opening_origin_insert BEFORE INSERT ON run_host_openings
WHEN NEW.coordination_origin IS NULL OR NEW.coordination_origin!=NEW.run_id
BEGIN SELECT RAISE(ABORT,'opening requires coordination origin'); END;
CREATE TRIGGER run_opening_origin_update BEFORE UPDATE OF coordination_origin ON run_host_openings
WHEN NEW.coordination_origin IS NOT OLD.coordination_origin
BEGIN SELECT RAISE(ABORT,'immutable opening origin'); END;
