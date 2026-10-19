use super::*;
use crate::{ContentHash, EventPayload as E};

fn run() -> RunId {
    "run-00000000000000000000000001".parse().unwrap()
}
fn key(value: &str) -> NodeKey {
    value.parse().unwrap()
}
fn events(payloads: Vec<E>) -> Vec<RunEvent> {
    payloads
        .into_iter()
        .enumerate()
        .map(|(index, payload)| RunEvent {
            run_id: run(),
            seq: index as u64 + 1,
            timestamp: chrono::DateTime::UNIX_EPOCH,
            payload,
        })
        .collect()
}
fn iteration(loop_id: &str, id: &str, index: u32) -> E {
    E::LoopIterationStarted {
        loop_id: key(loop_id),
        item: toml::from_str(&format!("id='{id}'\ntitle='{id} title'")).unwrap(),
        index,
    }
}
fn enter(attempt: u32) -> E {
    E::StageEntered {
        node: key("implement"),
        attempt,
    }
}
fn materialized() -> E {
    let mut graph: crate::Graph =
        toml::from_str(include_str!("../../../../examples/flow_minimal_agent.toml")).unwrap();
    let original = graph.start.clone();
    let mut node = graph.nodes.remove(&original).unwrap();
    node.id = key("implement");
    node.declared_outcomes[0].ledger_effect = crate::node::LedgerEffect::Verified;
    graph.nodes.insert(key("implement"), node);
    for edge in &mut graph.edges {
        if edge.from.node == original {
            edge.from.node = key("implement");
        }
    }
    graph.start = key("implement");
    E::PipelineMaterialized {
        graph: Box::new(graph),
        graph_hash: ContentHash::compute(b"test graph"),
    }
}
#[test]
fn split_discoveries_keep_identity_and_literal_sibling_order_after_dispatch() {
    let mut log = events(vec![
        iteration("tasks", "login", 0),
        enter(1),
        E::TaskDiscovered {
            task_id: "login-a".into(),
            discovered_from: "login".into(),
            title: "A".into(),
        },
        E::TaskDiscovered {
            task_id: "login-b".into(),
            discovered_from: "login".into(),
            title: "B".into(),
        },
    ]);
    let before = TaskExecutionView::compile(run(), &log).unwrap();
    assert_eq!(
        before
            .tasks
            .iter()
            .map(|task| task.task_id.as_str())
            .collect::<Vec<_>>(),
        ["login", "login-a", "login-b"]
    );
    assert!(before.tasks[1].started.is_none());
    let a = before.tasks[1].id;
    let more = vec![E::TaskSplit { loop_id: key("tasks"), index: 0, task: Some("login".into()), into: vec![toml::from_str("id='login-a'\ntitle='A'\ndescription='a description'\nacceptance_criteria=['criterion a']").unwrap(), toml::from_str("id='login-b'\ntitle='B'\nacceptance_criteria=['criterion b']").unwrap()] }, E::LoopIterationCompleted { loop_id: key("tasks"), index: 0, outcome: "completed".parse().unwrap() }, iteration("tasks", "login-a", 1), enter(1), iteration("tasks", "login-b", 2), enter(1), iteration("tasks", "logout", 3), enter(1), E::RunCompleted { terminal_node: key("end") }];
    for payload in more {
        log.push(RunEvent {
            run_id: run(),
            seq: log.len() as u64 + 1,
            timestamp: chrono::DateTime::UNIX_EPOCH,
            payload,
        });
    }
    let view = TaskExecutionView::compile(run(), &log).unwrap();
    assert_eq!(
        view.tasks
            .iter()
            .map(|task| task.task_id.as_str())
            .collect::<Vec<_>>(),
        ["login", "login-a", "login-b", "logout"]
    );
    assert_eq!(view.tasks[1].id, a);
    assert_eq!(view.tasks[1].description.as_deref(), Some("a description"));
    assert_eq!(view.tasks[1].acceptance_criteria, ["criterion a"]);
    assert_eq!(view.tasks[1].split_origin, Some(view.tasks[0].id));
    assert!(view.tasks[1].parent.is_none());
    assert!(view.tasks[0].replaced);
    assert!(view.tasks.iter().all(|task| !task.verified));
    assert_eq!(
        view.stages
            .iter()
            .map(|stage| stage.entered)
            .collect::<Vec<_>>(),
        [2, 8, 10, 12]
    );
}
#[test]
fn orphan_discovery_remains_visible_and_undispatched() {
    let log = events(vec![E::TaskDiscovered {
        task_id: "child".into(),
        discovered_from: "missing".into(),
        title: "Retained child".into(),
    }]);
    let view = TaskExecutionView::compile(run(), &log).unwrap();
    assert_eq!(view.tasks.len(), 1);
    assert_eq!(view.tasks[0].title, "Retained child");
    assert_eq!(view.tasks[0].scope, TaskScope::Unassigned);
    assert!(view.tasks[0].started.is_none());
    assert!(matches!(
        view.caveats[0],
        ExecutionCaveat::Unassigned { seq: 1, .. }
    ));
}
#[test]
fn envelope_errors_and_torn_prefix_are_explicit() {
    let mut log = events(vec![enter(1), enter(1)]);
    log[1].seq = 1;
    assert!(matches!(
        TaskExecutionView::compile(run(), &log),
        Err(TaskExecutionError::Sequence { .. })
    ));
    log[1].seq = 2;
    log[1].run_id = RunId::new();
    assert!(matches!(
        TaskExecutionView::compile(run(), &log),
        Err(TaskExecutionError::ForeignRun { .. })
    ));
    let mut prefix = events(vec![enter(1)]);
    prefix[0].seq = 7;
    let view = TaskExecutionView::compile(run(), &prefix).unwrap();
    assert_eq!(view.stages[0].entered, 7);
    assert!(matches!(
        view.caveats[0],
        ExecutionCaveat::PartialJournal {
            after: 0,
            before: 7
        }
    ));
    prefix[0].seq = 0;
    assert!(TaskExecutionView::compile(run(), &prefix).is_err());
}
#[test]
fn reused_node_attempts_and_task_ids_have_distinct_occurrences() {
    let log = events(vec![
        iteration("tasks", "same", 0),
        enter(1),
        E::StageCompleted {
            node: key("implement"),
            outcome: "done".parse().unwrap(),
        },
        iteration("tasks", "same", 1),
        E::StageInputsResolved {
            node: key("implement"),
            attempt: 1,
            bindings: Default::default(),
            memory_receipt: None,
        },
        enter(1),
        E::StageInputsResolved {
            node: key("implement"),
            attempt: 2,
            bindings: Default::default(),
            memory_receipt: None,
        },
    ]);
    let view = TaskExecutionView::compile(run(), &log).unwrap();
    assert_eq!(view.tasks.len(), 2);
    assert_ne!(view.tasks[0].id, view.tasks[1].id);
    assert_ne!(view.stages[0].entered, view.stages[1].entered);
    assert_eq!(view.stages[0].attempt, view.stages[1].attempt);
    assert_eq!(view.stages[1].task, Some(view.tasks[1].id));
    assert!(view.stages.iter().all(|stage| stage.inputs.is_none()));
    assert!(
        view.caveats
            .iter()
            .any(|caveat| matches!(caveat, ExecutionCaveat::Unassigned { seq: 5, .. }))
    );
}
#[test]
fn bound_verification_is_retained_then_invalidated_without_terminal_promotion() {
    let raw = events(vec![
        materialized(),
        iteration("tasks", "login", 0),
        enter(1),
        E::TaskVerified {
            task_id: "login".into(),
            node: key("implement"),
            evidence: ContentHash::compute(b"placeholder"),
            report: None,
        },
        E::RunCompleted {
            terminal_node: key("end"),
        },
    ]);
    let mut log = crate::verification_evidence::bind_fixture(&raw);
    let view = TaskExecutionView::compile(run(), &log).unwrap();
    assert!(view.tasks[0].verified);
    assert_eq!(view.tasks[0].status, RoadmapStatus::Completed);
    log.push(RunEvent {
        run_id: run(),
        seq: log.len() as u64 + 1,
        timestamp: chrono::DateTime::UNIX_EPOCH,
        payload: E::VerificationSubjectObserved { subject: None },
    });
    let invalidated = TaskExecutionView::compile(run(), &log).unwrap();
    assert!(!invalidated.tasks[0].verified);
    assert!(invalidated.terminal.is_some());
    let unbound = TaskExecutionView::compile(run(), &raw).unwrap();
    assert!(!unbound.tasks[0].verified);
}

