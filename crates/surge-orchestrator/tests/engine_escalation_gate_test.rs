//! Integration test: an implementer → verifier loop that exhausts its
//! `max_traversals` reaches the default escalation gate
//! (`surge_core::escalation`) instead of failing the run. "Retry" gives the
//! loop one more attempt carrying the verifier's findings; "stop" ends the
//! run through the gate's failure terminal. The persisted journal stays
//! trusted by the fold.

mod fixtures;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use surge_acp::bridge::event::BridgeEvent;
use surge_core::agent_config::AgentConfig;
use surge_core::edge::{Edge, EdgeKind, EdgePolicy, ExceededAction, PortRef};
use surge_core::graph::{Graph, GraphMetadata, SCHEMA_VERSION};
use surge_core::id::{GateRequestId, RunId, SessionId};
use surge_core::keys::{EdgeKey, NodeKey, OutcomeKey, ProfileKey};
use surge_core::node::{Node, NodeConfig, OutcomeDecl, Position};
use surge_core::run_event::EventPayload;
use surge_core::terminal_config::{TerminalConfig, TerminalKind};
use surge_orchestrator::engine::tools::worktree::WorktreeToolDispatcher;
use surge_orchestrator::engine::{Engine, EngineConfig, EngineRunConfig, RunOutcome};
use surge_persistence::runs::Storage;
use surge_persistence::runs::seq::EventSeq;

fn outcome(id: &str, kind: EdgeKind) -> OutcomeDecl {
    OutcomeDecl {
        id: OutcomeKey::try_from(id).unwrap(),
        description: id.into(),
        edge_kind_hint: kind,
        is_terminal: false,
        ledger_effect: Default::default(),
    }
}

fn agent(id: &str, outcomes: Vec<OutcomeDecl>) -> Node {
    Node {
        id: NodeKey::try_from(id).unwrap(),
        position: Position::default(),
        declared_outcomes: outcomes,
        config: NodeConfig::Agent(AgentConfig {
            profile: ProfileKey::try_from("implementer@1.0").unwrap(),
            prompt_overrides: None,
            tool_overrides: None,
            sandbox_override: None,
            approvals_override: None,
            bindings: vec![],
            rules_overrides: None,
            limits: surge_core::agent_config::NodeLimits::default(),
            hooks: vec![],
            custom_fields: BTreeMap::new(),
        }),
    }
}

fn edge(id: &str, from: &str, outcome: &str, to: &str, kind: EdgeKind) -> Edge {
    Edge {
        id: EdgeKey::try_from(id).unwrap(),
        from: PortRef {
            node: NodeKey::try_from(from).unwrap(),
            outcome: OutcomeKey::try_from(outcome).unwrap(),
        },
        to: NodeKey::try_from(to).unwrap(),
        kind,
        policy: EdgePolicy::default(),
    }
}

/// implement --done--> verify --passed--> end; verify --failed--> implement
/// (backtrack, at most once, escalate when exhausted, no declared route).
fn retry_loop_graph() -> Graph {
    let mut nodes = BTreeMap::new();
    for node in [
        agent("implement", vec![outcome("done", EdgeKind::Forward)]),
        agent(
            "verify",
            vec![
                outcome("passed", EdgeKind::Forward),
                outcome("failed", EdgeKind::Backtrack),
            ],
        ),
        Node {
            id: NodeKey::try_from("end").unwrap(),
            position: Position::default(),
            declared_outcomes: vec![],
            config: NodeConfig::Terminal(TerminalConfig {
                kind: TerminalKind::Success,
                message: None,
            }),
        },
    ] {
        nodes.insert(node.id.clone(), node);
    }
    let mut retry = edge(
        "e_retry",
        "verify",
        "failed",
        "implement",
        EdgeKind::Backtrack,
    );
    retry.policy.max_traversals = Some(1);
    retry.policy.on_max_exceeded = ExceededAction::Escalate;
    Graph {
        schema_version: SCHEMA_VERSION,
        metadata: GraphMetadata::new("escalation-gate-harness", chrono::Utc::now()),
        start: NodeKey::try_from("implement").unwrap(),
        nodes,
        edges: vec![
            edge("e_done", "implement", "done", "verify", EdgeKind::Forward),
            edge("e_passed", "verify", "passed", "end", EdgeKind::Forward),
            retry,
        ],
        subgraphs: BTreeMap::new(),
    }
}

