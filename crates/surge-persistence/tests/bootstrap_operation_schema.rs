//! Durable bootstrap acceptance has its own registry journal.

mod runtime_home_fixture {
    #[cfg(windows)]
    use surge_persistence::RuntimeHomeOwner;
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../scripts/test-support/runtime_home.rs"
    ));
}

use runtime_home_fixture::FixtureHome;

use surge_persistence::runs::{MockClock, registry::open_registry_pool};

#[test]
fn bootstrap_journal_exists_after_registry_reopen() {
    let temp = FixtureHome::new().unwrap();
    let clock = MockClock::new(1);
    drop(open_registry_pool(temp.path(), &clock).unwrap());
    let pool = open_registry_pool(temp.path(), &clock).unwrap();
    let connection = pool.get().unwrap();
    let table: Option<String> = connection
        .query_row(
            "SELECT name FROM sqlite_master WHERE type='table' AND name='bootstrap_operations'",
            [],
            |row| row.get(0),
        )
        .ok();
    assert_eq!(table.as_deref(), Some("bootstrap_operations"));
    drop(connection);
    drop(pool);
    temp.close().expect("close runtime home");
}