fn connection(
    session: SessionId,
    invocation: StageInvocationId,
    writer: crate::id::ExecutionWriterId,
) -> crate::execution_recovery::OpenedSession {
    use crate::execution_recovery::*;
    let descriptor = ProviderSessionDescriptor::new(
        ProviderSessionId::new(format!("provider-{session}")).unwrap(),
        invocation,
        "runtime".into(),
        ContentHash::compute(b"launch"),
        std::env::temp_dir(),
        SessionRestoreCapabilities {
            resume: true,
            load: false,
        },
        None,
    )
    .unwrap();
    let mut opened = OpenedSession::new(session, descriptor, SessionOpenMode::New).unwrap();
    opened.execution_writer = Some(process::ExecutionWriterObservation::new(writer, None).unwrap());
    opened
}
fn context(session: SessionId) -> crate::stage_tool::StageToolContext {
    crate::stage_tool::StageToolContext {
        run: run(),
        node: key("implement"),
        session,
        generation: crate::id::StageGenerationId::new(),
    }
}
fn accepted(
    context: crate::stage_tool::StageToolContext,
    session: SessionId,
    invocation: StageInvocationId,
) -> crate::execution_recovery::commit::StageOutcomeCommit {
    crate::execution_recovery::commit::StageOutcomeCommit::new(
        context,
        session,
        invocation,
        "done".parse().unwrap(),
        1,
        ContentHash::compute(b"accepted effect"),
    )
    .unwrap()
}
#[test]
fn reconnect_sessions_acceptance_and_exact_routes_retain_distinct_identities() {
    let first = SessionId::new();
    let second = SessionId::new();
    let invocation = StageInvocationId::new();
    let quota = StageInvocationId::new();
    let writer = crate::id::ExecutionWriterId::new();
    let one = context(first);
    let two = context(second);
    let mut log = events(vec![
        iteration("tasks", "login", 0),
        enter(1),
        E::QuotaStagePlanned {
            node: key("implement"),
            attempt: 1,
            stage_entry_seq: 2,
            logical_invocation: quota,
            control_generation: 1,
            policy_hash: ContentHash::compute(b"policy"),
        },
        E::SessionEstablishmentRequested {
            node: key("implement"),
            invocation,
            restore: false,
            authority: Some(one.clone()),
        },
        E::SessionOpened {
            handoff: None,
            opened: Some(connection(
                first,
                invocation,
                crate::id::ExecutionWriterId::new(),
            )),
            node: key("implement"),
            session: first,
            agent: "worker@1".into(),
            agent_id: Some("runtime".into()),
        },
        E::SessionClosed {
            session: first,
            disposition: crate::run_event::SessionDisposition::Normal,
        },
        E::SessionEstablishmentRequested {
            node: key("implement"),
            invocation,
            restore: true,
            authority: Some(two.clone()),
        },
        E::SessionOpened {
            handoff: None,
            opened: Some(connection(second, invocation, writer)),
            node: key("implement"),
            session: second,
            agent: "worker@1".into(),
            agent_id: Some("runtime".into()),
        },
        E::StageOutcomeCommitted {
            commit: accepted(two.clone(), second, invocation),
        },
        E::ExecutionWriterGroupStopped { writer },
        E::StageCompleted {
            node: key("implement"),
            outcome: "done".parse().unwrap(),
        },
        E::StageRouteCommitted {
            invocation,
            outcome_commit_seq: 9,
        },
        E::StageRouteCommitted {
            invocation,
            outcome_commit_seq: 9,
        },
        E::RunCompleted {
            terminal_node: key("end"),
        },
    ]);
    let view = TaskExecutionView::compile(run(), &log).unwrap();
    assert_eq!(view.stages.len(), 1);
    let stage = &view.stages[0];
    assert_eq!(stage.quota_invocation, Some(quota));
    assert_eq!(
        stage
            .sessions
            .iter()
            .map(|session| session.connection)
            .collect::<Vec<_>>(),
        [first, second]
    );
    assert_eq!(stage.sessions[0].authority.as_ref(), Some(&one));
    assert_eq!(stage.sessions[1].authority.as_ref(), Some(&two));
    assert_eq!(stage.sessions[1].invocation, Some(invocation));
    assert!(stage.sessions[0].closed.is_some());
    assert!(stage.sessions[0].writer_closed.is_none());
    assert_eq!(stage.sessions[1].group_stopped, Some(10));
    assert!(stage.sessions[1].writer_closed.is_none());
    assert_eq!(
        TaskExecutionView::compile(run(), &log[..12])
            .unwrap()
            .stages[0]
            .accepted[0]
            .routed,
        Some(12)
    );
    assert!(stage.accepted[0].routed.is_none());
    assert!(stage.accepted[0].conflicting);
    assert_eq!(stage.accepted[0].route_records, [12, 13]);
    assert!(
        view.caveats
            .contains(&ExecutionCaveat::ConflictingRoute { seq: 13 })
    );
    assert!(!view.tasks[0].verified);
    log.push(RunEvent {
        run_id: run(),
        seq: 15,
        timestamp: chrono::DateTime::UNIX_EPOCH,
        payload: E::ExecutionWriterClosed { writer },
    });
    assert_eq!(
        TaskExecutionView::compile(run(), &log).unwrap().stages[0].sessions[1].writer_closed,
        Some(15)
    );
}
#[test]
fn conflicting_provider_metadata_cannot_bind_an_acceptance() {
    let session = SessionId::new();
    let invocation = StageInvocationId::new();
    let authority = context(session);
    let log = events(vec![
        iteration("tasks", "login", 0),
        enter(1),
        E::SessionEstablishmentRequested {
            node: key("implement"),
            invocation,
            restore: false,
            authority: Some(authority.clone()),
        },
        E::SessionOpened {
            handoff: None,
            opened: Some(connection(
                SessionId::new(),
                StageInvocationId::new(),
                crate::id::ExecutionWriterId::new(),
            )),
            node: key("implement"),
            session,
            agent: "worker".into(),
            agent_id: None,
        },
        E::StageOutcomeCommitted {
            commit: accepted(authority, session, invocation),
        },
        E::StageRouteCommitted {
            invocation,
            outcome_commit_seq: 5,
        },
    ]);
    let view = TaskExecutionView::compile(run(), &log).unwrap();
    assert_eq!(view.stages[0].sessions.len(), 1);
    assert!(view.stages[0].accepted.is_empty());
    assert!(
        view.caveats
            .contains(&ExecutionCaveat::ConflictingRoute { seq: 6 })
    );
}

