//! Real Linear-API integration test. Requires:
//!   - LINEAR_TEST_API_TOKEN env var
//!   - LINEAR_TEST_WORKSPACE_ID env var
//!
//! Run with:
//!   cargo test -p surge-intake --test linear_real -- --ignored
//!
//! CI nightly cron sets the secrets and runs this.

use std::env;
use std::time::Duration;
use surge_intake::linear::source::{LinearConfig, LinearTaskSource};
use surge_intake::TaskSource;

fn env_or_skip(key: &str) -> Option<String> {
    match env::var(key) {
        Ok(v) if !v.is_empty() => Some(v),
        _ => {
            eprintln!("{key} not set; skipping");
            None
        }
    }
}

#[tokio::test]
#[ignore]
async fn lists_open_tasks_in_test_workspace() {
    let token = match env_or_skip("LINEAR_TEST_API_TOKEN") {
        Some(t) => t,
        None => return,
    };
    let workspace_id = match env_or_skip("LINEAR_TEST_WORKSPACE_ID") {
        Some(w) => w,
        None => return,
    };

    let cfg = LinearConfig {
        id: "linear:real".into(),
        display_name: "Linear · real test".into(),
        workspace_id,
        api_token: token,
        poll_interval: Duration::from_secs(60),
        label_filters: vec!["surge:test".into()],
    };
    let source = LinearTaskSource::new(cfg).expect("client init");
    let summaries = source
        .list_open_tasks()
        .await
        .expect("list_open_tasks should succeed against real Linear");
    eprintln!("got {} tasks", summaries.len());
    // No assert on count — workspace may be empty.
    // We assert structural integrity:
    for s in &summaries {
        assert!(
            s.task_id.as_str().starts_with("linear:"),
            "expected linear:* prefix, got {}",
            s.task_id.as_str()
        );
        assert!(
            s.url.starts_with("https://linear.app"),
            "expected linear.app URL, got {}",
            s.url
        );
    }
}
