//! Orchestrator — drives specs through the full pipeline.

pub mod context;
pub mod executor;
pub mod gates;
pub mod phases;
pub mod pipeline;
pub mod qa;

pub use phases::Phase;
pub use pipeline::{Orchestrator, OrchestratorConfig, PipelineResult};
