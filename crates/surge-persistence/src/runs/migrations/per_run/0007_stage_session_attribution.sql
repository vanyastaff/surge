ALTER TABLE stage_executions ADD COLUMN session_id TEXT;
CREATE INDEX idx_stage_executions_session ON stage_executions(session_id);
