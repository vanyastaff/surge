//! `surge migrate-spec` — translate a legacy `.spec.toml` into a `flow.toml`.
//!
//! Reads input via [`crate::legacy_spec::LegacySpecFile`] (a local
//! `serde::Deserialize` DTO that replaced the deleted `surge-spec` crate)
//! and emits a fresh `flow.toml` document via `toml_edit` so that comment
//! warnings can be carried into the output for human review.
//!
//! Introduced by the Legacy pipeline retirement milestone as the one-shot
//! migration path for users who authored `.spec.toml` files by hand.

pub mod handler;
pub mod mapping;

pub use handler::{MigrateSpecArgs, run};
