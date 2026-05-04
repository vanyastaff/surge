//! Engine — drives a frozen `Graph` through ACP sessions and persistence.
//!
//! See `docs/superpowers/specs/2026-05-03-surge-orchestrator-engine-m5-design.md`
//! for the full design contract. M5 ships sequential-pipeline-only support;
//! parallel/loops/subgraphs are M6 scope and rejected at run-start.

#![warn(missing_docs)]
#![warn(clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]
#![allow(clippy::missing_errors_doc)]
#![allow(clippy::missing_panics_doc)]

// Submodules added incrementally as later phases land.
pub mod config;
pub mod engine;
pub mod error;
pub mod handle;
pub mod predicates;
pub mod replay;
pub mod routing;
pub mod run_task;
pub mod sandbox_factory;
pub mod snapshot;
pub mod stage;
pub mod tools;
pub mod validate;

pub use config::{EngineConfig, EngineRunConfig, SnapshotPolicy};
pub use engine::Engine;
pub use error::EngineError;
pub use handle::{EngineRunEvent, RunHandle, RunOutcome};