#[test]
fn subtask_classifier_requires_exact_path_segments_and_supports_loop_items() {
    use crate::loop_config::*;
    let mut graph: crate::Graph =
        toml::from_str(include_str!("../../../../examples/flow_minimal_agent.toml")).unwrap();
    let node = graph.nodes.get_mut(&graph.start).unwrap();
    node.config = crate::NodeConfig::Loop(LoopConfig {
        iterates_over: IterableSource::LoopItem {
            var: "task".into(),
            jsonpath: "spec.subtasks".into(),
        },
        body: "body".parse().unwrap(),
        iteration_var_name: "item".into(),
        exit_condition: ExitCondition::AllItems,
        on_iteration_failure: FailurePolicy::Abort,
        parallelism: ParallelismMode::Sequential,
        gate_after_each: false,
    });
    let loop_id = graph.start.clone();
    let mut projection = super::compile::Projection::new(run());
    for (path, expected) in [
        ("spec.retry_subtasks", false),
        ("spec.subtasks_count", false),
        ("spec.subtasks", true),
        ("$.subtasks[*]", true),
    ] {
        let crate::NodeConfig::Loop(config) = &mut graph.nodes.get_mut(&loop_id).unwrap().config
        else {
            panic!("loop fixture")
        };
        config.iterates_over = IterableSource::LoopItem {
            var: "task".into(),
            jsonpath: path.into(),
        };
        projection.memory.verification_graph = Some(Box::new(graph.clone()));
        assert_eq!(projection.is_subtask(&loop_id), expected, "{path}");
    }
}

