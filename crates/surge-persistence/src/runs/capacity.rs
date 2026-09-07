//! Durable rate-limit capacity observation — the registry-level store
//! behind `surge_core::capacity`'s pure model (Task 12, R34–R38.1).
//!
//! Schema: see `migrations/registry/0015_runtime_capacity.sql`. One row per
//! canonical agent-runtime registry id (`Registry::normalize_agent_id`'s
//! result — see that module's doc, "Why the key is the runtime"): `claude`,
//! `claude-code`, and `claude-acp` all collapse to the same row because the
//! caller is expected to have already normalized before it gets here — this
//! store does not normalize, it only keys on whatever string it is given,
//! and the table's `PRIMARY KEY` is what makes repeated observations of the
//! same runtime collapse to one row rather than accumulate.
//!
//! This is *why* the model exists at all (see the module doc on
//! `surge_core::capacity`): the engine path's only per-process record of a
//! 429 is an in-memory `HealthTracker` that dies with the process. A fresh
//! process asking "is this runtime exhausted" before its first dispatch
//! needs an answer that outlives the process that observed the failure —
//! this table is that answer.

use chrono::DateTime;
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::types::Value;
use rusqlite::{OptionalExtension, params};
use surge_core::capacity::{CapacitySource, CapacityStatus, CapacityWindow, RemainingShare};

use crate::runs::error::StorageError;

/// Record (or replace) the durable capacity observation for one runtime.
///
/// `window.runtime()` must already be the canonical registry id — this
/// store does not call `Registry::normalize_agent_id` itself (it cannot:
/// `surge-persistence` does not depend on `surge-acp`, and `surge-core`'s
/// capacity model is deliberately I/O- and registry-free — see that
/// module's doc). Normalizing is the caller's job, same as it already is
/// for `StageError::RateLimited.runtime` (Task 12 M0/M1).
///
/// **This fails open, not closed, if the caller gets that wrong.** An
/// unnormalized `window.runtime()` (e.g. `"claude"` when every prior
/// observation was written under `"claude-acp"`) does not error here — it
/// silently writes (or reads back as) a *second* row. A caller that then
/// asks [`status`] for the un-normalized spelling before its next dispatch
/// gets [`CapacityStatus::NeverObserved`] for a runtime that is, in truth,
/// exhausted, and dispatches into it — the one failure mode R37 exists to
/// prevent. Newtyping the canonical id so this cannot type-check is tracked
/// as a required M3 acceptance criterion (the call site that would resolve
/// it lives in the engine, not here); until then, correctness depends on
/// every caller normalizing before it reaches this function.
///
/// Overwrites any previous observation for the same runtime in place: a
/// capacity window is a point sample of "the last thing Surge saw", not a
/// history, so there is one row per runtime, not one row per observation.
///
/// # Errors
/// Returns [`StorageError`] when the registry DB cannot be reached or the
/// write fails.
pub fn observe(
    pool: &Pool<SqliteConnectionManager>,
    window: &CapacityWindow,
) -> Result<(), StorageError> {
    let conn = pool.get().map_err(|e| StorageError::Pool(e.to_string()))?;
    conn.execute(
        "INSERT INTO runtime_capacity
            (runtime, remaining, resets_at_ms, window_secs, source)
         VALUES (?, ?, ?, ?, ?)
         ON CONFLICT(runtime) DO UPDATE SET
            remaining = excluded.remaining,
            resets_at_ms = excluded.resets_at_ms,
            window_secs = excluded.window_secs,
            source = excluded.source",
        params![
            window.runtime(),
            window.remaining().map(RemainingShare::get),
            window.resets_at().map(|dt| dt.timestamp_millis()),
            // `Duration::as_secs` is `u64`; the column is a signed SQLite
            // INTEGER. Never truncate silently (review finding) — a
            // `Duration` this large cannot come from anything this crate's
            // own `CapacityWindow::with_learned_window` produces (it is
            // bounded by the gap between two `chrono::DateTime<Utc>`
            // values, itself bounded to roughly +/-262,000 years, many
            // orders of magnitude under `i64::MAX` seconds), but
            // `CapacityWindow::from_parts` (Task 12 M2) is a second,
            // caller-supplied constructor with no such bound. Writing NULL
            // for a window this crate cannot honestly represent is the
            // same "do not fabricate" discipline `RemainingShare` already
            // applies on read; it is not reachable through any real caller
            // today, but `i64::try_from` mirrors the read side
            // (`status` below) exactly, at zero behavioral cost for every
            // real value.
            window
                .window()
                .and_then(|d| i64::try_from(d.as_secs()).ok()),
            window.source().as_str(),
        ],
    )?;
    Ok(())
}

