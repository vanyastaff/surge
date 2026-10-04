//! `surge-daemon` — long-running process that hosts the M7+ engine
//! and exposes it over IPC. See
//! `docs/ARCHITECTURE.md`
//! §3 and §6 for the design contract.

#![warn(missing_docs)]
#![warn(clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]
#![allow(clippy::missing_errors_doc)]
#![allow(clippy::missing_panics_doc)]

// Modules added incrementally in Phase 3+.
pub mod admission;
pub mod automation_merge_gate;
mod bootstrap_cancel;
mod bootstrap_continuation;
pub mod bootstrap_recovery;
pub mod bootstrap_runtime;
pub mod bootstrap_supervisor;
pub mod broadcast;
pub mod error;
pub mod inbox;
pub mod intake_completion;
mod intake_delivery;
pub mod lifecycle;
pub mod pidfile;
pub mod recovery;
pub mod server;
pub mod tracked_run;
pub mod wake_scheduler;

pub use error::DaemonError;
pub use server::{
    ServerConfig, run_runs_only, run_synthetic as run_synthetic_server, run_with_supervisor,
};

mod owned_flows;
mod work_items;
