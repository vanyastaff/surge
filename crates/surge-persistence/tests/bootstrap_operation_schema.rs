//! Durable bootstrap acceptance has its own registry journal.

use surge_persistence::runs::{MockClock, registry::open_registry_pool};

#[test]
fn bootstrap_journal_exists_after_registry_reopen() {
    let temp = tempfile::tempdir().unwrap();
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
}
