//! Issue-tracker / task-source integration for Surge.
//!
//! Defines [`TaskSource`] trait and the shared types and computational
//! pipelines (dedup, candidate enumeration, multiplexer) that feed
//! incoming work into the vibe-flow bootstrap pipeline.
//!
//! See `docs/ARCHITECTURE.md`.

pub mod candidates;
pub mod dedup;
pub mod error;
pub mod github;
pub mod linear;
pub mod policy;
pub mod router;
pub mod source;
pub mod testing;
pub mod types;

pub use error::{Error, Result};
pub use policy::{
    AutomationPolicy, TRIAGE_DECISION_EXTERNALLY_CLOSED, TRIAGE_DECISION_L0, resolve_policy,
};
pub use source::TaskSource;
pub use types::{
    Priority, TaskDetails, TaskEvent, TaskEventKind, TaskId, TaskSummary, Tier1Decision,
    TriageDecision,
};
