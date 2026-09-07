//! `surge run report <id> --format json|md|html` end to end (R14, R27,
//! R27.1, R28, R29, R33).
//!
//! `RunReport::compile` itself is a pure fold over event fixtures and is
//! unit-tested directly in `surge_core::run_report` (no DB, no CLI). This
//! file exercises the other half: the real `surge` binary resolving a run
//! id, reading its event log through `surge-persistence`, and rendering the
//! three formats — the same seed-events-directly-then-invoke-the-binary
//! pattern `cli_replay.rs` uses, so no agent runtime is needed.

use std::collections::BTreeMap;
use std::path::Path;

use surge_core::approvals::ApprovalPolicy;
use surge_core::content_hash::ContentHash;
use surge_core::graph::{Graph, GraphMetadata, SCHEMA_VERSION};
use surge_core::id::RunId;
use surge_core::keys::{NodeKey, OutcomeKey};
use surge_core::node::{Node, NodeConfig, Position};
use surge_core::run_event::{EventPayload, RunConfig, VersionedEventPayload};
use surge_core::sandbox::SandboxMode;
use surge_core::skill::SkillProvider;
use surge_core::terminal_config::{TerminalConfig, TerminalKind};
use surge_persistence::runs::Storage;

fn minimal_graph(node_name: &str) -> Graph {
    let node = NodeKey::try_from(node_name).unwrap();
    let mut nodes = BTreeMap::new();
    nodes.insert(
        node.clone(),
        Node {
            id: node.clone(),
            position: Position::default(),
            declared_outcomes: vec![],
            config: NodeConfig::Terminal(TerminalConfig {
                kind: TerminalKind::Success,
                message: None,
            }),
        },
    );
    Graph {
        schema_version: SCHEMA_VERSION,
        metadata: GraphMetadata {
            name: "run-report-cli-test".into(),
            description: None,
            template_origin: None,
            created_at: chrono::Utc::now(),
            author: None,
            archetype: None,
        },
        start: node,
        nodes,
        edges: vec![],
        subgraphs: BTreeMap::new(),
    }
}

fn run_config() -> RunConfig {
    RunConfig {
        budget: Default::default(),
        sandbox_default: SandboxMode::WorkspaceWrite,
        approval_default: ApprovalPolicy::OnRequest,
        auto_pr: false,
        mcp_servers: vec![],
    }
}

/// Seed a completed run exercising every report section but `memory_receipts`
/// (which no event carries yet — see `surge_core::run_report`'s module doc):
/// a node with an attempt, a skill bind, an artifact, token spend, a
/// declared outcome, a verifier verdict, an operator steer, and a
/// `RunCompleted` terminal event.
async fn seed_rich_completed_run(home: &Path) -> RunId {
    let storage = Storage::open(home).await.unwrap();
    let run = RunId::new();
    let worktree = home.to_path_buf();
    let writer = storage.create_run(run, &worktree, None).await.unwrap();

    let node = NodeKey::try_from("implement").unwrap();
    let graph = minimal_graph("implement");
    let graph_hash = ContentHash::compute(&serde_json::to_vec(&graph).unwrap());

    writer
        .append_events(vec![
            VersionedEventPayload::new(EventPayload::RunStarted {
                pipeline_template: None,
                project_path: worktree.clone(),
                initial_prompt: "seed".into(),
                config: run_config(),
            }),
            VersionedEventPayload::new(EventPayload::PipelineMaterialized {
                graph: Box::new(graph),
                graph_hash,
            }),
            VersionedEventPayload::new(EventPayload::StageEntered {
                node: node.clone(),
                attempt: 1,
            }),
            VersionedEventPayload::new(EventPayload::SkillBound {
                node: node.clone(),
                name: "archify".into(),
                provider: SkillProvider::ProjectDir,
                hash: ContentHash::compute(b"archify-pack"),
                gate_enabled: true,
            }),
            VersionedEventPayload::new(EventPayload::ArtifactProduced {
                node: node.clone(),
                artifact: ContentHash::compute(b"diff-content"),
                path: "artifacts/diff.patch".into(),
                name: "diff.patch".into(),
            }),
            VersionedEventPayload::new(EventPayload::TokensConsumed {
                session: surge_core::id::SessionId::new(),
                prompt_tokens: 500,
                output_tokens: 250,
                cache_hits: 10,
                model: "claude-opus-4-7".into(),
                cost_usd: Some(0.015),
            }),
            VersionedEventPayload::new(EventPayload::OutcomeReported {
                node: node.clone(),
                outcome: OutcomeKey::try_from("done").unwrap(),
                summary: "implemented the change".into(),
            }),
            VersionedEventPayload::new(EventPayload::SteerDelivered {
                id: "steer-1".into(),
                node: node.clone(),
                message: "keep the diff minimal".into(),
            }),
            VersionedEventPayload::new(EventPayload::StageCompleted {
                node: node.clone(),
                outcome: OutcomeKey::try_from("done").unwrap(),
            }),
            VersionedEventPayload::new(EventPayload::RunCompleted {
                terminal_node: node,
            }),
        ])
        .await
        .unwrap();
    drop(writer);
    drop(storage);
    run
}

