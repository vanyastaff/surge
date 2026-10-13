//! The await side of every human decision (#96).
//!
//! A run pauses for an operator at three places: a `HumanGate` node, the
//! skill-trust prompt (`stage::skill_binding`), and an agent's mid-turn
//! request-human-input tool call. All three wait here. A caller describes the
//! request ([`DecisionRequest`]) and receives one closed result
//! ([`DecisionOutcome`]); it only maps that result to its own: a routed gate
//! outcome, a bind/reject, a tool reply.
//!
//! The resolve side is not here: channels call `Engine::resolve_human_input`
//! / `Engine::resolve_gate_input` directly (ADR-0009).
//!
//! # Ordering and exit-path guarantees
//! - The decision sender is registered immediately before the request becomes
//!   durable, never earlier (ADR-0015); a restored request registers before
//!   waiting, since its request is already durable.
//! - Every node-keyed request carries its deadline contract in its schema
//!   (`x-surge-timeout-ms`, `null` for no deadline), so a restart can restore
//!   the wait from the log.
//! - The wait checks run cancellation first (biased), then the decision, then
//!   the caller's abandon signals, then the deadline.
//! - A decision that won the wait is persisted completely: a node-keyed answer
//!   arrives already committed by the engine and is verified against its
//!   receipt; a call-id-keyed answer arrives uncommitted and is committed here.
//! - The registry entry is removed on every exit path, and only while it is
//!   still this request's entry; a newer request on the same node is never
//!   deregistered by an older wait.
//! - Cancellation and abandonment write nothing about the decision; a timeout
//!   writes exactly one `HumanInputTimedOut`.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use surge_core::id::{GateRequestId, SessionId};
use surge_core::keys::{NodeKey, OutcomeKey};
use surge_core::run_event::{EventPayload, VersionedEventPayload};
use surge_persistence::runs::run_writer::RunWriter;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use crate::engine::stage::StageError;
use crate::engine::stage::human_gate::{GateResolutions, HumanGateResolution, PendingGate};

/// Schema key carrying a node-keyed request's original deadline contract.
pub(crate) const DEADLINE_CONTRACT_KEY: &str = "x-surge-timeout-ms";

/// Call-id-keyed registry for agent tool questions
/// (`call_id → sender`); the answer arrives uncommitted.
pub type ToolResolutions = tokio::sync::Mutex<HashMap<String, oneshot::Sender<serde_json::Value>>>;

/// A boxed one-shot signal borrowed for the duration of one wait.
pub(crate) type Signal<'a> = Pin<Box<dyn Future<Output = ()> + Send + 'a>>;

/// Who resolves the request, and the request's identity there.
pub(crate) enum Resolver<'a> {
    /// Node-keyed: `Engine::resolve_gate_input` commits `HumanInputResolved`
    /// before handing the answer over. `registry: None` means no operator
    /// can answer (the wait can only time out or be cancelled).
    Gate {
        /// The run's node-keyed registry.
        registry: Option<&'a GateResolutions>,
        /// Identity of this request; also its durable `call_id`.
        request_id: GateRequestId,
        /// Answers the engine may accept.
        allowed_outcomes: Vec<OutcomeKey>,
        /// Whether any outcome string is accepted.
        allow_freetext: bool,
    },
    /// Call-id-keyed: the waiter commits `HumanInputResolved` itself.
    Tool {
        /// The run's call-id-keyed registry.
        registry: &'a ToolResolutions,
        /// The provider's tool call id; also the durable `call_id`.
        call_id: String,
        /// The agent session asking.
        session: SessionId,
    },
}

impl Resolver<'_> {
    fn call_id(&self) -> String {
        match self {
            Self::Gate { request_id, .. } => request_id.to_string(),
            Self::Tool { call_id, .. } => call_id.clone(),
        }
    }
}

/// A request that is already durable, restored after a restart.
pub(crate) struct RestoredRequest<'a> {
    /// Sequence of the original `HumanInputRequested`.
    pub requested_seq: u64,
    /// Timestamp of the original `HumanInputRequested`.
    pub requested_at: chrono::DateTime<chrono::Utc>,
    /// The schema the original request recorded.
    pub recorded_schema: Option<&'a serde_json::Value>,
}

