//! Integration test: per-step model / reasoning level reach the agent through
//! ACP's standard `session/set_config_option`, and an option the agent does
//! not offer keeps the session from opening.

use std::collections::BTreeMap;
use std::str::FromStr;

use surge_acp::bridge::error::OpenSessionError;
use surge_acp::bridge::session::{ConfigCategory, ConfigSelection};
use surge_acp::bridge::{AcpBridge, AgentKind, AlwaysAllowSandbox, SessionConfig};
use surge_acp::client::PermissionPolicy;
use surge_core::OutcomeKey;
use tempfile::TempDir;

fn config(
    wt: &TempDir,
    record: &std::path::Path,
    selections: Vec<ConfigSelection>,
) -> SessionConfig {
    SessionConfig {
        effect_fence: None,
        writer_id: surge_core::id::ExecutionWriterId::new(),
        invocation: surge_core::id::StageInvocationId::new(),
        runtime: "fixture".into(),
        opening: Default::default(),
        config_selections: selections,
        stage_mcp: None,
        agent_kind: AgentKind::Mock {
            args: vec![
                "--scenario".into(),
                "echo".into(),
                "--config-options".into(),
                "--config-file".into(),
                record.display().to_string(),
            ],
        },
        working_dir: wt.path().to_path_buf(),
        system_prompt: "you are a mock".into(),
        declared_outcomes: vec![OutcomeKey::from_str("done").unwrap()],
        allows_escalation: false,
        tools: vec![],
        sandbox: Box::new(AlwaysAllowSandbox),
        permission_policy: PermissionPolicy::default(),
        bindings: BTreeMap::new(),
        env: Default::default(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn chosen_model_and_reasoning_level_are_set_on_the_agent_session() {
    let wt = TempDir::new().unwrap();
    let record = wt.path().join("config-choices.txt");
    let bridge = AcpBridge::with_defaults().expect("spawn bridge");
    let selections = vec![
        ConfigSelection {
            category: ConfigCategory::Model,
            value: "Mock Opus".into(),
            best_effort: false,
        },
        ConfigSelection {
            category: ConfigCategory::ThoughtLevel,
            value: "high".into(),
            best_effort: false,
        },
    ];
    let session = bridge
        .open_session(config(&wt, &record, selections))
        .await
        .expect("session opens with offered options");
    let recorded = std::fs::read_to_string(&record).expect("agent recorded the choices");
    assert_eq!(
        recorded.lines().collect::<Vec<_>>(),
        ["model=opus", "effort=high"]
    );
    bridge.close_session(session.session).await.unwrap();
    bridge.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_model_the_agent_does_not_offer_keeps_the_session_closed() {
    let wt = TempDir::new().unwrap();
    let record = wt.path().join("config-choices.txt");
    let bridge = AcpBridge::with_defaults().expect("spawn bridge");
    let error = bridge
        .open_session(config(
            &wt,
            &record,
            vec![ConfigSelection {
                category: ConfigCategory::Model,
                value: "gpt-9".into(),
                best_effort: false,
            }],
        ))
        .await
        .expect_err("an unavailable model must not silently fall back");
    assert!(
        matches!(&error, OpenSessionError::ConfigOptionUnavailable { requested, offered, .. }
            if requested == "gpt-9" && offered == "sonnet, opus"),
        "{error:?}"
    );
    assert!(!record.exists(), "nothing was set on the agent");
    bridge.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_best_effort_option_the_agent_does_not_offer_is_skipped() {
    let wt = TempDir::new().unwrap();
    let record = wt.path().join("config-choices.txt");
    let bridge = AcpBridge::with_defaults().expect("spawn bridge");
    let session = bridge
        .open_session(config(
            &wt,
            &record,
            vec![
                ConfigSelection {
                    category: ConfigCategory::ThoughtLevel,
                    value: "ultra-deep".into(),
                    best_effort: true,
                },
                ConfigSelection {
                    category: ConfigCategory::Model,
                    value: "Mock Opus".into(),
                    best_effort: false,
                },
            ],
        ))
        .await
        .expect("a floor the agent lacks must not fail the session");
    let recorded = std::fs::read_to_string(&record).expect("the model was still set");
    assert_eq!(recorded.lines().collect::<Vec<_>>(), ["model=opus"]);
    bridge.close_session(session.session).await.unwrap();
    bridge.shutdown().await.unwrap();
}
