//! Runtime-home fixtures retain directories without taking database ownership.
#[path = "fixtures/runtime_home.rs"]
mod runtime_home_fixture;

use runtime_home_fixture::FixtureHome;
use surge_core::memory::{ClaimStatus, Confidence, MemoryClaim, Provenance};
use surge_core::{ContentHash, MemoryClaimId};
use surge_persistence::memory::MemoryStore;
use surge_persistence::runs::Storage;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fixture_preserves_reopened_data_without_fencing_a_closed_database() {
    let home = FixtureHome::new().unwrap();
    let home_path = home.path().to_path_buf();
    #[cfg(windows)]
    let disposable_root = home.path().parent().unwrap().to_path_buf();
    #[cfg(not(windows))]
    let disposable_root = home_path.clone();
    let registry = home.path().join("db/registry.sqlite");
    {
        let storage = Storage::open(home.path()).await.unwrap();
        storage
            .acquire_registry_conn()
            .unwrap()
            .execute_batch(
                "CREATE TABLE fixture_receipt(value TEXT NOT NULL); \
                 INSERT INTO fixture_receipt VALUES ('same retained home');",
            )
            .unwrap();
    }
    {
        let reopened = Storage::open(home.path()).await.unwrap();
        let value: String = reopened
            .acquire_registry_conn()
            .unwrap()
            .query_row("SELECT value FROM fixture_receipt", [], |row| row.get(0))
            .unwrap();
        assert_eq!(value, "same retained home");
    }

    // The fixture still owns the home, but only SQLite consumers may fence this file.
    let moved = registry.with_extension("closed");
    std::fs::rename(&registry, &moved).unwrap();
    std::fs::rename(&moved, &registry).unwrap();

    let memory_path = home.path().join("memory.db");
    let text = "root memory database survives final connection closure";
    let claim = MemoryClaim::new(
        MemoryClaimId::new(),
        text,
        Provenance::verified(
            "fixture",
            ContentHash::compute(text.as_bytes()),
            "fixture",
            1,
        ),
        Confidence::Verified,
        ClaimStatus::Verified,
    )
    .unwrap();
    {
        MemoryStore::open(&memory_path)
            .unwrap()
            .add_claim(&claim)
            .unwrap();
    }
    {
        let reopened = MemoryStore::open(&memory_path).unwrap();
        assert_eq!(
            reopened.get_claim(claim.id()).unwrap().unwrap().text(),
            text
        );
    }
    home.close().unwrap();
    assert!(!home_path.exists());
    assert!(!disposable_root.exists());
}
