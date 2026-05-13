//! `surge migrate-spec` — translate a legacy `.spec.toml` into a `flow.toml`.
//!
//! This module is the only surviving direct caller of `surge-spec` after the
//! Legacy pipeline retirement milestone. It re-uses the deprecated
//! `SpecFile::load` parser for one-shot reads and emits a fresh `flow.toml`
//! document via `toml_edit` so that comments / warnings can be carried into
//! the output for human review.
//!
//! Phase 2 of `legacy-pipeline-retirement`. The crate dependency is removed
//! again in Phase 7 by swapping the parser for a local `serde::Deserialize`
//! DTO.

pub mod handler;
pub mod mapping;

pub use handler::{MigrateSpecArgs, run};
