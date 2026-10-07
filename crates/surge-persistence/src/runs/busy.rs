//! Registry busy handler that never parks a tokio worker.
//!
//! Registry SQLite is called synchronously from many async paths (task
//! ownership journal, inbox, roadmap and ledger stores). Under registry write
//! contention a statement waits in SQLite's busy handler for up to 5 s; with
//! rusqlite's default handler that sleep parks the calling tokio worker, and
//! a few concurrent waits stall every task on the runtime.
//!
//! [`install`] keeps SQLite's default backoff schedule and 5 s budget but
//! performs each sleep inside [`tokio::task::block_in_place`] when the caller
//! is a task on a multi-thread runtime, so the worker's core moves to another
//! thread while the statement waits. The call stays synchronous: a caller's
//! future still cannot be dropped mid-statement, so no write can commit after
//! its caller was cancelled. That is why the handler is preferred over moving
//! each call onto `spawn_blocking` (see `registry_exec` for the fence that
//! move requires).
//!
//! `block_in_place` panics on a current-thread runtime or under a
//! `Handle::block_on`, and a panic inside an owner-protected section aborts
//! the process. The handoff is therefore attempted only inside a task on a
//! multi-thread runtime; everywhere else (blocking threads, `Runtime::block_on`
//! roots, plain threads, current-thread runtimes) the handler sleeps directly.

use std::time::Duration;

use rusqlite::Connection;
use tokio::runtime::{Handle, RuntimeFlavor};

/// Total time one statement may wait for a lock, matching rusqlite's default.
const BUSY_BUDGET_MS: u64 = 5_000;

/// SQLite's `sqliteDefaultBusyCallback` schedule.
const DELAYS_MS: [u64; 12] = [1, 2, 5, 10, 15, 20, 25, 25, 25, 50, 50, 100];
const TOTALS_MS: [u64; 12] = [0, 1, 3, 8, 18, 33, 53, 78, 103, 128, 178, 228];

/// Replace the connection's busy handling with the worker-releasing handler.
///
/// Must run after anything that calls `busy_timeout`, which clears custom
/// handlers.
pub fn install(conn: &Connection) -> rusqlite::Result<()> {
    conn.busy_handler(Some(on_busy))
}

/// `count` is the number of prior invocations for the current lock attempt.
fn on_busy(count: i32) -> bool {
    let Some(delay) = next_delay(u64::try_from(count).unwrap_or(0)) else {
        return false;
    };
    if releases_worker() {
        tokio::task::block_in_place(|| std::thread::sleep(delay));
    } else {
        std::thread::sleep(delay);
    }
    true
}

fn next_delay(count: u64) -> Option<Duration> {
    let last = DELAYS_MS.len() - 1;
    let (delay, prior) = match usize::try_from(count) {
        Ok(index) if index <= last => (DELAYS_MS[index], TOTALS_MS[index]),
        _ => (
            DELAYS_MS[last],
            TOTALS_MS[last] + DELAYS_MS[last] * (count - last as u64),
        ),
    };
    let delay = delay.min(BUSY_BUDGET_MS.checked_sub(prior)?);
    (delay > 0).then(|| Duration::from_millis(delay))
}

/// True only where `block_in_place` is both safe and useful: a task running
/// on a multi-thread runtime (on a worker, or a blocking thread where it is a
/// no-op).
fn releases_worker() -> bool {
    tokio::task::try_id().is_some()
        && Handle::try_current()
            .is_ok_and(|handle| handle.runtime_flavor() == RuntimeFlavor::MultiThread)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    #[test]
    fn schedule_matches_sqlite_default_budget() {
        assert_eq!(next_delay(0), Some(Duration::from_millis(1)));
        assert_eq!(next_delay(11), Some(Duration::from_millis(100)));
        let total: u128 = (0..)
            .map_while(next_delay)
            .map(|delay| delay.as_millis())
            .sum();
        assert_eq!(total, u128::from(BUSY_BUDGET_MS));
    }

    #[test]
    fn handler_outside_runtime_sleeps_without_panicking() {
        assert!(!releases_worker());
        assert!(on_busy(0));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn handler_on_current_thread_runtime_does_not_hand_off() {
        assert!(!releases_worker());
        assert!(on_busy(0));
    }

    /// A synchronous registry write inside a task, waiting on another
    /// connection's write lock, must not stop the runtime's only worker.
    #[test]
    fn contended_registry_write_in_task_keeps_single_worker_running() {
        let home = crate::runtime_home_fixture::FixtureHome::new().unwrap();
        {
            let pool = crate::runs::registry::open_registry_pool(
                home.path(),
                &crate::runs::clock::SystemClock,
            )
            .unwrap();
            let holder = Connection::open(home.path().join("db").join("registry.sqlite")).unwrap();
            holder.execute_batch("BEGIN IMMEDIATE").unwrap();

            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .enable_all()
                .build()
                .unwrap();
            let ticks = Arc::new(AtomicU64::new(0));
            let ticker = ticks.clone();
            runtime.spawn(async move {
                loop {
                    ticker.fetch_add(1, Ordering::Relaxed);
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            });
            std::thread::sleep(Duration::from_millis(50));

            let write = runtime.spawn(async move {
                let conn = pool.get().unwrap();
                conn.execute("DELETE FROM runtime_capacity", [])
            });
            std::thread::sleep(Duration::from_millis(100));
            let before = ticks.load(Ordering::Relaxed);
            std::thread::sleep(Duration::from_millis(400));
            let during = ticks.load(Ordering::Relaxed) - before;

            holder.execute_batch("ROLLBACK").unwrap();
            runtime.block_on(write).unwrap().unwrap();
            assert!(
                during >= 10,
                "worker stalled while a registry write waited on the lock: {during} ticks in 400 ms"
            );
        }
        home.close().unwrap();
    }
}