/// When an unanswered request times out.
pub(crate) enum Deadline<'a> {
    /// Never: the request waits for a decision or cancellation.
    None,
    /// After `contract`, measured by a tokio sleep from the durable request
    /// time on the run writer's clock.
    After(Duration),
    /// After `contract`, but fired by the caller's signal instead of a sleep
    /// (tests fire or withhold it deterministically).
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the injected deadline seam is driven by tests")
    )]
    Injected {
        /// The configured duration recorded as the contract.
        contract: Duration,
        /// Completes when the deadline passes.
        fires: Signal<'a>,
    },
}

impl Deadline<'_> {
    fn contract(&self) -> Option<Duration> {
        match self {
            Self::None => None,
            Self::After(contract) | Self::Injected { contract, .. } => Some(*contract),
        }
    }
}

/// Why a wait ended without a decision, other than cancel or timeout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AbandonReason {
    /// The agent's provider prompt finished while the question was pending.
    PromptFinished,
    /// The MCP caller that asked disconnected.
    CallerDisconnected,
    /// The decision channel closed without an answer (its registry entry was
    /// dropped or replaced).
    ChannelClosed,
}

/// Everything one wait needs.
pub(crate) struct DecisionRequest<'a> {
    /// The run's writer.
    pub writer: &'a RunWriter,
    /// The node asking.
    pub node: &'a NodeKey,
    /// Who resolves it.
    pub resolver: Resolver<'a>,
    /// Operator-facing prompt (written only for a fresh request).
    pub prompt: String,
    /// Response schema. A node-keyed request gains its deadline contract.
    pub schema: Option<serde_json::Value>,
    /// The deadline policy for a fresh request. A restored node-keyed request
    /// keeps the contract its durable schema recorded.
    pub deadline: Deadline<'a>,
    /// `Some` when the request is already durable.
    pub restored: Option<RestoredRequest<'a>>,
    /// The run's cancellation token.
    pub cancel: &'a CancellationToken,
    /// Extra signals that end the wait, each with its reason.
    pub abandon: Vec<(AbandonReason, Signal<'a>)>,
}

/// A persisted operator decision.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Decision {
    /// The full response payload.
    pub response: serde_json::Value,
    /// The chosen outcome (node-keyed decisions only).
    pub outcome: Option<OutcomeKey>,
    /// Sequence of the durable `HumanInputResolved`.
    pub resolved_seq: u64,
}

/// How a wait ended.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum DecisionOutcome {
    /// The operator decided; the decision is durable.
    Decided(Decision),
    /// Nobody answered in time; `HumanInputTimedOut` is durable.
    TimedOut {
        /// Elapsed seconds as recorded (the configured deadline, saturating).
        elapsed_seconds: u32,
    },
    /// The run was cancelled; nothing about the decision was written.
    Cancelled,
    /// An abandon signal fired; nothing about the decision was written.
    Abandoned(AbandonReason),
}

/// The result of one wait, with where the durable request sits.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct HumanDecision {
    /// Sequence of the durable `HumanInputRequested`.
    pub requested_seq: u64,
    /// How the wait ended.
    pub outcome: DecisionOutcome,
}

