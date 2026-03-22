//! ACP (Agent Client Protocol) integration for Surge.
//!
//! This crate implements the ACP `Client` trait, providing agents with
//! access to the filesystem, terminals, and permission management.
//! It also manages agent connections through `AgentPool`.

pub mod client;
pub mod connection;

pub use client::{PermissionPolicy, SubtaskContext, SurgeClient, SurgeEvent};
pub use connection::{AgentConnection, SessionState};

// TODO: Phase 0 — implement AgentPool
