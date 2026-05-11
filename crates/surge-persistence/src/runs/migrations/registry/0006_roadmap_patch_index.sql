CREATE TABLE roadmap_patch_index (
    patch_id          TEXT    PRIMARY KEY,
    content_hash      TEXT    NOT NULL UNIQUE,
    run_id            TEXT,
    project_path      TEXT    NOT NULL,
    target_json       TEXT    NOT NULL,
    status            TEXT    NOT NULL,
    patch_artifact    TEXT,
    patch_path        TEXT,
    summary_hash      TEXT,
    decision          TEXT,
    decision_comment  TEXT,
    conflict_choice   TEXT,
    created_at        INTEGER NOT NULL,
    updated_at        INTEGER NOT NULL
);

CREATE INDEX idx_roadmap_patch_index_status
    ON roadmap_patch_index(status);

CREATE INDEX idx_roadmap_patch_index_project_status
    ON roadmap_patch_index(project_path, status);

CREATE INDEX idx_roadmap_patch_index_run
    ON roadmap_patch_index(run_id);

CREATE INDEX idx_roadmap_patch_index_updated
    ON roadmap_patch_index(updated_at);
