-- Frozen host policy and the original durable opening identify a logical stage.
CREATE TABLE work_item_quota_stages (
 run TEXT NOT NULL REFERENCES work_item_attempts(run),
 invocation TEXT NOT NULL, opening_seq INTEGER NOT NULL CHECK(opening_seq>0),
 node TEXT NOT NULL, policy_hash TEXT NOT NULL, policy TEXT NOT NULL,
 PRIMARY KEY(run,invocation)
);
-- Discoverable before stop/confirmation, independently of due-wake queries.
CREATE TABLE work_item_capacity_controls (
 sequence INTEGER PRIMARY KEY AUTOINCREMENT,
 run TEXT NOT NULL REFERENCES work_item_attempts(run),
 item TEXT NOT NULL REFERENCES work_items(id),
 attempt_generation INTEGER NOT NULL CHECK(attempt_generation>0),
 invocation TEXT NOT NULL, cycle_generation INTEGER NOT NULL CHECK(cycle_generation>0),
 source_revision INTEGER NOT NULL CHECK(source_revision>0),
 source_control INTEGER NOT NULL CHECK(source_control>=0),
 target_control INTEGER NOT NULL CHECK(target_control>source_control),
 reservation TEXT NOT NULL REFERENCES work_item_quota_candidates(receipt),
 wake_identity TEXT, reconciled INTEGER NOT NULL DEFAULT 0 CHECK(reconciled IN (0,1)),
 invalidated INTEGER NOT NULL DEFAULT 0 CHECK(invalidated IN (0,1)),
 body TEXT NOT NULL, UNIQUE(run,target_control),
 UNIQUE(run,invocation,cycle_generation,source_control),
 FOREIGN KEY(run,invocation,cycle_generation)
  REFERENCES work_item_quota_cycles(run,invocation,cycle_generation)
);
CREATE INDEX work_item_capacity_reconcile ON work_item_capacity_controls(reconciled,sequence);
-- Invocation may repeat for legitimate Resume/Load; the opening epoch cannot.
CREATE TABLE work_item_quota_handoffs (
 sequence INTEGER PRIMARY KEY AUTOINCREMENT,
 operation TEXT NOT NULL UNIQUE REFERENCES work_item_operations(operation),
 run TEXT NOT NULL REFERENCES work_item_attempts(run),
 item TEXT NOT NULL REFERENCES work_items(id),
 attempt_generation INTEGER NOT NULL CHECK(attempt_generation>0),
 logical_invocation TEXT NOT NULL,
 source_cycle INTEGER NOT NULL CHECK(source_cycle>0),
 source_control INTEGER NOT NULL CHECK(source_control>=0),
 wake_identity TEXT,
 target_cycle INTEGER NOT NULL CHECK(target_cycle>0),
 target_control INTEGER NOT NULL CHECK(target_control>=0),
 reservation TEXT NOT NULL UNIQUE REFERENCES work_item_quota_candidates(receipt),
 provider_invocation TEXT NOT NULL, launch_hash TEXT NOT NULL,
 state TEXT NOT NULL CHECK(state IN ('reserved','opening_unknown','established','executing','invalidated','attention')),
 body_hash TEXT NOT NULL, body TEXT NOT NULL,
 opened_seq INTEGER CHECK(opened_seq>0), internal_session TEXT,
 prompt_authorization_seq INTEGER CHECK(prompt_authorization_seq>0), diagnostic TEXT,
 FOREIGN KEY(run,logical_invocation,target_cycle)
  REFERENCES work_item_quota_cycles(run,invocation,cycle_generation)
);
CREATE UNIQUE INDEX work_item_quota_wake_grant
 ON work_item_quota_handoffs(run,logical_invocation,source_cycle,source_control,wake_identity)
 WHERE wake_identity IS NOT NULL;
CREATE INDEX work_item_quota_handoff_reconcile ON work_item_quota_handoffs(state,sequence);
