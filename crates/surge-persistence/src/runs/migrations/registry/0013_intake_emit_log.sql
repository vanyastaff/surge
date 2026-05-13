-- 0013_intake_emit_log.sql
--
-- Per-side-effect idempotency log for outbound intake actions
-- (tracker comments, label changes, merge proposals). Every outbound
-- action checks `has(source_id, task_id, event_kind, run_id)` before
-- emitting; the call site skips when the row already exists.
--
-- This is layered on top of any per-source idempotency the comment
-- poster already performs (GitHub exact-body match, Linear
-- idempotency keys). The emit-log catches retries that survive
-- daemon restarts and survive across multiple side-effect channels.
--
-- `event_kind` values:
--   - `triage_decision` — InboxCard / Duplicate / OOS / Unclear comment
--   - `run_started`     — "Surge run #X started" comment
--   - `run_completed`   — completion comment with PR link
--   - `run_failed`      — failure comment with stage / reason
--   - `run_aborted`     — abort comment (user-cancel, external close)
--   - `merge_proposed`  — L3 auto-merge action posted to tracker
--   - `merge_blocked`   — L3 merge gate blocked (red checks / no review)
--
-- The PK is the dedup key. `INSERT OR IGNORE` is the recommended
-- emission idiom: if the row already exists, the insert is a silent
-- no-op and the call site decides whether to skip the side-effect.

CREATE TABLE IF NOT EXISTS intake_emit_log (
    source_id     TEXT    NOT NULL,
    task_id       TEXT    NOT NULL,
    event_kind    TEXT    NOT NULL,
    run_id        TEXT    NOT NULL,
    recorded_at   INTEGER NOT NULL,    -- Unix epoch ms
    PRIMARY KEY (source_id, task_id, event_kind, run_id)
);

CREATE INDEX IF NOT EXISTS idx_intake_emit_log_recent
    ON intake_emit_log(recorded_at DESC);