#[test]
fn orphan_from_another_origin_is_not_rebound_by_child_id_alone() {
    let log = events(vec![
        E::TaskDiscovered {
            task_id: "child".into(),
            discovered_from: "origin-a".into(),
            title: "Orphan A".into(),
        },
        iteration("tasks", "origin-b", 0),
        enter(1),
        E::TaskSplit {
            loop_id: key("tasks"),
            index: 0,
            task: Some("origin-b".into()),
            into: vec![toml::from_str("id='child'\ntitle='Child B'").unwrap()],
        },
    ]);
    let view = TaskExecutionView::compile(run(), &log).unwrap();
    let children = view
        .tasks
        .iter()
        .filter(|task| task.task_id.as_str() == "child")
        .collect::<Vec<_>>();
    assert_eq!(children.len(), 2);
    assert_eq!(children[0].id.seq, 1);
    assert_eq!(
        children[0].discovered_from.as_ref().map(|id| id.as_str()),
        Some("origin-a")
    );
    assert_eq!(children[0].scope, TaskScope::Unassigned);
    assert_eq!(children[1].id.seq, 4);
    assert_eq!(
        children[1].discovered_from.as_ref().map(|id| id.as_str()),
        Some("origin-b")
    );
}
#[test]
fn ambiguous_repeated_origin_stays_unknown_until_exact_split_evidence() {
    let mut log = events(vec![
        iteration("tasks", "parent", 0),
        enter(1),
        iteration("tasks", "parent", 1),
        enter(1),
        E::TaskDiscovered {
            task_id: "child".into(),
            discovered_from: "parent".into(),
            title: "Child".into(),
        },
    ]);
    let before = TaskExecutionView::compile(run(), &log).unwrap();
    assert_eq!(before.tasks[2].scope, TaskScope::Unassigned);
    let anchor = before.tasks[2].id;
    log.push(RunEvent {
        run_id: run(),
        seq: 6,
        timestamp: chrono::DateTime::UNIX_EPOCH,
        payload: E::TaskSplit {
            loop_id: key("tasks"),
            index: 1,
            task: Some("parent".into()),
            into: vec![toml::from_str("id='child'\ntitle='Child'").unwrap()],
        },
    });
    let after = TaskExecutionView::compile(run(), &log).unwrap();
    assert_eq!(after.tasks[2].id, anchor);
    assert_eq!(after.tasks[2].split_origin, Some(after.tasks[1].id));
    assert!(after.tasks[2].scope.recorded().is_some());
}
#[test]
fn report_contract_preserves_order_escapes_authored_text_and_reports_unavailability() {
    let log = events(vec![iteration("tasks", "login", 0), enter(1), E::SessionOpened { handoff: None, opened: None, node: key("implement"), session: SessionId::new(), agent: "<i>worker</i>".into(), agent_id: Some("<script>runtime</script>".into()) }, E::TaskSplit { loop_id: key("tasks"), index: 0, task: Some("login".into()), into: vec![toml::from_str("id='login-a'\ntitle='<script>child|a</script>'\ndescription='<img src=x>'\nacceptance_criteria=['<b>criterion</b>']").unwrap(), toml::from_str("id='login-b'\ntitle='Child B'").unwrap()] }, iteration("tasks", "logout", 3), enter(1)]);
    let report = crate::RunReport::compile(run(), &log);
    let shared = TaskExecutionView::compile(run(), &log).unwrap();
    assert_eq!(report.task_execution.as_ref(), Some(&shared));
    let json = crate::run_report::render_json(&report).unwrap();
    let decoded: crate::RunReport = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded.task_execution, report.task_execution);
    let markdown = crate::run_report::render_markdown(&report);
    let html = crate::run_report::render_html(&report);
    for rendered in [&markdown, &html] {
        assert!(rendered.find("login-a").unwrap() < rendered.find("login-b").unwrap());
        assert!(rendered.find("login-b").unwrap() < rendered.find("logout").unwrap());
        assert!(!rendered.contains("<script>"));
        assert!(!rendered.contains("<img src=x>"));
        assert!(rendered.contains("criterion"));
        assert!(rendered.contains("worker"));
        assert!(rendered.contains("runtime"));
        assert!(!rendered.contains("<i>worker</i>"));
    }
    let mut invalid = log.clone();
    invalid[1].seq = invalid[0].seq;
    let unavailable = crate::RunReport::compile(run(), &invalid);
    assert!(unavailable.task_execution.is_none());
    assert!(
        unavailable
            .caveats
            .iter()
            .any(|caveat| caveat.contains("Task execution history unavailable"))
    );
    assert!(crate::run_report::render_markdown(&unavailable).contains("Unavailable; see caveats"));
}

