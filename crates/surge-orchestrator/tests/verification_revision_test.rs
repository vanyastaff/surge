//! Real workspace oracle for current proof across operator, ledger and reports.
mod fixtures;
use std::{collections::BTreeMap, path::PathBuf, sync::Arc};
use surge_core::{
    ContentHash, Graph, NodeKey, RunId,
    roadmap::{
        VerificationCheck, VerificationCheckResult, VerificationReportArtifact,
        VerificationReportOutcome,
    },
    run_event::{EventPayload, RunConfig, VersionedEventPayload},
    verification_evidence::{
        ProofFreshness, VerificationBinding, VerificationCriteria, VerificationSubject,
    },
};
use surge_orchestrator::operator::{
    LedgerQuery, compile_report, inbox::collect_entries, query_ledger,
};
use surge_persistence::runs::{Storage, run_writer::RunWriter};

struct Fixture {
    _home: tempfile::TempDir,
    _worktree: tempfile::TempDir,
    path: PathBuf,
    storage: Arc<Storage>,
    writer: Arc<RunWriter>,
    run: RunId,
    binding: VerificationBinding,
    item: Option<surge_core::id::WorkItemId>,
}
async fn fixture() -> Fixture {
    fixture_with_error_hook(None).await
}
async fn fixture_with_error_hook(command: Option<&str>) -> Fixture {
    fixture_owned(command, false).await
}
async fn fixture_owned(command: Option<&str>, owned: bool) -> Fixture {
    let home = tempfile::tempdir().unwrap();
    let worktree = tempfile::tempdir().unwrap();
    let path = worktree.path().to_path_buf();
    std::fs::write(path.join("code.rs"), "fn answer() -> u8 { 42 }\n").unwrap();
    for args in [
        vec!["init"],
        vec!["config", "user.name", "Fixture"],
        vec!["config", "user.email", "fixture@example.com"],
        vec!["add", "."],
        vec!["commit", "-m", "implementation"],
    ] {
        assert!(
            std::process::Command::new("git")
                .args(args)
                .current_dir(&path)
                .output()
                .unwrap()
                .status
                .success()
        );
    }
    let storage = Storage::open(home.path()).await.unwrap();
    let mut run = RunId::new();
    let mut item = None;
    let verify = NodeKey::try_from("verify").unwrap();
    let node = surge_core::node::Node {
        id: verify.clone(),
        position: Default::default(),
        declared_outcomes: vec![surge_core::node::OutcomeDecl {
            id: "passed".parse().unwrap(),
            description: "verification".into(),
            edge_kind_hint: surge_core::edge::EdgeKind::Forward,
            is_terminal: false,
            ledger_effect: surge_core::node::LedgerEffect::Verified,
        }],
        config: surge_core::node::NodeConfig::Terminal(
            surge_core::terminal_config::TerminalConfig {
                kind: surge_core::terminal_config::TerminalKind::Success,
                message: None,
            },
        ),
    };
    let mut nodes = BTreeMap::from([(verify.clone(), node)]);
    let start = if let Some(command) = command {
        let id = NodeKey::try_from("later").unwrap();
        let mut config: surge_core::agent_config::AgentConfig =
            toml::from_str("profile = 'implementer@1.0'").unwrap();
        config.limits.max_retries = 0;
        config.hooks.push(surge_core::hooks::Hook {
            id: "error-mutation".into(),
            trigger: surge_core::hooks::HookTrigger::OnError,
            matcher: Default::default(),
            command: command.into(),
            on_failure: surge_core::hooks::HookFailureMode::Warn,
            timeout_seconds: Some(3),
            inherit: Default::default(),
        });
        nodes.insert(
            id.clone(),
            surge_core::node::Node {
                id: id.clone(),
                position: Default::default(),
                declared_outcomes: vec![],
                config: surge_core::node::NodeConfig::Agent(config),
            },
        );
        id
    } else {
        verify.clone()
    };
    let graph = Graph {
        schema_version: 1,
        metadata: surge_core::graph::GraphMetadata::new("verification", chrono::Utc::now()),
        start,
        nodes,
        edges: vec![],
        subgraphs: Default::default(),
    };
    if owned {
        use surge_core::work_item::*;
        let workspace = WorkItemWorkspace {
            repository: path.join(".git").canonicalize().unwrap(),
            checkout: path.clone(),
            path: path.clone(),
            ownership: RunId::new().to_string(),
            branch: "fixture".into(),
            base_commit: "a".repeat(40),
        };
        let create = WorkItemCommand::Create {
            operation_id: surge_core::id::WorkItemOperationId::new(),
            project: path.clone(),
            title: "Current requirements oracle".into(),
            requirements: WorkItemRequirements::new(
                "Original accepted task".into(),
                vec!["fixed accepted criterion".into()],
            )
            .unwrap(),
        };
        let WorkItemResult::Detail(created) = storage
            .work_items()
            .mutate(&create, Some(&workspace), None, "human", 1)
            .unwrap()
        else {
            panic!("created")
        };
        let start = WorkItemCommand::Start {
            operation_id: surge_core::id::WorkItemOperationId::new(),
            item: created.item.id,
            expected_version: created.item.version,
            graph: Box::new(graph.clone()),
            quota_recovery: None,
        };
        let WorkItemResult::Attempt(attempt) = storage
            .work_items()
            .mutate(&start, None, Some("{}"), "host", 2)
            .unwrap()
        else {
            panic!("reserved")
        };
        run = attempt.run;
        item = Some(created.item.id);
    }
    let writer = Arc::new(storage.create_run(run, &path, None).await.unwrap());
    let observed = surge_git::fingerprint::observe(&path).unwrap().unwrap();
    let checkpoint = surge_git::checkpoint::capture_record(&path, run, 1)
        .unwrap()
        .unwrap();
    let binding = VerificationBinding {
        subject: VerificationSubject {
            repository: observed.repository,
            worktree: observed.worktree,
            tree: observed.tree,
            checkpoint: checkpoint.commit,
        },
        criteria: VerificationCriteria {
            hash: surge_core::verification_evidence::VerificationCriteria::from_task(
                &accepted_task(),
                &[],
            )
            .unwrap()
            .hash,
            epoch: surge_core::id::StageGenerationId::new(),
            required: vec!["criterion:1".into()],
            definitions: vec![("criterion:1".into(), "fixed accepted criterion".into())],
        },
    };
    for event in [
        EventPayload::RunStarted {
            pipeline_template: None,
            project_path: path.clone(),
            initial_prompt: "verify".into(),
            config: RunConfig {
                bootstrap_edit_loop_cap: None,
                budget: Default::default(),
                sandbox_default: surge_core::sandbox::SandboxMode::ReadOnly,
                approval_default: surge_core::approvals::ApprovalPolicy::OnRequest,
                auto_pr: false,
                mcp_servers: vec![],
            },
        },
        EventPayload::PipelineMaterialized {
            graph_hash: ContentHash::compute(&serde_json::to_vec(&graph).unwrap()),
            graph: Box::new(graph),
        },
        EventPayload::VerificationSubjectObserved {
            subject: Some(binding.subject.clone()),
        },
        EventPayload::VerificationCriteriaAccepted {
            task_id: "t1".into(),
            criteria: Some(binding.criteria.clone()),
        },
    ] {
        writer
            .append_event(VersionedEventPayload::new(event))
            .await
            .unwrap();
    }
    let report = VerificationReportArtifact {
        schema_version: 1,
        task_id: "t1".into(),
        outcome: VerificationReportOutcome::Passed,
        summary: "fixed oracle check".into(),
        checks: vec![VerificationCheck {
            command: "acceptance test".into(),
            result: VerificationCheckResult::Passed,
            covers: vec!["criterion:1".into()],
            note: None,
        }],
        evidence: vec![],
        binding: Some(binding.clone()),
    };
    let artifact = surge_persistence::artifacts::ArtifactStore::new(home.path().join("runs"))
        .put(
            run,
            "verification-report",
            toml::to_string(&report).unwrap().as_bytes(),
        )
        .await
        .unwrap();
    writer
        .append_event(VersionedEventPayload::new(EventPayload::ArtifactProduced {
            node: verify.clone(),
            artifact: artifact.hash,
            path: artifact.path,
            name: "verification-report".into(),
            source_path: None,
        }))
        .await
        .unwrap();
    writer
        .append_event(VersionedEventPayload::new(EventPayload::TaskVerified {
            task_id: "t1".into(),
            node: verify.clone(),
            evidence: artifact.hash,
            report: Some(report),
        }))
        .await
        .unwrap();
    if command.is_none() {
        writer
            .append_event(VersionedEventPayload::new(EventPayload::RunCompleted {
                terminal_node: verify,
            }))
            .await
            .unwrap();
    }
    storage.sync_task_ledger_index(run, &path).await.unwrap();
    Fixture {
        _home: home,
        _worktree: worktree,
        path,
        storage,
        writer,
        run,
        binding,
        item,
    }
}
async fn assert_all(f: &Fixture, expected: ProofFreshness) {
    let rows = query_ledger(
        &f.storage,
        &LedgerQuery {
            run_id: Some(f.run),
            limit: 10,
        },
    )
    .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].freshness, expected);
    assert_eq!(
        rows[0].is_evidence_backed(),
        expected == ProofFreshness::Current
    );
    let report = compile_report(&f.storage, &f.run.to_string())
        .await
        .unwrap();
    assert_eq!(report.freshness, expected);
    assert_eq!(
        report.evidence_backed,
        Some(expected == ProofFreshness::Current)
    );
    let entries = collect_entries(&f.storage, None, 10).await.unwrap();
    assert_eq!(
        entries[0].evidence_backed,
        if expected == ProofFreshness::Unknown {
            None
        } else {
            Some(expected == ProofFreshness::Current)
        }
    );
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dirty_same_head_invalidates_current_proof_on_every_surface() {
    let f = fixture().await;
    assert_all(&f, ProofFreshness::Current).await;
    let head = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&f.path)
        .output()
        .unwrap()
        .stdout;
    std::fs::write(f.path.join("code.rs"), "fn answer() -> u8 { 0 }\n").unwrap();
    assert_eq!(
        std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&f.path)
            .output()
            .unwrap()
            .stdout,
        head
    );
    assert_all(&f, ProofFreshness::Stale).await;
    std::fs::remove_file(f.path.join(".git/HEAD")).unwrap();
    assert_all(&f, ProofFreshness::Unknown).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn durable_criteria_a_b_a_and_rebuild_never_revive_old_proof() {
    let f = fixture().await;
    assert_all(&f, ProofFreshness::Current).await;
    let mut changed = f.binding.criteria.clone();
    changed.hash = ContentHash::compute(b"changed accepted criterion");
    changed.epoch = surge_core::id::StageGenerationId::new();
    let mut returned = f.binding.criteria.clone();
    returned.epoch = surge_core::id::StageGenerationId::new();
    for criteria in [changed, returned] {
        f.writer
            .append_event(VersionedEventPayload::new(
                EventPayload::VerificationCriteriaAccepted {
                    task_id: "t1".into(),
                    criteria: Some(criteria),
                },
            ))
            .await
            .unwrap();
    }
    f.storage
        .sync_task_ledger_index(f.run, &f.path)
        .await
        .unwrap();
    assert_all(&f, ProofFreshness::Stale).await;
    let old = f
        .storage
        .inspect_verification_proofs(f.run)
        .unwrap()
        .remove(0);
    let report: VerificationReportArtifact =
        toml::from_str(&std::fs::read_to_string(&old.report_path).unwrap()).unwrap();
    f.writer
        .append_event(VersionedEventPayload::new(EventPayload::TaskVerified {
            task_id: "t1".into(),
            node: NodeKey::try_from("verify").unwrap(),
            evidence: old.evidence,
            report: Some(report),
        }))
        .await
        .unwrap();
    f.storage
        .sync_task_ledger_index(f.run, &f.path)
        .await
        .unwrap();
    assert_all(&f, ProofFreshness::Stale).await;
    f.writer.rebuild_views().await.unwrap();
    f.storage
        .sync_task_ledger_index(f.run, &f.path)
        .await
        .unwrap();
    assert_all(&f, ProofFreshness::Stale).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_or_corrupt_sealed_blob_is_unknown_and_rebuild_cannot_credit_it() {
    let f = fixture().await;
    assert_all(&f, ProofFreshness::Current).await;
    let proof = f
        .storage
        .inspect_verification_proofs(f.run)
        .unwrap()
        .remove(0);
    std::fs::write(&proof.report_path, b"tampered evidence").unwrap();
    assert_all(&f, ProofFreshness::Unknown).await;
    f.writer.rebuild_views().await.unwrap();
    f.storage
        .sync_task_ledger_index(f.run, &f.path)
        .await
        .unwrap();
    assert!(
        !query_ledger(
            &f.storage,
            &LedgerQuery {
                run_id: Some(f.run),
                limit: 10
            }
        )
        .unwrap()
        .iter()
        .any(|row| row.is_evidence_backed())
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fresh_task_cannot_hide_another_authorized_legacy_claim() {
    let f = fixture().await;
    f.writer
        .append_event(VersionedEventPayload::new(EventPayload::TaskVerified {
            task_id: "legacy".into(),
            node: NodeKey::try_from("verify").unwrap(),
            evidence: ContentHash::compute(b"legacy"),
            report: None,
        }))
        .await
        .unwrap();
    f.storage
        .sync_task_ledger_index(f.run, &f.path)
        .await
        .unwrap();
    let rows = query_ledger(
        &f.storage,
        &LedgerQuery {
            run_id: Some(f.run),
            limit: 10,
        },
    )
    .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows.iter().filter(|row| row.is_evidence_backed()).count(),
        1
    );
    assert_ne!(
        compile_report(&f.storage, &f.run.to_string())
            .await
            .unwrap()
            .evidence_backed,
        Some(true)
    );
    assert_ne!(
        collect_entries(&f.storage, None, 10).await.unwrap()[0].evidence_backed,
        Some(true)
    );
}

fn accepted_task() -> surge_core::roadmap::RoadmapTask {
    let mut task = surge_core::roadmap::RoadmapTask::new("t1", "implementation");
    task.acceptance_criteria = vec!["fixed accepted criterion".into()];
    task
}
struct NoTools;
#[async_trait::async_trait]
impl surge_orchestrator::engine::tools::ToolDispatcher for NoTools {
    async fn dispatch(
        &self,
        _: &surge_orchestrator::engine::tools::ToolDispatchContext<'_>,
        call: &surge_orchestrator::engine::tools::ToolCall,
    ) -> surge_orchestrator::engine::tools::ToolResultPayload {
        surge_orchestrator::engine::tools::ToolResultPayload::Unsupported {
            message: call.tool.clone(),
        }
    }
}
async fn followup_stage(
    f: &Fixture,
    verifying: bool,
    invalidations: Vec<EventPayload>,
) -> Result<surge_core::OutcomeKey, surge_orchestrator::engine::stage::StageError> {
    use surge_core::{
        agent_config::AgentConfig,
        keys::OutcomeKey,
        loop_config::{ExitCondition, IterableSource, LoopConfig},
        node::{LedgerEffect, OutcomeDecl},
    };
    use surge_orchestrator::engine::{
        frames::{Frame, LoopFrame},
        stage::agent::{AgentStageParams, execute_agent_stage},
    };
    let reader = f.storage.open_run_reader(f.run).await.unwrap();
    let events = reader
        .read_events(surge_persistence::runs::EventSeq(0)..surge_persistence::runs::EventSeq(100))
        .await
        .unwrap();
    let mut memory = surge_core::run_state::RunMemory::default();
    for event in events {
        memory.apply_event(&surge_core::RunEvent {
            run_id: f.run,
            seq: event.seq.as_u64(),
            timestamp: chrono::DateTime::from_timestamp_millis(event.timestamp_ms).unwrap(),
            payload: event.payload.payload().clone(),
        });
    }
    let task = toml::Value::try_from(accepted_task()).unwrap();
    let frames = vec![Frame::Loop(LoopFrame {
        loop_node: "loop".parse().unwrap(),
        config: LoopConfig {
            iterates_over: IterableSource::Static(vec![]),
            body: "body".parse().unwrap(),
            iteration_var_name: "task".into(),
            exit_condition: ExitCondition::AllItems,
            on_iteration_failure: Default::default(),
            parallelism: Default::default(),
            gate_after_each: false,
        },
        items: vec![task],
        current_index: 0,
        attempts_remaining: 0,
        return_to: "end".parse().unwrap(),
        traversal_counts: Default::default(),
    })];
    let mock = Arc::new(fixtures::mock_bridge::MockBridge::new());
    let bridge: Arc<dyn surge_acp::bridge::BridgeFacade> = mock.clone();
    let session = surge_core::id::SessionId::new();
    mock.pin_next_session_id(session).await;
    mock.enqueue_event(surge_acp::bridge::BridgeEvent::OutcomeReported {
        session,
        outcome: OutcomeKey::try_from("reviewed").unwrap(),
        summary: "reviewed without edits".into(),
        artifacts_produced: vec![],
        verification_report: verifying.then(|| {
            Box::new(VerificationReportArtifact {
                schema_version: 1,
                task_id: "t1".into(),
                outcome: VerificationReportOutcome::Passed,
                summary: "check".into(),
                checks: vec![VerificationCheck {
                    command: "acceptance".into(),
                    result: VerificationCheckResult::Passed,
                    covers: vec!["criterion:1".into()],
                    note: None,
                }],
                evidence: vec![],
                binding: None,
            })
        }),
    })
    .await;
    let pumping = mock.clone();
    let write_handle = f.writer.clone();
    let pump = tokio::spawn(async move {
        pumping.wait_for_subscribe_count(1).await;
        for event in invalidations {
            write_handle
                .append_event(VersionedEventPayload::new(event))
                .await
                .unwrap();
        }
        pumping.pump_after_subscribe(1).await;
    });
    let mut cfg: AgentConfig = toml::from_str("profile = 'reviewer@1.0'").unwrap();
    cfg.sandbox_override = Some(surge_core::sandbox::SandboxConfig {
        mode: surge_core::sandbox::SandboxMode::ReadOnly,
        ..Default::default()
    });
    cfg.limits.max_retries = 0;
    let node = NodeKey::try_from(if verifying { "verify" } else { "reviewer" }).unwrap();
    let outcomes = [OutcomeDecl {
        id: "reviewed".parse().unwrap(),
        description: "no edits".into(),
        edge_kind_hint: surge_core::edge::EdgeKind::Forward,
        is_terminal: false,
        ledger_effect: if verifying {
            LedgerEffect::Verified
        } else {
            LedgerEffect::None
        },
    }];
    let dispatcher: Arc<dyn surge_orchestrator::engine::tools::ToolDispatcher> = Arc::new(NoTools);
    let resolutions = Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
    let hooks = surge_orchestrator::engine::hooks::HookExecutor::new();
    let store = surge_persistence::artifacts::ArtifactStore::new(f._home.path().join("runs"));
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(8),
        execute_agent_stage(AgentStageParams {
            quota_opening: None,
            quota_cycle: None,
            quota_owner: None,
            continuation: None,
            frames: &frames,
            cancel: tokio_util::sync::CancellationToken::new(),
            steers: vec![],
            node: &node,
            attempt: 1,
            agent_config: &cfg,
            bound_skills: &[],
            declared_outcomes: &outcomes,
            bridge: &bridge,
            writer: &f.writer,
            artifact_store: &store,
            worktree_path: &f.path,
            tool_dispatcher: &dispatcher,
            run_memory: &memory,
            run_id: f.run,
            tool_resolutions: &resolutions,
            human_input_timeout: std::time::Duration::from_secs(5),
            mcp_registry: None,
            mcp_servers: vec![],
            tool_call_loop_guard: Default::default(),
            output_spill: Default::default(),
            profile_registry: None,
            agent_registry: None,
            hook_executor: &hooks,
            pending_elevations: surge_orchestrator::engine::elevation::PendingElevations::new(),
            active_task_id: Some("t1".into()),
        }),
    )
    .await
    .unwrap();
    pump.await.unwrap();
    result
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn same_task_no_edit_reviewer_preserves_current_proof() {
    let f = fixture().await;
    assert_all(&f, ProofFreshness::Current).await;
    followup_stage(&f, false, vec![]).await.unwrap();
    f.storage
        .sync_task_ledger_index(f.run, &f.path)
        .await
        .unwrap();
    assert_all(&f, ProofFreshness::Current).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn verifier_rejects_observed_invalidation_even_when_code_returns() {
    let f = fixture().await;
    let mut changed = f.binding.subject.clone();
    changed.tree = "c".repeat(40);
    let result = followup_stage(
        &f,
        true,
        vec![
            EventPayload::VerificationSubjectObserved {
                subject: Some(changed),
            },
            EventPayload::VerificationSubjectObserved {
                subject: Some(f.binding.subject.clone()),
            },
        ],
    )
    .await;
    assert!(
        result.is_err(),
        "observed invalidation during the attempt must reject the candidate"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn observed_code_return_and_task_transition_reject_late_old_proof() {
    for task_transition in [false, true] {
        let f = fixture().await;
        let proof = f
            .storage
            .inspect_verification_proofs(f.run)
            .unwrap()
            .remove(0);
        let report: VerificationReportArtifact =
            toml::from_str(&std::fs::read_to_string(&proof.report_path).unwrap()).unwrap();
        if task_transition {
            f.writer
                .append_event(VersionedEventPayload::new(
                    EventPayload::TaskStatusChanged {
                        task_id: "t1".into(),
                        from: surge_core::RoadmapStatus::Completed,
                        to: surge_core::RoadmapStatus::ReadyForVerification,
                        authority_node: NodeKey::try_from("verify").unwrap(),
                    },
                ))
                .await
                .unwrap();
        } else {
            let mut changed = f.binding.subject.clone();
            changed.tree = "c".repeat(40);
            for subject in [changed, f.binding.subject.clone()] {
                f.writer
                    .append_event(VersionedEventPayload::new(
                        EventPayload::VerificationSubjectObserved {
                            subject: Some(subject),
                        },
                    ))
                    .await
                    .unwrap();
            }
        }
        f.writer
            .append_event(VersionedEventPayload::new(EventPayload::TaskVerified {
                task_id: "t1".into(),
                node: NodeKey::try_from("verify").unwrap(),
                evidence: proof.evidence,
                report: Some(report),
            }))
            .await
            .unwrap();
        f.storage
            .sync_task_ledger_index(f.run, &f.path)
            .await
            .unwrap();
        assert_all(&f, ProofFreshness::Stale).await;
        f.writer.rebuild_views().await.unwrap();
        f.storage
            .sync_task_ledger_index(f.run, &f.path)
            .await
            .unwrap();
        assert_all(&f, ProofFreshness::Stale).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn on_error_shell_mutation_fences_old_proof_even_after_restore() {
    use surge_orchestrator::engine::{Engine, EngineConfig};
    for hook_failure in [false, true] {
        let command = if hook_failure {
            "printf 'changed by host hook' > code.rs\nexit 1"
        } else {
            "printf 'changed by host hook' > code.rs"
        };
        let f = fixture_with_error_hook(Some(command)).await;
        let mock = Arc::new(fixtures::mock_bridge::MockBridge::new());
        mock.fail_next_send_message(
            surge_acp::bridge::SendMessageError::AgentAuthenticationFailed {
                details: "fixture failure".into(),
            },
        )
        .await;
        let dispatcher: Arc<dyn surge_orchestrator::engine::tools::ToolDispatcher> =
            Arc::new(NoTools);
        Arc::try_unwrap(f.writer)
            .ok()
            .unwrap()
            .close()
            .await
            .unwrap();
        let engine = Engine::new(mock, f.storage.clone(), dispatcher, EngineConfig::default());
        let handle = engine.resume_run(f.run, f.path.clone()).await.unwrap();
        assert!(matches!(
            tokio::time::timeout(std::time::Duration::from_secs(8), handle.await_completion())
                .await
                .unwrap()
                .unwrap(),
            surge_orchestrator::engine::handle::RunOutcome::Failed { .. }
        ));
        assert_eq!(
            std::fs::read_to_string(f.path.join("code.rs")).unwrap(),
            "changed by host hook"
        );
        std::fs::write(f.path.join("code.rs"), "fn answer() -> u8 { 42 }\n").unwrap();
        f.storage
            .sync_task_ledger_index(f.run, &f.path)
            .await
            .unwrap();
        assert_eq!(
            query_ledger(
                &f.storage,
                &LedgerQuery {
                    run_id: Some(f.run),
                    limit: 10
                }
            )
            .unwrap()[0]
                .freshness,
            ProofFreshness::Stale
        );
        assert!(!f.storage.inspect_verification_proofs(f.run).unwrap()[0].verified);
        assert!(matches!(
            compile_report(&f.storage, &f.run.to_string())
                .await
                .unwrap()
                .verdicts[0]
                .result,
            surge_core::run_report::VerdictResult::Superseded
        ));
        let rebuild = f.storage.open_run_writer(f.run).await.unwrap();
        rebuild.rebuild_views().await.unwrap();
        rebuild.close().await.unwrap();
        f.storage
            .sync_task_ledger_index(f.run, &f.path)
            .await
            .unwrap();
        assert_eq!(
            query_ledger(
                &f.storage,
                &LedgerQuery {
                    run_id: Some(f.run),
                    limit: 10
                }
            )
            .unwrap()[0]
                .freshness,
            ProofFreshness::Stale
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn superseded_accepted_requirements_cannot_reuse_a_still_current_historical_run_proof() {
    use surge_core::work_item::*;
    let f = fixture_owned(None, true).await;
    assert_all(&f, ProofFreshness::Current).await;
    let store = f.storage.work_items();
    let item = f.item.unwrap();
    let attempt = store.for_run(f.run).unwrap().unwrap();
    store
        .settle(
            f.run,
            attempt.binding.generation,
            WorkItemAttemptState::Completed,
            None,
        )
        .unwrap();
    let current = store.show(item).unwrap();
    store
        .mutate(
            &WorkItemCommand::Edit {
                operation_id: surge_core::id::WorkItemOperationId::new(),
                item,
                expected_version: current.item.version,
                expected_revision: 1,
                requirements: WorkItemRequirements::new(
                    "Accepted expanded scope".into(),
                    vec!["New required behavior".into()],
                )
                .unwrap(),
            },
            None,
            None,
            "human",
            3,
        )
        .unwrap();
    // Historical code/criteria proof remains valid for the old run, but cannot
    // prove the newly accepted work item revision.
    assert_all(&f, ProofFreshness::Current).await;
    assert_eq!(
        store
            .for_run(f.run)
            .unwrap()
            .unwrap()
            .accepted_revision_relation,
        AcceptedRevisionRelation::Superseded
    );
    let WorkItemResult::Attempts(page) = store
        .query(&WorkItemCommand::Attempts {
            item,
            after: None,
            limit: 10,
        })
        .unwrap()
    else {
        panic!("attempts")
    };
    assert_eq!(
        page.entries[0].accepted_revision_relation,
        AcceptedRevisionRelation::Superseded
    );
}