/// Wait until agent turn `index` (zero-based) has subscribed and sent its prompt.
async fn wait_for_turn(mock: &fixtures::mock_bridge::MockBridge, index: usize) {
    let reached = async {
        loop {
            let sent = mock
                .recorded_calls
                .lock()
                .await
                .iter()
                .filter(|call| {
                    matches!(
                        call,
                        fixtures::mock_bridge::RecordedCall::SendMessage { .. }
                    )
                })
                .count();
            if sent > index && mock.subscribe_count() > index {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    };
    tokio::time::timeout(Duration::from_secs(20), reached)
        .await
        .expect("agent turn reached");
}

/// Answer each agent turn in order with a scripted outcome, returning the
/// prompt each turn received.
async fn drive_agent_turns(
    mock: Arc<fixtures::mock_bridge::MockBridge>,
    script: &[(&'static str, &'static str)],
) -> tokio::task::JoinHandle<Vec<String>> {
    let script = script.to_vec();
    let sessions: Vec<SessionId> = script.iter().map(|_| SessionId::new()).collect();
    mock.pin_session_ids(sessions.clone()).await;
    tokio::spawn(async move {
        let mut prompts = Vec::new();
        for (index, ((key, summary), session)) in script.iter().zip(&sessions).enumerate() {
            wait_for_turn(&mock, index).await;
            prompts.push(mock.last_prompt().await.unwrap_or_default());
            mock.enqueue_event(BridgeEvent::OutcomeReported {
                session: *session,
                outcome: OutcomeKey::try_from(*key).unwrap(),
                summary: (*summary).into(),
                artifacts_produced: vec![],
                verification_report: None,
            })
            .await;
            mock.pump_scripted_events().await;
        }
        prompts
    })
}

async fn next_gate(
    tap: &mut tokio::sync::broadcast::Receiver<surge_orchestrator::engine::RunEventTap>,
    run: RunId,
) -> (NodeKey, GateRequestId) {
    loop {
        let event = tap.recv().await.unwrap();
        if event.run_id != run {
            continue;
        }
        if let EventPayload::HumanInputRequested {
            node,
            session: None,
            call_id: Some(call_id),
            ..
        } = event.event.payload.payload
        {
            return (node, GateRequestId::from_event_call_id(&call_id).unwrap());
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_restarted_host_reissues_the_escalation_gate_and_honours_stop() {
    tokio::time::timeout(Duration::from_secs(60), async {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();
        let build = |storage, mock: Arc<fixtures::mock_bridge::MockBridge>| {
            Engine::new(
                mock,
                storage,
                Arc::new(WorktreeToolDispatcher::new(dir.path().to_path_buf())),
                EngineConfig::default(),
            )
        };
        let mock = Arc::new(fixtures::mock_bridge::MockBridge::new());
        let engine = build(storage.clone(), mock.clone());
        let driver = drive_agent_turns(
            mock,
            &[
                ("done", "first"),
                ("failed", "unmet"),
                ("done", "second"),
                ("failed", "still unmet"),
            ],
        )
        .await;
        let run = RunId::new();
        let mut tap = engine.subscribe_tap();
        let handle = engine
            .start_run(run, retry_loop_graph(), dir.path().into(), EngineRunConfig::default())
            .await
            .unwrap();
        let old = next_gate(&mut tap, run).await;
        assert_eq!(old.0.as_str(), "verify_escalation");
        driver.await.unwrap();
        handle.completion.abort();
        assert!(handle.completion.await.unwrap_err().is_cancelled());
        drop(engine);
        drop(storage);

        let storage = Storage::open(dir.path()).await.unwrap();
        let engine = build(
            storage.clone(),
            Arc::new(fixtures::mock_bridge::MockBridge::new()),
        );
        let mut tap = engine.subscribe_tap();
        let resumed = engine.resume_run(run, dir.path().into()).await.unwrap();
        // The unanswered request is reissued under a fresh identity.
        let fresh = loop {
            let request = next_gate(&mut tap, run).await;
            if request.1 != old.1 {
                break request;
            }
        };
        assert_eq!(fresh.0, old.0);
        engine
            .resolve_gate_input(run, fresh.0, fresh.1, serde_json::json!({"outcome": "stop"}))
            .await
            .unwrap();
        let outcome = resumed.await_completion().await.unwrap();
        assert!(
            matches!(&outcome, RunOutcome::Failed { error } if error.contains("stopped by the operator")),
            "{outcome:?}"
        );
        storage
            .inspect_folded_run(run)
            .await
            .expect("a resumed journal through the default gate stays trusted");
    })
    .await
    .expect("restart fixture exceeded watchdog");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn exhausted_loop_asks_then_retries_with_findings_then_stops() {
    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::open(dir.path()).await.unwrap();
    let mock = Arc::new(fixtures::mock_bridge::MockBridge::new());
    let dispatcher = Arc::new(WorktreeToolDispatcher::new(dir.path().to_path_buf()));
    let engine = Engine::new(
        mock.clone(),
        storage.clone(),
        dispatcher,
        EngineConfig::default(),
    );

    // Six agent turns: implement, verify(failed) twice before the first gate,
    // then one retry round before the second gate.
    let script = [
        ("done", "first implementation"),
        ("failed", "Login criterion unmet."),
        ("done", "second implementation"),
        ("failed", "Login criterion still unmet."),
        ("done", "third implementation"),
        ("failed", "Still unmet after retry."),
    ];
    let driver = drive_agent_turns(mock.clone(), &script).await;

    let run = RunId::new();
    let mut tap = engine.subscribe_tap();
    let handle = engine
        .start_run(
            run,
            retry_loop_graph(),
            dir.path().into(),
            EngineRunConfig::default(),
        )
        .await
        .expect("start_run");
    let mut completion = handle.completion;
    let mut answers = ["retry", "stop"].into_iter();
    let mut gate_prompts = Vec::new();
    let outcome = tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            tokio::select! {
                outcome = &mut completion => break outcome.unwrap(),
                event = tap.recv() => {
                    let Ok(event) = event else { continue };
                    if event.run_id != run { continue; }
                    if let EventPayload::HumanInputRequested { node, call_id: Some(call_id), session: None, prompt, .. } = event.event.payload.payload {
                        gate_prompts.push((node.clone(), prompt));
                        let answer = answers.next().expect("only two decisions are expected");
                        engine
                            .resolve_gate_input(
                                run,
                                node,
                                GateRequestId::from_event_call_id(&call_id).unwrap(),
                                serde_json::json!({ "outcome": answer }),
                            )
                            .await
                            .unwrap();
                    }
                }
            }
        }
    })
    .await
    .expect("run finishes");
    let prompts = driver.await.unwrap();

    let RunOutcome::Failed { error } = &outcome else {
        panic!("stop must fail the run, got {outcome:?}");
    };
    assert!(
        error.contains("stopped by the operator after `verify` reached its attempt limit"),
        "{error}"
    );
    assert_eq!(gate_prompts.len(), 2);
    for (node, prompt) in &gate_prompts {
        assert_eq!(node.as_str(), "verify_escalation");
        assert!(
            prompt.contains("Attempt limit reached at verify"),
            "{prompt}"
        );
        assert!(prompt.contains("`verify` reported `failed` more than 1 times"));
    }
    // The retried implementer sees the verifier's findings, not the gate's.
    assert!(
        prompts[4].starts_with("## Feedback from the previous attempt"),
        "{}",
        prompts[4]
    );
    assert!(prompts[4].contains("Stage `verify` sent this work back with outcome `failed`"));
    assert!(prompts[4].contains("Summary: Login criterion still unmet."));
    assert!(prompts[2].contains("Summary: Login criterion unmet."));
    assert!(!prompts[0].contains("Feedback from the previous attempt"));

    storage
        .inspect_folded_run(run)
        .await
        .expect("a journal through the default gate stays trusted");
    let events: Vec<EventPayload> = storage
        .open_run_reader(run)
        .await
        .unwrap()
        .read_events(EventSeq::ZERO..EventSeq(u64::MAX))
        .await
        .unwrap()
        .into_iter()
        .map(|event| event.payload.payload)
        .collect();
    let traversed = |from: &str, to: &str, kind: EdgeKind| {
        events
            .iter()
            .filter(|event| {
                matches!(event, EventPayload::EdgeTraversed { from: f, to: t, kind: k, .. }
                    if f.as_str() == from && t.as_str() == to && *k == kind)
            })
            .count()
    };
    assert_eq!(
        traversed("verify", "verify_escalation", EdgeKind::Escalate),
        2
    );
    assert_eq!(
        traversed("verify_escalation", "implement", EdgeKind::Backtrack),
        1
    );
    assert_eq!(
        traversed("verify_escalation", "verify_stopped", EdgeKind::Forward),
        1
    );
    // The persisted graph is the flow as written; gates are derived.
    let persisted = events
        .iter()
        .find_map(|event| match event {
            EventPayload::PipelineMaterialized { graph, .. } => Some(graph),
            _ => None,
        })
        .unwrap();
    assert!(
        !persisted
            .nodes
            .contains_key(&NodeKey::try_from("verify_escalation").unwrap())
    );
}
