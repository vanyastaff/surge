-- Task 12 M2 — when a Parked run (`RunStatus::Parked`) is expected to
-- resume on its own. Additive column: NULL for every existing row and for
-- any run that never parks; old binaries that do not know this column
-- ignore it (see `migrations.rs`'s forward-only, additive-by-default
-- design). Written only by `registry::set_run_parked`, alongside the status
-- transition to `parked` in the same statement.
ALTER TABLE runs ADD COLUMN wake_at INTEGER;
