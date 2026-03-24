//! ACP (Agent Client Protocol) integration for Surge.
//!
//! This crate implements the ACP `Client` trait, providing agents with
//! access to the filesystem, terminals, and permission management.
//! It also manages agent connections through `AgentPool`.

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

pub use client::{PermissionPolicy, SubtaskContext, SurgeClient};
pub use connection::{AgentConnection, EffectiveCapabilities, SessionState};
pub use discovery::{AgentDiscovery, Platform};
pub use display::{
    AgentDetail, AgentSummary, Badge, BadgeKind, DisplayCapabilities, EffortConfig, EffortLevel,
    InstallMethod, Model, Permission, SessionEntry, SessionStatus, Usage, VersionInfo,
    detect_installed_version,
};
pub use health::{AgentHealth, HealthTracker};
pub use pool::{AgentPool, SessionHandle};
pub use process_tracker::{Pid, ProcessTracker};
pub use registry::{AgentCapability, AgentKind, DetectedAgent, Registry, RegistryEntry};
pub use router::{AgentRouter, RouteDecision};
pub use surge_core::SurgeEvent;
