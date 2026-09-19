-- The project task queue (ADR-0020). `.surge/roadmap.toml` in the repository
-- is the planning truth; this table is a mirror keyed by the roadmap file's
-- hash plus the execution truth a file cannot hold: dispatch state, run id,
-- attempt, enqueue time, skipped-dispatch count.
--
-- Two column families, with different write rules:
--   planning  (priority, depends_on_json, size) — re-mirrored whenever the
--             roadmap hash changes; the file wins.
--   execution (dispatch_state, run_id, attempt, enqueued_at,
--             skipped_dispatches) — written only by the scheduler; a mirror
--             pass never touches these.
--
-- `dispatch_state` is the single-flight primitive: the scheduler dispatches
-- with `UPDATE ... SET dispatch_state='dispatched', run_id=? WHERE
-- project_root=? AND task_id=? AND dispatch_state='queued'`. Zero rows
-- affected means another tick won the race — harmless, not an error.
--
-- `roadmap_hash` is recorded at mirror time and at dispatch (on the run's
-- `RunOrigin`); comparing the two makes a priority edit that raced a dispatch
-- visible without a second table.
CREATE TABLE task_queue (
    project_root       TEXT    NOT NULL,
    task_id            TEXT    NOT NULL,
    roadmap_hash       TEXT    NOT NULL,
    priority           TEXT    NOT NULL DEFAULT 'medium',
    depends_on_json    TEXT    NOT NULL DEFAULT '[]',
    size               TEXT,
    enqueued_at        INTEGER NOT NULL,
    skipped_dispatches INTEGER NOT NULL DEFAULT 0,
    dispatch_state     TEXT    NOT NULL DEFAULT 'queued',
    run_id             TEXT,
    attempt            INTEGER NOT NULL DEFAULT 1,
    updated_at         INTEGER NOT NULL,
    PRIMARY KEY (project_root, task_id)
);

-- Scheduler scan: dispatchable rows for one project, best-effort order is
-- enforced by QueuePolicy in Rust, not by SQL (the policy is the only place
-- that orders tasks).
CREATE INDEX idx_task_queue_project_state
    ON task_queue(project_root, dispatch_state);

-- Reconciliation scan: find the row a run belongs to.
CREATE INDEX idx_task_queue_run
    ON task_queue(run_id);

-- Per-project queue state the roadmap file cannot express. One row per
-- project, created on first dispatch; `paused` stops the tick and the wake
-- path (a parked task run of a paused project is not resumed).
CREATE TABLE project_queue (
    project_root TEXT    PRIMARY KEY NOT NULL,
    paused       INTEGER NOT NULL DEFAULT 0,
    updated_at   INTEGER NOT NULL
);
