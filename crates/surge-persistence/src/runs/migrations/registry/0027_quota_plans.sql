-- Preserve original opening bindings without fabricating provider events for plans.
ALTER TABLE work_item_quota_stages RENAME TO work_item_quota_stages_original;
CREATE TABLE work_item_quota_stages (
 run TEXT NOT NULL REFERENCES work_item_attempts(run), invocation TEXT NOT NULL,
 opening_seq INTEGER CHECK(opening_seq>0), plan_seq INTEGER CHECK(plan_seq>0),
 node TEXT NOT NULL, policy_hash TEXT NOT NULL, policy TEXT NOT NULL,
 CHECK((opening_seq IS NULL)!=(plan_seq IS NULL)), PRIMARY KEY(run,invocation)
);
INSERT INTO work_item_quota_stages(run,invocation,opening_seq,node,policy_hash,policy)
 SELECT run,invocation,opening_seq,node,policy_hash,policy FROM work_item_quota_stages_original;
DROP TABLE work_item_quota_stages_original;
CREATE TABLE work_item_capacity_skips (
 run TEXT NOT NULL, invocation TEXT NOT NULL, cycle_generation INTEGER NOT NULL,
 runtime TEXT NOT NULL, source_receipt TEXT NOT NULL, source_epoch INTEGER NOT NULL,
 checked_at_ms INTEGER NOT NULL, candidate TEXT NOT NULL,
 PRIMARY KEY(run,invocation,cycle_generation,runtime),
 FOREIGN KEY(run,invocation,cycle_generation) REFERENCES work_item_quota_cycles(run,invocation,cycle_generation)
);

ALTER TABLE recipe_opening_admissions ADD COLUMN configured_pin TEXT;
-- A no-provider exhaustion proof is a distinct capacity source, never a candidate attempt.
CREATE TABLE work_item_capacity_plan_proofs (
 receipt TEXT PRIMARY KEY, run TEXT NOT NULL, invocation TEXT NOT NULL,
 cycle_generation INTEGER NOT NULL, plan_seq INTEGER NOT NULL CHECK(plan_seq>0),
 body TEXT NOT NULL,
 FOREIGN KEY(run,invocation,cycle_generation) REFERENCES work_item_quota_cycles(run,invocation,cycle_generation)
);
ALTER TABLE work_item_capacity_controls RENAME TO work_item_capacity_controls_original;
CREATE TABLE work_item_capacity_controls (
 sequence INTEGER PRIMARY KEY AUTOINCREMENT,
 run TEXT NOT NULL REFERENCES work_item_attempts(run), item TEXT NOT NULL REFERENCES work_items(id),
 attempt_generation INTEGER NOT NULL CHECK(attempt_generation>0), invocation TEXT NOT NULL,
 cycle_generation INTEGER NOT NULL CHECK(cycle_generation>0), source_revision INTEGER NOT NULL CHECK(source_revision>0),
 source_control INTEGER NOT NULL CHECK(source_control>=0), target_control INTEGER NOT NULL CHECK(target_control>source_control),
 reservation TEXT REFERENCES work_item_quota_candidates(receipt),
 planned_receipt TEXT REFERENCES work_item_capacity_plan_proofs(receipt),
 wake_identity TEXT, reconciled INTEGER NOT NULL DEFAULT 0 CHECK(reconciled IN(0,1)),
 invalidated INTEGER NOT NULL DEFAULT 0 CHECK(invalidated IN(0,1)), body TEXT NOT NULL,
 CHECK((reservation IS NULL)!=(planned_receipt IS NULL)), UNIQUE(run,target_control),
 UNIQUE(run,invocation,cycle_generation,source_control),
 FOREIGN KEY(run,invocation,cycle_generation) REFERENCES work_item_quota_cycles(run,invocation,cycle_generation)
);
INSERT INTO work_item_capacity_controls(sequence,run,item,attempt_generation,invocation,cycle_generation,source_revision,source_control,target_control,reservation,wake_identity,reconciled,invalidated,body)
 SELECT sequence,run,item,attempt_generation,invocation,cycle_generation,source_revision,source_control,target_control,reservation,wake_identity,reconciled,invalidated,body FROM work_item_capacity_controls_original;
DROP TABLE work_item_capacity_controls_original;
CREATE INDEX work_item_capacity_reconcile ON work_item_capacity_controls(reconciled,sequence);
