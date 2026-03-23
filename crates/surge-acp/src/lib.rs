//! ACP (Agent Client Protocol) integration for Surge.
//!
//! This crate implements the ACP `Client` trait, providing agents with
//! access to the filesystem, terminals, and permission management.
//! It also manages agent connections through `AgentPool`.

pub mod agent_info;
pub mod client;
pub mod connection;
pub mod health;
pub mod pool;
pub mod registry;
pub mod router;
pub mod terminal;

pub use agent_info::{
    build_available_agent, build_configured_agent, detect_installed_version, vendor_color,
    AgentBadge, AgentCapabilities, AgentEffortConfig, AgentUsage, AvailableAgent, BadgeKind,
    ConfiguredAgent, EffortLevel, InstallStatus, ModelOption, PermissionSetting, SessionEntry,
    SessionStatus, VersionInfo,
};
pub use client::{PermissionPolicy, SubtaskContext, SurgeClient};
pub use connection::{AgentConnection, SessionState};
pub use health::{AgentHealth, HealthMonitor};
pub use pool::{AgentPool, SessionHandle};
pub use registry::{AgentCapability, DetectedAgent, Registry, RegistryEntry};
pub use router::{AgentRouter, RouteDecision};
pub use surge_core::SurgeEvent;
