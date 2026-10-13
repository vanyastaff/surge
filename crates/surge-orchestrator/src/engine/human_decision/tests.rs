//! Interface tests for the human decision wait (#96): a real temporary run
//! writer, real registries, a cancellation token and an injected deadline.

use std::sync::Arc;
use std::time::Duration;

use surge_core::id::{GateRequestId, RunId, SessionId};
use surge_core::keys::{NodeKey, OutcomeKey};
use surge_core::run_event::{EventPayload, VersionedEventPayload};
use surge_persistence::runs::Storage;
use surge_persistence::runs::run_writer::RunWriter;
use tokio_util::sync::CancellationToken;

use super::test_support::{answer_when_registered, gate_registry};
use super::*;

struct Fixture {
    dir: crate::runtime_home_fixture::FixtureHome,
    storage: Arc<Storage>,
    run: RunId,
    writer: RunWriter,
    node: NodeKey,
}

impl Fixture {
    async fn new() -> Self {
        let dir = crate::runtime_home_fixture::FixtureHome::new().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();
        let run = RunId::new();
        let writer = storage.create_run(run, dir.path(), None).await.unwrap();
        Self {
            dir,
            storage,
            run,
            writer,
            node: NodeKey::try_from("decide").unwrap(),
        }
    }

    async fn kinds(&self) -> Vec<&'static str> {
        let reader = self.storage.open_run_reader(self.run).await.unwrap();
        reader
            .read_events(
                surge_persistence::runs::EventSeq(0)..surge_persistence::runs::EventSeq(64),
            )
            .await
            .unwrap()
            .into_iter()
            .map(|event| event.payload.payload.discriminant_str())
            .collect()
    }

    async fn close(self) {
        self.writer.close().await.unwrap();
        drop(self.storage);
        self.dir.close().unwrap();
    }

    fn gate<'a>(
        &'a self,
        registry: Option<&'a GateResolutions>,
        request_id: GateRequestId,
        deadline: Deadline<'a>,
        cancel: &'a CancellationToken,
    ) -> DecisionRequest<'a> {
        DecisionRequest {
            writer: &self.writer,
            node: &self.node,
            resolver: Resolver::Gate {
                registry,
                request_id,
                allowed_outcomes: vec![outcome("approve"), outcome("reject")],
                allow_freetext: false,
            },
            prompt: "Approve?".into(),
            schema: Some(serde_json::json!({"type": "object"})),
            deadline,
            restored: None,
            cancel,
            abandon: Vec::new(),
        }
    }
}

fn outcome(key: &str) -> OutcomeKey {
    OutcomeKey::try_from(key).unwrap()
}

fn never<'a>() -> Signal<'a> {
    Box::pin(std::future::pending())
}

fn fired<'a>() -> Signal<'a> {
    Box::pin(std::future::ready(()))
}

