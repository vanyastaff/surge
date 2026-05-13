//! Shared test fixtures for surge-orchestrator integration tests.
//!
//! The legacy `.spec.toml` loaders (`load_simple_spec`, `load_dependency_spec`,
//! `fixture_path`) lived here while the spec pipeline existed. They were
//! removed as part of the **Legacy pipeline retirement** milestone — engine
//! tests use `examples/flow_*.toml` and the `bootstrap` / `mock_bridge`
//! sub-modules below instead.

pub mod bootstrap;
pub mod mock_bridge;
