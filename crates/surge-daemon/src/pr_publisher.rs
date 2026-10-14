//! Publishes an L3 ticket run's work as a pull request (#127): commit what
//! the agent changed on the run branch, push it to the project's `origin`,
//! and open (or adopt) the PR that closes the ticket. The engine calls this
//! at the run's `Success` terminal and records the result in the run's log.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use surge_intake::TaskSource;
use surge_intake::source::PullRequestDraft;
use surge_intake::types::TaskId;
use surge_orchestrator::engine::publish::{
    PublishOutcome, PublishRequest, PublishedPullRequest, PullRequestPublisher,
};

/// The remote a run branch is pushed to.
const REMOTE: &str = "origin";

/// Publishes one ticket's run.
pub struct TicketPullRequestPublisher {
    source: Arc<dyn TaskSource>,
    task_id: TaskId,
    ticket_title: String,
    /// The project checkout the run's worktree belongs to.
    project: PathBuf,
    /// HTTPS token for the push, when the tracker has one.
    token: Option<String>,
}

impl std::fmt::Debug for TicketPullRequestPublisher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TicketPullRequestPublisher")
            .field("task_id", &self.task_id)
            .field("project", &self.project)
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .finish_non_exhaustive()
    }
}

impl TicketPullRequestPublisher {
    /// A publisher for `task_id`'s run in `project`.
    #[must_use]
    pub fn new(
        source: Arc<dyn TaskSource>,
        task_id: TaskId,
        ticket_title: String,
        project: PathBuf,
        token: Option<String>,
    ) -> Self {
        Self {
            source,
            task_id,
            ticket_title,
            project,
            token,
        }
    }
}

#[async_trait]
impl PullRequestPublisher for TicketPullRequestPublisher {
    async fn publish(&self, request: &PublishRequest) -> Result<PublishOutcome, String> {
        let closing = closing_reference(&self.task_id)?;
        let draft_file = read_pr_draft(&request.worktree);
        let message = format!(
            "{}\n\n{closing}\n\nSurge run {}",
            self.ticket_title.trim(),
            request.run_id
        );
        let (worktree, project, token) = (
            request.worktree.clone(),
            self.project.clone(),
            self.token.clone(),
        );
        let pushed = tokio::task::spawn_blocking(move || {
            push_run_branch(&worktree, &project, &message, token.as_deref())
        })
        .await
        .map_err(|error| format!("publish task failed: {error}"))??;
        let Some((head_branch, head_sha)) = pushed else {
            return Ok(PublishOutcome::NothingToPublish);
        };
        let (title, body) = pr_text(draft_file.as_deref(), &self.ticket_title, &closing);
        let opened = self
            .source
            .open_pull_request(
                &self.task_id,
                &PullRequestDraft {
                    head_branch: head_branch.clone(),
                    title,
                    body,
                },
            )
            .await
            .map_err(|error| format!("could not open the pull request: {error}"))?;
        let pr = serde_json::from_value(serde_json::json!({
            "provider": self.source.provider(),
            "repository": repository_of(&self.task_id),
            "number": opened.number,
            "url": opened.url,
        }))
        .map_err(|error| format!("unusable pull request identity: {error}"))?;
        Ok(PublishOutcome::Published(PublishedPullRequest {
            pr,
            head_branch,
            head_sha,
        }))
    }
}

/// Commit the run's changes, and push the branch when it carries work.
/// `None`: nothing to publish (no new commit and the branch is still at the
/// project's `HEAD`).
fn push_run_branch(
    worktree: &Path,
    project: &Path,
    message: &str,
    token: Option<&str>,
) -> Result<Option<(String, String)>, String> {
    use surge_git::publish::{
        SURGE_OWNED_PATHS, branch_head, commit_changes, head_commit, push_branch,
    };
    let committed = commit_changes(worktree, message, SURGE_OWNED_PATHS)
        .map_err(|error| format!("could not commit the run's changes: {error}"))?;
    let (branch, head) =
        branch_head(worktree).map_err(|error| format!("could not read the run branch: {error}"))?;
    let base = head_commit(project)
        .map_err(|error| format!("could not read the project's HEAD: {error}"))?;
    if committed.is_none() && head == base {
        return Ok(None);
    }
    push_branch(worktree, REMOTE, &branch, token)
        .map_err(|error| format!("could not push {branch}: {error}"))?;
    Ok(Some((branch, head.to_string())))
}

/// `Closes #N` for a ticket `…#N`.
fn closing_reference(task_id: &TaskId) -> Result<String, String> {
    task_id
        .as_str()
        .rsplit_once('#')
        .and_then(|(_, number)| number.parse::<u64>().ok())
        .map(|number| format!("Closes #{number}"))
        .ok_or_else(|| format!("ticket {task_id} has no issue number to close"))
}

/// `owner/repo` of a ticket `provider:owner/repo#N`.
fn repository_of(task_id: &TaskId) -> String {
    let id = task_id.as_str();
    let after_provider = id.split_once(':').map_or(id, |(_, rest)| rest);
    after_provider
        .rsplit_once('#')
        .map_or(after_provider, |(repository, _)| repository)
        .to_owned()
}

/// The PR draft a flow may write (`pr.md`: title, `---`, body).
fn read_pr_draft(worktree: &Path) -> Option<String> {
    std::fs::read_to_string(worktree.join("pr.md")).ok()
}

/// Title and body from the draft when it has both parts, otherwise from the
/// ticket. The body always closes the ticket.
fn pr_text(draft: Option<&str>, ticket_title: &str, closing: &str) -> (String, String) {
    let drafted = draft.and_then(|text| {
        let (title, body) = text.split_once("\n---\n")?;
        let title = title.trim().trim_start_matches('#').trim();
        (!title.is_empty()).then(|| (title.to_owned(), body.trim().to_owned()))
    });
    let (title, mut body) = drafted.unwrap_or_else(|| {
        (
            ticket_title.trim().to_owned(),
            "Implemented by Surge.".to_owned(),
        )
    });
    if !body.contains(closing) {
        body = format!("{body}\n\n{closing}");
    }
    (title, body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_body_always_closes_the_ticket_and_a_draft_wins_when_complete() {
        let closing = "Closes #42";
        assert_eq!(
            pr_text(None, " Fix the parser ", closing),
            (
                "Fix the parser".to_owned(),
                "Implemented by Surge.\n\nCloses #42".to_owned()
            )
        );
        assert_eq!(
            pr_text(
                Some("# fix(parser): no panic\n---\n## Summary\n- guard depth\n"),
                "ticket",
                closing
            ),
            (
                "fix(parser): no panic".to_owned(),
                "## Summary\n- guard depth\n\nCloses #42".to_owned()
            )
        );
        let (_, body) = pr_text(Some("t\n---\nbody\n\nCloses #42"), "ticket", closing);
        assert_eq!(body.matches("Closes #42").count(), 1, "not repeated");
        assert_eq!(pr_text(Some("no separator"), "ticket", closing).0, "ticket");
    }

    #[test]
    fn ticket_identity_yields_the_issue_and_the_repository() {
        let id = TaskId::try_new("github_issues:acme/app#42").unwrap();
        assert_eq!(closing_reference(&id).unwrap(), "Closes #42");
        assert_eq!(repository_of(&id), "acme/app");
        assert!(closing_reference(&TaskId::try_new("mock:t").unwrap()).is_err());
    }
}