#[test]
fn unauthorized_claim_on_reused_id_never_copies_another_occurrences_proof() {
    let raw = events(vec![
        materialized(),
        iteration("tasks", "same", 0),
        enter(1),
        E::TaskVerified {
            task_id: "same".into(),
            node: key("implement"),
            evidence: ContentHash::compute(b"placeholder"),
            report: None,
        },
    ]);
    let mut log = crate::verification_evidence::bind_fixture(&raw);
    for payload in [
        iteration("tasks", "same", 1),
        E::StageEntered {
            node: key("untrusted"),
            attempt: 1,
        },
        E::TaskVerified {
            task_id: "same".into(),
            node: key("untrusted"),
            evidence: ContentHash::compute(b"unauthorized"),
            report: None,
        },
    ] {
        log.push(RunEvent {
            run_id: run(),
            seq: log.len() as u64 + 1,
            timestamp: chrono::DateTime::UNIX_EPOCH,
            payload,
        });
    }
    let view = TaskExecutionView::compile(run(), &log).unwrap();
    assert_eq!(view.tasks.len(), 2);
    assert!(view.tasks[0].verified);
    assert!(!view.tasks[1].verified);
    assert_eq!(view.tasks[1].status, RoadmapStatus::Pending);
}

#[test]
fn status_only_completion_on_reused_id_cannot_inherit_verification() {
    let raw = events(vec![
        materialized(),
        iteration("tasks", "same", 0),
        enter(1),
        E::TaskVerified {
            task_id: "same".into(),
            node: key("implement"),
            evidence: ContentHash::compute(b"placeholder"),
            report: None,
        },
    ]);
    let mut log = crate::verification_evidence::bind_fixture(&raw);
    for payload in [
        iteration("tasks", "same", 1),
        enter(1),
        E::TaskStatusChanged {
            task_id: "same".into(),
            from: RoadmapStatus::Pending,
            to: RoadmapStatus::Completed,
            authority_node: key("implement"),
        },
    ] {
        log.push(RunEvent {
            run_id: run(),
            seq: log.len() as u64 + 1,
            timestamp: chrono::DateTime::UNIX_EPOCH,
            payload,
        });
    }
    let view = TaskExecutionView::compile(run(), &log).unwrap();
    assert_eq!(view.tasks[1].status, RoadmapStatus::Completed);
    assert!(!view.tasks[1].verified);
}

