//! `MinimalBootstrapGraphBuilder` — populated in Task 3.2.

use crate::bootstrap::builder::{
    BootstrapBuildError, BootstrapGraphBuilder, BootstrapPrompt,
};
use async_trait::async_trait;
use std::path::PathBuf;
use surge_core::graph::Graph;
use surge_core::id::RunId;

/// Single-stage Agent bootstrap (filled in Task 3.2).
#[derive(Debug, Clone, Default)]
pub struct MinimalBootstrapGraphBuilder;

impl MinimalBootstrapGraphBuilder {
    /// Construct a new builder.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl BootstrapGraphBuilder for MinimalBootstrapGraphBuilder {
    async fn build(
        &self,
        _run_id: RunId,
        _prompt: BootstrapPrompt,
        _worktree: PathBuf,
    ) -> Result<Graph, BootstrapBuildError> {
        Err(BootstrapBuildError::GraphBuild(
            "MinimalBootstrapGraphBuilder is a placeholder; Task 3.2 fills this in".into(),
        ))
    }
}
