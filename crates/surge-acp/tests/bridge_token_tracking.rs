//! Integration test: cumulative token usage events arrive monotonically and
//! all precede SessionEnded.
//!
//! Note: `extract_usage` is stubbed in M3 (SDK 0.10.2's `UsageUpdate` exposes
//! only `used`/`size`, not per-request token breakdown — see spec §11.7).
//! This test passes vacuously: no `BridgeEvent::TokenUsage` events fire, but
//! the monotonicity assertion trivially holds and `saw_end` triggers.

use std::collections::BTreeMap;
use std::str::FromStr;
use std::time::Duration;

use surge_acp::bridge::{
    AcpBridge, AgentKind, AlwaysAllowSandbox, BridgeEvent, MessageContent, SessionConfig,
};
use surge_acp::client::PermissionPolicy;
use surge_core::OutcomeKey;
use tempfile::TempDir;
use tokio::time::timeout;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn token_usage_monotonic_and_precedes_session_end() {
    tokio::time::timeout(Duration::from_secs(30), inner_test())
        .await
        .expect("test exceeded 30s — likely deadlock in token_tracking path");
}

async fn inner_test() {
    let wt = TempDir::new().unwrap();
    // SAFETY: tokio multi-thread tests share env; this test runs alone.
    unsafe {
        std::env::set_var("MOCK_ACP_USAGE", "on");
    }

    let bridge = AcpBridge::with_defaults().unwrap();
    let mut events = bridge.subscribe();

    let cfg = SessionConfig {
        agent_kind: AgentKind::Mock {
            args: vec!["--scenario".into(), "long_streaming".into()],
        },
        working_dir: wt.path().to_path_buf(),
        system_prompt: "stream".into(),
        declared_outcomes: vec![OutcomeKey::from_str("done").unwrap()],
        allows_escalation: false,
        tools: vec![],
        sandbox: Box::new(AlwaysAllowSandbox),
        permission_policy: PermissionPolicy::default(),
        bindings: BTreeMap::new(),
    };

    let sid = bridge.open_session(cfg).await.unwrap();
    bridge
        .send_message(sid.clone(), MessageContent::Text("go".into()))
        .await
        .unwrap();

    // Drain events for up to 5s to collect any TokenUsage events and agent chunks.
    // The long_streaming scenario emits 20 chunks at 50ms each (~1s) plus a
    // UsageUpdate if MOCK_ACP_USAGE=on. After the mock finishes its prompt handler,
    // it waits for further prompts — we then close the session to trigger SessionEnded.
    let mut last_prompt = 0u32;
    let mut last_output = 0u32;
    let chunk_deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < chunk_deadline {
        match timeout(Duration::from_millis(200), events.recv()).await {
            Ok(Ok(BridgeEvent::TokenUsage {
                prompt_tokens,
                output_tokens,
                ..
            })) => {
                assert!(prompt_tokens >= last_prompt, "prompt_tokens not monotonic");
                assert!(output_tokens >= last_output, "output_tokens not monotonic");
                last_prompt = prompt_tokens;
                last_output = output_tokens;
            }
            Ok(Ok(BridgeEvent::SessionEnded { session, .. })) if session == sid => {
                // Unexpected: session ended before we closed it.
                // Still treat it as success since saw_end will be confirmed below.
                break;
            }
            _ => continue,
        }
    }

    // Close the session — this triggers subprocess_waiter to emit SessionEnded.
    // Monotonicity of any pending TokenUsage is guaranteed by close_session_impl's
    // flush_pending_token_usage call (spec §5.7).
    bridge.close_session(sid.clone()).await.ok();

    // Wait for SessionEnded — confirm no stray TokenUsage follows it.
    let mut saw_end = false;
    let end_deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    while tokio::time::Instant::now() < end_deadline {
        match timeout(Duration::from_millis(200), events.recv()).await {
            Ok(Ok(BridgeEvent::TokenUsage {
                prompt_tokens,
                output_tokens,
                ..
            })) => {
                // A flush from close_session_impl — still before SessionEnded, valid.
                assert!(!saw_end, "TokenUsage arrived after SessionEnded — ordering violated");
                assert!(prompt_tokens >= last_prompt, "prompt_tokens not monotonic (post-close)");
                assert!(output_tokens >= last_output, "output_tokens not monotonic (post-close)");
                last_prompt = prompt_tokens;
                last_output = output_tokens;
            }
            Ok(Ok(BridgeEvent::SessionEnded { session, .. })) if session == sid => {
                saw_end = true;
                // Drain a bit more to confirm no stray TokenUsage follows.
                let post_end = timeout(Duration::from_millis(300), events.recv()).await;
                match post_end {
                    Ok(Ok(BridgeEvent::TokenUsage { .. })) => {
                        panic!("TokenUsage arrived after SessionEnded");
                    }
                    _ => break,
                }
            }
            _ => continue,
        }
    }
    assert!(saw_end, "never observed SessionEnded for {sid}");

    unsafe {
        std::env::remove_var("MOCK_ACP_USAGE");
    }
    bridge.shutdown().await.unwrap();
}
