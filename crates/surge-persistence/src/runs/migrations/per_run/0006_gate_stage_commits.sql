-- Host decision effects and their route consumption. The trusted journal reader
-- validates request/answer authority before this index can authorize recovery.
CREATE TABLE gate_stage_commits (
    request_id TEXT NOT NULL,
    stage_entry_seq INTEGER NOT NULL REFERENCES events(seq),
    committed_seq INTEGER NOT NULL UNIQUE REFERENCES events(seq),
    node_id TEXT NOT NULL,
    outcome TEXT NOT NULL,
    disposition TEXT NOT NULL,
    routed_seq INTEGER UNIQUE REFERENCES events(seq),
    PRIMARY KEY (request_id, stage_entry_seq)
);

CREATE INDEX idx_gate_stage_unconsumed ON gate_stage_commits(committed_seq)
    WHERE routed_seq IS NULL;
