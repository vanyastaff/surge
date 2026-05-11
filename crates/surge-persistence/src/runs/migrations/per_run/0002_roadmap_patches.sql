CREATE TABLE roadmap_patches (
    patch_id                  TEXT    PRIMARY KEY,
    target_json               TEXT    NOT NULL,
    status                    TEXT    NOT NULL,
    patch_artifact            TEXT,
    patch_path                TEXT,
    summary_hash              TEXT,
    decision                  TEXT,
    decision_comment          TEXT,
    conflict_choice           TEXT,
    amended_roadmap_artifact  TEXT,
    amended_roadmap_path      TEXT,
    amended_flow_artifact     TEXT,
    amended_flow_path         TEXT,
    roadmap_artifact          TEXT,
    roadmap_path              TEXT,
    flow_artifact             TEXT,
    flow_path                 TEXT,
    active_pickup             TEXT,
    created_seq               INTEGER NOT NULL,
    updated_seq               INTEGER NOT NULL,
    created_at                INTEGER NOT NULL,
    updated_at                INTEGER NOT NULL
);

CREATE INDEX idx_roadmap_patches_status ON roadmap_patches(status);
CREATE INDEX idx_roadmap_patches_updated ON roadmap_patches(updated_seq);