/// Request (or restore) one human decision and wait for it.
///
/// # Errors
/// - [`StageError::Storage`] when writing the request, the resolution or the
///   timeout fails; the registry entry is removed first.
/// - [`StageError::RecoveryRequired`] when a restored request's deadline
///   contract is missing, invalid or contradicts the request, or when an
///   engine-committed answer's receipt contradicts the request.
#[must_use = "a human decision must be mapped to the caller's result"]
pub(crate) async fn await_decision(
    request: DecisionRequest<'_>,
) -> Result<HumanDecision, StageError> {
    let DecisionRequest {
        writer,
        node,
        resolver,
        prompt,
        schema,
        deadline,
        restored,
        cancel,
        abandon,
    } = request;
    let node_keyed = matches!(resolver, Resolver::Gate { .. });
    let contract = match &restored {
        Some(restored) if node_keyed => {
            verify_restored_contract(schema.clone(), restored.recorded_schema)?
        },
        _ => deadline.contract(),
    };
    let remaining = contract.map(|contract| {
        let elapsed = restored.as_ref().map_or(Duration::ZERO, |restored| {
            elapsed_since(restored.requested_at, writer.clock().now_ms())
        });
        contract.saturating_sub(elapsed)
    });
    let timer: Signal<'_> = match (remaining, deadline) {
        (None, _) => Box::pin(std::future::pending()),
        (Some(_), Deadline::Injected { fires, .. }) => fires,
        (Some(remaining), Deadline::None | Deadline::After(_)) => {
            Box::pin(tokio::time::sleep(remaining))
        },
    };

    let pending = Pending {
        prompt,
        schema: if node_keyed {
            Some(with_deadline_contract(
                schema.unwrap_or_else(|| serde_json::json!({})),
                contract,
            )?)
        } else {
            schema
        },
        restored_seq: restored.as_ref().map(|restored| restored.requested_seq),
        contract,
    };

    // Nothing fallible may sit between registration and the guarded wait:
    // every exit after this line passes through `deregister`.
    let call_id = resolver.call_id();
    let receiver = register(&resolver, node, writer).await;
    let result = wait_registered(
        writer, node, &resolver, &call_id, receiver, pending, cancel, abandon, timer,
    )
    .await;
    deregister(&resolver, node).await;
    result
}

/// What still has to happen for a registered request.
struct Pending {
    prompt: String,
    schema: Option<serde_json::Value>,
    restored_seq: Option<u64>,
    contract: Option<Duration>,
}

enum Receiver {
    Gate(oneshot::Receiver<HumanGateResolution>),
    Tool(oneshot::Receiver<serde_json::Value>),
    /// No registry: nobody can answer.
    Unanswerable,
}

enum Waited {
    Gate(HumanGateResolution),
    Tool(serde_json::Value),
    Closed,
}

