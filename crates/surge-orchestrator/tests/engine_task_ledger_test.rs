//! Phase 1 M3 — engine emits task-ledger events from agent stages, and the
//! sealed-verifier gate rejects a `Verified` outcome from a non-read-only
//! sandbox.
//!
//! These drive `execute_agent_stage` directly (the model used by
//! `engine_agent_artifact_emission_test`) so the ledger emission and authority
//! gate are exercised without the full loop machinery. The `active_task_id`
//! that a real run derives from the loop frame is supplied explicitly here.

mod fixtures;
use fixtures::runtime_home as runtime_home_fixture;
use runtime_home_fixture::FixtureHome;

use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use surge_acp::bridge::event::BridgeEvent;
use surge_acp::bridge::facade::BridgeFacade;
use surge_core::agent_config::{AgentConfig, NodeLimits};
use surge_core::edge::EdgeKind;
use surge_core::id::SessionId;
use surge_core::keys::{NodeKey, OutcomeKey, ProfileKey};
use surge_core::node::{LedgerEffect, OutcomeDecl};
use surge_core::roadmap::RoadmapTaskId;
use surge_core::run_event::EventPayload;
use surge_core::sandbox::{SandboxConfig, SandboxMode};
use surge_orchestrator::engine::hooks::HookExecutor;
use surge_orchestrator::engine::stage::agent::{AgentStageParams, execute_agent_stage};
use surge_orchestrator::engine::tools::{
    ToolCall, ToolDispatchContext, ToolDispatcher, ToolResultPayload,
};
use surge_persistence::runs::{EventSeq, Storage};

struct UnusedDispatcher;

#[async_trait::async_trait]
impl ToolDispatcher for UnusedDispatcher {
    async fn dispatch(&self, _ctx: &ToolDispatchContext<'_>, call: &ToolCall) -> ToolResultPayload {
        ToolResultPayload::Unsupported {
            message: format!("unused: {}", call.tool),
        }
    }
}

fn agent_cfg(sandbox: Option<SandboxMode>, max_retries: u32) -> AgentConfig {
    AgentConfig {
        profile: ProfileKey::try_from("implementer@1.0").unwrap(),
        prompt_overrides: None,
        tool_overrides: None,
        sandbox_override: sandbox.map(|mode| SandboxConfig {
            mode,
            ..Default::default()
        }),
        approvals_override: None,
        bindings: vec![],
        rules_overrides: None,
        limits: NodeLimits {
            max_retries,
            ..Default::default()
        },
        hooks: vec![],
        custom_fields: Default::default(),
    }
}

fn outcome_decl(id: &str, effect: LedgerEffect) -> OutcomeDecl {
    OutcomeDecl {
        id: OutcomeKey::try_from(id).unwrap(),
        description: format!("{id} outcome"),
        edge_kind_hint: EdgeKind::Forward,
        is_terminal: false,
        ledger_effect: effect,
    }
}

/// Drive one agent stage to report `outcome`, returning the ordered event
/// payloads appended to the run log.
async fn run_stage(
    dir: &std::path::Path,
    cfg: &AgentConfig,
    declared: &[OutcomeDecl],
    node_name: &str,
    outcome: &str,
    artifacts_produced: Vec<String>,
    active_task_id: Option<surge_core::roadmap::RoadmapTaskId>,
) -> (Result<OutcomeKey, String>, Vec<EventPayload>) {
    run_stage_steered(
        dir,
        cfg,
        declared,
        node_name,
        outcome,
        artifacts_produced,
        active_task_id,
        Vec::new(),
        None,
    )
    .await
    .0
}

