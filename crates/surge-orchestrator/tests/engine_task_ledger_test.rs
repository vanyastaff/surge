//! Phase 1 M3 — engine emits task-ledger events from agent stages, and the
//! sealed-verifier gate rejects a `Verified` outcome from a non-read-only
//! sandbox.
//!
//! These drive `execute_agent_stage` directly (the model used by
//! `engine_agent_artifact_emission_test`) so the ledger emission and authority
//! gate are exercised without the full loop machinery. The `active_task_id`
//! that a real run derives from the loop frame is supplied explicitly here.

mod fixtures;

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
    active_task_id: Option<String>,
) -> (Result<OutcomeKey, String>, Vec<EventPayload>) {
    let storage = Storage::open(dir).await.unwrap();
    let run_id = surge_core::id::RunId::new();
    let writer = storage.create_run(run_id, dir, None).await.unwrap();
    let artifact_store = surge_persistence::artifacts::ArtifactStore::new(dir.join("runs"));

    let mock = Arc::new(fixtures::mock_bridge::MockBridge::new());
    let bridge: Arc<dyn BridgeFacade> = mock.clone();
    let session_id = SessionId::new();
    mock.pin_next_session_id(session_id).await;
    mock.enqueue_event(BridgeEvent::OutcomeReported {
        session: session_id,
        outcome: OutcomeKey::from_str(outcome).unwrap(),
        summary: "done".into(),
        artifacts_produced,
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

    let result = execute_agent_stage(AgentStageParams {
        node: &node,
        agent_config: cfg,
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
        profile_registry: None,
        hook_executor: &hook_executor,
        pending_elevations: surge_orchestrator::engine::elevation::PendingElevations::new(),
        active_task_id,
    })
    .await
    .map_err(|e| e.to_string());
    let _ = pump.await;

    let reader = storage.open_run_reader(run_id).await.unwrap();
    let events = reader
        .read_events(EventSeq(0)..EventSeq(256))
        .await
        .unwrap();
    let payloads = events
        .iter()
        .map(|e| e.payload.payload().clone())
        .collect();
    (result, payloads)
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
    let dir = tempfile::tempdir().unwrap();
    tokio::fs::write(
        dir.path().join("verification-report.md"),
        b"# Verification report\nall checks pass\n",
    )
    .await
    .unwrap();
    let expected_evidence = surge_core::content_hash::ContentHash::compute(
        b"# Verification report\nall checks pass\n",
    );

    let cfg = agent_cfg(Some(SandboxMode::ReadOnly), 3); // sealed
    let declared = [outcome_decl("passed", LedgerEffect::Verified)];
    let (result, payloads) = run_stage(
        dir.path(),
        &cfg,
        &declared,
        "verify_1",
        "passed",
        vec!["verification-report.md".into()],
        Some("m1-t1".into()),
    )
    .await;

    assert_eq!(result.unwrap().as_ref(), "passed");
    let verified = payloads
        .iter()
        .find_map(|p| match p {
            EventPayload::TaskVerified {
                task_id,
                node,
                evidence,
            } => Some((task_id.clone(), node.clone(), *evidence)),
            _ => None,
        })
        .expect("TaskVerified emitted");
    assert_eq!(verified.0, "m1-t1");
    assert_eq!(verified.1.as_ref(), "verify_1");
    assert_eq!(
        verified.2, expected_evidence,
        "evidence is the verification-report artifact hash"
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
    let discovered: Vec<(String, String, String)> = payloads
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
