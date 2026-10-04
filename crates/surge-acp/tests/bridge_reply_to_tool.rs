//! Legacy bookkeeping cannot turn an ACP display call into a result-bearing tool.

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
        effect_fence: None,
        writer_id: surge_core::id::ExecutionWriterId::new(),
        invocation: surge_core::id::StageInvocationId::new(),
        runtime: "fixture".into(),
        opening: Default::default(),
        config_selections: Vec::new(),
        stage_mcp: None,
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
        env: Default::default(),
    };

    let sid = bridge.open_session(cfg).await.unwrap().session;

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
async fn reply_to_observed_call_id_cannot_fabricate_tool_result() {
    let wt = TempDir::new().unwrap();
    let bridge = AcpBridge::with_defaults().unwrap();
    let mut events = bridge.subscribe();

    let cfg = SessionConfig {
        effect_fence: None,
        writer_id: surge_core::id::ExecutionWriterId::new(),
        invocation: surge_core::id::StageInvocationId::new(),
        runtime: "fixture".into(),
        opening: Default::default(),
        config_selections: Vec::new(),
        stage_mcp: None,
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
        env: Default::default(),
    };

    let sid = bridge.open_session(cfg).await.unwrap().session;
    bridge
        .send_message(sid, MessageContent::Text("?".into()))
        .await
        .unwrap();

    // Wait for the ToolObserved event so we have a real call_id to reply to.
    let mut call_id: Option<String> = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline && call_id.is_none() {
        if let Ok(Ok(BridgeEvent::ToolObserved {
            session,
            call_id: cid,
            ..
        })) = timeout(Duration::from_millis(200), events.recv()).await
            && session == sid
        {
            call_id = Some(cid);
        }
    }
    let call_id = call_id.expect("ToolObserved with call_id within 5s");

    let error = bridge
        .reply_to_tool(
            sid,
            call_id.clone(),
            ToolResultPayload::Ok {
                result_json: "\"ack\"".into(),
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(error, ReplyToToolError::UnknownCallId(id) if id == call_id));
    bridge.close_session(sid).await.unwrap();
    bridge.shutdown().await.unwrap();
    assert!(
        !std::iter::from_fn(|| events.try_recv().ok())
            .any(|event| matches!(event, BridgeEvent::ToolResult { .. }))
    );
}
