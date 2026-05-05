//! `surge-mcp` — MCP (Model Context Protocol) client integration for
//! `surge-orchestrator` agent stages. Wraps the official `rmcp` crate
//! with surge-flavoured registry, connection state, restart policy.
//!
//! See `docs/superpowers/specs/2026-05-04-surge-orchestrator-engine-m7-design.md`
//! §3.4, §5.6, §7 for the design contract.

#![warn(missing_docs)]
#![warn(clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]
#![allow(clippy::missing_errors_doc)]
#![allow(clippy::missing_panics_doc)]

// Modules added incrementally in Phase 7+.
pub mod connection;
pub use connection::McpServerConnection;

pub mod error;
pub use error::McpError;
