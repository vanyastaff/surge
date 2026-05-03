//! Orchestrator — drives specs through the full pipeline.

pub mod budget;
pub mod circuit_breaker;
pub mod conflict;
pub mod context;
pub mod executor;
pub mod gates;
pub mod parallel;
pub mod phases;
pub mod pipeline;
pub mod planner;
pub mod project;
pub mod qa;
pub mod retry;
pub mod schedule;
pub mod engine;

pub use budget::{BudgetStatus, BudgetTracker};
pub use parallel::ParallelExecutor;
pub use phases::Phase;
pub use pipeline::{Orchestrator, OrchestratorConfig, PipelineResult};
pub use planner::PlannerPhase;
pub use project::{ProjectConfig, ProjectExecutor, ProjectResult};
