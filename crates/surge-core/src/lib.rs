//! Core types and configuration for Surge.

// `state.rs` declares its `mod tests` followed by `impl Display for TaskState`
// at the end of the file. Rust 1.95 adds `clippy::items_after_test_module` which
// flags this pre-existing legacy layout. Reorganizing the legacy file is out of
// scope for M1 (pure addition strategy); allow at crate level instead.
#![allow(clippy::items_after_test_module)]

pub mod approvals;
pub mod config;
pub mod content_hash;
pub mod edge;
pub mod error;
pub mod event;
pub mod hooks;
pub mod id;
pub mod keys;
pub mod roadmap;
pub mod sandbox;
pub mod spec;
pub mod state;
pub mod terminal_config;

pub use config::SurgeConfig;
pub use error::SurgeError;
pub use event::{
    PlanEntry, PlanPriority, PlanStatus, SurgeEvent, ToolCallStatus, ToolDiff, ToolKind,
    ToolLocation, VersionedEvent,
};
pub use id::{RunId, SessionId, SpecId, SubtaskId, TaskId};
pub use roadmap::{Priority, RoadmapItem, RoadmapStatus, Timeline, TimelineBatch};
pub use spec::{AcceptanceCriteria, Complexity, Spec, Subtask, SubtaskExecution, SubtaskState};
pub use state::TaskState;
