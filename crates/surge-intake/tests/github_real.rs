//! Real GitHub-API integration test. Requires:
//!   - GITHUB_TEST_PAT env var (Personal Access Token)
//!   - GITHUB_TEST_OWNER env var
//!   - GITHUB_TEST_REPO env var
//!
//! Run with: `cargo test -p surge-intake --test github_real -- --ignored`
//!
//! CI nightly cron supplies the secrets to exercise this path.

use std::env;
use std::time::Duration;
use surge_intake::TaskSource;
use surge_intake::github::source::{GitHubConfig, GitHubIssuesTaskSource};

fn env_or_skip(key: &str) -> Option<String> {
    match env::var(key) {
        Ok(v) if !v.is_empty() => Some(v),
        _ => {
            eprintln!("{key} not set; skipping");
            None
        },
    }
}

#[tokio::test]
#[ignore]
async fn lists_open_tasks_in_test_repo() {
    let token = match env_or_skip("GITHUB_TEST_PAT") {
        Some(t) => t,
        None => return,
    };
    let owner = match env_or_skip("GITHUB_TEST_OWNER") {
        Some(o) => o,
        None => return,
    };
    let repo = match env_or_skip("GITHUB_TEST_REPO") {
        Some(r) => r,
        None => return,
    };

    let cfg = GitHubConfig {
        id: "github:real".into(),
        display_name: "GitHub · real test".into(),
        owner,
        repo,
        api_token: token,
        poll_interval: Duration::from_secs(60),
        label_filters: vec!["surge:test".into()],
    };
    let source = GitHubIssuesTaskSource::new(cfg).expect("client init");
    let summaries = source
        .list_open_tasks()
        .await
        .expect("list_open_tasks should succeed against real GitHub");
    eprintln!("got {} tasks", summaries.len());
    for s in &summaries {
        assert!(
            s.task_id.as_str().starts_with("github_issues:"),
            "expected github_issues:* prefix, got {}",
            s.task_id.as_str()
        );
        assert!(
            s.url.starts_with("https://github.com"),
            "expected github.com URL, got {}",
            s.url
        );
    }
}