#[test]
fn gap_quarantines_active_task_stage_and_verification_certainty() {
    let raw = events(vec![
        materialized(),
        iteration("tasks", "login", 0),
        enter(1),
        E::TaskDiscovered {
            task_id: "login-a".into(),
            discovered_from: "login".into(),
            title: "A".into(),
        },
        E::TaskVerified {
            task_id: "login".into(),
            node: key("implement"),
            evidence: ContentHash::compute(b"placeholder"),
            report: None,
        },
    ]);
    let mut log = crate::verification_evidence::bind_fixture(&raw);
    let first = TaskExecutionView::compile(run(), &log).unwrap();
    assert!(first.tasks[0].verified);
    let next = log.len() as u64 + 2;
    log.push(RunEvent {
        run_id: run(),
        seq: next,
        timestamp: chrono::DateTime::UNIX_EPOCH,
        payload: E::StageCompleted {
            node: key("implement"),
            outcome: "done".parse().unwrap(),
        },
    });
    log.push(RunEvent {
        run_id: run(),
        seq: next + 1,
        timestamp: chrono::DateTime::UNIX_EPOCH,
        payload: enter(1),
    });
    let view = TaskExecutionView::compile(run(), &log).unwrap();
    assert!(view.stages[0].continuity_unknown.is_some());
    assert!(view.tasks[0].continuity_unknown.is_some());
    assert!(view.tasks[1].continuity_unknown.is_some());
    assert!(view.tasks[1].started.is_none());
    let report = crate::run_report::RunReport::compile(run(), &log);
    for rendered in [
        crate::run_report::render_markdown(&report),
        crate::run_report::render_html(&report),
    ] {
        assert!(rendered.contains("continuation unknown after a journal gap"));
        assert!(!rendered.contains("discovered; not dispatched"));
    }
    assert!(
        view.stages[0].result.is_none(),
        "a missing event may have changed the active occurrence"
    );
    assert!(
        view.stages[1].task.is_none(),
        "cached task scope is not evidence after a gap"
    );
    assert!(
        !view.tasks[0].verified,
        "cached criteria/subject/graph may have changed in the gap"
    );
}
#[test]
fn mismatched_iteration_completion_index_never_pops_the_active_occurrence() {
    let log = events(vec![
        iteration("tasks", "login", 0),
        E::LoopIterationCompleted {
            loop_id: key("tasks"),
            index: 1,
            outcome: "completed".parse().unwrap(),
        },
        enter(1),
    ]);
    let view = TaskExecutionView::compile(run(), &log).unwrap();
    assert!(view.tasks[0].finished.is_none());
    assert!(view.iterations[0].completed.is_none());
    assert_eq!(view.stages[0].task, Some(view.tasks[0].id));
    assert!(
        view.caveats
            .iter()
            .any(|caveat| matches!(caveat, ExecutionCaveat::Unassigned { seq: 2, .. }))
    );
}
#[test]
fn conflicting_acceptances_of_one_invocation_revoke_route_certainty() {
    let session = SessionId::new();
    let invocation = StageInvocationId::new();
    let authority = context(session);
    let commit = accepted(authority.clone(), session, invocation);
    let log = events(vec![
        iteration("tasks", "login", 0),
        enter(1),
        E::SessionEstablishmentRequested {
            node: key("implement"),
            invocation,
            restore: false,
            authority: Some(authority),
        },
        E::SessionOpened {
            handoff: None,
            opened: Some(connection(
                session,
                invocation,
                crate::id::ExecutionWriterId::new(),
            )),
            node: key("implement"),
            session,
            agent: "worker".into(),
            agent_id: Some("runtime".into()),
        },
        E::StageOutcomeCommitted {
            commit: commit.clone(),
        },
        E::StageRouteCommitted {
            invocation,
            outcome_commit_seq: 5,
        },
        E::StageOutcomeCommitted { commit },
        E::StageRouteCommitted {
            invocation,
            outcome_commit_seq: 7,
        },
    ]);
    let view = TaskExecutionView::compile(run(), &log).unwrap();
    assert_eq!(
        view.stages[0].accepted.len(),
        2,
        "retain both contradictory recorded markers"
    );
    assert!(
        view.stages[0]
            .accepted
            .iter()
            .all(|accepted| accepted.routed.is_none()),
        "conflicting acceptance cannot prove any unqualified route for that invocation"
    );
    assert!(
        view.caveats
            .iter()
            .any(|caveat| matches!(caveat, ExecutionCaveat::ConflictingRoute { .. }))
    );
}

