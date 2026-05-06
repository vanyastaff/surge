//! ACP (Agent Client Protocol) integration for Surge.
//!
//! This crate implements the ACP `Client` trait, providing agents with
//! access to the filesystem, terminals, and permission management.
//! It also manages agent connections through `AgentPool`.

// Pre-existing legacy code; M5 does not modify these modules.
// These allows suppress pedantic lints that fire in legacy files when
// clippy::pedantic is requested transitively (e.g. by surge-orchestrator).
#![allow(clippy::doc_markdown)]
#![allow(clippy::uninlined_format_args)]
#![allow(clippy::must_use_candidate)]
#![allow(clippy::map_unwrap_or)]
#![allow(clippy::too_many_lines)]
#![allow(clippy::items_after_statements)]
#![allow(clippy::redundant_closure_for_method_calls)]
#![allow(clippy::struct_excessive_bools)]
#![allow(clippy::cast_lossless)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_possible_wrap)]
#![allow(clippy::cast_precision_loss)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::default_trait_access)]
#![allow(clippy::derivable_impls)]
#![allow(clippy::duration_suboptimal_units)]
#![allow(clippy::excessive_nesting)]
#![allow(clippy::explicit_iter_loop)]
#![allow(clippy::if_not_else)]
#![allow(clippy::ignored_unit_patterns)]
#![allow(clippy::implicit_clone)]
#![allow(clippy::manual_let_else)]
#![allow(clippy::match_same_arms)]
#![allow(clippy::missing_fields_in_debug)]
#![allow(clippy::needless_pass_by_value)]
#![allow(clippy::needless_raw_string_hashes)]
#![allow(clippy::redundant_else)]
#![allow(clippy::single_match_else)]
#![allow(clippy::struct_field_names)]
#![allow(clippy::trivially_copy_pass_by_ref)]
#![allow(clippy::unnecessary_wraps)]
#![allow(clippy::unnested_or_patterns)]
#![allow(clippy::unreadable_literal)]
#![allow(clippy::unused_async)]
#![allow(clippy::unused_self)]
#![allow(clippy::used_underscore_binding)]
#![allow(clippy::bool_to_int_with_if)]
#![allow(clippy::borrow_as_ptr)]

pub mod client;
pub mod connection;
pub mod discovery;
pub mod display;
pub mod health;
pub mod pool;
pub mod process_tracker;
pub mod registry;
pub mod router;
pub mod secrets;
pub mod terminal;
pub mod transport;

// New (M3) — Surge ACP bridge. Pure addition, legacy modules untouched.
pub mod bridge;
pub mod shared;

pub use client::{PermissionPolicy, SubtaskContext, SurgeClient};
pub use connection::{AgentConnection, EffectiveCapabilities, SessionState};
pub use discovery::{AgentDiscovery, Platform};
pub use display::{
    AgentDetail, AgentSummary, Badge, BadgeKind, DisplayCapabilities, EffortConfig, EffortLevel,
    InstallMethod, Model, Permission, SessionEntry, SessionStatus, Usage, VersionInfo,
    detect_installed_version,
};
pub use health::{AgentHealth, HealthStatus, HealthTracker};
pub use pool::{AgentPool, SessionHandle};
pub use process_tracker::{Pid, ProcessTracker};
pub use registry::{AgentCapability, DetectedAgent, Registry, RegistryEntry};
pub use router::{AgentRouter, RouteDecision};
pub use surge_core::SurgeEvent;
