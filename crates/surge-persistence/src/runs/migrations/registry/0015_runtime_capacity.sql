-- Task 12 M2 — durable rate-limit capacity observation, keyed by the
-- canonical agent-runtime registry id (`Registry::normalize_agent_id`'s
-- result), never a raw `agent_id` and never an "account" (there is no
-- representable account key today — see `surge_core::capacity`'s module
-- doc, "Why the key is the runtime"). `claude`, `claude-code`, and
-- `claude-acp` all resolve to the same runtime and therefore the same row.
--
-- One row per runtime: a capacity window is a point sample of "the last
-- thing Surge saw", not a history, so `runs::capacity::observe` upserts in
-- place rather than appending.
--
-- Columns mirror `surge_core::capacity::CapacityWindow` field-for-field.
-- Nullable exactly where that type's fields are `Option` (`remaining`,
-- `resets_at_ms`, `window_secs`) and nowhere else: every field Surge learns
-- only by observation is nullable, and `runtime`/`source` (always known at
-- write time) are not. No TTL and no default: a `resets_at_ms` that has
-- already elapsed is not deleted or reset here — staleness is already
-- representable one layer up (`CapacityWindow::seconds_until_reset` returns
-- `None` once `resets_at` is in the past), so this schema does not invent a
-- second mechanism for the same fact.
--
-- Deliberately no `observed_at_ms` column (review finding, M2): nothing in
-- this workspace reads it -- `runs::capacity::status` doesn't project it,
-- and no other query selects it -- while every `observe` call would have
-- to invent a value for it. Add it back later, as one additive migration,
-- if a real reader shows up; until then it is a column nobody can honestly
-- claim to need, which is exactly what this module's own philosophy
-- (observe only what has an actual consumer, never speculatively) argues
-- against carrying.
CREATE TABLE runtime_capacity (
    runtime      TEXT    PRIMARY KEY NOT NULL,
    remaining    REAL,
    resets_at_ms INTEGER,
    window_secs  INTEGER,
    source       TEXT    NOT NULL
);
