//! `LinearTaskSource` — `TaskSource` impl over `lineark_sdk::Client`.

use crate::source::TaskSource;
use crate::types::{TaskDetails, TaskEvent, TaskEventKind, TaskId, TaskSummary};
use crate::{Error, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures::stream::{self, BoxStream};
use lineark_sdk::generated::inputs::{
    CommentCreateInput, DateComparator, IssueFilter, IssueLabelCollectionFilter, IssueLabelFilter,
    IssueUpdateInput, StringComparator,
};
use lineark_sdk::generated::types::{Comment, Issue, IssueLabel};
use lineark_sdk::{Client, LinearError, MaybeUndefined};
use std::time::Duration;
use tokio::sync::Mutex;
use tracing::warn;

/// Configuration for `LinearTaskSource`.
///
/// Contains API credentials, workspace identifier, polling interval, and label filters
/// to determine which tasks are ingested.
#[derive(Debug, Clone)]
pub struct LinearConfig {
    /// Stable identifier for this source instance (e.g., `"linear:wsp_acme"`).
    pub id: String,
    /// Human-readable name for UI display (e.g., `"ACME Linear"`).
    pub display_name: String,
    /// Linear workspace identifier (used in task IDs).
    pub workspace_id: String,
    /// Linear API token.
    pub api_token: String,
    /// How often to poll for new issues.
    pub poll_interval: Duration,
    /// Label names to filter on (e.g., `["surge:enabled"]`). Empty means no label filter.
    pub label_filters: Vec<String>,
}

/// Adapter for Linear issues via `lineark-sdk`.
///
/// Implements `TaskSource` by polling the Linear API for issues matching the configured labels
/// and updated-at timestamp.
pub struct LinearTaskSource {
    id: String,
    display_name: String,
    workspace_id: String,
    client: Client,
    poll_interval: Duration,
    label_filters: Vec<String>,
    last_seen_updated_at: Mutex<Option<DateTime<Utc>>>,
}

impl LinearTaskSource {
    /// Create a new Linear task source from configuration.
    ///
    /// # Errors
    ///
    /// Returns `Error::AuthFailed` if the token is invalid or empty.
    pub fn new(config: LinearConfig) -> Result<Self> {
        let client = Client::from_token(&config.api_token)
            .map_err(|e| Error::AuthFailed(e.to_string()))?;
        Ok(Self {
            id: config.id,
            display_name: config.display_name,
            workspace_id: config.workspace_id,
            client,
            poll_interval: config.poll_interval,
            label_filters: config.label_filters,
            last_seen_updated_at: Mutex::new(None),
        })
    }

    /// Override the Linear GraphQL endpoint URL (primarily for testing with wiremock).
    ///
    /// # Example
    ///
    /// ```ignore
    /// let mut source = LinearTaskSource::new(cfg)?;
    /// source.set_base_url("http://localhost:7777/graphql".into());
    /// ```
    pub fn set_base_url(&mut self, url: String) {
        self.client.set_base_url(url);
    }

    /// Convert a Linear Issue to TaskDetails.
    fn issue_to_details(&self, issue: &Issue) -> TaskDetails {
        let identifier = issue.identifier.as_deref().unwrap_or("unknown").to_string();
        let task_id = TaskId::try_new(format!("linear:{}/{}", self.workspace_id, identifier))
            .unwrap_or_else(|_| TaskId::try_new("linear:invalid").unwrap());

        let labels = issue
            .labels
            .as_ref()
            .and_then(|conn| conn.nodes.as_ref())
            .map(|nodes| {
                nodes
                    .iter()
                    .filter_map(|label| label.name.as_ref().cloned())
                    .collect()
            })
            .unwrap_or_default();

        TaskDetails {
            task_id,
            source_id: self.id.clone(),
            title: issue.title.as_deref().unwrap_or("").to_string(),
            description: issue.description.as_deref().unwrap_or("").to_string(),
            status: issue
                .state
                .as_ref()
                .and_then(|s| s.name.as_ref())
                .cloned()
                .unwrap_or_else(|| "unknown".to_string()),
            labels,
            url: issue.url.as_deref().unwrap_or("").to_string(),
            created_at: issue.created_at.unwrap_or_else(Utc::now),
            updated_at: issue.updated_at.unwrap_or_else(Utc::now),
            assignee: issue
                .assignee
                .as_ref()
                .and_then(|a| a.name.as_ref())
                .cloned(),
            raw_payload: serde_json::to_value(issue).unwrap_or_default(),
        }
    }

    /// Resolve a label name to its UUID via Linear API.
    async fn resolve_label_id(&self, label_name: &str) -> Result<String> {
        let filter = IssueLabelFilter {
            name: MaybeUndefined::Value(StringComparator {
                eq: MaybeUndefined::Value(label_name.to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };

        let labels = self.client
            .issue_labels::<IssueLabel>()
            .filter(filter)
            .first(1)
            .send()
            .await
            .map_err(map_err)?;

        labels
            .nodes
            .first()
            .and_then(|l| l.id.as_ref().cloned())
            .ok_or_else(|| Error::Internal(format!("label '{}' not found", label_name)))
    }
}

#[async_trait]
impl TaskSource for LinearTaskSource {
    /// Returns the stable identifier of this source.
    fn id(&self) -> &str {
        &self.id
    }

    /// Returns the human-readable name for UI display.
    fn display_name(&self) -> &str {
        &self.display_name
    }

    /// Returns the provider type tag.
    fn provider(&self) -> &'static str {
        "linear"
    }

    /// Stream of incoming task events.
    ///
    /// Polls the Linear API periodically for issues matching the label filters,
    /// emitting one TaskEvent per cycle. Filters by `updated_at > last_seen_updated_at`
    /// to avoid re-emitting unchanged tasks.
    fn watch_for_tasks<'a>(&'a self) -> BoxStream<'a, Result<TaskEvent>> {
        Box::pin(stream::unfold((), move |_| async move {
            loop {
                tokio::time::sleep(self.poll_interval).await;

                let result = self.fetch_and_emit_events().await;
                match result {
                    Ok(events) if !events.is_empty() => {
                        return Some((Ok(events[0].clone()), ()));
                    }
                    Err(e) => {
                        warn!("error polling linear issues: {}", e);
                        return Some((Err(e), ()));
                    }
                    Ok(_) => continue,
                }
            }
        }))
    }

    /// Fetch full details of a single task.
    async fn fetch_task(&self, id: &TaskId) -> Result<TaskDetails> {
        let identifier = id
            .as_str()
            .rsplit('/')
            .next()
            .ok_or_else(|| Error::InvalidTaskId(id.as_str().into()))?
            .to_string();

        let issue = self.client
            .issue::<Issue>(identifier)
            .await
            .map_err(map_err)?;

        Ok(self.issue_to_details(&issue))
    }

    /// List currently open tasks (up to 25).
    async fn list_open_tasks(&self) -> Result<Vec<TaskSummary>> {
        let filter = if !self.label_filters.is_empty() {
            let label_name = &self.label_filters[0];
            let label_filter = IssueLabelFilter {
                name: MaybeUndefined::Value(StringComparator {
                    eq: MaybeUndefined::Value(label_name.to_string()),
                    ..Default::default()
                }),
                ..Default::default()
            };
            IssueFilter {
                labels: MaybeUndefined::Value(Box::new(IssueLabelCollectionFilter {
                    some: MaybeUndefined::Value(Box::new(label_filter)),
                    ..Default::default()
                })),
                ..Default::default()
            }
        } else {
            IssueFilter::default()
        };

        let result = self.client
            .issues::<Issue>()
            .filter(filter)
            .first(25)
            .send()
            .await
            .map_err(map_err)?;

        Ok(result
            .nodes
            .iter()
            .map(|issue| TaskSummary {
                task_id: TaskId::try_new(format!(
                    "linear:{}/{}",
                    self.workspace_id,
                    issue.identifier.as_deref().unwrap_or("unknown")
                ))
                .unwrap_or_else(|_| TaskId::try_new("linear:invalid").unwrap()),
                title: issue.title.as_deref().unwrap_or("").to_string(),
                status: issue
                    .state
                    .as_ref()
                    .and_then(|s| s.name.as_ref())
                    .cloned()
                    .unwrap_or_else(|| "unknown".to_string()),
                url: issue.url.as_deref().unwrap_or("").to_string(),
                updated_at: issue.updated_at.unwrap_or_else(Utc::now),
            })
            .collect())
    }

    /// Acknowledge a task (no-op for Linear; storage-side tracking only).
    async fn acknowledge_task(&self, _id: &TaskId) -> Result<()> {
        Ok(())
    }

    /// Post a comment on a Linear issue.
    async fn post_comment(&self, id: &TaskId, body: &str) -> Result<()> {
        let identifier = id
            .as_str()
            .rsplit('/')
            .next()
            .ok_or_else(|| Error::InvalidTaskId(id.as_str().into()))?
            .to_string();

        let input = CommentCreateInput {
            issue_id: MaybeUndefined::Value(identifier),
            body: MaybeUndefined::Value(body.to_string()),
            ..Default::default()
        };

        let _: Comment = self.client
            .comment_create(input)
            .await
            .map_err(map_err)?;

        Ok(())
    }

    /// Set or remove a label on a Linear issue.
    ///
    /// # Errors
    ///
    /// Returns `Error::Internal` if the label cannot be resolved or the update fails.
    async fn set_label(&self, id: &TaskId, label: &str, present: bool) -> Result<()> {
        let identifier = id
            .as_str()
            .rsplit('/')
            .next()
            .ok_or_else(|| Error::InvalidTaskId(id.as_str().into()))?
            .to_string();

        let label_id = self.resolve_label_id(label).await?;

        let update = if present {
            IssueUpdateInput {
                added_label_ids: MaybeUndefined::Value(vec![label_id]),
                ..Default::default()
            }
        } else {
            IssueUpdateInput {
                removed_label_ids: MaybeUndefined::Value(vec![label_id]),
                ..Default::default()
            }
        };

        let _: Issue = self.client
            .issue_update(update, identifier)
            .await
            .map_err(map_err)?;

        Ok(())
    }

    /// Read the current labels on a Linear issue.
    async fn read_labels(&self, id: &TaskId) -> Result<Vec<String>> {
        Ok(self.fetch_task(id).await?.labels)
    }
}

impl LinearTaskSource {
    async fn fetch_and_emit_events(&self) -> Result<Vec<TaskEvent>> {
        let mut filter = if !self.label_filters.is_empty() {
            let label_name = &self.label_filters[0];
            let label_filter = IssueLabelFilter {
                name: MaybeUndefined::Value(StringComparator {
                    eq: MaybeUndefined::Value(label_name.to_string()),
                    ..Default::default()
                }),
                ..Default::default()
            };
            IssueFilter {
                labels: MaybeUndefined::Value(Box::new(IssueLabelCollectionFilter {
                    some: MaybeUndefined::Value(Box::new(label_filter)),
                    ..Default::default()
                })),
                ..Default::default()
            }
        } else {
            IssueFilter::default()
        };

        let last_seen = *self.last_seen_updated_at.lock().await;
        if let Some(ts) = last_seen {
            filter.updated_at = MaybeUndefined::Value(DateComparator {
                gte: MaybeUndefined::Value(serde_json::json!(ts.to_rfc3339())),
                ..Default::default()
            });
        }

        let result = self.client
            .issues::<Issue>()
            .filter(filter)
            .first(25)
            .send()
            .await
            .map_err(map_err)?;

        if let Some(latest_issue) = result.nodes.first() {
            if let Some(updated_at) = latest_issue.updated_at {
                let mut last_seen_guard = self.last_seen_updated_at.lock().await;
                *last_seen_guard = Some(updated_at);
            }
        }

        let events = result
            .nodes
            .iter()
            .map(|issue| TaskEvent {
                source_id: self.id.clone(),
                task_id: TaskId::try_new(format!(
                    "linear:{}/{}",
                    self.workspace_id,
                    issue.identifier.as_deref().unwrap_or("unknown")
                ))
                .unwrap_or_else(|_| TaskId::try_new("linear:invalid").unwrap()),
                kind: TaskEventKind::NewTask,
                seen_at: Utc::now(),
                raw_payload: serde_json::to_value(issue).unwrap_or_default(),
            })
            .collect();

        Ok(events)
    }
}

/// Map `lineark_sdk::LinearError` to `crate::Error`.
fn map_err(e: LinearError) -> Error {
    match e {
        LinearError::Authentication(msg) | LinearError::Forbidden(msg) => {
            Error::AuthFailed(msg)
        }
        LinearError::RateLimited { retry_after, message: _ } => {
            let retry_after_secs = retry_after.map(|f| f as u64).unwrap_or(60);
            Error::RateLimited { retry_after_secs }
        }
        LinearError::Network(e) => Error::Network(e.to_string()),
        LinearError::HttpError { status, .. } if status >= 500 => {
            Error::Network(e.to_string())
        }
        _ => Error::Internal(e.to_string()),
    }
}
