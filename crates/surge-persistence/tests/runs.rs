//! Phase 12 integration tests for the M2 surge-persistence runs storage layer.
//!
//! Each submodule covers a focused integration scenario; see the per-module
//! docstring for the specific behavior under test. All tests use isolated
//! `~/.surge/` homes via `tempfile::TempDir` and a deterministic `MockClock`.
//!
//! Module layout: this file is the test-binary root. The `runs` module below
//! re-exports the fixtures + per-test modules so tests can `use
//! crate::runs::fixtures::...` regardless of which test file they live in.

#[path = "runs_inner/mod.rs"]
mod runs;
