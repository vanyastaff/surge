//! Core types and configuration for Surge.

// `state.rs` declares its `mod tests` followed by `impl Display for TaskState`
// at the end of the file. Rust 1.95 adds `clippy::items_after_test_module` which
// flags this pre-existing legacy layout. Reorganizing the legacy file is out of
// scope for M1 (pure addition strategy); allow at crate level instead.
#![allow(clippy::items_after_test_module)]
// Pre-existing legacy code; M5 does not modify these modules.
// These allows suppress pedantic lints that fire in legacy files
// (config.rs, event.rs, run_state.rs, validation.rs, etc.) when
// clippy::pedantic is requested by a dependent crate (surge-orchestrator).
#![allow(clippy::doc_markdown)]
#![allow(clippy::uninlined_format_args)]
#![allow(clippy::struct_excessive_bools)]
#![allow(clippy::items_after_statements)]
#![allow(clippy::redundant_closure_for_method_calls)]
#![allow(clippy::must_use_candidate)]
#![allow(clippy::map_unwrap_or)]
#![allow(clippy::too_many_lines)]
#![allow(clippy::large_enum_variant)]

pub mod error;

// Legacy modules — untouched in M1.
pub mod config;
pub mod event;
pub mod id;
pub mod roadmap;
pub mod spec;
pub mod state;

// New modules — Surge data model.
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
pub mod mcp_config;
pub mod migrations;
pub mod node;
pub mod notify_config;
pub mod predicate;
pub mod profile;
pub mod run_event;
pub mod run_state;
pub mod run_status;
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

// ── New re-exports (Surge data model) ──
pub use content_hash::ContentHash;
pub use edge::{Edge, EdgeKind, EdgePolicy, ExceededAction, PortRef};
pub use graph::{Graph, GraphMetadata, SCHEMA_VERSION, Subgraph};
pub use keys::{EdgeKey, NodeKey, OutcomeKey, ProfileKey, SubgraphKey, TemplateKey};
pub use mcp_config::{McpServerRef, McpTransportConfig};
pub use migrations::{IdentityV1, MigrationChain, migrate_payload};
pub use node::{Node, NodeConfig, NodeKind, OutcomeDecl, Position};
pub use notify_config::NotifyChannelKind;
pub use profile::{Profile, Role, RoleCategory};
pub use run_event::{
    BootstrapDecision, BootstrapStage, ElevationDecision, EventPayload, RunConfig, RunEvent,
    SessionDisposition, VersionedEventPayload,
};
pub use run_state::{Cursor, FoldError, RunMemory, RunState, TerminalReason};
pub use run_status::{ParseRunStatusError, RunStatus};
pub use validation::{
    NoOpResolver, ReferenceResolver, Severity, ValidationError, ValidationErrorKind, validate,
    validate_with_resolver,
};
