//! Task 12 M0 measurement: does a real ACP JSON-RPC error round trip (mock
//! agent process -> stdio -> `agent_client_protocol` deserialization ->
//! `connection.prompt(...).await` on the bridge side) preserve enough of the
//! raw error text for `classify_prompt_dispatch_error` to still recover
//! `SendMessageError::RateLimited { retry_after: Some(_), .. }`?
//!
//! The pure-function measurement table (all the shapes, no subprocess) lives
//! in `surge-acp/src/bridge/worker.rs`'s `#[cfg(test)] mod tests` and runs in
//! the normal gate. This one test instead proves the *wire*, not just the
//! classifier: `agent-client-protocol-schema::Error`'s `Display` renders
//! `message` verbatim (checked against that crate's source — see
//! `error.rs::impl Display for Error`), so this is the empirical
//! confirmation that nothing about serialization, the JSON-RPC envelope, or
//! `.to_string()` mangles it along the way.
//!
//! `#[ignore]`d per the task's own test-strategy item 7: `just ci`'s `test`
//! recipe does not pass `--ignored`, so this is explicitly OUT of `just ci`
//! (it needs the `mock_acp_agent` binary built, ~tens of ms of subprocess
//! spawn/handshake overhead per run). **Not** out of the project's full
//! gate, though: `-p surge-acp` was added to `just test-ignored`
//! (`justfile`), which `just ci-full` does run — so this test is exercised
//! there, just not on every `just ci` invocation. Run explicitly with
//! `cargo test -p surge-acp --test bridge_rate_limit_classification -- --ignored`.

use std::collections::BTreeMap;
use std::str::FromStr;
use std::time::Duration;

use surge_acp::bridge::error::SendMessageError;
use surge_acp::bridge::{AcpBridge, AgentKind, AlwaysAllowSandbox, MessageContent, SessionConfig};
use surge_acp::client::PermissionPolicy;
use surge_core::OutcomeKey;
use tempfile::TempDir;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "Task 12 M0 measurement: real mock_acp_agent subprocess round trip, not part of `just ci` (its `test` recipe excludes --ignored); run under `just ci-full`/`just test-ignored`"]
async fn real_429_with_retry_after_survives_the_acp_wire_as_rate_limited() {
    let wt = TempDir::new().unwrap();
    let bridge = AcpBridge::with_defaults().unwrap();

    let cfg = SessionConfig {
        agent_kind: AgentKind::Mock {
            args: vec!["--scenario".into(), "prompt_error=429_retry_after".into()],
        },
        working_dir: wt.path().to_path_buf(),
        system_prompt: "x".into(),
        declared_outcomes: vec![OutcomeKey::from_str("done").unwrap()],
        allows_escalation: false,
        tools: vec![],
        sandbox: Box::new(AlwaysAllowSandbox),
        permission_policy: PermissionPolicy::default(),
        bindings: BTreeMap::new(),
    };

    let sid = bridge.open_session(cfg).await.unwrap();

    let err = bridge
        .send_message(
            sid,
            MessageContent::Text("trigger the scripted error".into()),
        )
        .await
        .expect_err("mock_acp_agent's prompt_error scenario must reject the prompt");

    match err {
        SendMessageError::RateLimited {
            retry_after,
            details,
        } => {
            assert_eq!(
                retry_after,
                Some(Duration::from_secs(30)),
                "the real ACP wire must preserve enough of the mock's \"429 Too Many \
                 Requests: Retry-After: 30\" message for the classifier to recover 30s, \
                 got details: {details:?}"
            );
            assert!(
                details.contains("429"),
                "details should preserve raw text, got: {details}"
            );
        },
        other => {
            panic!("expected SendMessageError::RateLimited from the real ACP wire, got: {other:?}")
        },
    }

    bridge.shutdown().await.unwrap();
}
