CREATE TABLE runs (
    id            TEXT    PRIMARY KEY,
    project_path  TEXT    NOT NULL,
    pipeline_template TEXT,
    status        TEXT    NOT NULL,
    started_at    INTEGER NOT NULL,
    ended_at      INTEGER,
    daemon_pid    INTEGER
);
CREATE INDEX idx_runs_status  ON runs(status);
CREATE INDEX idx_runs_started ON runs(started_at DESC);