/// Clear any durable exhaustion record for `runtime`. Idempotent — a
/// runtime with no row is a no-op, not an error.
///
/// # Why this exists (Task 12 M3 review, BLOCKING #1)
///
/// A `runtime_capacity` row otherwise lives forever: nothing before this
/// function ever deleted one, and `observe`'s only real caller is the
/// post-429 path in `surge-orchestrator::engine::run_task` — meaning the
/// row can only ever be *refreshed* by a genuine dispatch attempt. For an
/// exhausted window with no learned reset time (`resets_at: None`), the
/// caller's own `CapacityPolicy::decide` parks on `blind_backoff` every
/// single time it is consulted — with no attempt ever happening to refresh
/// (or refute) that stale conclusion, a run can re-park on the same
/// unrefreshed row forever. Calling this once a real, non-rate-limited
/// dispatch on `runtime` completes breaks that cycle: the outcome (success
/// or a non-capacity failure) is itself proof the runtime is not currently
/// exhausted, so the stale row is deleted rather than left to keep
/// answering `Known(exhausted)` to every future read.
///
/// # Errors
/// Returns [`StorageError`] when the registry DB cannot be reached.
pub fn clear(pool: &Pool<SqliteConnectionManager>, runtime: &str) -> Result<(), StorageError> {
    let conn = pool.get().map_err(|e| StorageError::Pool(e.to_string()))?;
    conn.execute(
        "DELETE FROM runtime_capacity WHERE runtime = ?",
        params![runtime],
    )?;
    Ok(())
}

