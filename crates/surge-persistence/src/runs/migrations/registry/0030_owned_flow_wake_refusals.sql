-- Informational receipt and delivery ACK are separate ownership records.
CREATE TABLE owned_flow_wake_refusals (
 key TEXT PRIMARY KEY,
 hash TEXT NOT NULL,
 body TEXT NOT NULL,
 run TEXT NOT NULL REFERENCES work_item_attempts(run),
 operation TEXT NOT NULL REFERENCES owned_flow_operations(operation),
 item TEXT NOT NULL REFERENCES work_items(id),
 attempt_generation INTEGER NOT NULL CHECK(attempt_generation>0),
 control_generation INTEGER NOT NULL CHECK(control_generation>0),
 invocation TEXT NOT NULL,
 cycle_generation INTEGER NOT NULL CHECK(cycle_generation>0),
 source_revision INTEGER NOT NULL CHECK(source_revision>0),
 wake_identity TEXT NOT NULL,
 UNIQUE(run,attempt_generation,control_generation,invocation,cycle_generation,source_revision,wake_identity),
 UNIQUE(key,hash),
 FOREIGN KEY(run,control_generation) REFERENCES work_item_execution_controls(run,generation)
);
CREATE TRIGGER owned_flow_wake_refusal_no_update BEFORE UPDATE ON owned_flow_wake_refusals
 BEGIN SELECT RAISE(ABORT,'owned flow wake refusal is immutable'); END;
CREATE TRIGGER owned_flow_wake_refusal_no_delete BEFORE DELETE ON owned_flow_wake_refusals
 BEGIN SELECT RAISE(ABORT,'owned flow wake refusal is immutable'); END;
CREATE TABLE owned_flow_wake_refusal_outbox (
 key TEXT PRIMARY KEY,
 hash TEXT NOT NULL,
 delivered_event_seq INTEGER CHECK(delivered_event_seq>0),
 FOREIGN KEY(key,hash) REFERENCES owned_flow_wake_refusals(key,hash)
);
CREATE TRIGGER owned_flow_wake_refusal_ack_identity BEFORE UPDATE ON owned_flow_wake_refusal_outbox
 WHEN OLD.key IS NOT NEW.key OR OLD.hash IS NOT NEW.hash
 OR (OLD.delivered_event_seq IS NOT NULL AND OLD.delivered_event_seq IS NOT NEW.delivered_event_seq)
 BEGIN SELECT RAISE(ABORT,'owned flow wake refusal acknowledgement is immutable'); END;
CREATE TRIGGER owned_flow_wake_refusal_ack_no_delete BEFORE DELETE ON owned_flow_wake_refusal_outbox
 BEGIN SELECT RAISE(ABORT,'owned flow wake refusal acknowledgement is immutable'); END;
