//! Bootstrap-graph construction.
//!
//! The `BootstrapGraphBuilder` trait abstracts how a user prompt becomes the
//! initial `Graph` for an `Engine::start_run` invocation. Today's
//! `MinimalBootstrapGraphBuilder` produces a single-Agent graph; future
//! `StagedBootstrapGraphBuilder` (RFC-0004) will produce the 6-node prelude
//! Description Author → Approve → Roadmap Planner → Approve → Flow Generator
//! → Approve.

mod builder;
mod minimal;

pub use builder::{BootstrapBuildError, BootstrapGraphBuilder, BootstrapPrompt};
pub use minimal::MinimalBootstrapGraphBuilder;
