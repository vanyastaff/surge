//! Integration tests for `AcpBridge::reply_to_tool` — M5.1.
//!
//! M3 worker shipped a stub that logged + acked any `ReplyToTool` command
//! without checking session validity or call-id existence. M5.1 replaces the
//! stub with proper bookkeeping:
//!
//! - Unknown session → `ReplyToToolError::SessionGone`
//! - Unknown call_id within an open session → `ReplyToToolError::UnknownCallId`
//! - Valid (session, call_id) → emits `BridgeEvent::ToolResult` with the
//!   engine-supplied payload, returns `Ok(())`.
//!
//! Note on ACP semantics: ACP itself has no client→agent "tool result"
//! protocol method. Bridge bookkeeping is internal only — observers see the
//! `ToolResult` event but the agent subprocess does not receive a wire-level
//! reply. Agents communicate completion of their own tool execution via
//! `SessionUpdate::ToolCallUpdate` notifications which they fire themselves.
//! Surge's reply API exists so that the engine can correlate dispatcher
//! results with the originating `BridgeEvent::ToolCall` events for
//! observability + persistence.

use std::collections::BTreeMap;
use std::str::FromStr;
use std::time::Duration;

use surge_acp::bridge::error::ReplyToToolError;
use surge_acp::bridge::{
    AcpBridge, AgentKind, AlwaysAllowSandbox, BridgeEvent, MessageContent, SessionConfig,
    ToolResultPayload,
};
use surge_acp::client::PermissionPolicy;
use surge_core::{OutcomeKey, SessionId};
use tempfile::TempDir;
use tokio::time::timeout;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reply_to_unknown_session_returns_session_gone() {
    let bridge = AcpBridge::with_defaults().unwrap();

    // Random SessionId that was never opened.
    let unknown = SessionId::new();
    let err = bridge
        .reply_to_tool(
            unknown,
            "fake-call".into(),
            ToolResultPayload::Ok {
                result_json: "{}".into(),
            },
        )
        .await
        .unwrap_err();

    assert!(
        matches!(err, ReplyToToolError::SessionGone),
        "expected SessionGone, got {err:?}",
    );

    bridge.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reply_to_unknown_call_id_within_session_returns_unknown_call_id() {
    let wt = TempDir::new().unwrap();
    let bridge = AcpBridge::with_defaults().unwrap();

    // Open a real session via mock so the SessionId is valid in the worker map.
    // `echo` scenario doesn't fire any tool calls, so any call_id we pass is
    // guaranteed to be unknown.
    let cfg = SessionConfig {
        agent_kind: AgentKind::Mock {
            args: vec!["--scenario".into(), "echo".into()],
        },
        working_dir: wt.path().to_path_buf(),
        system_prompt: "noop".into(),
        declared_outcomes: vec![OutcomeKey::from_str("done").unwrap()],
        allows_escalation: false,
        tools: vec![],
        sandbox: Box::new(AlwaysAllowSandbox),
        permission_policy: PermissionPolicy::default(),
        bindings: BTreeMap::new(),
    };

    let sid = bridge.open_session(cfg).await.unwrap();

    let err = bridge
        .reply_to_tool(
            sid,
            "no-such-call-id".into(),
            ToolResultPayload::Ok {
                result_json: "{}".into(),
            },
        )
        .await
        .unwrap_err();

    assert!(
        matches!(err, ReplyToToolError::UnknownCallId(ref s) if s == "no-such-call-id"),
        "expected UnknownCallId(\"no-such-call-id\"), got {err:?}",
    );

    bridge.close_session(sid).await.ok();
    bridge.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reply_to_valid_call_id_emits_tool_result_and_returns_ok() {
    let wt = TempDir::new().unwrap();
    let bridge = AcpBridge::with_defaults().unwrap();
    let mut events = bridge.subscribe();

    let cfg = SessionConfig {
        agent_kind: AgentKind::Mock {
            args: vec!["--scenario".into(), "human_input".into()],
        },
        working_dir: wt.path().to_path_buf(),
        system_prompt: "ask".into(),
        declared_outcomes: vec![OutcomeKey::from_str("done").unwrap()],
        allows_escalation: true,
        tools: vec![],
        sandbox: Box::new(AlwaysAllowSandbox),
        permission_policy: PermissionPolicy::default(),
        bindings: BTreeMap::new(),
    };

    let sid = bridge.open_session(cfg).await.unwrap();
    bridge
        .send_message(sid, MessageContent::Text("?".into()))
        .await
        .unwrap();

    // Wait for the HumanInputRequested event so we have a real call_id to reply to.
    let mut call_id: Option<String> = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline && call_id.is_none() {
        if let Ok(Ok(BridgeEvent::HumanInputRequested {
            session,
            call_id: cid,
            ..
        })) = timeout(Duration::from_millis(200), events.recv()).await
            && session == sid
        {
            call_id = Some(cid);
        }
    }
    let call_id = call_id.expect("HumanInputRequested with call_id within 5s");

    // Reply with a payload — should succeed AND broadcast a matching ToolResult event.
    bridge
        .reply_to_tool(
            sid,
            call_id.clone(),
            ToolResultPayload::Ok {
                result_json: "\"ack\"".into(),
            },
        )
        .await
        .expect("reply_to_tool should succeed for a valid call_id");

    // Verify that a ToolResult event with matching call_id was broadcast.
    let mut saw_result = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while tokio::time::Instant::now() < deadline && !saw_result {
        if let Ok(Ok(BridgeEvent::ToolResult {
            session,
            call_id: cid,
            payload,
        })) = timeout(Duration::from_millis(200), events.recv()).await
            && session == sid
            && cid == call_id
        {
            assert!(
                matches!(payload, ToolResultPayload::Ok { ref result_json } if result_json == "\"ack\""),
                "payload mismatch: {payload:?}",
            );
            saw_result = true;
        }
    }
    assert!(saw_result, "expected BridgeEvent::ToolResult after reply");

    // Replying twice for the same call_id is now invalid — bookkeeping has been cleared.
    let err = bridge
        .reply_to_tool(
            sid,
            call_id.clone(),
            ToolResultPayload::Ok {
                result_json: "{}".into(),
            },
        )
        .await
        .unwrap_err();
    assert!(
        matches!(err, ReplyToToolError::UnknownCallId(_)),
        "second reply should fail with UnknownCallId, got {err:?}",
    );

    bridge.close_session(sid).await.ok();
    bridge.shutdown().await.unwrap();
}
