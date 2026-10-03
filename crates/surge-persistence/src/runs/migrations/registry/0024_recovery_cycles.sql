-- The early placeholder has no trustworthy probe/account/ownership evidence.
-- Preserve every row for audit, but never import it into actionable recovery.
ALTER TABLE work_item_recovery_cycles RENAME TO work_item_recovery_cycles_legacy;
CREATE TABLE work_item_quota_cycles (
 run TEXT NOT NULL REFERENCES work_item_attempts(run), invocation TEXT NOT NULL,
 cycle_generation INTEGER NOT NULL CHECK(cycle_generation > 0),
 item TEXT NOT NULL REFERENCES work_items(id), attempt_generation INTEGER NOT NULL CHECK(attempt_generation > 0),
 control_generation INTEGER NOT NULL CHECK(control_generation >= 0), revision INTEGER NOT NULL CHECK(revision > 0 AND typeof(revision)='integer'),
 closed INTEGER NOT NULL DEFAULT 0 CHECK(closed IN (0,1)), selected_runtime TEXT,
 probe_count INTEGER NOT NULL DEFAULT 0 CHECK(probe_count >= 0 AND typeof(probe_count)='integer'),
 wake TEXT, wake_at_ms INTEGER, PRIMARY KEY(run, invocation, cycle_generation)
);
CREATE UNIQUE INDEX work_item_quota_open ON work_item_quota_cycles(run,invocation) WHERE closed=0;
CREATE INDEX work_item_quota_due ON work_item_quota_cycles(wake_at_ms,run,invocation) WHERE closed=0 AND wake_at_ms IS NOT NULL;
CREATE TABLE work_item_quota_candidates (
 run TEXT NOT NULL, invocation TEXT NOT NULL, cycle_generation INTEGER NOT NULL,
 candidate_key TEXT NOT NULL, runtime TEXT NOT NULL, receipt TEXT NOT NULL UNIQUE, body TEXT NOT NULL, observation TEXT, exhaustion TEXT, observed TEXT, typed_exhaustion TEXT,
 PRIMARY KEY(run,invocation,cycle_generation,candidate_key),
 UNIQUE(run,invocation,cycle_generation,runtime),
 FOREIGN KEY(run,invocation,cycle_generation) REFERENCES work_item_quota_cycles(run,invocation,cycle_generation)
);