/// Like [`run_stage`] but injects operator steer messages and also returns the
/// mock bridge so the caller can inspect the prompt that was actually sent.
#[allow(clippy::too_many_arguments)]
async fn run_stage_steered(
    dir: &std::path::Path,
    cfg: &AgentConfig,
    declared: &[OutcomeDecl],
    node_name: &str,
    outcome: &str,
    artifacts_produced: Vec<String>,
    active_task_id: Option<surge_core::roadmap::RoadmapTaskId>,
    steers: Vec<surge_orchestrator::engine::steer::QueuedSteer>,
    verification_report: Option<surge_core::roadmap::VerificationReportArtifact>,
) -> (
    (Result<OutcomeKey, String>, Vec<EventPayload>),
    Arc<fixtures::mock_bridge::MockBridge>,
) {
    let storage_home = FixtureHome::new().unwrap();
    let fixture_result = {
        let storage = Storage::open(storage_home.path()).await.unwrap();
        let frames = if let Some(task) = active_task_id
            .as_ref()
            .filter(|_| verification_report.is_some())
        {
            for args in [
                vec!["init"],
                vec!["config", "user.name", "Fixture"],
                vec!["config", "user.email", "fixture@example.com"],
                vec!["add", "."],
                vec!["commit", "--allow-empty", "-m", "fixture"],
            ] {
                assert!(
                    std::process::Command::new("git")
                        .args(args)
                        .current_dir(dir)
                        .output()
                        .unwrap()
                        .status
                        .success()
                );
            }
            let id = task.as_str();
            let item: toml::Value = toml::from_str(&format!(
            "id = {id:?}\ntitle = \"Acceptance\"\nacceptance_criteria = [\"Acceptance works\"]\n"
        ))
        .unwrap();
            vec![surge_orchestrator::engine::frames::Frame::Loop(
                surge_orchestrator::engine::frames::LoopFrame {
                    loop_node: NodeKey::try_from("tasks").unwrap(),
                    config: surge_core::loop_config::LoopConfig {
                        iterates_over: surge_core::loop_config::IterableSource::Static(vec![
                            item.clone(),
                        ]),
                        body: surge_core::SubgraphKey::try_from("body").unwrap(),
                        iteration_var_name: "task".into(),
                        exit_condition: surge_core::loop_config::ExitCondition::AllItems,
                        on_iteration_failure: Default::default(),
                        parallelism: Default::default(),
                        gate_after_each: false,
                    },
                    items: vec![item],
                    current_index: 0,
                    attempts_remaining: 0,
                    return_to: NodeKey::try_from("end").unwrap(),
                    traversal_counts: Default::default(),
                },
            )]
        } else {
            Vec::new()
        };
        let run_id = surge_core::id::RunId::new();
        let writer = storage.create_run(run_id, dir, None).await.unwrap();
        if dir.join("reject-ledger.fixture").exists() {
            let journal = storage_home
                .path()
                .join("runs")
                .join(run_id.to_string())
                .join("events.sqlite");
            let output = std::process::Command::new("python3").args([
            "-c",
            "import sqlite3,sys; c=sqlite3.connect(sys.argv[1]); c.execute(\"CREATE TRIGGER fixture_reject_ledger BEFORE INSERT ON events WHEN NEW.kind='TaskStatusChanged' BEGIN SELECT RAISE(ABORT, 'fixture ledger failure'); END\"); c.commit()",
        ]).arg(journal).output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let artifact_store =
            surge_persistence::artifacts::ArtifactStore::new(storage_home.path().join("runs"));

        let mock = Arc::new(fixtures::mock_bridge::MockBridge::new());
        let bridge: Arc<dyn BridgeFacade> = mock.clone();
        let session_id = SessionId::new();
        mock.pin_next_session_id(session_id).await;
        mock.enqueue_event(BridgeEvent::OutcomeReported {
            session: session_id,
            outcome: OutcomeKey::from_str(outcome).unwrap(),
            summary: "done".into(),
            artifacts_produced,

            verification_report: verification_report.map(Box::new),
        })
        .await;

        let mock_for_pump = mock.clone();
        let pump = tokio::spawn(async move {
            mock_for_pump.pump_after_subscribe(1).await;
        });

        let dispatcher: Arc<dyn ToolDispatcher> = Arc::new(UnusedDispatcher);
        let memory = surge_core::run_state::RunMemory::default();
        let node = NodeKey::try_from(node_name).unwrap();
        let tool_resolutions =
            std::sync::Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
        let hook_executor = HookExecutor::new();

        let result = tokio::time::timeout(
            Duration::from_secs(8),
            execute_agent_stage(AgentStageParams {
                quota_opening: None,
                quota_cycle: None,
                quota_owner: None,
                continuation: None,
                frames: &frames,
                cancel: tokio_util::sync::CancellationToken::new(),
                steers: steers.clone(),
                node: &node,
                attempt: 1,
                agent_config: cfg,
                bound_skills: &[],
                declared_outcomes: declared,
                bridge: &bridge,
                writer: &writer,
                artifact_store: &artifact_store,
                worktree_path: dir,
                tool_dispatcher: &dispatcher,
                run_memory: &memory,
                run_id,
                tool_resolutions: &tool_resolutions,
                human_input_timeout: Duration::from_secs(5),
                mcp_registry: None,
                mcp_servers: Vec::new(),
                tool_call_loop_guard: surge_core::loop_config::ToolCallLoopGuardConfig::default(),
                output_spill: surge_core::spill_config::OutputSpillConfig::default(),
                profile_registry: if cfg.profile.as_str() == "verifier@2.0" {
                    Some(Arc::new(
                        surge_orchestrator::profile_loader::ProfileRegistry::new(
                            surge_orchestrator::profile_loader::DiskProfileSet::empty(),
                        ),
                    ))
                } else {
                    None
                },
                agent_registry: None,
                hook_executor: &hook_executor,
                pending_elevations: surge_orchestrator::engine::elevation::PendingElevations::new(),
                active_task_id,
            }),
        )
        .await
        .unwrap_or_else(|_| {
            Err(surge_orchestrator::engine::stage::StageError::Internal(
                "bounded fixture timeout".into(),
            ))
        })
        .map_err(|e| e.to_string());
        pump.await.unwrap();

        let reader = storage.open_run_reader(run_id).await.unwrap();
        let events = reader
            .read_events(EventSeq(0)..EventSeq(256))
            .await
            .unwrap();
        let payloads: Vec<EventPayload> =
            events.iter().map(|e| e.payload.payload().clone()).collect();
        writer.close().await.unwrap();

        if result.is_err() {
            eprintln!(
                "stage failure {result:?}, events={payloads:?}, last_prompt={:?}",
                mock.last_prompt().await
            );
        }
        ((result, payloads), mock)
    };
    storage_home.close().unwrap();
    fixture_result
}

#[tokio::test(flavor = "multi_thread")]
async fn accepted_outcome_is_not_committed_when_its_required_ledger_effect_fails() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("reject-ledger.fixture"), "fixture").unwrap();
    let (result, events) = run_stage(
        dir.path(),
        &agent_cfg(None, 0),
        &[outcome_decl("done", LedgerEffect::ReadyForVerification)],
        "implement",
        "done",
        Vec::new(),
        Some(RoadmapTaskId::from("task-1")),
    )
    .await;
    assert!(result.is_err(), "rejected ledger write must fail the stage");
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, EventPayload::OutcomeReported { .. })),
        "an outcome without its ledger effect is not a committed outcome"
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, EventPayload::StageOutcomeCommitted { .. })),
        "rejected required effects cannot publish an acceptance marker"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn accepted_outcome_has_a_durable_invocation_commit_marker() {
    let dir = tempfile::tempdir().unwrap();
    let (result, events) = run_stage(
        dir.path(),
        &agent_cfg(None, 0),
        &[outcome_decl("done", LedgerEffect::None)],
        "implement",
        "done",
        Vec::new(),
        None,
    )
    .await;
    assert!(result.is_ok(), "{result:?}");
    assert!(
        events
            .iter()
            .any(|event| event.discriminant_str() == "StageOutcomeCommitted"),
        "raw OutcomeReported cannot distinguish an accepted stage commit after a host crash"
    );
    let (position, commit) = events
        .iter()
        .enumerate()
        .find_map(|(position, event)| {
            if let EventPayload::StageOutcomeCommitted { commit } = event {
                Some((position, commit))
            } else {
                None
            }
        })
        .unwrap();
    let opened = events
        .iter()
        .find_map(|event| {
            if let EventPayload::SessionOpened {
                opened: Some(opened),
                ..
            } = event
            {
                Some(opened)
            } else {
                None
            }
        })
        .unwrap();
    assert_eq!(commit.provider_connection(), opened.session);
    assert_eq!(commit.invocation(), opened.descriptor.invocation());
    assert_eq!(commit.context().node.as_str(), "implement");
    assert_ne!(
        commit.context().session,
        opened.session,
        "MCP authority and provider handles must remain distinct"
    );
    let count = commit.effects_count() as usize;
    let effects: Vec<_> = events[position - count..position]
        .iter()
        .cloned()
        .map(surge_core::VersionedEventPayload::new)
        .collect();
    assert_eq!(
        commit.effects_hash(),
        &surge_core::ContentHash::compute(&serde_json::to_vec(&effects).unwrap())
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ready_for_verification_outcome_emits_task_status_changed() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = agent_cfg(None, 3); // default WorkspaceWrite — allowed for impl
    let declared = [outcome_decl(
        "ready_for_verification",
        LedgerEffect::ReadyForVerification,
    )];
    let (result, payloads) = run_stage(
        dir.path(),
        &cfg,
        &declared,
        "impl_1",
        "ready_for_verification",
        vec![],
        Some("m1-t1".into()),
    )
    .await;

    assert_eq!(result.unwrap().as_ref(), "ready_for_verification");
    let status_changed = payloads
        .iter()
        .find_map(|p| match p {
            EventPayload::TaskStatusChanged {
                task_id, from, to, ..
            } => Some((task_id.clone(), *from, *to)),
            _ => None,
        })
        .expect("TaskStatusChanged emitted");
    assert_eq!(status_changed.0, "m1-t1");
    assert_eq!(status_changed.1, surge_core::RoadmapStatus::Pending);
    assert_eq!(
        status_changed.2,
        surge_core::RoadmapStatus::ReadyForVerification
    );
    assert!(
        !payloads
            .iter()
            .any(|p| matches!(p, EventPayload::TaskVerified { .. })),
        "no TaskVerified for a ready_for_verification outcome"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sealed_verifier_verified_outcome_emits_task_verified() {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .try_init();
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = agent_cfg(Some(SandboxMode::ReadOnly), 3);
    cfg.profile = ProfileKey::try_from("verifier@2.0").unwrap();
    cfg.bindings = vec![
        surge_core::agent_config::Binding {
            source: surge_core::agent_config::ArtifactSource::Static {
                content: "Acceptance works".into(),
            },
            target: surge_core::agent_config::TemplateVar("spec".into()),
            optional: false,
        },
        surge_core::agent_config::Binding {
            source: surge_core::agent_config::ArtifactSource::Static {
                content: String::new(),
            },
            target: surge_core::agent_config::TemplateVar("project_memory".into()),
            optional: true,
        },
    ];
    let declared = [outcome_decl("passed", LedgerEffect::Verified)];
    let report: surge_core::roadmap::VerificationReportArtifact = toml::from_str("task_id = \"m1-t1\"\noutcome = \"passed\"\nsummary = \"Acceptance checked\"\n[[checks]]\ncommand = \"cargo test\"\nresult = \"passed\"\ncovers = [\"criterion:1\"]\n").unwrap();
    let ((result, payloads), _) = run_stage_steered(
        dir.path(),
        &cfg,
        &declared,
        "verify_1",
        "passed",
        vec![],
        Some("m1-t1".into()),
        vec![],
        Some(report),
    )
    .await;
    assert_eq!(result.unwrap().as_ref(), "passed");
    let (task, node, evidence, report) = payloads
        .iter()
        .find_map(|event| match event {
            EventPayload::TaskVerified {
                task_id,
                node,
                evidence,
                report: Some(report),
            } => Some((task_id, node, evidence, report)),
            _ => None,
        })
        .expect("bound proof emitted");
    assert_eq!(task.as_str(), "m1-t1");
    assert_eq!(node.as_str(), "verify_1");
    assert_eq!(
        *evidence,
        surge_core::ContentHash::compute(toml::to_string(report).unwrap().as_bytes())
    );
    assert!(report.binding.is_some());
    assert!(
        !dir.path().join("verification-report.toml").exists(),
        "sealed report never writes the checked tree"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn discovered_tasks_artifact_emits_task_discovered() {
    let dir = tempfile::tempdir().unwrap();
    tokio::fs::write(
        dir.path().join("discovered-tasks.toml"),
        b"schema_version = 1\n\n\
          [[tasks]]\nid = \"m1-t9\"\ntitle = \"Handle empty export\"\n\n\
          [[tasks]]\nid = \"m1-t10\"\ntitle = \"Add pagination\"\n",
    )
    .await
    .unwrap();

    let cfg = agent_cfg(None, 3);
    let declared = [outcome_decl("implemented", LedgerEffect::None)];
    let (result, payloads) = run_stage(
        dir.path(),
        &cfg,
        &declared,
        "impl_1",
        "implemented",
        vec!["discovered-tasks.toml".into()],
        Some("m1-t1".into()),
    )
    .await;

    assert_eq!(result.unwrap().as_ref(), "implemented");
    let discovered: Vec<(RoadmapTaskId, RoadmapTaskId, String)> = payloads
        .iter()
        .filter_map(|p| match p {
            EventPayload::TaskDiscovered {
                task_id,
                discovered_from,
                title,
            } => Some((task_id.clone(), discovered_from.clone(), title.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(discovered.len(), 2);
    assert_eq!(discovered[0].0, "m1-t9");
    assert_eq!(discovered[0].1, "m1-t1");
    assert_eq!(discovered[0].2, "Handle empty export");
    assert_eq!(discovered[1].0, "m1-t10");
    assert_eq!(discovered[1].1, "m1-t1");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn malformed_discovered_tasks_artifact_is_skipped() {
    let dir = tempfile::tempdir().unwrap();
    tokio::fs::write(
        dir.path().join("discovered-tasks.toml"),
        b"this is = not valid = toml [[[",
    )
    .await
    .unwrap();

    let cfg = agent_cfg(None, 3);
    let declared = [outcome_decl("implemented", LedgerEffect::None)];
    let (result, payloads) = run_stage(
        dir.path(),
        &cfg,
        &declared,
        "impl_1",
        "implemented",
        vec!["discovered-tasks.toml".into()],
        Some("m1-t1".into()),
    )
    .await;

    // The stage still succeeds; the malformed artifact is skipped.
    assert_eq!(result.unwrap().as_ref(), "implemented");
    assert!(
        !payloads
            .iter()
            .any(|p| matches!(p, EventPayload::TaskDiscovered { .. })),
        "a malformed discovered-tasks artifact emits no events"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unsealed_verified_outcome_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    // Default WorkspaceWrite sandbox (not sealed) + max_retries=0 so the first
    // rejection fails the stage immediately.
    let cfg = agent_cfg(None, 0);
    let declared = [outcome_decl("passed", LedgerEffect::Verified)];
    let (result, payloads) = run_stage(
        dir.path(),
        &cfg,
        &declared,
        "verify_1",
        "passed",
        vec![],
        Some("m1-t1".into()),
    )
    .await;

    assert!(
        result.is_err(),
        "an unsealed Verified outcome must fail the stage, got {result:?}"
    );
    let rejected = payloads.iter().find_map(|p| match p {
        EventPayload::OutcomeRejectedByHook { hook_id, .. } => Some(hook_id.clone()),
        _ => None,
    });
    assert_eq!(
        rejected.as_deref(),
        Some("verification_authority"),
        "the sealed-verifier gate recorded the rejection"
    );
    assert!(
        !payloads
            .iter()
            .any(|p| matches!(p, EventPayload::TaskVerified { .. })),
        "no TaskVerified from an unsealed sandbox"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn project_memory_note_is_stamped_with_provenance() {
    let dir = tempfile::tempdir().unwrap();
    let mem_dir = dir.path().join(".surge/memory");
    tokio::fs::create_dir_all(&mem_dir).await.unwrap();
    tokio::fs::write(
        mem_dir.join("auth.md"),
        b"# Auth invariant\nTokens are always HS256.\n",
    )
    .await
    .unwrap();

    let cfg = agent_cfg(None, 3);
    let declared = [outcome_decl("implemented", LedgerEffect::None)];
    let (result, _payloads) = run_stage(
        dir.path(),
        &cfg,
        &declared,
        "impl_1",
        "implemented",
        vec![".surge/memory/auth.md".into()],
        None,
    )
    .await;
    assert_eq!(result.unwrap().as_ref(), "implemented");

    let stamped = tokio::fs::read_to_string(mem_dir.join("auth.md"))
        .await
        .unwrap();
    assert!(
        stamped.starts_with("<!-- surge:memory run="),
        "note should be stamped with provenance, got:\n{stamped}"
    );
    assert!(stamped.contains("node=impl_1 -->"));
    assert!(stamped.contains("Tokens are always HS256."));

    // Idempotent: re-running the stage must not double-stamp.
    let (_r, _p) = run_stage(
        dir.path(),
        &cfg,
        &declared,
        "impl_1",
        "implemented",
        vec![".surge/memory/auth.md".into()],
        None,
    )
    .await;
    let twice = tokio::fs::read_to_string(mem_dir.join("auth.md"))
        .await
        .unwrap();
    assert_eq!(
        twice.matches("<!-- surge:memory").count(),
        1,
        "provenance marker must not be duplicated on re-run"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn steer_is_injected_into_prompt_and_recorded() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = agent_cfg(None, 3);
    let declared = [outcome_decl("implemented", LedgerEffect::None)];
    let steers = vec![
        surge_orchestrator::engine::steer::QueuedSteer {
            id: "s1".into(),
            message: "prefer axum over actix".into(),
        },
        surge_orchestrator::engine::steer::QueuedSteer {
            id: "s2".into(),
            message: "do not touch migrations".into(),
        },
    ];
    let ((result, payloads), mock) = run_stage_steered(
        dir.path(),
        &cfg,
        &declared,
        "impl_1",
        "implemented",
        vec![],
        None,
        steers,
        None,
    )
    .await;
    assert_eq!(result.unwrap().as_ref(), "implemented");

    // The steer text was prepended to the prompt the agent actually received.
    let prompt = mock.last_prompt().await.expect("a prompt was sent");
    assert!(prompt.contains("Operator steering"), "prompt:\n{prompt}");
    assert!(
        prompt.contains("prefer axum over actix"),
        "prompt:\n{prompt}"
    );
    assert!(
        prompt.contains("do not touch migrations"),
        "prompt:\n{prompt}"
    );

    // Both deliveries were recorded as SteerDelivered events (audit trail).
    let delivered: Vec<(String, String)> = payloads
        .iter()
        .filter_map(|p| match p {
            EventPayload::SteerDelivered { id, message, .. } => Some((id.clone(), message.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(delivered.len(), 2);
    assert_eq!(delivered[0].0, "s1");
    assert_eq!(delivered[1].0, "s2");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sealed_verifier_cannot_synthesize_missing_report_evidence() {
    let dir = tempfile::tempdir().unwrap();
    let declared = [outcome_decl("passed", LedgerEffect::Verified)];
    let (result, payloads) = run_stage(
        dir.path(),
        &agent_cfg(Some(SandboxMode::ReadOnly), 0),
        &declared,
        "verify_missing",
        "passed",
        Vec::new(),
        Some(RoadmapTaskId::from("m1-t1")),
    )
    .await;
    assert!(
        result.is_err(),
        "a sealed sandbox alone cannot certify without a report: {result:?}"
    );
    assert!(
        !payloads
            .iter()
            .any(|event| matches!(event, EventPayload::TaskVerified { .. }))
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn generic_bundled_verifier_accepts_unbound_inline_audit_without_task_proof() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = generic_verifier_config();
    let declared = [outcome_decl("passed", LedgerEffect::Verified)];
    let report:surge_core::roadmap::VerificationReportArtifact=toml::from_str("task_id='whole-run'\noutcome='passed'\nsummary='audit'\n[[checks]]\ncommand='acceptance'\nresult='passed'\n").unwrap();
    let ((result, payloads), _) = run_stage_steered(
        dir.path(),
        &cfg,
        &declared,
        "verify",
        "passed",
        vec![],
        None,
        vec![],
        Some(report),
    )
    .await;
    assert!(
        result.is_ok(),
        "valid generic audit should route without task proof: {result:?}"
    );
    assert!(
        !payloads
            .iter()
            .any(|payload| matches!(payload, EventPayload::TaskVerified { .. }))
    );
    assert!(!dir.path().join("verification-report.toml").exists());
}

fn generic_verifier_config() -> AgentConfig {
    let mut cfg = agent_cfg(Some(SandboxMode::ReadOnly), 0);
    cfg.profile = "verifier@2.0".parse().unwrap();
    cfg.bindings = vec![
        surge_core::agent_config::Binding {
            source: surge_core::agent_config::ArtifactSource::Static {
                content: "Acceptance works".into(),
            },
            target: surge_core::agent_config::TemplateVar("spec".into()),
            optional: false,
        },
        surge_core::agent_config::Binding {
            source: surge_core::agent_config::ArtifactSource::Static {
                content: String::new(),
            },
            target: surge_core::agent_config::TemplateVar("project_memory".into()),
            optional: true,
        },
    ];
    cfg
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn generic_audit_rejects_missing_skipped_cancelled_or_agent_binding() {
    use surge_core::roadmap::{VerificationCheckResult, VerificationReportArtifact};
    let base:VerificationReportArtifact=toml::from_str("task_id='whole-run'\noutcome='passed'\nsummary='audit'\n[[checks]]\ncommand='acceptance'\nresult='passed'\n").unwrap();
    let mut skipped = base.clone();
    skipped.checks[0].result = VerificationCheckResult::Skipped;
    let mut cancelled = base.clone();
    cancelled.checks[0].result = VerificationCheckResult::Cancelled;
    let mut forged = base;
    forged.binding = Some(surge_core::verification_evidence::VerificationBinding {
        subject: surge_core::verification_evidence::VerificationSubject {
            repository: "/repo/.git".into(),
            worktree: "/repo".into(),
            tree: "a".repeat(40),
            checkpoint: "b".repeat(40),
        },
        criteria: surge_core::verification_evidence::VerificationCriteria {
            hash: surge_core::ContentHash::compute(b"fake"),
            epoch: surge_core::id::StageGenerationId::new(),
            required: vec![],
            definitions: vec![],
        },
    });
    for report in [None, Some(skipped), Some(cancelled), Some(forged)] {
        let dir = tempfile::tempdir().unwrap();
        let cfg = generic_verifier_config();
        let declared = [outcome_decl("passed", LedgerEffect::Verified)];
        let ((result, payloads), _) = run_stage_steered(
            dir.path(),
            &cfg,
            &declared,
            "verify",
            "passed",
            vec![],
            None,
            vec![],
            report,
        )
        .await;
        assert!(result.is_err());
        assert!(
            !payloads
                .iter()
                .any(|payload| matches!(payload, EventPayload::TaskVerified { .. }))
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn generic_bundled_failed_audit_routes_without_task_proof() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = generic_verifier_config();
    let declared = [outcome_decl("failed", LedgerEffect::FailedVerification)];
    let report: surge_core::roadmap::VerificationReportArtifact = toml::from_str("task_id='whole-run'\noutcome='failed'\nsummary='check failed'\n[[checks]]\ncommand='acceptance'\nresult='failed'\n").unwrap();
    let ((result, payloads), _) = run_stage_steered(
        dir.path(),
        &cfg,
        &declared,
        "verify",
        "failed",
        vec![],
        None,
        vec![],
        Some(report.clone()),
    )
    .await;
    assert!(
        result.is_ok(),
        "valid generic failed audit must route: {result:?}"
    );
    assert!(
        !payloads
            .iter()
            .any(|payload| matches!(payload, EventPayload::TaskVerified { .. }))
    );
    let mut forged = report;
    forged.binding = Some(surge_core::verification_evidence::VerificationBinding {
        subject: surge_core::verification_evidence::VerificationSubject {
            repository: "/repo/.git".into(),
            worktree: "/repo".into(),
            tree: "a".repeat(40),
            checkpoint: "b".repeat(40),
        },
        criteria: surge_core::verification_evidence::VerificationCriteria {
            hash: surge_core::ContentHash::compute(b"fake"),
            epoch: surge_core::id::StageGenerationId::new(),
            required: vec![],
            definitions: vec![],
        },
    });
    let ((result, payloads), _) = run_stage_steered(
        dir.path(),
        &cfg,
        &declared,
        "verify",
        "failed",
        vec![],
        None,
        vec![],
        Some(forged),
    )
    .await;
    assert!(
        result.is_err(),
        "agent-supplied failed binding must be rejected"
    );
    assert!(
        !payloads
            .iter()
            .any(|payload| matches!(payload, EventPayload::TaskVerified { .. }))
    );
}
