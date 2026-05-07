//! `surge-daemon` — long-running process that hosts the M7+ engine
//! and exposes it over IPC. See
//! `docs/superpowers/specs/2026-05-04-surge-orchestrator-engine-m7-design.md`
//! §3 and §6 for the design contract.

#![warn(missing_docs)]
#![warn(clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]
#![allow(clippy::missing_errors_doc)]
#![allow(clippy::missing_panics_doc)]

// Modules added incrementally in Phase 3+.
pub mod admission;
pub mod broadcast;
pub mod error;
pub mod inbox;
pub mod intake_completion;
pub mod lifecycle;
pub mod pidfile;
pub mod server;

pub use error::DaemonError;
pub use server::{ServerConfig, run as run_server, run_with_registry};
