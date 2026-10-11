//! SQLite pool construction; database pools retain independent connection limits.

use std::sync::Arc;

use crate::SqliteConnectionManager;
use scheduled_thread_pool::{OnPoolDropBehavior, ScheduledThreadPool};

pub(super) fn sqlite_pool_builder() -> r2d2::Builder<SqliteConnectionManager> {
    // r2d2 schedules only Weak<SharedPool> jobs. While a DB owner or an
    // in-flight connection job exists, its config retains this executor.
    // After the last owner drops, waiting for a dead reaper's next 30-second
    // tick would retain three idle threads per expired reader pool.
    let executor = ScheduledThreadPool::builder()
        .num_threads(3)
        .thread_name_pattern("surge-sqlite-{}")
        .on_drop_behavior(OnPoolDropBehavior::DiscardPendingScheduled)
        .build();
    tracing::debug!(
        workers = 3,
        "created owner-scoped SQLite maintenance executor"
    );
    let builder = r2d2::Pool::builder().thread_pool(Arc::new(executor));
    #[cfg(test)]
    let builder = builder.connection_customizer(Box::new(tests::WorkerProbe));
    builder
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{LazyLock, Mutex, OnceLock};
    use std::thread::ThreadId;

    static EXITED: LazyLock<Mutex<HashSet<ThreadId>>> =
        LazyLock::new(|| Mutex::new(HashSet::new()));

    struct ExitProbe(ThreadId);
    impl Drop for ExitProbe {
        fn drop(&mut self) {
            EXITED.lock().unwrap().insert(self.0);
        }
    }
    thread_local! {
        static EXIT: ExitProbe = ExitProbe(std::thread::current().id());
    }

    static PROBE_HOME: OnceLock<PathBuf> = OnceLock::new();

    static CONNECTIONS: AtomicUsize = AtomicUsize::new(0);
    static WORKERS: LazyLock<Mutex<HashSet<ThreadId>>> =
        LazyLock::new(|| Mutex::new(HashSet::new()));

    /// r2d2 calls this after manager.connect on its scheduled worker, not the
    /// reader's caller thread. The probe observes actual connections in both
    /// production construction sites, rather than counting our own factory calls.
    #[derive(Debug)]
    pub(super) struct WorkerProbe;

    impl
        r2d2::CustomizeConnection<
            <crate::SqliteConnectionManager as r2d2::ManageConnection>::Connection,
            rusqlite::Error,
        > for WorkerProbe
    {
        fn on_acquire(
            &self,
            connection: &mut <crate::SqliteConnectionManager as r2d2::ManageConnection>::Connection,
        ) -> Result<(), rusqlite::Error> {
            // Parallel unit tests may build unrelated pools in this process.
            // Observe only actual SQLite connections belonging to this fixture.
            let owned = PROBE_HOME.get().is_some_and(|home| {
                connection.path().is_some_and(|path| {
                    Path::new(path)
                        .canonicalize()
                        .is_ok_and(|path| path.starts_with(home))
                })
            });
            if !owned {
                return Ok(());
            }
            EXIT.with(|_| {});
            WORKERS.lock().unwrap().insert(std::thread::current().id());
            CONNECTIONS.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn reader_pool_churn_releases_actual_maintenance_workers_promptly() {
        let home = crate::runtime_home_fixture::FixtureHome::new().unwrap();
        {
            PROBE_HOME.set(home.path().canonicalize().unwrap()).unwrap();
            let storage = super::super::storage::Storage::open(home.path())
                .await
                .unwrap();
            let run = surge_core::RunId::new();
            let writer = storage.create_run(run, home.path(), None).await.unwrap();
            writer
                .append_event(surge_core::VersionedEventPayload::new(
                    surge_core::EventPayload::RunAborted {
                        reason: "resource regression fixture".into(),
                    },
                ))
                .await
                .unwrap();
            writer.flush().await.unwrap();
            let mut readers = Vec::new();
            for _ in 0..64 {
                let reader = storage.open_run_reader(run).await.unwrap();
                let events = reader.read_run_events().await.unwrap();
                assert_eq!(events.len(), 1);
                assert_eq!(events[0].run_id, run);
                readers.push(reader);
            }
            assert_eq!(
                readers.len(),
                64,
                "reader pools must coexist during the measurement"
            );
            for reader in &readers {
                assert_eq!(
                    reader.read_run_events().await.unwrap().len(),
                    1,
                    "retained pools must remain usable before their owners release"
                );
            }
            assert!(
                WORKERS.lock().unwrap().is_disjoint(&EXITED.lock().unwrap()),
                "maintenance workers must remain alive while their database owners are retained"
            );
            drop(readers);
            for _ in 0..64 {
                let reader = storage.open_run_reader(run).await.unwrap();
                assert_eq!(reader.read_run_events().await.unwrap().len(), 1);
            }
            let observed = WORKERS.lock().unwrap().clone();
            let connections = CONNECTIONS.load(Ordering::Relaxed);
            writer.close().await.unwrap();
            assert!(
                connections >= 130,
                "probe must observe registry, writer reader and 128 independent reader connections: {connections}"
            );
            drop(storage);
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
            loop {
                let exited = EXITED.lock().unwrap().clone();
                if observed.is_subset(&exited) {
                    break;
                }
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "database owners released, but only {} of {} observed maintenance workers exited before deadline",
                    observed.intersection(&exited).count(),
                    observed.len()
                );
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            println!(
                "actual maintenance worker identities exited promptly for 64 retained + 64 churned readers: {}, actual connection establishments: {connections}",
                observed.len()
            );
            assert!(
                !observed.is_empty(),
                "probe must observe actual connection establishment"
            );
        }
        home.close().unwrap();
    }
}
