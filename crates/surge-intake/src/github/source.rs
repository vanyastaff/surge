//! `GitHubIssuesTaskSource` — `TaskSource` impl backed by GitHub REST.

use crate::github::client::GitHubClient;
use crate::source::{MergeReadiness, TaskSource};
use crate::types::{TaskDetails, TaskEvent, TaskEventKind, TaskId, TaskSummary};
use crate::{Error, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures::stream::{self, BoxStream};
use std::collections::{HashMap, VecDeque};
use std::time::Duration;
use tokio::sync::Mutex;
use tracing::warn;

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

    async fn build_events_from_page_items(
        &self,
        items: Vec<octocrab::models::issues::Issue>,
        since: Option<DateTime<Utc>>,
    ) -> VecDeque<Result<TaskEvent>> {
        let mut queue = VecDeque::new();
        let mut last_updated = since;

        for issue in items.iter() {
            let is_newer = last_updated.map(|u| issue.updated_at > u).unwrap_or(true);
            if is_newer {
                last_updated = Some(issue.updated_at);
            }
            queue.push_back(Ok(TaskEvent {
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

        queue
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
    /// Uses an internal queue to emit all issues from a single fetch before polling again.
    ///
    /// Rate-limit errors and authentication failures are mapped to `Error::RateLimited`
    /// and `Error::AuthFailed` respectively, allowing the consumer to implement
    /// backoff logic.
    #[allow(clippy::excessive_nesting)]
    fn watch_for_tasks<'a>(&'a self) -> BoxStream<'a, Result<TaskEvent>> {
        use std::collections::VecDeque;

        type State<'a> = (&'a GitHubIssuesTaskSource, VecDeque<Result<TaskEvent>>);
        let initial: State<'a> = (self, VecDeque::new());

        Box::pin(stream::unfold(initial, |(this, mut queue)| async move {
            // Drain the queue first; emit one event per stream pull.
            if let Some(item) = queue.pop_front() {
                return Some((item, (this, queue)));
            }

            // Queue empty: fetch a fresh page and populate the queue.
            loop {
                tokio::time::sleep(this.poll_interval).await;
                let since = *this.last_seen_updated_at.lock().await;

                // Build the paginated request(s).
                //
                // octocrab's `.labels(&[a, b])` joins labels with a comma which
                // GitHub interprets as AND. The RFC's L0/L1/L3 levels (e.g.
                // `surge:enabled` vs `surge:auto`) are alternatives, so when
                // we have more than one label filter we issue one request per
                // label and union the results by issue id.
                let owner = this.client.owner.clone();
                let repo = this.client.repo.clone();
                let labels = this.label_filters.clone();
                let issue_handler = this.client.octocrab.issues(&owner, &repo);

                let pages = if labels.is_empty() {
                    let mut b = issue_handler.list().state(octocrab::params::State::Open);
                    if let Some(s) = since {
                        b = b.since(s);
                    }
                    match b.send().await {
                        Ok(p) => vec![p],
                        Err(e) => {
                            warn!(error = %e, "github poll failed");
                            let mapped = Self::map_octocrab_error(&e);
                            return Some((Err(mapped), (this, queue)));
                        },
                    }
                } else {
                    let mut all = Vec::new();
                    let mut error: Option<octocrab::Error> = None;
                    for label in labels.iter() {
                        let one_label = vec![label.clone()];
                        let mut b = issue_handler
                            .list()
                            .state(octocrab::params::State::Open)
                            .labels(&one_label);
                        if let Some(s) = since {
                            b = b.since(s);
                        }
                        match b.send().await {
                            Ok(p) => all.push(p),
                            Err(e) => {
                                warn!(error = %e, label = %label, "github poll for label failed");
                                error = Some(e);
                                break;
                            },
                        }
                    }
                    if let Some(e) = error {
                        let mapped = Self::map_octocrab_error(&e);
                        return Some((Err(mapped), (this, queue)));
                    }
                    all
                };

                // Union: dedup by issue.id.
                let mut seen_ids = std::collections::HashSet::new();
                let mut union_items = Vec::new();
                for page in pages {
                    for issue in page.items {
                        if seen_ids.insert(issue.id.into_inner()) {
                            union_items.push(issue);
                        }
                    }
                }

                queue = this.build_events_from_page_items(union_items, since).await;
                if let Some(item) = queue.pop_front() {
                    return Some((item, (this, queue)));
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
        let issue_handler = self
            .client
            .octocrab
            .issues(&self.client.owner, &self.client.repo);

        // Same OR-semantics handling as `watch_for_tasks`: if multiple labels
        // are configured, query each separately and union by issue id.
        let pages = if self.label_filters.is_empty() {
            vec![
                issue_handler
                    .list()
                    .state(octocrab::params::State::Open)
                    .send()
                    .await
                    .map_err(|e| Error::Network(e.to_string()))?,
            ]
        } else {
            let mut all = Vec::new();
            for label in &self.label_filters {
                let one_label = vec![label.clone()];
                let page = issue_handler
                    .list()
                    .state(octocrab::params::State::Open)
                    .labels(&one_label)
                    .send()
                    .await
                    .map_err(|e| Error::Network(e.to_string()))?;
                all.push(page);
            }
            all
        };

        let mut seen = std::collections::HashSet::new();
        let mut summaries = Vec::new();
        for page in pages {
            for i in page.items {
                if !seen.insert(i.id.into_inner()) {
                    continue;
                }
                summaries.push(TaskSummary {
                    task_id: self.task_id(i.number),
                    title: i.title.clone(),
                    status: Self::issue_state_to_string(&i.state),
                    url: i.html_url.to_string(),
                    updated_at: i.updated_at,
                });
            }
        }
        Ok(summaries)
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

    /// Evaluate whether the PR linked to this issue is ready to auto-merge.
    ///
    /// **MVP assumption:** the PR number equals the issue number. This is
    /// the common GitHub workflow where Surge creates one PR per ticket
    /// in the same numbering sequence. Multi-PR or branch-linked workflows
    /// will need an explicit PR-to-issue mapping; tracked as a follow-up.
    ///
    /// Logical "not ready" outcomes (failing checks, missing approval,
    /// PR not found, draft, merged) are returned as
    /// [`MergeReadiness::Blocked`] with a human-readable reason. Transport
    /// failures (network, auth, rate-limit) propagate as [`Error`].
    async fn check_merge_readiness(&self, id: &TaskId) -> Result<MergeReadiness> {
        let number = parse_issue_number(id.as_str())?;
        let pulls = self
            .client
            .octocrab
            .pulls(&self.client.owner, &self.client.repo);

        // 1. Resolve the PR. A 404 here means the issue number is not
        // backed by a PR — surface that as a clear blocked reason.
        let pr = match pulls.get(number).await {
            Ok(p) => p,
            Err(octocrab::Error::GitHub { source, .. })
                if source.message.to_lowercase().contains("not found") =>
            {
                return Ok(MergeReadiness::Blocked(format!(
                    "no pull request found for #{number} (Surge assumes PR# == issue#)"
                )));
            },
            Err(e) => return Err(Self::map_octocrab_error(&e)),
        };

        // 2. Fast-fail on terminal PR states.
        if pr.merged_at.is_some() {
            return Ok(MergeReadiness::Blocked(format!(
                "PR #{number} is already merged"
            )));
        }
        if matches!(pr.draft, Some(true)) {
            return Ok(MergeReadiness::Blocked(format!(
                "PR #{number} is in draft state"
            )));
        }
        if !matches!(pr.state, Some(octocrab::models::IssueState::Open)) {
            return Ok(MergeReadiness::Blocked(format!("PR #{number} is not open")));
        }

        // 3. Inspect mergeable_state — only `Clean` and `HasHooks` proceed.
        // See octocrab::models::pulls::MergeableState for canonical values.
        use octocrab::models::pulls::MergeableState;
        match pr.mergeable_state {
            Some(MergeableState::Clean) | Some(MergeableState::HasHooks) => {
                // proceed to review check
            },
            Some(MergeableState::Behind) => {
                return Ok(MergeReadiness::Blocked(format!(
                    "PR #{number} is behind the base branch — rebase or merge upstream"
                )));
            },
            Some(MergeableState::Blocked) => {
                return Ok(MergeReadiness::Blocked(format!(
                    "PR #{number} is blocked by required checks or missing review"
                )));
            },
            Some(MergeableState::Dirty) => {
                return Ok(MergeReadiness::Blocked(format!(
                    "PR #{number} has merge conflicts"
                )));
            },
            Some(MergeableState::Unstable) => {
                return Ok(MergeReadiness::Blocked(format!(
                    "PR #{number} has non-required checks failing"
                )));
            },
            Some(MergeableState::Draft) => {
                return Ok(MergeReadiness::Blocked(format!(
                    "PR #{number} is in draft state"
                )));
            },
            Some(MergeableState::Unknown) | None => {
                return Ok(MergeReadiness::Blocked(format!(
                    "PR #{number} mergeable_state not yet computed by GitHub"
                )));
            },
            Some(other) => {
                return Ok(MergeReadiness::Blocked(format!(
                    "PR #{number} mergeable_state unrecognised ({other:?})"
                )));
            },
        }

        // 4. Reviews — need at least one APPROVED, no later CHANGES_REQUESTED.
        let reviews_page = pulls
            .list_reviews(number)
            .per_page(100)
            .send()
            .await
            .map_err(|e| Self::map_octocrab_error(&e))?;

        Ok(evaluate_reviews(number, &reviews_page.items))
    }
}

/// Reduce a list of reviews to a Ready/Blocked verdict.
///
/// For each author we keep only their **latest** non-comment review
/// (Approved / ChangesRequested / Dismissed). Comments and pending
/// reviews don't count. A current `ChangesRequested` blocks; otherwise
/// at least one current `Approved` is required.
fn evaluate_reviews(pr_number: u64, reviews: &[octocrab::models::pulls::Review]) -> MergeReadiness {
    let mut latest_per_author: HashMap<String, &octocrab::models::pulls::Review> = HashMap::new();
    for review in reviews {
        // Reviews are listed in chronological order; later writes win.
        let Some(state) = &review.state else {
            continue;
        };
        if matches!(
            state,
            octocrab::models::pulls::ReviewState::Commented
                | octocrab::models::pulls::ReviewState::Pending
        ) {
            continue;
        }
        let author = review
            .user
            .as_ref()
            .map(|u| u.login.clone())
            .unwrap_or_default();
        latest_per_author.insert(author, review);
    }

    let has_changes_requested = latest_per_author.values().any(|r| {
        matches!(
            r.state,
            Some(octocrab::models::pulls::ReviewState::ChangesRequested)
        )
    });
    let approved_count = latest_per_author
        .values()
        .filter(|r| {
            matches!(
                r.state,
                Some(octocrab::models::pulls::ReviewState::Approved)
            )
        })
        .count();

    if has_changes_requested {
        return MergeReadiness::Blocked(format!(
            "PR #{pr_number} has reviewers requesting changes"
        ));
    }
    if approved_count == 0 {
        return MergeReadiness::Blocked(format!("PR #{pr_number} has no approving reviews"));
    }
    MergeReadiness::Ready
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
