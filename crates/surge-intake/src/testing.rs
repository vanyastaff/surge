//! Test utilities for `surge-intake`. Always compiled (not feature-gated)
//! so consumer crates can use `MockTaskSource` in their integration tests.

use crate::source::TaskSource;
use crate::types::{TaskDetails, TaskEvent, TaskId, TaskSummary};
use crate::{Error, Result};
use async_trait::async_trait;
use futures::stream::{self, BoxStream};
use std::collections::HashMap;
use tokio::sync::Mutex;

/// In-memory `TaskSource` for tests.
///
/// Push events with `push_event`, then assert outcomes via the inspection
/// methods (`posted_comments`, `recorded_labels`).
pub struct MockTaskSource {
    id: String,
    display_name: String,
    provider: &'static str,
    events: Mutex<Vec<TaskEvent>>,
    open_tasks: Mutex<HashMap<TaskId, TaskDetails>>,
    posted_comments: Mutex<Vec<(TaskId, String)>>,
    recorded_labels: Mutex<Vec<(TaskId, String, bool)>>,
    fail_post_comment: Mutex<bool>,
}

impl MockTaskSource {
    /// Construct a new mock with stable identifier and provider tag.
    pub fn new(id: impl Into<String>, provider: &'static str) -> Self {
        let id = id.into();
        Self {
            display_name: format!("Mock · {id}"),
            id,
            provider,
            events: Mutex::new(Vec::new()),
            open_tasks: Mutex::new(HashMap::new()),
            posted_comments: Mutex::new(Vec::new()),
            recorded_labels: Mutex::new(Vec::new()),
            fail_post_comment: Mutex::new(false),
        }
    }

    /// Queue an event to be emitted by `watch_for_tasks`.
    pub async fn push_event(&self, ev: TaskEvent) {
        self.events.lock().await.push(ev);
    }

    /// Add a task that `fetch_task` and `list_open_tasks` should return.
    pub async fn put_task(&self, details: TaskDetails) {
        self.open_tasks.lock().await.insert(details.task_id.clone(), details);
    }

    /// Snapshot of comments posted via `post_comment`.
    pub async fn posted_comments(&self) -> Vec<(TaskId, String)> {
        self.posted_comments.lock().await.clone()
    }

    /// Snapshot of label changes performed via `set_label`.
    pub async fn recorded_labels(&self) -> Vec<(TaskId, String, bool)> {
        self.recorded_labels.lock().await.clone()
    }

    /// Make the next (and subsequent) `post_comment` calls fail with `Error::Network`.
    pub async fn arm_post_comment_failure(&self) {
        *self.fail_post_comment.lock().await = true;
    }
}

#[async_trait]
impl TaskSource for MockTaskSource {
    fn id(&self) -> &str {
        &self.id
    }

    fn display_name(&self) -> &str {
        &self.display_name
    }

    fn provider(&self) -> &'static str {
        self.provider
    }

    fn watch_for_tasks<'a>(&'a self) -> BoxStream<'a, Result<TaskEvent>> {
        // Borrow `self.events` for the lifetime of the stream. Once the queue
        // drains the stream ends; tests that need a long-running stream should
        // push events before consuming.
        Box::pin(stream::unfold(&self.events, |events| async move {
            let next = {
                let mut guard = events.lock().await;
                if guard.is_empty() {
                    None
                } else {
                    Some(guard.remove(0))
                }
            };
            next.map(|ev| (Ok(ev), events))
        }))
    }

    async fn fetch_task(&self, id: &TaskId) -> Result<TaskDetails> {
        self.open_tasks
            .lock()
            .await
            .get(id)
            .cloned()
            .ok_or_else(|| Error::Internal(format!("task not found: {id}")))
    }

    async fn list_open_tasks(&self) -> Result<Vec<TaskSummary>> {
        let map = self.open_tasks.lock().await;
        Ok(map
            .values()
            .map(|d| TaskSummary {
                task_id: d.task_id.clone(),
                title: d.title.clone(),
                status: d.status.clone(),
                url: d.url.clone(),
                updated_at: d.updated_at,
            })
            .collect())
    }

    async fn acknowledge_task(&self, _id: &TaskId) -> Result<()> {
        Ok(())
    }

    async fn post_comment(&self, id: &TaskId, body: &str) -> Result<()> {
        if *self.fail_post_comment.lock().await {
            return Err(Error::Network("simulated post_comment failure".into()));
        }
        self.posted_comments
            .lock()
            .await
            .push((id.clone(), body.to_string()));
        Ok(())
    }

    async fn set_label(&self, id: &TaskId, label: &str, present: bool) -> Result<()> {
        self.recorded_labels
            .lock()
            .await
            .push((id.clone(), label.to_string(), present));
        Ok(())
    }

    async fn read_labels(&self, id: &TaskId) -> Result<Vec<String>> {
        Ok(self
            .open_tasks
            .lock()
            .await
            .get(id)
            .map(|d| d.labels.clone())
            .unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::TaskEventKind;
    use chrono::Utc;
    use futures::StreamExt;

    fn sample_event() -> TaskEvent {
        TaskEvent {
            source_id: "mock:test".into(),
            task_id: TaskId::try_new("mock:test#1").unwrap(),
            kind: TaskEventKind::NewTask,
            seen_at: Utc::now(),
            raw_payload: serde_json::json!({}),
        }
    }

    #[tokio::test]
    async fn mock_emits_pushed_events() {
        let src = MockTaskSource::new("mock:test", "mock");
        src.push_event(sample_event()).await;
        let mut stream = src.watch_for_tasks();
        let first = stream.next().await.unwrap().unwrap();
        assert!(matches!(first.kind, TaskEventKind::NewTask));
        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn mock_records_posted_comments() {
        let src = MockTaskSource::new("mock:test", "mock");
        let id = TaskId::try_new("mock:test#1").unwrap();
        src.post_comment(&id, "hello").await.unwrap();
        let comments = src.posted_comments().await;
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].1, "hello");
    }

    #[tokio::test]
    async fn mock_post_comment_failure() {
        let src = MockTaskSource::new("mock:test", "mock");
        src.arm_post_comment_failure().await;
        let id = TaskId::try_new("mock:test#1").unwrap();
        let err = src.post_comment(&id, "x").await.unwrap_err();
        assert!(matches!(err, Error::Network(_)));
    }

    #[tokio::test]
    async fn mock_records_labels() {
        let src = MockTaskSource::new("mock:test", "mock");
        let id = TaskId::try_new("mock:test#1").unwrap();
        src.set_label(&id, "surge:enabled", true).await.unwrap();
        let labels = src.recorded_labels().await;
        assert_eq!(labels.len(), 1);
        assert!(labels[0].2);
    }
}