#[test]
fn explicit_new_anchors_and_bound_evidence_can_restore_proof_after_a_gap() {
    let raw = events(vec![
        materialized(),
        iteration("tasks", "login", 0),
        enter(1),
        E::TaskVerified {
            task_id: "login".into(),
            node: key("implement"),
            evidence: ContentHash::compute(b"placeholder"),
            report: None,
        },
    ]);
    let mut log = crate::verification_evidence::bind_fixture(&raw);
    let fresh = crate::verification_evidence::bind_fixture(&raw);
    let first_new = log.len() as u64 + 2;
    for (offset, mut event) in fresh.into_iter().enumerate() {
        event.seq = first_new + offset as u64;
        log.push(event);
    }
    let view = TaskExecutionView::compile(run(), &log).unwrap();
    assert_eq!(view.tasks.len(), 2);
    assert!(!view.tasks[0].verified);
    assert!(view.tasks[1].verified);
    assert!(matches!(view.tasks[1].scope, TaskScope::Partial { .. }));
    assert_eq!(view.stages[1].task, Some(view.tasks[1].id));
}

#[test]
fn gap_preserves_recorded_completions() {
    let mut log = events(vec![
        iteration("tasks", "login", 0),
        enter(1),
        E::StageCompleted {
            node: key("implement"),
            outcome: "done".parse().unwrap(),
        },
        E::LoopIterationCompleted {
            loop_id: key("tasks"),
            index: 0,
            outcome: "completed".parse().unwrap(),
        },
        enter(1),
    ]);
    log[4].seq = 6;
    let view = TaskExecutionView::compile(run(), &log).unwrap();
    assert!(view.tasks[0].finished.is_some());
    assert_eq!(view.tasks[0].continuity_unknown, None);
    assert!(view.stages[0].result.is_some());
    assert_eq!(view.stages[0].continuity_unknown, None);
}

