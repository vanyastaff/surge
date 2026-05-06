//! Issue-tracker / task-source integration for Surge.
//!
//! Defines [`TaskSource`] trait and the shared types and computational
//! pipelines (dedup, candidate enumeration, multiplexer) that feed
//! incoming work into the vibe-flow bootstrap pipeline.
//!
//! See `docs/revision/rfcs/0010-issue-tracker-integration.md`.

pub mod candidates;
pub mod dedup;
pub mod error;
pub mod router;
pub mod source;
pub mod testing;
pub mod types;

pub use error::{Error, Result};

// Re-exports for trait + types are added by tasks T1.1–T1.5.