/// Seed a run whose log stops mid-flight: `StageEntered` with no matching
/// `StageCompleted`/`StageFailed`, and no terminal lifecycle event at all —
/// the crashed/still-running case R27.1 names explicitly.
async fn seed_torn_run(home: &Path) -> RunId {
    let storage = Storage::open(home).await.unwrap();
    let run = RunId::new();
    let worktree = home.to_path_buf();
    let writer = storage.create_run(run, &worktree, None).await.unwrap();

    writer
        .append_events(vec![
            VersionedEventPayload::new(EventPayload::RunStarted {
                pipeline_template: None,
                project_path: worktree.clone(),
                initial_prompt: "seed".into(),
                config: run_config(),
            }),
            VersionedEventPayload::new(EventPayload::StageEntered {
                node: NodeKey::try_from("implement").unwrap(),
                attempt: 1,
            }),
        ])
        .await
        .unwrap();
    drop(writer);
    drop(storage);
    run
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn report_json_reconstructs_every_populated_section() {
    let tmp = tempfile::tempdir().unwrap();
    let run = seed_rich_completed_run(tmp.path()).await;

    let assert = assert_cmd::Command::cargo_bin("surge")
        .unwrap()
        .env("SURGE_HOME", tmp.path())
        .args(["run", "report", &run.to_string(), "--format", "json"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let json: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("stdout must be JSON: {e}\n{stdout}"));

    // `RunId` serializes as the bare ULID (no `run-` prefix) — see
    // `surge_core::id`'s `define_id!` macro doc; `Display` (used by
    // `run.to_string()`) adds the prefix for human-facing text instead.
    assert_eq!(json["run_id"], run.as_ulid().to_string());
    assert_eq!(json["completion"]["status"], "completed");

    let nodes = json["nodes"].as_array().expect("nodes array");
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0]["node"], "implement");
    assert_eq!(nodes[0]["attempts"], 1);
    assert_eq!(nodes[0]["status"]["state"], "completed");

    let skills = json["skills"].as_array().expect("skills array");
    assert_eq!(skills.len(), 1, "R14: every bound skill must be listed");
    assert_eq!(skills[0]["name"], "archify");
    assert_eq!(skills[0]["provider"], "project_dir");
    assert_eq!(skills[0]["gate_enabled"], true);

    let evidence = json["evidence"].as_array().expect("evidence array");
    assert_eq!(evidence.len(), 1);
    assert_eq!(evidence[0]["name"], "diff.patch");

    let outcomes = json["outcomes"].as_array().expect("outcomes array");
    assert_eq!(outcomes.len(), 1);
    assert_eq!(outcomes[0]["outcome"], "done");

    let steers = json["steers"].as_array().expect("steers array");
    assert_eq!(steers.len(), 1);
    assert_eq!(steers[0]["id"], "steer-1");

    assert_eq!(json["cost"]["prompt_tokens"], 500);
    assert_eq!(json["cost"]["output_tokens"], 250);
    assert_eq!(json["cost"]["cache_hits"], 10);
    assert_eq!(
        json["cost"]["uncosted_token_events"], 0,
        "this fixture's TokensConsumed always carries a price"
    );

    assert_eq!(json["header"]["initial_prompt"], "seed");
    assert!(
        json["header"]["first_event_at"].is_string(),
        "header must carry the first event's timestamp"
    );
    assert!(
        json["header"]["last_event_at"].is_string(),
        "header must carry the last event's timestamp"
    );

    // Named limitation (module doc): no event carries a context-pack
    // receipt yet, so this stays empty even on a rich, completed run.
    assert!(json["memory_receipts"].as_array().unwrap().is_empty());
    // ...but the JSON form still names that gap explicitly, so a machine
    // consumer cannot mistake the empty array for "memory was not used."
    let caveats = json["caveats"].as_array().expect("caveats array");
    assert!(
        caveats
            .iter()
            .any(|c| c.as_str().unwrap_or_default().contains("memory_receipts")),
        "caveats: {caveats:?}"
    );
}

