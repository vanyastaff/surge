//! Publishing a successful run's work as a pull request (#127).
//!
//! The engine owns *when*: at a `Success` terminal, before `RunCompleted`, so
//! the run's own log records the PR (events after the terminal are writer
//! bugs, #95). The host owns *how* — commit, push, open — through
//! [`PullRequestPublisher`], set on [`super::config::EngineRunConfig`] only
//! for runs that should publish (L3 tickets). A failure to publish never
//! fails the run: it completes unpublished, and the merge gate then blocks
//! visibly because no pull request is linked.

use std::path::PathBuf;

use async_trait::async_trait;
use surge_core::id::RunId;
use surge_core::run_event::{EventPayload, VersionedEventPayload};
use surge_core::work_item::WorkItemPr;
use surge_persistence::runs::run_writer::RunWriter;

/// What the engine hands the publisher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishRequest {
    /// The run whose work is published.
    pub run_id: RunId,
    /// The run's git worktree.
    pub worktree: PathBuf,
}

/// The pull request that now carries the run's work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishedPullRequest {
    /// Provider, repository, number and URL.
    pub pr: WorkItemPr,
    /// The branch the PR merges from.
    pub head_branch: String,
    /// The commit the PR points at.
    pub head_sha: String,
}

/// What publishing did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublishOutcome {
    /// A pull request was opened (or an open one adopted).
    Published(PublishedPullRequest),
    /// The run changed nothing, so there is nothing to publish.
    NothingToPublish,
}

/// Publishes a run's worktree as a pull request.
#[async_trait]
pub trait PullRequestPublisher: Send + Sync + std::fmt::Debug {
    /// Commit what the run left, push its branch and open the pull request.
    ///
    /// # Errors
    /// An operator-readable reason; it must never contain credentials.
    async fn publish(&self, request: &PublishRequest) -> Result<PublishOutcome, String>;
}

/// Publish before the terminal and record the PR in the run's log. Skips a
/// run whose log already records one (a resume after the record).
pub(crate) async fn publish_before_terminal(
    publisher: &dyn PullRequestPublisher,
    writer: &RunWriter,
    request: PublishRequest,
) {
    if already_recorded(writer).await {
        tracing::info!(run_id = %request.run_id, "pull request already recorded; not publishing again");
        return;
    }
    match publisher.publish(&request).await {
        Ok(PublishOutcome::Published(opened)) => {
            let number = opened.pr.number;
            let recorded = writer
                .append_event(VersionedEventPayload::new(
                    EventPayload::PullRequestOpened {
                        pr: opened.pr,
                        head_branch: opened.head_branch,
                        head_sha: opened.head_sha,
                    },
                ))
                .await;
            match recorded {
                Ok(_) => {
                    tracing::info!(run_id = %request.run_id, pr = number, "pull request opened");
                },
                Err(error) => tracing::error!(
                    run_id = %request.run_id,
                    pr = number,
                    %error,
                    "pull request opened but not recorded; the merge gate falls back to the issue's links"
                ),
            }
        },
        Ok(PublishOutcome::NothingToPublish) => {
            tracing::info!(run_id = %request.run_id, "run changed nothing; no pull request");
        },
        Err(reason) => tracing::error!(
            run_id = %request.run_id,
            %reason,
            "could not publish the run's pull request; it completes unpublished"
        ),
    }
}

async fn already_recorded(writer: &RunWriter) -> bool {
    match writer.reader().read_run_events().await {
        Ok(events) => events
            .iter()
            .any(|event| matches!(event.payload, EventPayload::PullRequestOpened { .. })),
        Err(error) => {
            tracing::warn!(%error, "cannot read the run log before publishing; publishing anyway (the provider adopts an open PR)");
            false
        },
    }
}
