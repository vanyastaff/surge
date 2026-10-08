//! Actual SQLite close/ownership tests under the dedicated standard Windows user.
use super::OwnedSqliteConnectionManager;
use crate::{RuntimeHomeOwner, state_home::SqliteNamespaceOwner};
use r2d2::ManageConnection;
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

fn home() -> (tempfile::TempDir, RuntimeHomeOwner, PathBuf) {
    let profile = RuntimeHomeOwner::user_profile_path().unwrap();
    let fixture = tempfile::Builder::new()
        .prefix("surge-sqlite-owner-")
        .tempdir_in(profile)
        .unwrap();
    let owner = RuntimeHomeOwner::prepare(&fixture.path().join("state")).unwrap();
    let database = owner.path().join("probe.sqlite");
    (fixture, owner, database)
}

#[test]
#[ignore = "mandatory dedicated-standard-user Windows CI checked SQLite ownership oracle"]
fn connection_owners_outlive_manager_and_close_wal_normally() {
    let (fixture, home, path) = home();
    let namespace = SqliteNamespaceOwner::standalone(&path).unwrap();
    let weak = Arc::downgrade(&namespace);
    let manager = OwnedSqliteConnectionManager::file(&path, namespace);
    let first = manager.connect().unwrap();
    first.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE actual(value INTEGER); INSERT INTO actual VALUES(7)").unwrap();
    let second = manager.connect().unwrap();
    assert_eq!(
        second
            .query_row("SELECT value FROM actual", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        7
    );
    let wal = path.with_file_name("probe.sqlite-wal");
    let shm = path.with_file_name("probe.sqlite-shm");
    assert!(wal.is_file() && shm.is_file());
    crate::state_home::test_security::assert_sqlite_sidefile(&wal);
    crate::state_home::test_security::assert_sqlite_sidefile(&shm);
    weak.upgrade().unwrap().verify().unwrap();
    drop(manager);
    first.close().unwrap();
    assert!(
        weak.upgrade().is_some(),
        "actual second connection must retain ownership after manager close"
    );
    second
        .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
        .unwrap();
    second.close().unwrap();
    assert!(weak.upgrade().is_none());
    assert!(
        !wal.exists() && !shm.exists(),
        "normal final SQLite close must remove sidefiles"
    );
    let moved = path.with_extension("moved");
    std::fs::rename(&path, &moved).unwrap();
    std::fs::rename(&moved, &path).unwrap();
    let reopened = SqliteNamespaceOwner::standalone(&path).unwrap();
    let manager = OwnedSqliteConnectionManager::file(&path, reopened);
    let connection = manager.connect().unwrap();
    assert_eq!(
        connection
            .query_row("SELECT value FROM actual", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        7
    );
    connection.close().unwrap();
    drop(manager);
    drop(home);
    fixture.close().unwrap();
    println!("stage1 checked SQLite manager/connection lifetime and WAL close=PASS");
}

#[test]
#[ignore = "mandatory dedicated-standard-user Windows CI checked SQLite ownership oracle"]
fn failed_real_initializer_closes_before_releasing_namespace() {
    let (fixture, home, path) = home();
    let namespace = SqliteNamespaceOwner::standalone(&path).unwrap();
    let weak = Arc::downgrade(&namespace);
    let entered = Arc::new(AtomicBool::new(false));
    let observed = entered.clone();
    let manager =
        OwnedSqliteConnectionManager::file(&path, namespace).with_init(move |connection| {
            observed.store(true, Ordering::SeqCst);
            connection.execute_batch("this is deliberately invalid SQLite syntax")
        });
    assert!(manager.connect().is_err());
    assert!(
        entered.load(Ordering::SeqCst),
        "failure must come from the real initializer"
    );
    assert!(
        weak.upgrade().is_some(),
        "manager still owns the original database"
    );
    drop(manager);
    assert!(weak.upgrade().is_none());
    std::fs::rename(&path, path.with_extension("moved")).unwrap();
    drop(home);
    fixture.close().unwrap();
    println!("stage1 checked SQLite initializer failure settlement=PASS");
}

fn busy_child(path: &Path, receipt: &Path) {
    let namespace = SqliteNamespaceOwner::standalone(path).unwrap();
    let weak = Arc::downgrade(&namespace);
    let manager = OwnedSqliteConnectionManager::file(path, namespace);
    let connection = manager.connect().unwrap();
    drop(manager);
    let statement = connection.prepare("SELECT 1").unwrap();
    // Safe Rust can lose a live statement; this is the real SQLITE_BUSY trigger.
    std::mem::forget(statement);
    let (owner, error) = connection.close().unwrap_err();
    assert_eq!(
        error.sqlite_error_code(),
        Some(rusqlite::ErrorCode::DatabaseBusy)
    );
    assert!(
        weak.upgrade().is_some(),
        "failed explicit close must return complete ownership"
    );
    assert!(std::fs::rename(path, path.with_extension("moved")).is_err());
    std::fs::write(receipt, b"actual SQLITE_BUSY; returned owner alive").unwrap();
    drop(owner); // Must abort. If a broken implementation returns, the child test succeeds.
}

fn bounded_exit(child: &mut std::process::Child) -> std::process::ExitStatus {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        if std::time::Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    child.kill().unwrap();
    let reap_deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        assert!(
            child.try_wait().unwrap().is_none(),
            "checked close child exceeded deadline"
        );
        assert!(
            std::time::Instant::now() < reap_deadline,
            "checked close child did not settle after kill"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[test]
#[ignore = "mandatory dedicated-standard-user Windows CI checked SQLite ownership oracle"]
fn busy_close_returns_owner_and_drop_is_fatal() {
    const CHILD: &str = "SURGE_CHECKED_SQLITE_BUSY_CHILD";
    if let Some(path) = std::env::var_os(CHILD) {
        busy_child(
            Path::new(&path),
            &Path::new(&path).with_extension("receipt"),
        );
        return;
    }
    let (fixture, home, path) = home();
    let receipt = path.with_extension("receipt");
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "runs::connection::windows_tests::busy_close_returns_owner_and_drop_is_fatal",
            "--ignored",
            "--nocapture",
        ])
        .env(CHILD, &path)
        .spawn()
        .unwrap();
    let status = bounded_exit(&mut child);
    assert!(
        !status.success(),
        "dropping a genuinely busy owner must not return normally"
    );
    assert_eq!(
        std::fs::read(receipt).unwrap(),
        b"actual SQLITE_BUSY; returned owner alive"
    );
    std::fs::rename(&path, path.with_extension("moved")).unwrap();
    drop(home);
    fixture.close().unwrap();
    println!("stage1 actual SQLITE_BUSY returns ownership and fatal Drop settles process=PASS");
}
