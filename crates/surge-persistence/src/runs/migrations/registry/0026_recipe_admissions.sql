-- Admission precedes every comparable provider effect. No historical backfill.
CREATE TABLE recipe_opening_admissions (
    epoch INTEGER PRIMARY KEY AUTOINCREMENT,
    execution_writer TEXT NOT NULL UNIQUE,
    invocation TEXT NOT NULL,
    runtime TEXT NOT NULL,
    launch_hash TEXT NOT NULL,
    exhaustion_receipt TEXT
);
CREATE INDEX recipe_opening_latest ON recipe_opening_admissions(runtime, launch_hash, epoch DESC);