/// Seed a run stopped by a guard escalation — no terminal event, but the
/// escalation itself is in the log. R33's "why did it stop" must survive
/// through the real CLI path, not just the unit-tested compiler.
async fn seed_escalation_stopped_run(home: &Path) -> RunId {
    let storage = Storage::open(home).await.unwrap();
    let run = RunId::new();
    let worktree = home.to_path_buf();
    let writer = storage.create_run(run, &worktree, None).await.unwrap();

    writer
        .append_events(vec![
            VersionedEventPayload::new(EventPayload::RunStarted {
                pipeline_template: None,
                project_path: worktree,
                initial_prompt: "seed".into(),
                config: run_config(),
            }),
            VersionedEventPayload::new(EventPayload::EscalationRequested {
                stage: None,
                reason: "node has run past its wall-clock budget".into(),
                cause: surge_core::run_event::EscalationCause::LoopGuardNodeDeadline,
            }),
        ])
        .await
        .unwrap();
    drop(writer);
    drop(storage);
    run
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn report_names_why_a_guard_stopped_run_is_incomplete() {
    let tmp = tempfile::tempdir().unwrap();
    let run = seed_escalation_stopped_run(tmp.path()).await;

    let assert = assert_cmd::Command::cargo_bin("surge")
        .unwrap()
        .env("SURGE_HOME", tmp.path())
        .args(["run", "report", &run.to_string(), "--format", "json"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let json: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(json["completion"]["status"], "incomplete");
    let escalations = json["escalations"].as_array().expect("escalations array");
    assert_eq!(escalations.len(), 1);
    assert_eq!(escalations[0]["cause"], "loop_guard_node_deadline");
}

/// Seed a run parked on a provider rate limit — R27.1/§1(16): parked is not
/// a bare "not finished," it is a distinct, log-provable pause.
async fn seed_parked_run(home: &Path) -> RunId {
    let storage = Storage::open(home).await.unwrap();
    let run = RunId::new();
    let worktree = home.to_path_buf();
    let writer = storage.create_run(run, &worktree, None).await.unwrap();

    writer
        .append_events(vec![
            VersionedEventPayload::new(EventPayload::RunStarted {
                pipeline_template: None,
                project_path: worktree.clone(),
                initial_prompt: "seed".into(),
                config: run_config(),
            }),
            VersionedEventPayload::new(EventPayload::RunParked {
                wake_at: chrono::Utc::now(),
                runtime: Some("claude-acp".into()),
                worktree,
                basis: surge_core::capacity::WakeBasis::ObservedReset,
                reason: "rate limited".into(),
            }),
        ])
        .await
        .unwrap();
    drop(writer);
    drop(storage);
    run
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn report_renders_a_parked_run_distinctly_from_not_finished() {
    let tmp = tempfile::tempdir().unwrap();
    let run = seed_parked_run(tmp.path()).await;

    let assert = assert_cmd::Command::cargo_bin("surge")
        .unwrap()
        .env("SURGE_HOME", tmp.path())
        .args(["run", "report", &run.to_string()])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();

    assert!(stdout.contains("PARKED"), "got:\n{stdout}");
    assert!(
        !stdout.contains("This run has not finished"),
        "a parked run is a proven pause, not an unexplained stall:\n{stdout}"
    );
}

/// A hook-rejected outcome must not read as an accepted one — the CLI-level
/// half of the internal-consistency point the review round raised.
async fn seed_run_with_a_hook_rejected_outcome(home: &Path) -> RunId {
    let storage = Storage::open(home).await.unwrap();
    let run = RunId::new();
    let worktree = home.to_path_buf();
    let writer = storage.create_run(run, &worktree, None).await.unwrap();
    let node = NodeKey::try_from("implement").unwrap();
    let outcome = OutcomeKey::try_from("done").unwrap();

    writer
        .append_events(vec![
            VersionedEventPayload::new(EventPayload::RunStarted {
                pipeline_template: None,
                project_path: worktree,
                initial_prompt: "seed".into(),
                config: run_config(),
            }),
            VersionedEventPayload::new(EventPayload::OutcomeReported {
                node: node.clone(),
                outcome: outcome.clone(),
                summary: "looked done".into(),
            }),
            VersionedEventPayload::new(EventPayload::OutcomeRejectedByHook {
                node,
                outcome,
                hook_id: "test-runner".into(),
            }),
        ])
        .await
        .unwrap();
    drop(writer);
    drop(storage);
    run
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn report_shows_a_hook_rejected_outcome_as_rejected_not_accepted() {
    let tmp = tempfile::tempdir().unwrap();
    let run = seed_run_with_a_hook_rejected_outcome(tmp.path()).await;

    let assert = assert_cmd::Command::cargo_bin("surge")
        .unwrap()
        .env("SURGE_HOME", tmp.path())
        .args(["run", "report", &run.to_string(), "--format", "json"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let json: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    let outcomes = json["outcomes"].as_array().expect("outcomes array");
    assert_eq!(
        outcomes.len(),
        1,
        "the rejection updates the entry in place"
    );
    assert_eq!(outcomes[0]["status"]["outcome_status"], "rejected_by_hook");
    assert_eq!(outcomes[0]["status"]["hook_id"], "test-runner");
}

/// R27.1: a run whose log never reaches a terminal event still compiles a
/// report, through the real CLI path — no error, and the incomplete banner
/// is visible in the default (Markdown) rendering.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn report_defaults_to_markdown_and_flags_an_incomplete_run() {
    let tmp = tempfile::tempdir().unwrap();
    let run = seed_torn_run(tmp.path()).await;

    let assert = assert_cmd::Command::cargo_bin("surge")
        .unwrap()
        .env("SURGE_HOME", tmp.path())
        .args(["run", "report", &run.to_string()])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();

    assert!(
        stdout.contains("## Nodes"),
        "must render as Markdown by default"
    );
    assert!(
        stdout.contains("This run has not finished"),
        "R27.1: a torn run must say so explicitly, got:\n{stdout}"
    );
}

/// R29: the HTML form is one self-contained file, reachable through the
/// real CLI path — no external stylesheet, script, or CDN reference.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn report_html_format_is_self_contained() {
    let tmp = tempfile::tempdir().unwrap();
    let run = seed_rich_completed_run(tmp.path()).await;

    let assert = assert_cmd::Command::cargo_bin("surge")
        .unwrap()
        .env("SURGE_HOME", tmp.path())
        .args(["run", "report", &run.to_string(), "--format", "html"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();

    assert!(stdout.contains("<!doctype html>"));
    assert!(stdout.contains("<style>"), "styles must be inlined");
    assert!(!stdout.contains("<link"));
    assert!(!stdout.to_lowercase().contains("<script"));
    assert!(!stdout.contains("http://") && !stdout.contains("https://"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn report_rejects_an_unknown_run_id() {
    let tmp = tempfile::tempdir().unwrap();

    assert_cmd::Command::cargo_bin("surge")
        .unwrap()
        .env("SURGE_HOME", tmp.path())
        .args(["run", "report", &RunId::new().to_string()])
        .assert()
        .failure();
}
