//! Common fixtures for integration tests in the `runs/` suite.

use std::path::PathBuf;
use std::sync::Arc;

use surge_core::RunId;
use surge_persistence::runs::{MockClock, Storage};
use tempfile::TempDir;

/// Shared per-test scaffold: an isolated `~/.surge/` directory, a deterministic
/// clock, an opened `Storage`, and a fresh `RunId` ready to be used.
pub struct TestRun {
    /// TempDir kept alive for the test's lifetime — drop = cleanup.
    pub _tmp: TempDir,
    /// Path of the isolated home directory used for this test.
    pub home: PathBuf,
    /// The opened storage facade.
    pub storage: Arc<Storage>,
    /// Mock clock (cloneable handle); use `advance` to control timestamps.
    pub clock: MockClock,
    /// A pre-allocated run id; tests are free to allocate more.
    pub run_id: RunId,
}

/// Initialize a fresh `Storage` rooted at a new tempdir with a deterministic clock.
pub async fn setup() -> TestRun {
    let tmp = TempDir::new().expect("tempdir");
    let home = tmp.path().to_path_buf();
    let clock = MockClock::new(1_700_000_000_000);
    let storage = Storage::open_with(&home, Arc::new(clock.clone()))
        .await
        .expect("storage open");
    let run_id = RunId::new();
    TestRun {
        _tmp: tmp,
        home,
        storage,
        clock,
        run_id,
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
