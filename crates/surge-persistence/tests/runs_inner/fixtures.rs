//! Common fixtures for integration tests in the `runs/` suite.

use std::path::PathBuf;
use std::sync::Arc;

use surge_core::RunId;
use surge_persistence::runs::{MockClock, Storage};
mod runtime_home_fixture {
    #[cfg(windows)]
    use surge_persistence::RuntimeHomeOwner;
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../scripts/test-support/runtime_home.rs"
    ));
}

use runtime_home_fixture::FixtureHome;

/// Shared per-test scaffold: an isolated `~/.surge/` directory, a deterministic
/// clock, an opened `Storage`, and a fresh `RunId` ready to be used.
///
/// Several fields are declared `#[allow(dead_code)]` because individual tests
/// often need only a subset (e.g., a writer test ignores `home`, a stale-pid
/// test ignores `clock`). Keeping the full set on `TestRun` lets `setup()`
/// stay one-size-fits-all.
#[allow(dead_code)]
pub struct TestRun {
    /// Path of the isolated home directory used for this test.
    pub home: PathBuf,
    /// The opened storage facade.
    pub storage: Arc<Storage>,
    /// Mock clock (cloneable handle); use `advance` to control timestamps.
    pub clock: MockClock,
    /// A pre-allocated run id; tests are free to allocate more.
    pub run_id: RunId,
    /// Released only after all storage, writer and reader owners settle.
    pub home_fixture: FixtureHome,
}

impl TestRun {
    pub fn close(self) -> std::io::Result<()> {
        drop(self.storage);
        self.home_fixture.close()
    }
}

/// Initialize a fresh `Storage` rooted at a new tempdir with a deterministic clock.
pub async fn setup() -> TestRun {
    let tmp = FixtureHome::new().expect("tempdir");
    let home = tmp.path().to_path_buf();
    let clock = MockClock::new(1_700_000_000_000);
    let storage = Storage::open_with(&home, Arc::new(clock.clone()))
        .await
        .expect("storage open");
    let run_id = RunId::new();
    TestRun {
        home,
        storage,
        clock,
        run_id,
        home_fixture: tmp,
    }
}

/// A minimal `TokensConsumed` payload used to populate event logs cheaply.
///
/// Indexed by `idx` so the payload's tokens vary across calls (helpful for
/// debugging mid-stream failures by inspecting payload contents).
pub fn dummy_payload(idx: u64) -> surge_core::VersionedEventPayload {
    use surge_core::run_event::{EventPayload, VersionedEventPayload};
    VersionedEventPayload::new(EventPayload::TokensConsumed {
        session: surge_core::SessionId::new(),
        prompt_tokens: idx as u32,
        output_tokens: (idx * 2) as u32,
        cache_hits: 0,
        model: "test-model".into(),
        cost_usd: None,
    })
}
