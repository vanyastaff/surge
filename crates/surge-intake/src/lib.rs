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
pub mod github;
pub mod linear;
pub mod router;
pub mod source;
pub mod testing;
pub mod types;

pub use error::{Error, Result};
pub use source::TaskSource;
pub use types::{
    Priority, TaskDetails, TaskEvent, TaskEventKind, TaskId, TaskSummary, Tier1Decision,
    TriageDecision,
};
