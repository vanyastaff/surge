//! #127: the GitHub source works on the PR the gate resolved, never on the
//! issue's own number (GitHub issues and PRs share one numbering sequence,
//! so they never coincide), and reads the issue's links from GitHub itself.

use std::time::Duration;

use surge_intake::TaskSource;
use surge_intake::github::{GitHubConfig, GitHubIssuesTaskSource};
use surge_intake::source::MergeReadiness;
use surge_intake::types::TaskId;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn source(server: &MockServer) -> GitHubIssuesTaskSource {
    GitHubIssuesTaskSource::with_api_base(
        GitHubConfig {
            id: "github_issues:acme/app".into(),
            display_name: "GitHub · app".into(),
            owner: "acme".into(),
            repo: "app".into(),
            api_token: "test-token".into(),
            poll_interval: Duration::from_secs(60),
            label_filters: Vec::new(),
            merge_method: surge_core::config::MergeMethod::Squash,
        },
        Some(&server.uri()),
    )
    .unwrap()
}

fn issue_42() -> TaskId {
    TaskId::try_new("github_issues:acme/app#42").unwrap()
}

#[tokio::test]
async fn linked_pull_requests_are_the_open_prs_that_close_the_issue() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": { "repository": { "issue": { "closedByPullRequestsReferences": { "nodes": [
                { "number": 61, "state": "OPEN" },
                { "number": 57, "state": "OPEN" },
                { "number": 50, "state": "MERGED" },
                { "number": 57, "state": "OPEN" },
            ] } } } }
        })))
        .expect(1)
        .mount(&server)
        .await;
    let linked = source(&server)
        .linked_pull_requests(&issue_42())
        .await
        .unwrap();
    assert_eq!(linked, [57, 61], "open only, sorted, deduplicated");
}

#[tokio::test]
async fn an_unreadable_link_response_links_nothing() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": { "repository": { "issue": null } }
        })))
        .mount(&server)
        .await;
    assert_eq!(
        source(&server)
            .linked_pull_requests(&issue_42())
            .await
            .unwrap(),
        Vec::<u64>::new()
    );
}

#[tokio::test]
async fn readiness_and_merge_ask_for_the_resolved_pr_not_the_issue_number() {
    let server = MockServer::start().await;
    // The issue's own number is never fetched as a PR.
    Mock::given(path("/repos/acme/app/pulls/42"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/app/pulls/57"))
        .respond_with(ResponseTemplate::new(404).set_body_json(serde_json::json!({
            "message": "Not Found",
            "documentation_url": "https://docs.github.com/rest",
        })))
        .expect(2)
        .mount(&server)
        .await;
    let source = source(&server);

    let readiness = source.check_merge_readiness(&issue_42(), 57).await.unwrap();
    assert_eq!(
        readiness,
        MergeReadiness::Blocked("pull request #57 not found".into())
    );
    assert!(
        source.merge_pr(&issue_42(), 57, None).await.is_err(),
        "a missing PR is not merged"
    );
}
