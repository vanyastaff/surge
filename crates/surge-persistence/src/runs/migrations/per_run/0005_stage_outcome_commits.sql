-- Replayable consumption index; the validated event journal remains authoritative.
CREATE TABLE stage_outcome_commits (
    invocation TEXT PRIMARY KEY,
    committed_seq INTEGER NOT NULL UNIQUE REFERENCES events(seq),
    node_id TEXT NOT NULL,
    outcome TEXT NOT NULL,
    routed_seq INTEGER UNIQUE REFERENCES events(seq)
);

-- Historical outcome-only events stay unbound. Read-only inspection still checks
-- typed markers' authority, adjacent effects digest and exact consumption identity.
INSERT OR IGNORE INTO stage_outcome_commits(invocation, committed_seq, node_id, outcome)
SELECT json_extract(CAST(payload AS TEXT), '$.payload.commit.invocation'), seq,
       json_extract(CAST(payload AS TEXT), '$.payload.commit.context.node'),
       json_extract(CAST(payload AS TEXT), '$.payload.commit.outcome')
FROM events WHERE kind='StageOutcomeCommitted'
 AND json_type(CAST(payload AS TEXT), '$.payload.commit.invocation')='text'
 AND json_type(CAST(payload AS TEXT), '$.payload.commit.context.node')='text'
 AND json_type(CAST(payload AS TEXT), '$.payload.commit.outcome')='text'
ORDER BY seq;

CREATE INDEX idx_stage_route_identity ON events (
    json_extract(CAST(payload AS TEXT), '$.payload.invocation'),
    json_extract(CAST(payload AS TEXT), '$.payload.outcome_commit_seq')
) WHERE kind='StageRouteCommitted';

UPDATE stage_outcome_commits SET routed_seq=(
    SELECT MIN(seq) FROM events WHERE kind='StageRouteCommitted'
     AND json_extract(CAST(payload AS TEXT), '$.payload.invocation')=stage_outcome_commits.invocation
     AND json_extract(CAST(payload AS TEXT), '$.payload.outcome_commit_seq')=stage_outcome_commits.committed_seq
);
