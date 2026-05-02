CREATE TABLE events (
    seq            INTEGER PRIMARY KEY,
    timestamp      INTEGER NOT NULL,
    kind           TEXT    NOT NULL,
    payload        BLOB    NOT NULL,
    schema_version INTEGER NOT NULL DEFAULT 1
);
CREATE INDEX idx_events_kind ON events(kind);
CREATE INDEX idx_events_ts   ON events(timestamp);

CREATE TRIGGER trg_events_no_update BEFORE UPDATE ON events
BEGIN SELECT RAISE(FAIL, 'events table is append-only'); END;

CREATE TRIGGER trg_events_no_delete BEFORE DELETE ON events
BEGIN SELECT RAISE(FAIL, 'events table is append-only'); END;

CREATE TABLE stage_executions (
    node_id     TEXT    NOT NULL,
    attempt     INTEGER NOT NULL,
    started_seq INTEGER NOT NULL,
    ended_seq   INTEGER,
    started_at  INTEGER NOT NULL,
    ended_at    INTEGER,
    outcome     TEXT,
    cost_usd    REAL    DEFAULT 0,
    tokens_in   INTEGER DEFAULT 0,
    tokens_out  INTEGER DEFAULT 0,
    PRIMARY KEY(node_id, attempt)
);

CREATE TABLE artifacts (
    id                TEXT    PRIMARY KEY,
    produced_by_node  TEXT,
    produced_at_seq   INTEGER NOT NULL,
    name              TEXT    NOT NULL,
    path              TEXT    NOT NULL,
    size_bytes        INTEGER NOT NULL,
    content_hash      TEXT    NOT NULL
);
CREATE INDEX idx_artifacts_node ON artifacts(produced_by_node);
CREATE INDEX idx_artifacts_name ON artifacts(name);

CREATE TABLE pending_approvals (
    seq           INTEGER PRIMARY KEY,
    node_id       TEXT    NOT NULL,
    channel       TEXT    NOT NULL,
    requested_at  INTEGER NOT NULL,
    payload_hash  TEXT    NOT NULL,
    delivered     INTEGER DEFAULT 0,
    message_id    INTEGER
);
CREATE INDEX idx_approvals_node ON pending_approvals(node_id);

CREATE TABLE cost_summary (
    metric      TEXT PRIMARY KEY,
    value       REAL NOT NULL,
    updated_at  INTEGER NOT NULL
);

CREATE TABLE graph_snapshots (
    at_seq            INTEGER PRIMARY KEY,
    snapshot          BLOB    NOT NULL,
    bytes_compressed  INTEGER NOT NULL
);
