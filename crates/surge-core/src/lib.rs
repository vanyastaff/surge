//! Core types and configuration for Surge.

pub mod config;
pub mod error;
pub mod id;
pub mod spec;
pub mod state;

pub use config::SurgeConfig;
pub use error::SurgeError;
pub use id::{SpecId, SubtaskId, TaskId};
pub use spec::{AcceptanceCriteria, Complexity, Spec, Subtask};
pub use state::TaskState;
