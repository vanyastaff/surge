//! `BootstrapGraphBuilder` trait + shared types.

use async_trait::async_trait;
use std::path::PathBuf;
use surge_core::graph::Graph;
use surge_core::id::RunId;
use surge_intake::types::Priority;
use thiserror::Error;

/// Build the initial `Graph` for a user-initiated run.
///
/// Implementations are stateless and shareable behind `Arc<dyn ...>`. The
/// daemon's inbox consumer holds one and invokes `build` on each Start tap.
#[async_trait]
pub trait BootstrapGraphBuilder: Send + Sync {
    /// Build a bootstrap graph for the given prompt and project context.
    ///
    /// `run_id` is the engine RunId already allocated by the caller; the
    /// builder may bake it into node IDs or leave it implicit. `worktree`
    /// is the absolute path of the worktree the run will execute in;
    /// builders that read project context (existing files, git status)
    /// should consult this directory only.
    async fn build(
        &self,
        run_id: RunId,
        prompt: BootstrapPrompt,
        worktree: PathBuf,
    ) -> Result<Graph, BootstrapBuildError>;
}

/// Free-text prompt + structured ticket metadata that the builder may use.
///
/// `MinimalBootstrapGraphBuilder` only reads `description`. Future
/// `StagedBootstrapGraphBuilder` will read all fields to populate the
/// Description Author preamble.
#[derive(Debug, Clone)]
pub struct BootstrapPrompt {
    /// Ticket title or one-line user prompt.
    pub title: String,
    /// Full body of the user's request.
    pub description: String,
    /// Optional URL of the originating ticket.
    pub tracker_url: Option<String>,
    /// Priority assigned by Triage Author (or none).
    pub priority: Option<Priority>,
    /// Tracker labels visible at intake time.
    pub labels: Vec<String>,
}

/// Errors a builder may report.
#[derive(Debug, Error)]
pub enum BootstrapBuildError {
    /// Caller-supplied prompt is malformed (e.g., empty description).
    #[error("invalid prompt: {0}")]
    InvalidPrompt(String),
    /// Internal graph construction failed.
    #[error("graph construction failed: {0}")]
    GraphBuild(String),
    /// A required profile is not present in the registry.
    #[error("profile not available: {0}")]
    ProfileMissing(String),
}
