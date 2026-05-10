mod fixtures;

use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;

use surge_acp::bridge::event::BridgeEvent;
use surge_acp::bridge::facade::BridgeFacade;
use surge_core::SessionId;
use surge_core::keys::OutcomeKey;
use surge_orchestrator::project_context::{
    ProjectContextOptions, ProjectContextStatus, ScanLimits, describe_project,
    describe_project_with_bridge, scan_project,
};

async fn send_message_recorded(mock: &fixtures::mock_bridge::MockBridge) -> bool {
    let calls = mock.recorded_calls.lock().await;
    calls.iter().any(|call| {
        matches!(
            call,
            fixtures::mock_bridge::RecordedCall::SendMessage { .. }
        )
    })
}

fn write_project_files(root: &std::path::Path) {
    std::fs::write(
        root.join("Cargo.toml"),
        r#"[workspace]
resolver = "2"
members = []

[workspace.dependencies]
tokio = "1"
rusqlite = "0.32"
agent-client-protocol = "0.10"
clap = "4"
"#,
    )
    .unwrap();
    std::fs::write(
        root.join("AGENTS.md"),
        "- No `unwrap()` / `expect()` in library code.\n- Use `tracing::*` macros.\n- Workspace-managed dependencies.\n",
    )
    .unwrap();
    std::fs::write(root.join("justfile"), "build:\n    cargo build\n").unwrap();
    std::fs::write(
        root.join("surge.toml"),
        r#"default_agent = "claude-acp"
bot_token = "super-secret-token"
chat_id = "123456"
bot_token_env = "SURGE_TELEGRAM_BOT_TOKEN"
"#,
    )
    .unwrap();
}

#[test]
fn scan_project_is_deterministic_and_redacts_secret_like_values() {
    let temp = tempfile::tempdir().unwrap();
    write_project_files(temp.path());

    let first = scan_project(temp.path(), ScanLimits::default()).unwrap();
    let second = scan_project(temp.path(), ScanLimits::default()).unwrap();

    assert_eq!(first.hash, second.hash);
    assert_eq!(first.scan_context, second.scan_context);
    assert!(!first.scan_context.contains("super-secret-token"));
    assert!(!first.scan_context.contains("123456"));
    assert!(first.scan_context.contains("SURGE_TELEGRAM_BOT_TOKEN"));

    let surge_toml = first
        .files
        .iter()
        .find(|file| file.relative_path.as_path() == Path::new("surge.toml"))
        .unwrap();
    assert_eq!(surge_toml.redaction_count, 2);
}

#[test]
fn scan_project_records_size_budget_and_generated_directory_skips() {
    let temp = tempfile::tempdir().unwrap();
    write_project_files(temp.path());
    std::fs::write(temp.path().join("README.md"), "x".repeat(128)).unwrap();
    std::fs::create_dir_all(temp.path().join("target")).unwrap();

    let scan = scan_project(
        temp.path(),
        ScanLimits {
            max_file_bytes: 32,
            max_total_bytes: 4096,
        },
    )
    .unwrap();

    assert!(scan.skipped_files.iter().any(|skipped| {
        skipped.relative_path.as_path() == Path::new("README.md")
            && skipped.reason == "oversized_file"
            && skipped.byte_len == Some(128)
            && skipped.hash.is_none()
    }));
    assert!(scan.skipped_files.iter().any(|skipped| {
        skipped.relative_path.as_path() == Path::new("target")
            && skipped.reason == "generated_or_heavy_directory"
    }));
}

