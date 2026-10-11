//! `surge-mcp` — MCP (Model Context Protocol) client integration for
//! `surge-orchestrator` agent stages. Wraps the official `rmcp` crate
//! with surge-flavoured registry, connection state, restart policy.
//!
//! See `docs/ARCHITECTURE.md`
//! §3.4, §5.6, §7 for the design contract.

#![warn(missing_docs)]
#![warn(clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]
#![allow(clippy::missing_errors_doc)]
#![allow(clippy::missing_panics_doc)]

// Modules added incrementally in Phase 7+.
pub mod connection;
pub use connection::{McpHealth, McpServerConnection, stderr_log_path};

/// Typed transport cleanup outcomes; host writer proof is a separate boundary.
pub mod cleanup;
pub mod error;
pub use error::McpError;

pub mod redact;

pub mod diagnostics;

pub mod registry;
/// Injected run-owned child writer observation.
pub mod writer_observer;
pub use registry::{McpContent, McpRegistry, McpServerCatalog, McpToolEntry, McpToolResult};

/// Authenticated per-session stdio stage-tool transport.
pub mod stage;

mod child_settlement;
pub use child_settlement::shutdown_children_and_join;