#[test]
fn recorded_recovery_cold_projection_preserves_tasks_stages_and_nonterminal_result() {
    let start = E::RunStarted {
        pipeline_template: None,
        project_path: "/project".into(),
        initial_prompt: "build".into(),
        config: crate::run_event::RunConfig {
            bootstrap_edit_loop_cap: None,
            sandbox_default: crate::sandbox::SandboxMode::WorkspaceWrite,
            approval_default: crate::approvals::ApprovalPolicy::OnRequest,
            auto_pr: false,
            mcp_servers: vec![],
            budget: Default::default(),
        },
    };
    let log = events(vec![
        start,
        iteration("tasks", "login", 0),
        enter(1),
        E::RunRecoveryRequired {
            control_generation: 0,
            diagnostic: "operator inspection".into(),
        },
    ]);
    let view = TaskExecutionView::compile(run(), &log).unwrap();
    assert_eq!(view.tasks.len(), 1);
    assert_eq!(view.stages.len(), 1);
    assert!(view.tasks[0].finished.is_none());
    assert!(!view.tasks[0].verified);
    assert!(view.stages[0].result.is_none());
    assert!(view.stages[0].failure.is_none());
    assert!(view.terminal.is_none());
    assert_eq!(view.recovery.active().unwrap().seq, 4);
    let cold_log: Vec<RunEvent> =
        serde_json::from_slice(&serde_json::to_vec(&log).unwrap()).unwrap();
    assert_eq!(TaskExecutionView::compile(run(), &cold_log).unwrap(), view);
    let partial = TaskExecutionView::compile(run(), &cold_log[1..]).unwrap();
    assert!(partial.recovery.active().is_none());
    assert_eq!(
        partial.recovery.latest().unwrap().diagnostic,
        "operator inspection"
    );
    assert_eq!(partial.tasks.len(), 1);
    assert!(partial.terminal.is_none());
}
