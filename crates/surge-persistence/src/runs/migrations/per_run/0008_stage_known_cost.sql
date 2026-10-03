ALTER TABLE stage_executions ADD COLUMN known_cost_usd REAL;
ALTER TABLE stage_executions ADD COLUMN cost_unknown INTEGER NOT NULL DEFAULT 0;
