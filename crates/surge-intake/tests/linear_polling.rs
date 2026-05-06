//! Linear polling test using wiremock — no real Linear account required.
//!
//! This test verifies that `LinearTaskSource::watch_for_tasks` correctly polls the Linear API
//! and emits TaskEvent for issues matching the configured label filters.
//!
//! Leverages lineark-sdk's `set_base_url` method to mock the GraphQL endpoint.

use std::sync::Arc;
use std::time::Duration;
use surge_intake::linear::source::{LinearConfig, LinearTaskSource};
use surge_intake::types::TaskEventKind;
use surge_intake::TaskSource;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn linear_polling_emits_event_for_new_issue() {
    let server = MockServer::start().await;

    // Mock the Linear GraphQL endpoint to return a single issue with the surge:enabled label.
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": {
                "issues": {
                    "nodes": [{
                        "id": "uuid-abc-1",
                        "identifier": "ABC-1",
                        "title": "Fix parser panic",
                        "description": "Parser crashes on invalid input",
                        "url": "https://linear.app/test/issue/ABC-1",
                        "state": {
                            "id": "state-in-progress",
                            "name": "In Progress",
                            "type": "started"
                        },
                        "labels": {
                            "nodes": [
                                {"id": "label-1", "name": "surge:enabled"}
                            ]
                        },
                        "assignee": {
                            "id": "user-1",
                            "name": "Alice"
                        },
                        "createdAt": "2026-05-06T10:00:00.000Z",
                        "updatedAt": "2026-05-06T10:00:00.000Z"
                    }],
                    "pageInfo": {
                        "hasNextPage": false,
                        "endCursor": "c1"
                    }
                }
            }
        })))
        .mount(&server)
        .await;

    let cfg = LinearConfig {
        id: "linear:test".into(),
        display_name: "Linear · test".into(),
        workspace_id: "wsp_test".into(),
        api_token: "lin_test_token_abc123".into(),
        poll_interval: Duration::from_millis(50),
        label_filters: vec!["surge:enabled".into()],
    };

    let mut source = LinearTaskSource::new(cfg).expect("failed to create LinearTaskSource");
    source.set_base_url(format!("{}/graphql", server.uri()));

    let source = Arc::new(source);
    let mut stream = source.watch_for_tasks();

    // Poll once from the stream.
    let event = tokio::time::timeout(Duration::from_secs(2), async {
        futures::stream::StreamExt::next(&mut stream).await
    })
    .await
    .expect("timeout waiting for event")
    .expect("stream ended unexpectedly")
    .expect("failed to get event from stream");

    // Verify the event structure.
    assert_eq!(event.source_id, "linear:test");
    assert_eq!(event.task_id.as_str(), "linear:wsp_test/ABC-1");
    assert!(matches!(event.kind, TaskEventKind::NewTask));

    // Verify the raw payload contains the issue data.
    let raw = &event.raw_payload;
    assert_eq!(raw["identifier"], "ABC-1");
    assert_eq!(raw["title"], "Fix parser panic");
}

#[tokio::test]
async fn linear_source_accessor_methods() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": {
                "issues": {
                    "nodes": [],
                    "pageInfo": {"hasNextPage": false, "endCursor": null}
                }
            }
        })))
        .mount(&server)
        .await;

    let cfg = LinearConfig {
        id: "linear:acme".into(),
        display_name: "ACME Linear".into(),
        workspace_id: "wsp_acme".into(),
        api_token: "token_acme_xyz".into(),
        poll_interval: Duration::from_secs(60),
        label_filters: vec!["surge:enabled".into()],
    };

    let mut source = LinearTaskSource::new(cfg).expect("failed to create source");
    source.set_base_url(format!("{}/graphql", server.uri()));

    // Verify accessor methods return expected values.
    assert_eq!(source.id(), "linear:acme");
    assert_eq!(source.display_name(), "ACME Linear");
    assert_eq!(source.provider(), "linear");
}

#[tokio::test]
async fn linear_polling_retries_on_rate_limit() {
    let server = MockServer::start().await;

    // First request returns 429 (rate limited).
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .respond_with(ResponseTemplate::new(429).append_header("retry-after", "1"))
        .mount(&server)
        .await;

    let cfg = LinearConfig {
        id: "linear:test".into(),
        display_name: "Linear · test".into(),
        workspace_id: "wsp_test".into(),
        api_token: "token_test".into(),
        poll_interval: Duration::from_millis(50),
        label_filters: vec!["surge:enabled".into()],
    };

    let mut source = LinearTaskSource::new(cfg).expect("failed to create source");
    source.set_base_url(format!("{}/graphql", server.uri()));

    let source = Arc::new(source);
    let mut stream = source.watch_for_tasks();

    // Expect the first event to be an error.
    let result = tokio::time::timeout(Duration::from_secs(2), async {
        futures::stream::StreamExt::next(&mut stream).await
    })
    .await
    .expect("timeout")
    .expect("stream ended")
    .expect_err("expected error");

    // Verify that the error is RateLimited.
    let err_str = result.to_string();
    assert!(
        err_str.contains("RateLimited") || err_str.contains("rate"),
        "expected rate limit error, got: {}",
        err_str
    );
}

#[tokio::test]
async fn linear_polling_with_no_label_filters() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/graphql"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": {
                "issues": {
                    "nodes": [{
                        "id": "uuid-xyz-1",
                        "identifier": "XYZ-1",
                        "title": "Unrestricted issue",
                        "description": "",
                        "url": "https://linear.app/test/issue/XYZ-1",
                        "state": {
                            "id": "state-todo",
                            "name": "Todo",
                            "type": "unstarted"
                        },
                        "labels": {"nodes": []},
                        "assignee": null,
                        "createdAt": "2026-05-06T11:00:00.000Z",
                        "updatedAt": "2026-05-06T11:00:00.000Z"
                    }],
                    "pageInfo": {"hasNextPage": false, "endCursor": "c1"}
                }
            }
        })))
        .mount(&server)
        .await;

    let cfg = LinearConfig {
        id: "linear:test".into(),
        display_name: "Linear · test".into(),
        workspace_id: "wsp_test".into(),
        api_token: "token_test".into(),
        poll_interval: Duration::from_millis(50),
        label_filters: vec![], // No label filters
    };

    let mut source = LinearTaskSource::new(cfg).expect("failed to create source");
    source.set_base_url(format!("{}/graphql", server.uri()));

    let source = Arc::new(source);
    let mut stream = source.watch_for_tasks();

    let event = tokio::time::timeout(Duration::from_secs(2), async {
        futures::stream::StreamExt::next(&mut stream).await
    })
    .await
    .expect("timeout")
    .expect("stream ended")
    .expect("failed to get event");

    assert_eq!(event.task_id.as_str(), "linear:wsp_test/XYZ-1");
}

#[tokio::test]
async fn linear_source_rejects_empty_token() {
    let cfg = LinearConfig {
        id: "linear:test".into(),
        display_name: "Linear · test".into(),
        workspace_id: "wsp_test".into(),
        api_token: "".into(), // Empty token
        poll_interval: Duration::from_secs(60),
        label_filters: vec![],
    };

    let result = LinearTaskSource::new(cfg);
    match result {
        Ok(_) => panic!("expected error for empty token, but got success"),
        Err(err) => {
            let err_str = err.to_string();
            assert!(
                err_str.contains("Auth") || err_str.to_lowercase().contains("token"),
                "expected auth error, got: {}",
                err_str
            );
        }
    }
}