#[test]
fn describe_project_writes_then_reports_no_change() {
    let temp = tempfile::tempdir().unwrap();
    write_project_files(temp.path());
    let output = temp.path().join("project.md");

    let first = describe_project(ProjectContextOptions::new(
        temp.path().to_path_buf(),
        output.clone(),
    ))
    .unwrap();

    assert_eq!(first.status, ProjectContextStatus::Drafted);
    let content = std::fs::read_to_string(&output).unwrap();
    assert!(content.contains("surge:project-context scan_hash="));
    assert!(content.contains("profile=project-context-author@1.0.0"));
    assert_eq!(first.normalized_agent_id, "claude-acp");

    let second = describe_project(ProjectContextOptions::new(
        temp.path().to_path_buf(),
        output.clone(),
    ))
    .unwrap();

    assert_eq!(second.status, ProjectContextStatus::NoChange);
    assert_eq!(first.scan_hash, second.scan_hash);
    assert_eq!(first.output_hash, second.output_hash);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn describe_project_with_bridge_invokes_project_context_author() {
    let temp = tempfile::tempdir().unwrap();
    write_project_files(temp.path());
    let authored_path = temp.path().join("authored-project.md");
    let authored = "# Authored project context\n\n## Project name\nbridge-smoke\n";
    std::fs::write(&authored_path, authored).unwrap();

    let mock = Arc::new(fixtures::mock_bridge::MockBridge::new());
    let bridge: Arc<dyn BridgeFacade> = mock.clone();
    let session = SessionId::new();
    mock.pin_next_session_id(session).await;
    mock.enqueue_event(BridgeEvent::OutcomeReported {
        session,
        outcome: OutcomeKey::from_str("drafted").unwrap(),
        summary: "drafted project context".into(),
        artifacts_produced: vec!["authored-project.md".into()],
    })
    .await;

    let mock_for_pump = mock.clone();
    let pump = tokio::spawn(async move {
        for _ in 0..50 {
            if send_message_recorded(&mock_for_pump).await {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        mock_for_pump.pump_scripted_events().await;
    });

    let output = temp.path().join("project.md");
    let outcome = describe_project_with_bridge(
        ProjectContextOptions::new(temp.path().to_path_buf(), output.clone()),
        bridge,
    )
    .await
    .unwrap();
    pump.await.unwrap();

    assert_eq!(outcome.status, ProjectContextStatus::Drafted);
    let output = std::fs::read_to_string(output).unwrap();
    assert!(output.contains("surge:project-context scan_hash="));
    assert!(output.contains("profile=project-context-author@1.0.0"));
    assert!(output.contains(authored));

    let calls = mock.recorded_calls.lock().await;
    assert!(
        calls
            .iter()
            .any(|call| { matches!(call, fixtures::mock_bridge::RecordedCall::OpenSession) })
    );
    assert!(calls.iter().any(|call| {
        matches!(
            call,
            fixtures::mock_bridge::RecordedCall::SendMessage { .. }
        )
    }));
    assert!(
        calls
            .iter()
            .any(|call| { matches!(call, fixtures::mock_bridge::RecordedCall::CloseSession(_)) })
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn describe_project_with_bridge_rejects_artifacts_outside_project_root() {
    let temp = tempfile::tempdir().unwrap();
    write_project_files(temp.path());
    let outside = tempfile::NamedTempFile::new().unwrap();

    let mock = Arc::new(fixtures::mock_bridge::MockBridge::new());
    let bridge: Arc<dyn BridgeFacade> = mock.clone();
    let session = SessionId::new();
    mock.pin_next_session_id(session).await;
    mock.enqueue_event(BridgeEvent::OutcomeReported {
        session,
        outcome: OutcomeKey::from_str("drafted").unwrap(),
        summary: "drafted project context".into(),
        artifacts_produced: vec![outside.path().display().to_string()],
    })
    .await;

    let mock_for_pump = mock.clone();
    let pump = tokio::spawn(async move {
        for _ in 0..50 {
            if send_message_recorded(&mock_for_pump).await {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        mock_for_pump.pump_scripted_events().await;
    });

    let err = describe_project_with_bridge(
        ProjectContextOptions::new(temp.path().to_path_buf(), temp.path().join("project.md")),
        bridge,
    )
    .await
    .unwrap_err();
    pump.await.unwrap();

    assert!(err.to_string().contains("escapes project root"));
}
