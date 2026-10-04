-- Provisional operations remain durable even when preparation dies. No private
-- transports occur in these public snapshots; private objects are immutable files.
CREATE TABLE owned_flow_operations (
 operation TEXT PRIMARY KEY, request_identity TEXT NOT NULL,
 item TEXT NOT NULL UNIQUE, run TEXT NOT NULL UNIQUE, workspace_owner TEXT NOT NULL UNIQUE,
 operation_lock_identity TEXT NOT NULL, launch_lock_identity TEXT,
 token TEXT NOT NULL, state TEXT NOT NULL CHECK(state IN ('preparing','abandoned','accepted')),
 source_snapshot TEXT, config_snapshot TEXT, inputs_manifest TEXT,
 receipt TEXT, intent_state TEXT CHECK(intent_state IN ('pending','started','canceled')),
 created_at_ms INTEGER NOT NULL,
 CHECK((state='accepted')=(receipt IS NOT NULL)),
 CHECK((receipt IS NOT NULL)=(intent_state IS NOT NULL))
);
CREATE TRIGGER owned_flow_first_identity_immutable BEFORE UPDATE ON owned_flow_operations
 WHEN OLD.operation != NEW.operation OR OLD.request_identity != NEW.request_identity
 OR OLD.item != NEW.item OR OLD.run != NEW.run OR OLD.workspace_owner != NEW.workspace_owner
 OR OLD.operation_lock_identity != NEW.operation_lock_identity
 OR (OLD.launch_lock_identity IS NOT NULL AND OLD.launch_lock_identity IS NOT NEW.launch_lock_identity)
 OR (OLD.source_snapshot IS NOT NULL AND OLD.source_snapshot IS NOT NEW.source_snapshot)
 OR (OLD.config_snapshot IS NOT NULL AND OLD.config_snapshot IS NOT NEW.config_snapshot)
 OR (OLD.inputs_manifest IS NOT NULL AND OLD.inputs_manifest IS NOT NEW.inputs_manifest)
 OR (OLD.receipt IS NOT NULL AND OLD.receipt IS NOT NEW.receipt)
 BEGIN SELECT RAISE(ABORT,'owned flow first snapshot is immutable'); END;
