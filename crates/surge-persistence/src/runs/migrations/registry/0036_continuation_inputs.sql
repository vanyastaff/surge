ALTER TABLE owned_flow_operations ADD COLUMN input_purpose TEXT;
CREATE TRIGGER owned_flow_input_purpose_immutable
BEFORE UPDATE OF input_purpose ON owned_flow_operations
WHEN NEW.input_purpose IS NOT OLD.input_purpose
BEGIN SELECT RAISE(ABORT, 'owned Flow input purpose is immutable'); END;

CREATE TABLE owned_flow_continuation_requirements (
    operation TEXT PRIMARY KEY REFERENCES owned_flow_operations(operation),
    source_bootstrap TEXT NOT NULL,
    planning_run TEXT NOT NULL REFERENCES runs(id),
    source_run TEXT NOT NULL UNIQUE REFERENCES runs(id),
    intent_fingerprint TEXT NOT NULL,
    capture_fingerprint TEXT NOT NULL,
    establishment_nonce TEXT NOT NULL,
    seed_binding TEXT,
    pending_ref TEXT,
    seed_manifest TEXT,
    CHECK ((seed_binding IS NULL) = (pending_ref IS NULL)),
    CHECK (seed_manifest IS NULL OR pending_ref IS NOT NULL)
);
CREATE TRIGGER continuation_requirement_identity_immutable
BEFORE UPDATE OF operation, source_bootstrap, planning_run, source_run,
    intent_fingerprint, capture_fingerprint, establishment_nonce
ON owned_flow_continuation_requirements
BEGIN SELECT RAISE(ABORT, 'continuation source association is immutable'); END;
CREATE TRIGGER continuation_requirement_retained
BEFORE DELETE ON owned_flow_continuation_requirements
BEGIN SELECT RAISE(ABORT, 'continuation source association is retained'); END;

CREATE TABLE owned_flow_source_pins (
    operation TEXT NOT NULL REFERENCES owned_flow_operations(operation),
    source_run TEXT NOT NULL REFERENCES runs(id),
    role TEXT NOT NULL CHECK (role IN ('planning', 'implementation')),
    PRIMARY KEY (operation, role)
);
CREATE TRIGGER continuation_pin_not_tombstoned
BEFORE INSERT ON owned_flow_source_pins
WHEN EXISTS (SELECT 1 FROM run_host_tombstones WHERE run_id=NEW.source_run)
BEGIN SELECT RAISE(ABORT, 'continuation source deletion already admitted'); END;
CREATE TRIGGER continuation_pin_immutable BEFORE UPDATE ON owned_flow_source_pins
BEGIN SELECT RAISE(ABORT, 'continuation source pin is immutable'); END;
CREATE TRIGGER continuation_pin_retained BEFORE DELETE ON owned_flow_source_pins
BEGIN SELECT RAISE(ABORT, 'continuation source pin is retained'); END;

CREATE TABLE owned_flow_private_object_pins (
    operation TEXT NOT NULL REFERENCES owned_flow_operations(operation),
    object_ref TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('seed')),
    PRIMARY KEY (operation, object_ref)
);
CREATE TRIGGER continuation_object_pin_immutable BEFORE UPDATE ON owned_flow_private_object_pins
BEGIN SELECT RAISE(ABORT, 'continuation object pin is immutable'); END;
CREATE TRIGGER continuation_object_pin_retained BEFORE DELETE ON owned_flow_private_object_pins
BEGIN SELECT RAISE(ABORT, 'continuation object pin is retained'); END;

CREATE TRIGGER continuation_seed_binding_write_once
BEFORE UPDATE OF seed_binding,pending_ref ON owned_flow_continuation_requirements
WHEN (OLD.seed_binding IS NOT NULL AND NEW.seed_binding IS NOT OLD.seed_binding)
  OR (OLD.pending_ref IS NOT NULL AND NEW.pending_ref IS NOT OLD.pending_ref)
BEGIN SELECT RAISE(ABORT, 'continuation seed reservation is immutable'); END;
CREATE TRIGGER continuation_seed_manifest_write_once
BEFORE UPDATE OF seed_manifest ON owned_flow_continuation_requirements
WHEN OLD.seed_manifest IS NOT NULL AND NEW.seed_manifest IS NOT OLD.seed_manifest
BEGIN SELECT RAISE(ABORT, 'continuation seed manifest is immutable'); END;
