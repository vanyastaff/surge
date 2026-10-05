//! Registry SQLite off the async executor.
//!
//! Registry connections carry rusqlite's default 5 s busy handler, so one
//! statement can sleep for that long while another connection or process
//! holds the registry write lock. Run directly from an `async fn`, that sleep
//! parks a tokio worker; with two workers, two such calls stall every task on
//! the runtime. Every `Storage` registry operation therefore runs on
//! [`tokio::task::spawn_blocking`], the convention the reader and task-ledger
//! paths already follow.
//!
//! The owner-scoped r2d2 maintenance executor (`pool.rs`) is deliberately not
//! used: its three threads also establish pool connections, so queries parked
//! in the busy handler there could starve the `pool.get()` they themselves
//! wait on.
//!
//! # Cancellation
//!
//! A synchronous body inside an `async fn` either never ran (the future was
//! dropped before its first poll) or finished before the caller could
//! observe anything. A blocking task survives its caller, which would add a
//! third outcome: a status, park or capacity write landing after the caller
//! dropped, possibly after a later write that superseded it. [`write`] keeps
//! the original two outcomes with a commit fence:
//!
//! - the closure runs inside `BEGIN IMMEDIATE`, so the busy wait happens
//!   before any decision is made;
//! - holding the registry write lock, the task commits to the write only if
//!   the caller has not been dropped, otherwise it rolls back;
//! - a caller dropped after that decision waits in `Drop` for the commit,
//!   which cannot busy-wait because the write lock is already held.
//!
//! Any write ordered after the caller's drop therefore observes either no
//! effect or the fully committed one, never a late commit. [`read`] has no
//! fence: an abandoned read has no effect.

use std::sync::{Arc, Condvar, Mutex, PoisonError};

use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::{Connection, Transaction, TransactionBehavior};

use crate::runs::error::StorageError;

/// Run a registry read on the blocking pool.
pub(crate) async fn read<T, F>(
    pool: &Pool<SqliteConnectionManager>,
    operation: F,
) -> Result<T, StorageError>
where
    F: FnOnce(&Connection) -> Result<T, StorageError> + Send + 'static,
    T: Send + 'static,
{
    let pool = pool.clone();
    tokio::task::spawn_blocking(move || {
        let conn = pool.get().map_err(|e| StorageError::Pool(e.to_string()))?;
        operation(&conn)
    })
    .await
    .map_err(join_error)?
}