/// Answer like `Engine::resolve_gate_input`: commit through the waiter's own
/// recorder first, then hand the receipt over.
fn commit_when_registered(
    registry: Arc<GateResolutions>,
    node: NodeKey,
    response: serde_json::Value,
    receipt_response: serde_json::Value,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let pending = registry.lock().await.remove(&node);
            if let Some(pending) = pending {
                let seq = pending
                    .recorder
                    .append_event(VersionedEventPayload::new(
                        EventPayload::HumanInputResolved {
                            node: node.clone(),
                            call_id: Some(pending.request_id.to_string()),
                            response: receipt_response,
                        },
                    ))
                    .await
                    .unwrap()
                    .as_u64();
                let _ = pending.sender.send(HumanGateResolution {
                    committed_seq: Some(seq),
                    outcome: outcome(response["outcome"].as_str().unwrap()),
                    response,
                });
                return;
            }
            tokio::task::yield_now().await;
        }
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn engine_committed_answer_is_verified_and_returned() {
    let fx = Fixture::new().await;
    let registry = gate_registry();
    let cancel = CancellationToken::new();
    let answer = serde_json::json!({"outcome": "approve"});
    let operator = commit_when_registered(
        registry.clone(),
        fx.node.clone(),
        answer.clone(),
        answer.clone(),
    );
    let waited = await_decision(fx.gate(
        Some(&registry),
        GateRequestId::new(),
        Deadline::Injected {
            contract: Duration::from_secs(60),
            fires: never(),
        },
        &cancel,
    ))
    .await
    .unwrap();
    operator.await.unwrap();
    let DecisionOutcome::Decided(decision) = waited.outcome else {
        panic!("expected a decision, got {:?}", waited.outcome);
    };
    assert_eq!(decision.outcome, Some(outcome("approve")));
    assert_eq!(decision.response, answer);
    assert_eq!(
        fx.kinds().await,
        ["HumanInputRequested", "HumanInputResolved"]
    );
    assert!(registry.lock().await.is_empty());
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn contradicting_receipt_is_recovery_required() {
    let fx = Fixture::new().await;
    let registry = gate_registry();
    let cancel = CancellationToken::new();
    let operator = commit_when_registered(
        registry.clone(),
        fx.node.clone(),
        serde_json::json!({"outcome": "approve"}),
        serde_json::json!({"outcome": "reject"}),
    );
    let result = await_decision(fx.gate(
        Some(&registry),
        GateRequestId::new(),
        Deadline::None,
        &cancel,
    ))
    .await;
    operator.await.unwrap();
    assert!(
        matches!(result, Err(StageError::RecoveryRequired(_))),
        "{result:?}"
    );
    assert!(registry.lock().await.is_empty());
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uncommitted_tool_answer_is_committed_by_the_waiter() {
    let fx = Fixture::new().await;
    let registry: Arc<ToolResolutions> = Arc::default();
    let cancel = CancellationToken::new();
    let operator = {
        let registry = registry.clone();
        tokio::spawn(async move {
            loop {
                let sender = registry.lock().await.remove("call-1");
                if let Some(sender) = sender {
                    let _ = sender.send(serde_json::json!({"answer": "yes"}));
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
    };
    let waited = await_decision(DecisionRequest {
        writer: &fx.writer,
        node: &fx.node,
        resolver: Resolver::Tool {
            registry: &registry,
            call_id: "call-1".into(),
            session: SessionId::new(),
        },
        prompt: "Which?".into(),
        schema: None,
        deadline: Deadline::Injected {
            contract: Duration::from_secs(60),
            fires: never(),
        },
        restored: None,
        cancel: &cancel,
        abandon: Vec::new(),
    })
    .await
    .unwrap();
    operator.await.unwrap();
    assert!(matches!(
        &waited.outcome,
        DecisionOutcome::Decided(Decision { outcome: None, response, .. })
            if response == &serde_json::json!({"answer": "yes"})
    ));
    assert_eq!(
        fx.kinds().await,
        ["HumanInputRequested", "HumanInputResolved"]
    );
    assert!(registry.lock().await.is_empty());
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fired_deadline_records_one_timeout_with_the_configured_elapsed() {
    let fx = Fixture::new().await;
    let registry = gate_registry();
    let cancel = CancellationToken::new();
    let waited = await_decision(fx.gate(
        Some(&registry),
        GateRequestId::new(),
        Deadline::Injected {
            contract: Duration::from_secs(90),
            fires: fired(),
        },
        &cancel,
    ))
    .await
    .unwrap();
    assert_eq!(
        waited.outcome,
        DecisionOutcome::TimedOut {
            elapsed_seconds: 90
        }
    );
    assert_eq!(
        fx.kinds().await,
        ["HumanInputRequested", "HumanInputTimedOut"]
    );
    assert!(registry.lock().await.is_empty());
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_deadline_never_times_out() {
    let fx = Fixture::new().await;
    let registry = gate_registry();
    let cancel = CancellationToken::new();
    let stop = {
        let cancel = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            cancel.cancel();
        })
    };
    let waited = await_decision(fx.gate(
        Some(&registry),
        GateRequestId::new(),
        Deadline::None,
        &cancel,
    ))
    .await
    .unwrap();
    stop.await.unwrap();
    assert_eq!(waited.outcome, DecisionOutcome::Cancelled);
    let kinds = fx.kinds().await;
    assert_eq!(kinds, ["HumanInputRequested"]);
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_wins_and_writes_nothing_about_the_decision() {
    let fx = Fixture::new().await;
    let registry = gate_registry();
    let cancel = CancellationToken::new();
    cancel.cancel();
    // Even with an answer and a fired deadline ready, cancellation is first.
    let _answer = answer_when_registered(
        registry.clone(),
        fx.node.clone(),
        HumanGateResolution {
            committed_seq: None,
            outcome: outcome("approve"),
            response: serde_json::json!({"outcome": "approve"}),
        },
    );
    let waited = await_decision(fx.gate(
        Some(&registry),
        GateRequestId::new(),
        Deadline::Injected {
            contract: Duration::from_secs(1),
            fires: fired(),
        },
        &cancel,
    ))
    .await
    .unwrap();
    assert_eq!(waited.outcome, DecisionOutcome::Cancelled);
    assert_eq!(fx.kinds().await, ["HumanInputRequested"]);
    assert!(registry.lock().await.is_empty());
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn request_carries_its_deadline_contract() {
    let fx = Fixture::new().await;
    let cancel = CancellationToken::new();
    cancel.cancel();
    let _ = await_decision(fx.gate(
        None,
        GateRequestId::new(),
        Deadline::After(Duration::from_secs(7)),
        &cancel,
    ))
    .await
    .unwrap();
    let _ = await_decision(fx.gate(None, GateRequestId::new(), Deadline::None, &cancel))
        .await
        .unwrap();
    let reader = fx.storage.open_run_reader(fx.run).await.unwrap();
    let contracts: Vec<_> = reader
        .read_events(surge_persistence::runs::EventSeq(0)..surge_persistence::runs::EventSeq(8))
        .await
        .unwrap()
        .into_iter()
        .filter_map(|event| match event.payload.payload {
            EventPayload::HumanInputRequested {
                schema: Some(schema),
                ..
            } => Some(schema[DEADLINE_CONTRACT_KEY].clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        contracts,
        [serde_json::json!(7000), serde_json::Value::Null]
    );
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restore_keeps_the_original_remaining_deadline() {
    let fx = Fixture::new().await;
    let cancel = CancellationToken::new();
    let schema = serde_json::json!({"type": "object"});
    let recorded = with_deadline_contract(schema.clone(), Some(Duration::from_secs(60))).unwrap();
    let now = chrono::DateTime::from_timestamp_millis(fx.writer.clock().now_ms()).unwrap();

    // Requested two minutes ago with a one-minute contract: already expired,
    // even though the current policy would allow an hour.
    let mut request = fx.gate(
        None,
        GateRequestId::new(),
        Deadline::After(Duration::from_secs(3600)),
        &cancel,
    );
    request.restored = Some(RestoredRequest {
        requested_seq: 1,
        requested_at: now - chrono::Duration::minutes(2),
        recorded_schema: Some(&recorded),
    });
    let waited = tokio::time::timeout(Duration::from_secs(5), await_decision(request))
        .await
        .expect("an expired restored deadline fires at once")
        .unwrap();
    assert_eq!(
        waited,
        HumanDecision {
            requested_seq: 1,
            outcome: DecisionOutcome::TimedOut {
                elapsed_seconds: 60
            },
        }
    );
    // A restored request is never re-requested.
    assert_eq!(fx.kinds().await, ["HumanInputTimedOut"]);
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restore_rejects_a_contradicting_contract() {
    let fx = Fixture::new().await;
    let cancel = CancellationToken::new();
    let now = chrono::DateTime::from_timestamp_millis(fx.writer.clock().now_ms()).unwrap();
    let missing = serde_json::json!({"type": "object"});
    let changed = with_deadline_contract(
        serde_json::json!({"type": "object", "extra": true}),
        Some(Duration::from_secs(60)),
    )
    .unwrap();
    for recorded in [&missing, &changed] {
        let registry = gate_registry();
        let mut request = fx.gate(
            Some(&registry),
            GateRequestId::new(),
            Deadline::None,
            &cancel,
        );
        request.restored = Some(RestoredRequest {
            requested_seq: 1,
            requested_at: now,
            recorded_schema: Some(recorded),
        });
        let result = await_decision(request).await;
        assert!(
            matches!(result, Err(StageError::RecoveryRequired(_))),
            "{result:?}"
        );
        assert!(registry.lock().await.is_empty());
    }
    assert_eq!(fx.kinds().await, Vec::<&str>::new());
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn abandon_signal_returns_its_reason_and_deregisters() {
    let fx = Fixture::new().await;
    let registry: Arc<ToolResolutions> = Arc::default();
    let cancel = CancellationToken::new();
    let waited = await_decision(DecisionRequest {
        writer: &fx.writer,
        node: &fx.node,
        resolver: Resolver::Tool {
            registry: &registry,
            call_id: "call-1".into(),
            session: SessionId::new(),
        },
        prompt: "Which?".into(),
        schema: None,
        deadline: Deadline::None,
        restored: None,
        cancel: &cancel,
        abandon: vec![
            (AbandonReason::PromptFinished, never()),
            (AbandonReason::CallerDisconnected, fired()),
        ],
    })
    .await
    .unwrap();
    assert_eq!(
        waited.outcome,
        DecisionOutcome::Abandoned(AbandonReason::CallerDisconnected)
    );
    assert_eq!(fx.kinds().await, ["HumanInputRequested"]);
    assert!(registry.lock().await.is_empty());
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_newer_request_on_the_node_is_never_deregistered() {
    let fx = Fixture::new().await;
    let registry = gate_registry();
    let cancel = CancellationToken::new();
    let newer = GateRequestId::new();
    let replace = {
        let registry = registry.clone();
        let node = fx.node.clone();
        let recorder = fx.writer.event_recorder();
        tokio::spawn(async move {
            loop {
                let mut guard = registry.lock().await;
                if let Some(pending) = guard.get_mut(&node) {
                    let (sender, _rx) = tokio::sync::oneshot::channel();
                    *pending = PendingGate {
                        recorder,
                        allowed_outcomes: Vec::new(),
                        allow_freetext: true,
                        request_id: newer,
                        sender,
                    };
                    return;
                }
                drop(guard);
                tokio::task::yield_now().await;
            }
        })
    };
    let waited = await_decision(fx.gate(
        Some(&registry),
        GateRequestId::new(),
        Deadline::None,
        &cancel,
    ))
    .await
    .unwrap();
    replace.await.unwrap();
    assert_eq!(
        waited.outcome,
        DecisionOutcome::Abandoned(AbandonReason::ChannelClosed)
    );
    assert!(
        registry
            .lock()
            .await
            .get(&fx.node)
            .is_some_and(|pending| pending.request_id == newer),
        "the older wait must leave the newer request registered"
    );
    fx.close().await;
}
