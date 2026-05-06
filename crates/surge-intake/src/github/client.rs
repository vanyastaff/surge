//! Thin wrapper over `octocrab` providing `GitHubClient` with auth + helpers.
//!
//! Used by `GitHubIssuesTaskSource` (T6.2) to call REST endpoints. Currently
//! exposes the raw [`octocrab::Octocrab`] instance plus the configured
//! `(owner, repo)` pair, so callers can drive `.issues(owner, repo)`,
//! `.list_comments(...)`, etc. directly.

use crate::{Error, Result};
use octocrab::Octocrab;
use std::sync::Arc;

/// Authenticated GitHub REST client targeting a single repository.
#[derive(Clone)]
pub struct GitHubClient {
    /// Underlying `octocrab` instance, shared via `Arc` for cheap cloning.
    pub octocrab: Arc<Octocrab>,
    /// Repository owner (user or organisation).
    pub owner: String,
    /// Repository name.
    pub repo: String,
}

impl GitHubClient {
    /// Construct a new client authenticated with the given Personal Access Token.
    pub fn new(api_token: &str, owner: String, repo: String) -> Result<Self> {
        let octo = Octocrab::builder()
            .personal_token(api_token.to_string())
            .build()
            .map_err(|e| Error::AuthFailed(format!("octocrab build: {e}")))?;
        Ok(Self {
            octocrab: Arc::new(octo),
            owner,
            repo,
        })
    }
}