/// Run a fenced registry write on the blocking pool.
///
/// `operation` runs inside a `BEGIN IMMEDIATE` transaction that commits only
/// if the caller is still waiting; see the module docs.
pub(crate) async fn write<T, F>(
    pool: &Pool<SqliteConnectionManager>,
    operation: F,
) -> Result<T, StorageError>
where
    F: FnOnce(&Transaction<'_>) -> Result<T, StorageError> + Send + 'static,
    T: Send + 'static,
{
    let fence = Arc::new(CommitFence::default());
    let _caller = CallerGuard(fence.clone());
    let pool = pool.clone();
    tokio::task::spawn_blocking(move || {
        let mut conn = pool.get().map_err(|e| StorageError::Pool(e.to_string()))?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(commit) = fence.begin_commit() else {
            tracing::debug!("registry write abandoned by its caller before commit");
            return Err(StorageError::OperationRejected(
                "registry write abandoned by its caller before commit".into(),
            ));
        };
        let outcome = match operation(&tx) {
            Ok(value) => tx.commit().map(|()| value).map_err(StorageError::from),
            Err(error) => {
                drop(tx);
                Err(error)
            },
        };
        drop(commit);
        outcome
    })
    .await
    .map_err(join_error)?
}

fn join_error(error: tokio::task::JoinError) -> StorageError {
    StorageError::Pool(format!("registry blocking task failed: {error}"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FencePhase {
    Pending,
    Abandoned,
    Committing,
    Settled,
}

#[derive(Debug)]
struct CommitFence {
    phase: Mutex<FencePhase>,
    settled: Condvar,
}

impl Default for CommitFence {
    fn default() -> Self {
        Self {
            phase: Mutex::new(FencePhase::Pending),
            settled: Condvar::new(),
        }
    }
}

impl CommitFence {
    /// Decide, under the registry write lock, whether this write proceeds.
    fn begin_commit(self: &Arc<Self>) -> Option<CommitToken> {
        let mut phase = self.phase.lock().unwrap_or_else(PoisonError::into_inner);
        if *phase != FencePhase::Pending {
            return None;
        }
        *phase = FencePhase::Committing;
        Some(CommitToken(self.clone()))
    }
}

/// Held by the blocking task from the commit decision until the transaction
/// has committed or rolled back; dropping it (also while unwinding) releases
/// a caller waiting in [`CallerGuard::drop`].
struct CommitToken(Arc<CommitFence>);

impl Drop for CommitToken {
    fn drop(&mut self) {
        let mut phase = self.0.phase.lock().unwrap_or_else(PoisonError::into_inner);
        *phase = FencePhase::Settled;
        self.0.settled.notify_all();
    }
}

/// Lives in the caller's future. Dropping it before the commit decision
/// abandons the write; dropping it after waits for that commit to settle.
struct CallerGuard(Arc<CommitFence>);

impl Drop for CallerGuard {
    fn drop(&mut self) {
        let mut phase = self.0.phase.lock().unwrap_or_else(PoisonError::into_inner);
        match *phase {
            FencePhase::Pending => *phase = FencePhase::Abandoned,
            FencePhase::Committing => {
                // Bounded: the write lock is held, so the remaining statement
                // and COMMIT never enter the busy handler.
                while *phase == FencePhase::Committing {
                    phase = self
                        .0
                        .settled
                        .wait(phase)
                        .unwrap_or_else(PoisonError::into_inner);
                }
            },
            FencePhase::Abandoned | FencePhase::Settled => {},
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn registry(home: &std::path::Path) -> Pool<SqliteConnectionManager> {
        crate::runs::registry::open_registry_pool(home, &crate::runs::clock::SystemClock).unwrap()
    }

    fn lock_registry(home: &std::path::Path) -> Connection {
        let conn = Connection::open(home.join("db").join("registry.sqlite")).unwrap();
        conn.execute_batch("BEGIN IMMEDIATE").unwrap();
        conn
    }

    fn capacity_rows(pool: &Pool<SqliteConnectionManager>) -> i64 {
        pool.get()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM runtime_capacity", [], |row| {
                row.get(0)
            })
            .unwrap()
    }

    async fn observe_write(
        pool: Pool<SqliteConnectionManager>,
        finished: tokio::sync::oneshot::Sender<()>,
    ) -> Result<(), StorageError> {
        write(&pool, move |tx| {
            let _finished = finished;
            tx.execute(
                "INSERT INTO runtime_capacity (runtime, remaining, resets_at_ms, window_secs, source)
                 VALUES ('fenced', NULL, NULL, NULL, 'observed')",
                [],
            )?;
            Ok(())
        })
        .await
    }

    /// A caller dropped while its write waits on the registry lock must
    /// leave no effect, even after the lock is released.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn write_abandoned_while_busy_never_commits() {
        let home = tempfile::tempdir().unwrap();
        let pool = registry(home.path());
        let holder = lock_registry(home.path());
        let (finished, task_done) = tokio::sync::oneshot::channel();

        let pending = observe_write(pool.clone(), finished);
        let timed_out = tokio::time::timeout(Duration::from_millis(200), pending).await;
        assert!(
            timed_out.is_err(),
            "write must still be waiting on the lock"
        );

        holder.execute_batch("ROLLBACK").unwrap();
        drop(holder);
        // The closure (and its sender) drops when the blocking task decides.
        tokio::time::timeout(Duration::from_secs(10), task_done)
            .await
            .expect("abandoned blocking task must settle")
            .expect_err("abandoned closure must never run");
        assert_eq!(capacity_rows(&pool), 0, "abandoned write committed late");
    }

    /// A write whose caller stays put commits once contention clears, and
    /// waiting does not occupy an async worker.
    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn write_waiting_on_lock_leaves_async_worker_free() {
        let home = tempfile::tempdir().unwrap();
        let pool = registry(home.path());
        let holder = lock_registry(home.path());

        let (finished, _task_done) = tokio::sync::oneshot::channel();
        let pending = tokio::spawn(observe_write(pool.clone(), finished));
        // With one worker, this timer only fires if the write is not
        // blocking it inside the busy handler.
        let started = std::time::Instant::now();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(started.elapsed() < Duration::from_secs(1));

        holder.execute_batch("ROLLBACK").unwrap();
        drop(holder);
        pending.await.unwrap().unwrap();
        assert_eq!(capacity_rows(&pool), 1);
    }

    #[test]
    fn caller_drop_after_commit_decision_waits_for_settlement() {
        let fence = Arc::new(CommitFence::default());
        let caller = CallerGuard(fence.clone());
        let token = fence.begin_commit().expect("pending fence admits commit");
        let settled = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            drop(token);
        });
        let started = std::time::Instant::now();
        drop(caller);
        assert!(started.elapsed() >= Duration::from_millis(40));
        assert_eq!(
            *fence.phase.lock().unwrap(),
            FencePhase::Settled,
            "caller drop must return only after the commit settled"
        );
        settled.join().unwrap();
    }

    #[test]
    fn caller_drop_before_commit_decision_abandons() {
        let fence = Arc::new(CommitFence::default());
        drop(CallerGuard(fence.clone()));
        assert!(fence.begin_commit().is_none());
    }
}
