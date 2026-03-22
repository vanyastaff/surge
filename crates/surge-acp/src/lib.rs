//! ACP (Agent Client Protocol) integration for Surge.
//!
//! This crate implements the ACP `Client` trait, providing agents with
//! access to the filesystem, terminals, and permission management.
//! It also manages agent connections through `AgentPool`.

pub mod client;
pub mod connection;
pub mod pool;

pub use client::{PermissionPolicy, SubtaskContext, SurgeClient};
pub use surge_core::SurgeEvent;
pub use connection::{AgentConnection, SessionState};
pub use pool::{AgentPool, SessionHandle};
