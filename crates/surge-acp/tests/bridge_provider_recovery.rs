//! Provider identity is proved on the actual ACP wire across process replacement.
use std::{collections::BTreeMap, path::Path};
use surge_acp::bridge::*;
use surge_core::execution_recovery::*;

fn config(root: &Path, mode: &str) -> SessionConfig {
    SessionConfig {
        writer_id: surge_core::id::ExecutionWriterId::new(),
        invocation: surge_core::id::StageInvocationId::new(),
        runtime: "fixture-provider".into(),
        opening: SessionOpening::New,
        config_selections: vec![],
        stage_mcp: None,
        agent_kind: AgentKind::Custom {
            binary: env!("CARGO_BIN_EXE_mock_acp_agent").into(),
            args: vec![
                "--session-store".into(),
                root.join("provider-sessions.json").display().to_string(),
                "--wire-log".into(),
                root.join("wire.jsonl").display().to_string(),
                "--session-capabilities".into(),
                mode.into(),
            ],
        },
        working_dir: root.into(),
        system_prompt: "retained accepted context".into(),
        declared_outcomes: vec![surge_core::OutcomeKey::try_from("done").unwrap()],
        allows_escalation: false,
        tools: vec![],
        sandbox: Box::new(AlwaysAllowSandbox),
        permission_policy: surge_acp::client::PermissionPolicy::default(),
        bindings: BTreeMap::new(),
        env: BTreeMap::new(),
    }
}
#[tokio::test(flavor = "multi_thread")]
async fn actual_saved_provider_id_is_restored_with_identical_cwd_without_new_session() {
    for (advertised, expected) in [
        ("both", SessionOpenMode::Resume),
        ("resume", SessionOpenMode::Resume),
        ("load", SessionOpenMode::Load),
    ] {
        let root = tempfile::tempdir().unwrap();
        let first = AcpBridge::with_defaults().unwrap();
        let opened = first
            .open_session(config(root.path(), advertised))
            .await
            .unwrap();
        let saved = opened.descriptor.clone();
        first.close_session(opened.session).await.unwrap();
        first.shutdown().await.unwrap();
        let second = AcpBridge::with_defaults().unwrap();
        let mut continued = config(root.path(), advertised);
        continued.invocation = saved.invocation();
        continued.opening = SessionOpening::Continue(saved.clone());
        let restored = second.open_session(continued).await.unwrap();
        assert_eq!(restored.mode, expected);
        assert_eq!(
            restored.descriptor.provider_session_id(),
            saved.provider_session_id()
        );
        assert_eq!(restored.descriptor.cwd(), saved.cwd());
        assert_ne!(restored.session, opened.session);
        second.close_session(restored.session).await.unwrap();
        second.shutdown().await.unwrap();
        let wire: Vec<serde_json::Value> = std::fs::read_to_string(root.path().join("wire.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(
            wire.iter()
                .filter(|entry| entry["operation"] == "new_session")
                .count(),
            1
        );
        assert_eq!(wire.len(), 2);
        assert_eq!(
            wire[1]["request"]["sessionId"],
            saved.provider_session_id().as_str()
        );
        assert_eq!(wire[1]["request"]["cwd"], wire[0]["request"]["cwd"]);
    }
}
#[tokio::test(flavor = "multi_thread")]
async fn unsupported_restore_does_not_silently_create_a_provider_session() {
    let root = tempfile::tempdir().unwrap();
    let bridge = AcpBridge::with_defaults().unwrap();
    let first = bridge
        .open_session(config(root.path(), "none"))
        .await
        .unwrap();
    bridge.close_session(first.session).await.unwrap();
    let mut request = config(root.path(), "none");
    request.invocation = first.descriptor.invocation();
    request.opening = SessionOpening::Continue(first.descriptor);
    assert!(bridge.open_session(request).await.is_err());
    bridge.shutdown().await.unwrap();
    let wire = std::fs::read_to_string(root.path().join("wire.jsonl")).unwrap();
    assert_eq!(wire.lines().count(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn load_history_cannot_authorize_a_permission_before_a_new_prompt() {
    let root = tempfile::tempdir().unwrap();
    let bridge = AcpBridge::with_defaults().unwrap();
    let mut request = config(root.path(), "load");
    if let AgentKind::Custom { args, .. } = &mut request.agent_kind {
        args.push("--load-history-permission".into());
        args.push("--permission".into());
    }
    let invocation = request.invocation;
    let first = bridge.open_session(request).await.unwrap();
    bridge.close_session(first.session).await.unwrap();
    let mut request = config(root.path(), "load");
    request.invocation = invocation;
    if let AgentKind::Custom { args, .. } = &mut request.agent_kind {
        args.push("--load-history-permission".into());
        args.push("--permission".into());
    }
    request.opening = SessionOpening::Continue(first.descriptor);
    let restored = bridge.open_session(request).await.unwrap();
    let wire: Vec<serde_json::Value> = std::fs::read_to_string(root.path().join("wire.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let response = wire
        .iter()
        .find(|entry| entry["operation"] == "historical_permission_response")
        .unwrap();
    assert_eq!(
        response["request"]["outcome"]["outcome"], "cancelled",
        "replayed permission must have no current authority: {response}"
    );
    bridge
        .send_message(
            restored.session,
            MessageContent::Text("fresh continuation".into()),
        )
        .await
        .unwrap();
    let wire: Vec<serde_json::Value> = std::fs::read_to_string(root.path().join("wire.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let current = wire
        .iter()
        .find(|entry| entry["operation"] == "current_permission_response")
        .unwrap();
    assert_eq!(current["request"]["outcome"]["outcome"], "selected");
    assert_eq!(current["request"]["outcome"]["optionId"], "allow");
    bridge.close_session(restored.session).await.unwrap();
    bridge.shutdown().await.unwrap();
}
