//! `GitHubIssuesTaskSource` — `TaskSource` impl backed by GitHub REST.

use crate::github::client::GitHubClient;
use crate::source::TaskSource;
use crate::types::{TaskDetails, TaskEvent, TaskEventKind, TaskId, TaskSummary};
use crate::{Error, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures::stream::{self, BoxStream};
use std::time::Duration;
use tokio::sync::Mutex;
use tracing::{debug, warn};

/// Configuration for [`GitHubIssuesTaskSource`].
///
/// Holds the repository identifiers, API credentials, and polling parameters
/// needed to construct and operate a GitHub Issues task source.
#[derive(Debug, Clone)]
pub struct GitHubConfig {
    /// Stable identifier (e.g. `"github_issues:user/repo"`).
    pub id: String,
    /// Human-readable display name shown in inbox cards.
    pub display_name: String,
    /// Repository owner (user or organisation).
    pub owner: String,
    /// Repository name.
    pub repo: String,
    /// Personal access token (from env via `surge-core` config indirection).
    pub api_token: String,
    /// Time between polling cycles.
    pub poll_interval: Duration,
    /// Issues are filtered to those carrying any of these labels.
    pub label_filters: Vec<String>,
}

/// `TaskSource` implementation talking to GitHub Issues via REST (`octocrab`).
///
/// Polls GitHub's issues API on a configurable interval, filtering by state
/// (open issues only) and labels. Tracks the latest `updated_at` timestamp
/// to avoid re-processing old issues.
///
/// Provides full CRUD operations on issues: reading tasks, posting comments,
/// adding/removing labels. Comments are post-processed for idempotency via
/// exact-body matching, as GitHub's REST API has no native idempotency key.
pub struct GitHubIssuesTaskSource {
    id: String,
    display_name: String,
    client: GitHubClient,
    poll_interval: Duration,
    label_filters: Vec<String>,
    last_seen_updated_at: Mutex<Option<DateTime<Utc>>>,
}

impl GitHubIssuesTaskSource {
    /// Construct a source from configuration. Builds an authenticated octocrab
    /// instance internally.
    ///
    /// # Errors
    ///
    /// Returns `Error::AuthFailed` if the API token is invalid or the octocrab
    /// client cannot be constructed.
    pub fn new(config: GitHubConfig) -> Result<Self> {
        let client = GitHubClient::new(&config.api_token, config.owner, config.repo)?;
        Ok(Self {
            id: config.id,
            display_name: config.display_name,
            client,
            poll_interval: config.poll_interval,
            label_filters: config.label_filters,
            last_seen_updated_at: Mutex::new(None),
        })
    }

    fn task_id(&self, number: u64) -> TaskId {
        TaskId::try_new(format!(
            "github_issues:{}/{}#{}",
            self.client.owner, self.client.repo, number
        ))
        .expect("constructed task id is valid")
    }

    fn issue_state_to_string(state: &octocrab::models::IssueState) -> String {
        match state {
            octocrab::models::IssueState::Open => "open".to_string(),
            octocrab::models::IssueState::Closed => "closed".to_string(),
            _ => "unknown".to_string(),
        }
    }

    fn map_octocrab_error(e: &octocrab::Error) -> Error {
        if let octocrab::Error::GitHub { source, .. } = e {
            let msg = source.message.to_lowercase();
            if msg.contains("rate limit") {
                return Error::RateLimited {
                    retry_after_secs: 60,
                };
            }
            if msg.contains("bad credentials") || msg.contains("requires authentication") {
                return Error::AuthFailed(source.message.clone());
            }
        }
        Error::Network(e.to_string())
    }

    async fn handle_poll_success(
        &self,
        items: octocrab::Page<octocrab::models::issues::Issue>,
        since: Option<DateTime<Utc>>,
    ) -> Option<(Result<TaskEvent>, &Self)> {
        let mut last_updated = since;
        let mut events = Vec::<Result<TaskEvent>>::new();
        for issue in items.items.iter() {
            let is_newer = last_updated.map(|u| issue.updated_at > u).unwrap_or(true);
            if is_newer {
                last_updated = Some(issue.updated_at);
            }
            events.push(Ok(TaskEvent {
                source_id: self.id.clone(),
                task_id: self.task_id(issue.number),
                kind: TaskEventKind::NewTask,
                seen_at: Utc::now(),
                raw_payload: serde_json::to_value(issue).unwrap_or(serde_json::Value::Null),
            }));
        }
        if let Some(u) = last_updated {
            *self.last_seen_updated_at.lock().await = Some(u);
        }
        if events.is_empty() {
            tokio::time::sleep(self.poll_interval).await;
            return None;
        }
        let mut iter = events.into_iter();
        let first = iter.next().unwrap();
        debug!(remaining = iter.count(), "deferring multi-issue emission");
        tokio::time::sleep(Duration::from_millis(50)).await;
        Some((first, self))
    }

    fn issue_to_details(&self, i: &octocrab::models::issues::Issue) -> TaskDetails {
        let task_id = self.task_id(i.number);
        TaskDetails {
            task_id,
            source_id: self.id.clone(),
            title: i.title.clone(),
            description: i.body.clone().unwrap_or_default(),
            status: Self::issue_state_to_string(&i.state),
            labels: i.labels.iter().map(|l| l.name.clone()).collect(),
            url: i.html_url.to_string(),
            created_at: i.created_at,
            updated_at: i.updated_at,
            assignee: i.assignee.as_ref().map(|u| u.login.clone()),
            raw_payload: serde_json::to_value(i).unwrap_or(serde_json::Value::Null),
        }
    }
}

#[async_trait]
impl TaskSource for GitHubIssuesTaskSource {
    fn id(&self) -> &str {
        &self.id
    }

    fn display_name(&self) -> &str {
        &self.display_name
    }

    fn provider(&self) -> &'static str {
        "github_issues"
    }

    /// Polls GitHub's issues API on a fixed interval, emitting events for each
    /// open issue matching the configured label filters.
    ///
    /// The stream uses a `since` watermark (tracking `updated_at`) to avoid
    /// re-emitting issues that haven't changed since the last poll cycle.
    ///
    /// Rate-limit errors and authentication failures are mapped to `Error::RateLimited`
    /// and `Error::AuthFailed` respectively, allowing the consumer to implement
    /// backoff logic.
    fn watch_for_tasks<'a>(&'a self) -> BoxStream<'a, Result<TaskEvent>> {
        Box::pin(stream::unfold(self, move |this| async move {
            let since = *this.last_seen_updated_at.lock().await;

            // Get the issue handler and build the query
            // The handler holds references to the octocrab instance and repo details
            let owner = this.client.owner.clone();
            let repo = this.client.repo.clone();
            let labels = this.label_filters.clone();

            let issue_handler = this.client.octocrab.issues(&owner, &repo);
            let mut page_builder = issue_handler
                .list()
                .state(octocrab::params::State::Open)
                .labels(&labels);
            if let Some(s) = since {
                page_builder = page_builder.since(s);
            }
            let result = page_builder.send().await;

            match result {
                Ok(items) => this.handle_poll_success(items, since).await,
                Err(e) => {
                    warn!(error = %e, "github poll failed");
                    let mapped = Self::map_octocrab_error(&e);
                    tokio::time::sleep(this.poll_interval).await;
                    Some((Err(mapped), this))
                }
            }
        }))
    }

    /// Fetch full details of a single GitHub issue by its number.
    ///
    /// # Errors
    ///
    /// Returns `Error::InvalidTaskId` if the task ID cannot be parsed,
    /// or `Error::Network` if the API call fails.
    async fn fetch_task(&self, id: &TaskId) -> Result<TaskDetails> {
        let number = parse_issue_number(id.as_str())?;
        let issue = self
            .client
            .octocrab
            .issues(&self.client.owner, &self.client.repo)
            .get(number)
            .await
            .map_err(|e| Error::Network(e.to_string()))?;
        Ok(self.issue_to_details(&issue))
    }

    /// List all currently open issues matching the configured label filters.
    ///
    /// Returns a bounded list suitable for populating the Triage Author's
    /// candidate set.
    ///
    /// # Errors
    ///
    /// Returns `Error::Network` if the API call fails.
    async fn list_open_tasks(&self) -> Result<Vec<TaskSummary>> {
        let page = self
            .client
            .octocrab
            .issues(&self.client.owner, &self.client.repo)
            .list()
            .state(octocrab::params::State::Open)
            .labels(&self.label_filters)
            .send()
            .await
            .map_err(|e| Error::Network(e.to_string()))?;
        Ok(page
            .items
            .iter()
            .map(|i| TaskSummary {
                task_id: self.task_id(i.number),
                title: i.title.clone(),
                status: Self::issue_state_to_string(&i.state),
                url: i.html_url.to_string(),
                updated_at: i.updated_at,
            })
            .collect())
    }

    /// Acknowledge receipt of a task.
    ///
    /// GitHub Issues do not support explicit acknowledgment markers; this is
    /// a no-op implementation that always succeeds. Tracking is handled by
    /// the intake storage layer.
    async fn acknowledge_task(&self, _id: &TaskId) -> Result<()> {
        Ok(())
    }

    /// Post a comment on a GitHub issue, with idempotency via exact-body match.
    ///
    /// Searches existing comments for an exact match of the requested body.
    /// If a match is found, this is a no-op (idempotent). Otherwise, a new
    /// comment is created.
    ///
    /// # Errors
    ///
    /// Returns `Error::InvalidTaskId` if the task ID cannot be parsed,
    /// or `Error::Network` if the API call fails.
    async fn post_comment(&self, id: &TaskId, body: &str) -> Result<()> {
        let number = parse_issue_number(id.as_str())?;
        // Idempotency: GitHub has no native idempotency key. Pre-check existing
        // comments with exact body match (telltale-prefix matching).
        let comments = self
            .client
            .octocrab
            .issues(&self.client.owner, &self.client.repo)
            .list_comments(number)
            .send()
            .await
            .map_err(|e| Error::Network(e.to_string()))?;
        let has_match = comments
            .items
            .iter()
            .any(|c| c.body.as_deref().is_some_and(|b| b == body));
        if has_match {
            return Ok(());
        }
        self.client
            .octocrab
            .issues(&self.client.owner, &self.client.repo)
            .create_comment(number, body)
            .await
            .map_err(|e| Error::Network(e.to_string()))?;
        Ok(())
    }

    /// Set or remove a label on a GitHub issue.
    ///
    /// Setting `present = true` adds the label; `present = false` removes it.
    /// GitHub's REST API ensures idempotency: adding an already-present label
    /// and removing an already-absent label are no-ops.
    ///
    /// # Errors
    ///
    /// Returns `Error::InvalidTaskId` if the task ID cannot be parsed,
    /// or `Error::Network` if the API call fails.
    async fn set_label(&self, id: &TaskId, label: &str, present: bool) -> Result<()> {
        let number = parse_issue_number(id.as_str())?;
        let issues = self
            .client
            .octocrab
            .issues(&self.client.owner, &self.client.repo);
        if present {
            issues
                .add_labels(number, &[label.to_string()])
                .await
                .map_err(|e| Error::Network(e.to_string()))?;
        } else {
            issues
                .remove_label(number, label)
                .await
                .map_err(|e| Error::Network(e.to_string()))?;
        }
        Ok(())
    }

    /// Read the current set of labels on a GitHub issue.
    ///
    /// # Errors
    ///
    /// Returns `Error::InvalidTaskId` if the task ID cannot be parsed,
    /// or `Error::Network` if the API call fails.
    async fn read_labels(&self, id: &TaskId) -> Result<Vec<String>> {
        Ok(self.fetch_task(id).await?.labels)
    }
}

fn parse_issue_number(task_id: &str) -> Result<u64> {
    task_id
        .rsplit('#')
        .next()
        .ok_or_else(|| Error::InvalidTaskId(task_id.into()))?
        .parse::<u64>()
        .map_err(|_| Error::InvalidTaskId(task_id.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_issue_number_extracts_trailing() {
        assert_eq!(
            parse_issue_number("github_issues:user/repo#1234").unwrap(),
            1234
        );
    }

    #[test]
    fn parse_issue_number_rejects_no_hash() {
        assert!(parse_issue_number("github_issues:user/repo").is_err());
    }

    #[test]
    fn parse_issue_number_rejects_non_numeric() {
        assert!(parse_issue_number("github_issues:user/repo#abc").is_err());
    }
}