impl Receiver {
    async fn recv(self) -> Waited {
        match self {
            Self::Gate(rx) => rx.await.map_or(Waited::Closed, Waited::Gate),
            Self::Tool(rx) => rx.await.map_or(Waited::Closed, Waited::Tool),
            Self::Unanswerable => std::future::pending().await,
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn wait_registered(
    writer: &RunWriter,
    node: &NodeKey,
    resolver: &Resolver<'_>,
    call_id: &str,
    receiver: Receiver,
    pending: Pending,
    cancel: &CancellationToken,
    abandon: Vec<(AbandonReason, Signal<'_>)>,
    timer: Signal<'_>,
) -> Result<HumanDecision, StageError> {
    let requested_seq = match pending.restored_seq {
        Some(seq) => seq,
        None => writer
            .append_event(VersionedEventPayload::new(
                EventPayload::HumanInputRequested {
                    node: node.clone(),
                    session: match resolver {
                        Resolver::Gate { .. } => None,
                        Resolver::Tool { session, .. } => Some(*session),
                    },
                    call_id: Some(call_id.to_owned()),
                    prompt: pending.prompt,
                    schema: pending.schema,
                },
            ))
            .await
            .map_err(|error| StageError::Storage(error.to_string()))?
            .as_u64(),
    };

    let abandoned = async move {
        if abandon.is_empty() {
            std::future::pending::<AbandonReason>().await
        } else {
            let signals = abandon.into_iter().map(|(reason, signal)| {
                Box::pin(async move {
                    signal.await;
                    reason
                })
            });
            futures::future::select_all(signals).await.0
        }
    };
    let waited = tokio::select! {
        biased;
        () = cancel.cancelled() => return Ok(HumanDecision { requested_seq, outcome: DecisionOutcome::Cancelled }),
        waited = receiver.recv() => waited,
        reason = abandoned => return Ok(HumanDecision { requested_seq, outcome: DecisionOutcome::Abandoned(reason) }),
        () = timer => {
            let elapsed_seconds = pending
                .contract
                .map_or(u32::MAX, |contract| u32::try_from(contract.as_secs()).unwrap_or(u32::MAX));
            writer
                .append_event(VersionedEventPayload::new(EventPayload::HumanInputTimedOut {
                    node: node.clone(),
                    call_id: Some(call_id.to_owned()),
                    elapsed_seconds,
                }))
                .await
                .map_err(|error| StageError::Storage(error.to_string()))?;
            return Ok(HumanDecision { requested_seq, outcome: DecisionOutcome::TimedOut { elapsed_seconds } });
        },
    };

    let decision = match (waited, resolver) {
        (Waited::Closed, _) => {
            return Ok(HumanDecision {
                requested_seq,
                outcome: DecisionOutcome::Abandoned(AbandonReason::ChannelClosed),
            });
        },
        (Waited::Gate(resolution), Resolver::Gate { request_id, .. }) => Decision {
            resolved_seq: persist_gate_resolution(writer, node, *request_id, &resolution).await?,
            outcome: Some(resolution.outcome),
            response: resolution.response,
        },
        (Waited::Tool(response), Resolver::Tool { .. }) => Decision {
            resolved_seq: writer
                .append_event(VersionedEventPayload::new(
                    EventPayload::HumanInputResolved {
                        node: node.clone(),
                        call_id: Some(call_id.to_owned()),
                        response: response.clone(),
                    },
                ))
                .await
                .map_err(|error| StageError::Storage(error.to_string()))?
                .as_u64(),
            outcome: None,
            response,
        },
        (Waited::Gate(_) | Waited::Tool(_), _) => {
            return Err(StageError::Internal(
                "human decision arrived on a foreign registry".into(),
            ));
        },
    };
    Ok(HumanDecision {
        requested_seq,
        outcome: DecisionOutcome::Decided(decision),
    })
}

/// Register this request's sender; the caller makes the request durable
/// immediately after.
async fn register(resolver: &Resolver<'_>, node: &NodeKey, writer: &RunWriter) -> Receiver {
    match resolver {
        Resolver::Gate { registry: None, .. } => Receiver::Unanswerable,
        Resolver::Gate {
            registry: Some(registry),
            request_id,
            allowed_outcomes,
            allow_freetext,
        } => {
            let (sender, rx) = oneshot::channel();
            registry.lock().await.insert(
                node.clone(),
                PendingGate {
                    recorder: writer.event_recorder(),
                    allowed_outcomes: allowed_outcomes.clone(),
                    allow_freetext: *allow_freetext,
                    request_id: *request_id,
                    sender,
                },
            );
            Receiver::Gate(rx)
        },
        Resolver::Tool {
            registry, call_id, ..
        } => {
            let (sender, rx) = oneshot::channel();
            registry.lock().await.insert(call_id.clone(), sender);
            Receiver::Tool(rx)
        },
    }
}

/// Remove this request's entry, and only while it is still this request's.
async fn deregister(resolver: &Resolver<'_>, node: &NodeKey) {
    match resolver {
        Resolver::Gate { registry: None, .. } => {},
        Resolver::Gate {
            registry: Some(registry),
            request_id,
            ..
        } => {
            let mut registry = registry.lock().await;
            if registry
                .get(node)
                .is_some_and(|pending| pending.request_id == *request_id)
            {
                registry.remove(node);
            }
        },
        Resolver::Tool {
            registry, call_id, ..
        } => {
            // The call id is the identity: provider tool call ids are unique
            // within a session, so the key never names another request.
            registry.lock().await.remove(call_id);
        },
    }
}

/// Validate an engine-committed answer, or commit a direct stage-fixture
/// answer (`committed_seq: None`).
pub(crate) async fn persist_gate_resolution(
    writer: &RunWriter,
    node: &NodeKey,
    request_id: GateRequestId,
    resolution: &HumanGateResolution,
) -> Result<u64, StageError> {
    if let Some(seq) = resolution.committed_seq {
        let events = writer
            .read_events(
                surge_persistence::runs::EventSeq(seq)
                    ..surge_persistence::runs::EventSeq(seq).next(),
            )
            .await
            .map_err(|error| StageError::Storage(error.to_string()))?;
        let receipt_matches = matches!(
            events.as_slice(),
            [event] if matches!(
                event.payload.payload(),
                EventPayload::HumanInputResolved { node: accepted_node, call_id: Some(call_id), response }
                    if accepted_node == node
                        && call_id == &request_id.to_string()
                        && response == &resolution.response
            )
        );
        if !receipt_matches {
            return Err(StageError::RecoveryRequired(
                "gate response receipt contradicts its owned request".into(),
            ));
        }
        Ok(seq)
    } else {
        let seq = writer
            .append_event(VersionedEventPayload::new(
                EventPayload::HumanInputResolved {
                    node: node.clone(),
                    call_id: Some(request_id.to_string()),
                    response: resolution.response.clone(),
                },
            ))
            .await
            .map_err(|error| StageError::Storage(error.to_string()))?;
        Ok(seq.as_u64())
    }
}

/// `schema` with its deadline contract embedded (`null` for no deadline).
pub(crate) fn with_deadline_contract(
    mut schema: serde_json::Value,
    contract: Option<Duration>,
) -> Result<serde_json::Value, StageError> {
    let value = contract
        .map(|contract| u64::try_from(contract.as_millis()).map(serde_json::Value::from))
        .transpose()
        .map_err(|error| StageError::RecoveryRequired(error.to_string()))?
        .unwrap_or(serde_json::Value::Null);
    let object = schema.as_object_mut().ok_or_else(|| {
        StageError::RecoveryRequired("decision schema is not a JSON object".into())
    })?;
    object.insert(DEADLINE_CONTRACT_KEY.to_owned(), value);
    Ok(schema)
}

/// The original deadline contract of a restored node-keyed request, after
/// checking that its recorded schema is exactly `schema` plus that contract.
///
/// # Errors
/// [`StageError::RecoveryRequired`] when the contract is missing or invalid,
/// or the recorded schema contradicts the request.
pub(crate) fn verify_restored_contract(
    schema: Option<serde_json::Value>,
    recorded: Option<&serde_json::Value>,
) -> Result<Option<Duration>, StageError> {
    let contract = recorded_contract(recorded)?;
    let expected = schema
        .map(|schema| with_deadline_contract(schema, contract))
        .transpose()?;
    if recorded != expected.as_ref() {
        return Err(StageError::RecoveryRequired(
            "durable decision request contradicts its current contract".into(),
        ));
    }
    Ok(contract)
}

/// The deadline contract a durable request recorded.
fn recorded_contract(schema: Option<&serde_json::Value>) -> Result<Option<Duration>, StageError> {
    match schema.and_then(|schema| schema.get(DEADLINE_CONTRACT_KEY)) {
        Some(serde_json::Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .map(Duration::from_millis)
            .map(Some)
            .ok_or_else(|| {
                StageError::RecoveryRequired(
                    "decision request has an invalid original deadline".into(),
                )
            }),
        None => Err(StageError::RecoveryRequired(
            "decision request has no original deadline contract".into(),
        )),
    }
}

/// Wall-clock time since a durable request, never negative.
fn elapsed_since(requested_at: chrono::DateTime<chrono::Utc>, now_ms: i64) -> Duration {
    u64::try_from(now_ms.saturating_sub(requested_at.timestamp_millis()))
        .map_or(Duration::ZERO, Duration::from_millis)
}

/// Test stand-ins for the resolve side.
#[cfg(test)]
pub(crate) mod test_support {
    use std::sync::Arc;

    use surge_core::keys::NodeKey;

    use crate::engine::stage::human_gate::{GateResolutions, HumanGateResolution};

    /// An empty node-keyed registry.
    pub(crate) fn gate_registry() -> Arc<GateResolutions> {
        Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()))
    }

    /// Answer `node`'s request once its sender is registered, exactly as
    /// `Engine::resolve_gate_input` hands an answer over (minus the engine's
    /// own commit when `committed_seq` is `None`).
    pub(crate) fn answer_when_registered(
        registry: Arc<GateResolutions>,
        node: NodeKey,
        resolution: HumanGateResolution,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            loop {
                let pending = registry.lock().await.remove(&node);
                if let Some(pending) = pending {
                    let _ = pending.sender.send(resolution);
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
    }
}

#[cfg(test)]
mod tests;