/// Point-read the durable capacity status for one canonical agent-runtime
/// id. Never writes.
///
/// - No row for `runtime` → [`CapacityStatus::NeverObserved`] — nobody has
///   called [`observe`] for this runtime: a fresh install, or a run whose
///   profile never resolved to a registry entry at all (the legacy
///   no-profile-registry path — Task 12 plan point 10 — never calls
///   `observe` in the first place, precisely so this reads as
///   `NeverObserved` rather than a fabricated row).
/// - A row exists but a column holds a value that cannot become its typed
///   form — `remaining` outside `RemainingShare::new`'s `[0.0, 1.0]` range
///   *or* not a number at all (SQLite columns are typed by affinity, not
///   constraint: `INSERT INTO runtime_capacity (...) VALUES ('abc', ...)`
///   stores `remaining` as genuine `TEXT`, verified directly), `source` not
///   a recognized label, or `resets_at_ms`/`window_secs` not an integer or
///   outside what their target types can represent — →
///   [`CapacityStatus::Unclassified`] — **never**
///   [`CapacityStatus::NeverObserved`]. Those mean different things (see
///   that type's doc): a decode failure is evidence Surge saw *something*
///   it cannot read, not evidence it saw nothing. This is the one property
///   this function exists to guarantee, and it holds for a wrong-*type*
///   column value exactly as much as a wrong-*range* one — see
///   `RemainingShare`'s doc for why a corrupt value slipping through as
///   `NeverObserved` would read as "capacity available" and let a caller
///   dispatch into an exhausted window.
/// - Otherwise → [`CapacityStatus::Known`].
///
/// # Errors
/// Returns [`StorageError`] when the registry DB cannot be reached.
pub fn status(
    pool: &Pool<SqliteConnectionManager>,
    runtime: &str,
) -> Result<CapacityStatus, StorageError> {
    let conn = pool.get().map_err(|e| StorageError::Pool(e.to_string()))?;
    // Read every column as the untyped `rusqlite::types::Value` rather than
    // `Option<f64>`/`Option<i64>`/`String` directly: SQLite's column types
    // are affinities, not constraints, so a hand-corrupted (or
    // future-binary-written) row can hold a value of the "wrong" storage
    // class for its column, and a typed `row.get` would surface that as a
    // hard `rusqlite::Error` — a hard error this function's own doc
    // promises never to produce (it promises `Unclassified`). Matching on
    // `Value` lets every column's decode fail *as data*, not as a `Result`
    // this function would otherwise have no way to route to `Unclassified`.
    let row = conn
        .query_row(
            "SELECT remaining, resets_at_ms, window_secs, source
             FROM runtime_capacity WHERE runtime = ?",
            params![runtime],
            |row| {
                Ok((
                    row.get::<_, Value>(0)?,
                    row.get::<_, Value>(1)?,
                    row.get::<_, Value>(2)?,
                    row.get::<_, Value>(3)?,
                ))
            },
        )
        .optional()?;

    let Some((remaining_value, resets_at_value, window_value, source_value)) = row else {
        return Ok(CapacityStatus::NeverObserved);
    };

    let remaining = match remaining_value {
        Value::Null => None,
        // `Value::Integer` is accepted too: a whole-number share (`0`,
        // `1`) is a legitimate value REAL affinity would normally have
        // already widened to `Real` on write, but this reader does not
        // assume the write path was the one in this file.
        Value::Real(v) => match RemainingShare::new(v) {
            Ok(share) => Some(share),
            Err(_) => return Ok(CapacityStatus::Unclassified),
        },
        #[expect(
            clippy::cast_precision_loss,
            reason = "an i64 share is [0, 1] in practice; \
            precision loss only matters far outside that range, where RemainingShare::new \
            already rejects the value"
        )]
        Value::Integer(v) => match RemainingShare::new(v as f64) {
            Ok(share) => Some(share),
            Err(_) => return Ok(CapacityStatus::Unclassified),
        },
        Value::Text(_) | Value::Blob(_) => return Ok(CapacityStatus::Unclassified),
    };

    let source = match source_value {
        Value::Text(s) => match s.parse::<CapacitySource>() {
            Ok(source) => source,
            Err(_) => return Ok(CapacityStatus::Unclassified),
        },
        Value::Null | Value::Integer(_) | Value::Real(_) | Value::Blob(_) => {
            return Ok(CapacityStatus::Unclassified);
        },
    };

    let resets_at = match resets_at_value {
        Value::Null => None,
        Value::Integer(ms) => match DateTime::from_timestamp_millis(ms) {
            Some(dt) => Some(dt),
            None => return Ok(CapacityStatus::Unclassified),
        },
        Value::Real(_) | Value::Text(_) | Value::Blob(_) => {
            return Ok(CapacityStatus::Unclassified);
        },
    };

    let window = match window_value {
        Value::Null => None,
        Value::Integer(secs) => match u64::try_from(secs) {
            Ok(secs) => Some(std::time::Duration::from_secs(secs)),
            Err(_) => return Ok(CapacityStatus::Unclassified),
        },
        Value::Real(_) | Value::Text(_) | Value::Blob(_) => {
            return Ok(CapacityStatus::Unclassified);
        },
    };

    Ok(CapacityStatus::Known(CapacityWindow::from_parts(
        runtime, window, remaining, resets_at, source,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runs::clock::MockClock;
    use crate::runs::registry::open_registry_pool;
    use crate::runs::storage::Storage;
    use tempfile::TempDir;

    fn observed_at() -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc)
    }

    /// A window with every learnable field populated (`resets_at` from a
    /// retry-after, `window` learned from a prior reset) — used so
    /// round-trip tests exercise all three `Option` columns, not just
    /// `remaining`.
    fn full_window(runtime: &str) -> CapacityWindow {
        let previous_reset_at = observed_at();
        let second_reset_at = observed_at() + chrono::Duration::seconds(3600);
        // `retry_after: Some(ZERO)`, not `None`: a `None` retry-after
        // leaves `resets_at` itself `None` (see `observed_429`'s doc), which
        // would make `with_learned_window` a no-op below. `ZERO` sets
        // `resets_at` to exactly `second_reset_at`, keeping the learned
        // `window` (the gap between `previous_reset_at` and `resets_at`) a
        // round 3600s instead of an easy-to-miscompute offset.
        CapacityWindow::observed_429(runtime, Some(std::time::Duration::ZERO), second_reset_at)
            .with_learned_window(previous_reset_at)
    }

    #[test]
    fn status_is_never_observed_for_a_fresh_runtime() {
        let tmp = TempDir::new().unwrap();
        let clock = MockClock::new(1_700_000_000_000);
        let pool = open_registry_pool(tmp.path(), &clock).unwrap();

        assert_eq!(
            status(&pool, "claude-acp").unwrap(),
            CapacityStatus::NeverObserved
        );
    }

    #[test]
    fn observe_then_status_round_trips_a_known_window() {
        let tmp = TempDir::new().unwrap();
        let clock = MockClock::new(1_700_000_000_000);
        let pool = open_registry_pool(tmp.path(), &clock).unwrap();

        let window = full_window("claude-acp");
        observe(&pool, &window).unwrap();

        let got = status(&pool, "claude-acp").unwrap();
        let CapacityStatus::Known(got_window) = got else {
            panic!("expected Known, got {got:?}");
        };
        assert_eq!(got_window.runtime(), "claude-acp");
        assert_eq!(got_window.remaining(), Some(RemainingShare::EXHAUSTED));
        assert_eq!(
            got_window.resets_at(),
            Some(observed_at() + chrono::Duration::seconds(3600))
        );
        assert_eq!(
            got_window.window(),
            Some(std::time::Duration::from_secs(3600))
        );
        assert_eq!(got_window.source(), CapacitySource::Observed429);
    }

    /// Review finding: every other test in this file writes `remaining` as
    /// exactly `0.0` (`observed_429` hard-codes `EXHAUSTED`) and `source` as
    /// exactly `Observed429` — so `status`'s `Value::Real` arm never sees a
    /// genuinely fractional number, `Value::Real` is never distinguished
    /// from `Value::Integer` on a value where that distinction is even
    /// live, and `CapacitySource::AcpUsage`'s column label never round-trips
    /// at all. One `from_parts` round trip through a fractional
    /// `RemainingShare` and `AcpUsage` closes all three at once.
    #[test]
    fn fractional_remaining_and_acp_usage_source_round_trip() {
        let tmp = TempDir::new().unwrap();
        let clock = MockClock::new(1_700_000_000_000);
        let pool = open_registry_pool(tmp.path(), &clock).unwrap();

        let window = CapacityWindow::from_parts(
            "claude-acp",
            Some(std::time::Duration::from_secs(3600)),
            Some(RemainingShare::new(0.25).unwrap()),
            Some(observed_at()),
            CapacitySource::AcpUsage,
        );
        observe(&pool, &window).unwrap();

        let got = status(&pool, "claude-acp").unwrap();
        let CapacityStatus::Known(got_window) = got else {
            panic!("expected Known, got {got:?}");
        };
        assert_eq!(got_window.runtime(), "claude-acp");
        assert_eq!(
            got_window.remaining(),
            Some(RemainingShare::new(0.25).unwrap())
        );
        assert_eq!(got_window.resets_at(), Some(observed_at()));
        assert_eq!(
            got_window.window(),
            Some(std::time::Duration::from_secs(3600))
        );
        assert_eq!(got_window.source(), CapacitySource::AcpUsage);
    }

    /// Task 12 M2 acceptance criterion 7: an observation must outlive the
    /// process that made it. `Storage::open` twice against the same home is
    /// the same test a fresh process opening the same `~/.surge/` directory
    /// would face — a second in-process pool over the same SQLite file, not
    /// a second in-memory cache. Asserts all five persisted columns
    /// round-trip (not just the `Known` discriminant): a window with every
    /// `Option` field populated, same as the round-trip test above.
    #[tokio::test(flavor = "multi_thread")]
    async fn observation_survives_a_new_storage_open_of_the_same_home() {
        let tmp = TempDir::new().unwrap();

        let storage_a = Storage::open(tmp.path()).await.unwrap();
        let window = full_window("claude-acp");
        observe(&storage_a.registry_pool, &window).unwrap();
        drop(storage_a);

        let storage_b = Storage::open(tmp.path()).await.unwrap();
        let got = status(&storage_b.registry_pool, "claude-acp").unwrap();
        let CapacityStatus::Known(got_window) = got else {
            panic!(
                "observation must still be Known after a fresh Storage::open of the same home, got {got:?}"
            );
        };
        assert_eq!(got_window.runtime(), "claude-acp");
        assert_eq!(got_window.remaining(), Some(RemainingShare::EXHAUSTED));
        assert_eq!(
            got_window.resets_at(),
            Some(observed_at() + chrono::Duration::seconds(3600))
        );
        assert_eq!(
            got_window.window(),
            Some(std::time::Duration::from_secs(3600))
        );
        assert_eq!(got_window.source(), CapacitySource::Observed429);
    }

    /// **Empirically NOT what the M2 plan assumed** (verified here, not
    /// taken on faith): a `NaN` cannot actually reach this code from a
    /// SQLite `REAL` column at all. SQLite silently rewrites `NaN` to
    /// `NULL` at the moment of storage -- confirmed both through a bound
    /// `f64::NAN` parameter (`sqlite3_bind_double`, which is what `rusqlite`
    /// uses under the hood) and through an in-SQL `0.0/0.0` expression; both
    /// read back as `typeof(v) = 'null'`. So a `NaN` write to `remaining`
    /// degrades to "no observation of `remaining`" (`Known` with
    /// `remaining: None`), not to a corrupt, unreadable value. (`+inf` *is*
    /// stored faithfully by SQLite — verified separately — but is caught by
    /// the same `RemainingShare::new` range check as any other
    /// out-of-`[0.0, 1.0]` value, so it needs no test of its own beyond the
    /// generic out-of-range test below.)
    #[test]
    fn nan_written_to_remaining_is_stored_as_null_by_sqlite_itself() {
        let tmp = TempDir::new().unwrap();
        let clock = MockClock::new(1_700_000_000_000);
        let pool = open_registry_pool(tmp.path(), &clock).unwrap();

        let window = CapacityWindow::observed_429("claude-acp", None, observed_at());
        observe(&pool, &window).unwrap();
        {
            let conn = pool.get().unwrap();
            conn.execute(
                "UPDATE runtime_capacity SET remaining = ? WHERE runtime = ?",
                params![f64::NAN, "claude-acp"],
            )
            .unwrap();
            let stored_type: String = conn
                .query_row(
                    "SELECT typeof(remaining) FROM runtime_capacity WHERE runtime = ?",
                    params!["claude-acp"],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(
                stored_type, "null",
                "SQLite must have rewritten NaN to NULL"
            );
        }

        let CapacityStatus::Known(got) = status(&pool, "claude-acp").unwrap() else {
            panic!(
                "a NULL remaining column is a known window with no remaining share, not Unclassified"
            );
        };
        assert_eq!(got.remaining(), None);
    }

    /// The corruption `status` actually has to guard against: an
    /// out-of-`[0.0, 1.0]` but non-`NaN` value. Unlike `NaN` (see the test
    /// above), SQLite stores this faithfully -- it is the real, reachable
    /// case `RemainingShare::new`'s range check exists to catch on read.
    #[test]
    fn out_of_range_remaining_column_reads_as_unclassified_not_never_observed() {
        let tmp = TempDir::new().unwrap();
        let clock = MockClock::new(1_700_000_000_000);
        let pool = open_registry_pool(tmp.path(), &clock).unwrap();

        let window = CapacityWindow::observed_429("claude-acp", None, observed_at());
        observe(&pool, &window).unwrap();
        {
            let conn = pool.get().unwrap();
            conn.execute(
                "UPDATE runtime_capacity SET remaining = ? WHERE runtime = ?",
                params![1.5_f64, "claude-acp"],
            )
            .unwrap();
        }

        assert_eq!(
            status(&pool, "claude-acp").unwrap(),
            CapacityStatus::Unclassified,
            "an out-of-range remaining column must never read as capacity available"
        );
    }

    /// A wrong-*type* column value, not just a wrong-*range* one: SQLite
    /// affinities do not reject non-numeric text stored into a `REAL`
    /// column, so `remaining` can genuinely hold `TEXT` (verified directly
    /// via `typeof(remaining)`). `status` must degrade this to
    /// `Unclassified` exactly like the out-of-range case, not surface a
    /// hard `StorageError` — the doc above promises `Unclassified` for any
    /// column that "cannot become its typed form", not only a numerically
    /// out-of-range one.
    #[test]
    fn non_numeric_remaining_column_reads_as_unclassified() {
        let tmp = TempDir::new().unwrap();
        let clock = MockClock::new(1_700_000_000_000);
        let pool = open_registry_pool(tmp.path(), &clock).unwrap();

        let window = CapacityWindow::observed_429("claude-acp", None, observed_at());
        observe(&pool, &window).unwrap();
        {
            let conn = pool.get().unwrap();
            conn.execute(
                "UPDATE runtime_capacity SET remaining = 'not-a-number' WHERE runtime = ?",
                params!["claude-acp"],
            )
            .unwrap();
            let stored_type: String = conn
                .query_row(
                    "SELECT typeof(remaining) FROM runtime_capacity WHERE runtime = ?",
                    params!["claude-acp"],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(
                stored_type, "text",
                "non-numeric text must round-trip as TEXT, not be rejected by SQLite itself"
            );
        }

        assert_eq!(
            status(&pool, "claude-acp").unwrap(),
            CapacityStatus::Unclassified,
            "a wrong-type column must degrade to Unclassified, not a hard error"
        );
    }

    #[test]
    fn unrecognized_source_label_reads_as_unclassified() {
        let tmp = TempDir::new().unwrap();
        let clock = MockClock::new(1_700_000_000_000);
        let pool = open_registry_pool(tmp.path(), &clock).unwrap();

        let window = CapacityWindow::observed_429("claude-acp", None, observed_at());
        observe(&pool, &window).unwrap();
        {
            let conn = pool.get().unwrap();
            conn.execute(
                "UPDATE runtime_capacity SET source = 'a_future_source_this_binary_predates' \
                 WHERE runtime = ?",
                params!["claude-acp"],
            )
            .unwrap();
        }

        assert_eq!(
            status(&pool, "claude-acp").unwrap(),
            CapacityStatus::Unclassified
        );
    }

    #[test]
    fn observe_upserts_in_place_not_a_history() {
        let tmp = TempDir::new().unwrap();
        let clock = MockClock::new(1_700_000_000_000);
        let pool = open_registry_pool(tmp.path(), &clock).unwrap();

        observe(
            &pool,
            &CapacityWindow::observed_429("claude-acp", None, observed_at()),
        )
        .unwrap();
        observe(
            &pool,
            &CapacityWindow::observed_429(
                "claude-acp",
                Some(std::time::Duration::from_secs(90)),
                observed_at(),
            ),
        )
        .unwrap();

        let row_count: i64 = pool
            .get()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM runtime_capacity", [], |r| r.get(0))
            .unwrap();
        assert_eq!(row_count, 1, "one runtime must stay one row");

        let CapacityStatus::Known(window) = status(&pool, "claude-acp").unwrap() else {
            panic!("expected Known");
        };
        assert_eq!(
            window.resets_at(),
            Some(observed_at() + chrono::Duration::seconds(90)),
            "the second observation must replace the first, not merge with it"
        );
    }

    #[test]
    fn clear_removes_the_row_and_status_reads_never_observed_again() {
        let tmp = TempDir::new().unwrap();
        let clock = MockClock::new(1_700_000_000_000);
        let pool = open_registry_pool(tmp.path(), &clock).unwrap();

        observe(
            &pool,
            &CapacityWindow::observed_429("claude-acp", None, observed_at()),
        )
        .unwrap();
        assert!(matches!(
            status(&pool, "claude-acp").unwrap(),
            CapacityStatus::Known(_)
        ));

        clear(&pool, "claude-acp").unwrap();

        assert_eq!(
            status(&pool, "claude-acp").unwrap(),
            CapacityStatus::NeverObserved,
            "a cleared runtime must read back as never observed, not as a stale Known"
        );
    }

    #[test]
    fn clear_on_a_runtime_with_no_row_is_a_harmless_no_op() {
        let tmp = TempDir::new().unwrap();
        let clock = MockClock::new(1_700_000_000_000);
        let pool = open_registry_pool(tmp.path(), &clock).unwrap();

        // No `observe` call at all for this runtime.
        clear(&pool, "claude-acp").unwrap();

        assert_eq!(
            status(&pool, "claude-acp").unwrap(),
            CapacityStatus::NeverObserved
        );
    }

    /// Task 12 plan point (3c)/(3f): the three registry aliases for one
    /// runtime must produce one row, not three. This store does not
    /// normalize -- it is proving the *table's* half of that guarantee
    /// (a `PRIMARY KEY` collapses repeated observations of the same
    /// already-normalized key), which is the half this crate can own
    /// without depending on `surge-acp::Registry::normalize_agent_id`.
    #[test]
    fn repeated_observation_of_the_same_canonical_runtime_stays_one_row() {
        let tmp = TempDir::new().unwrap();
        let clock = MockClock::new(1_700_000_000_000);
        let pool = open_registry_pool(tmp.path(), &clock).unwrap();

        // Simulates a caller that already normalized `claude`, `claude-code`,
        // and `claude-acp` down to the one canonical id before calling
        // `observe` -- three observations of the same key, not three keys.
        for _ in 0..3 {
            observe(
                &pool,
                &CapacityWindow::observed_429("claude-acp", None, observed_at()),
            )
            .unwrap();
        }

        let row_count: i64 = pool
            .get()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM runtime_capacity", [], |r| r.get(0))
            .unwrap();
        assert_eq!(row_count, 1);
    }

    /// The other half of the same guarantee, made explicit rather than left
    /// as an assertion in a doc comment: this store does NOT normalize, so
    /// three *unnormalized* aliases really do fragment into three rows. If
    /// `observe`/`status` ever grew ad hoc normalization to "fix" this
    /// locally, this test would start failing the moment the fix disagreed
    /// with `surge-acp::Registry::normalize_agent_id` (the one true
    /// normalizer) in even one case -- so the guard stays here, not there.
    #[test]
    fn unnormalized_aliases_are_not_collapsed_by_this_store_alone() {
        let tmp = TempDir::new().unwrap();
        let clock = MockClock::new(1_700_000_000_000);
        let pool = open_registry_pool(tmp.path(), &clock).unwrap();

        for alias in ["claude", "claude-code", "claude-acp"] {
            observe(
                &pool,
                &CapacityWindow::observed_429(alias, None, observed_at()),
            )
            .unwrap();
        }

        let row_count: i64 = pool
            .get()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM runtime_capacity", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            row_count, 3,
            "normalization is the caller's job (Registry::normalize_agent_id, \
             a surge-acp concern) -- this store keys on whatever string it is given"
        );
    }

    /// Write-side counterpart to the read-side `u64::try_from` guard
    /// (review finding): a `Duration` too large for the signed `INTEGER`
    /// column must not wrap into a garbage negative number via `as i64` --
    /// it must write `NULL` (a window length this store cannot honestly
    /// represent) instead. Only reachable via `CapacityWindow::from_parts`
    /// (Task 12 M2) -- no constructor this crate ships produces a `Duration`
    /// anywhere near this magnitude.
    #[test]
    fn absurdly_large_window_duration_writes_null_not_a_wrapped_value() {
        let tmp = TempDir::new().unwrap();
        let clock = MockClock::new(1_700_000_000_000);
        let pool = open_registry_pool(tmp.path(), &clock).unwrap();

        let window = CapacityWindow::from_parts(
            "claude-acp",
            Some(std::time::Duration::from_secs(u64::MAX)),
            None,
            None,
            CapacitySource::Observed429,
        );
        observe(&pool, &window).unwrap();

        let CapacityStatus::Known(got) = status(&pool, "claude-acp").unwrap() else {
            panic!("expected Known");
        };
        assert_eq!(
            got.window(),
            None,
            "an unrepresentable Duration must degrade to NULL, not a wrapped/negative value"
        );
    }
}
