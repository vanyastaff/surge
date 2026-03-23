//! Core types and configuration for Surge.

pub mod config;
pub mod error;
pub mod event;
pub mod id;
pub mod spec;
pub mod state;

pub use config::SurgeConfig;
pub use error::SurgeError;
pub use event::{
    PlanEntry, PlanPriority, PlanStatus, SurgeEvent, ToolCallStatus, ToolDiff, ToolKind,
    ToolLocation, VersionedEvent,
};
pub use id::{SpecId, SubtaskId, TaskId};
pub use spec::{AcceptanceCriteria, Complexity, Spec, Subtask, SubtaskExecution, SubtaskState};
pub use state::TaskState;
