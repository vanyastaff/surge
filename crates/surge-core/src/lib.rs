//! Core types and configuration for Surge.

// `state.rs` declares its `mod tests` followed by `impl Display for TaskState`
// at the end of the file. Rust 1.95 adds `clippy::items_after_test_module` which
// flags this pre-existing legacy layout. Reorganizing the legacy file is out of
// scope for M1 (pure addition strategy); allow at crate level instead.
#![allow(clippy::items_after_test_module)]

pub mod error;

// Legacy modules — untouched in M1.
pub mod config;
pub mod event;
pub mod id;
pub mod roadmap;
pub mod spec;
pub mod state;

// New modules — vibe-flow data model.
pub mod agent_config;
pub mod approvals;
pub mod branch_config;
pub mod content_hash;
pub mod edge;
pub mod graph;
pub mod hooks;
pub mod human_gate_config;
pub mod keys;
pub mod loop_config;
pub mod node;
pub mod notify_config;
pub mod profile;
pub mod run_event;
pub mod run_state;
pub mod sandbox;
pub mod subgraph_config;
pub mod terminal_config;
pub mod validation;

// ── Legacy re-exports (kept stable) ──
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

// ── New re-exports (vibe-flow data model) ──
pub use content_hash::ContentHash;
pub use edge::{Edge, EdgeKind, EdgePolicy, ExceededAction, PortRef};
pub use graph::{Graph, GraphMetadata, SCHEMA_VERSION, Subgraph};
pub use keys::{EdgeKey, NodeKey, OutcomeKey, ProfileKey, SubgraphKey, TemplateKey};
pub use node::{Node, NodeConfig, NodeKind, OutcomeDecl, Position};
pub use profile::{Profile, Role, RoleCategory};
pub use run_event::{
    BootstrapDecision, BootstrapStage, ElevationDecision, EventPayload, RunConfig, RunEvent,
    SessionDisposition, VersionedEventPayload,
};
pub use run_state::{Cursor, FoldError, RunMemory, RunState, TerminalReason};
pub use validation::{Severity, ValidationError, ValidationErrorKind, validate};
