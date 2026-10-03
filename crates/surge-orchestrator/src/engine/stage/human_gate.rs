//! `NodeKind::HumanGate` execution.
//!
//! M5 model: pause the run, emit `HumanInputRequested`, wait for either an
//! external `Engine::resolve_human_input` call or the configured timeout.
//! On timeout, apply `HumanGateConfig::on_timeout` (Reject / Escalate /
//! Continue). M5 treats Escalate as Reject (no escalation channels) and
//! Continue without a default outcome as a configuration error.

use crate::engine::stage::{StageError, StageResult};
use std::time::Duration;
use surge_core::approvals::ApprovalChannel;
use surge_core::execution_recovery::gate_commit::{
    GateCommitAnswer, GateCommitDisposition, GateCommitRequest, GateStageCommit, gate_effects_hash,
    gate_response_hash,
};
use surge_core::human_gate_config::{HumanGateConfig, HumanGateMode, TimeoutAction};
use surge_core::keys::{NodeKey, OutcomeKey};
use surge_core::run_event::{
    BootstrapDecision, EscalationCause, EventPayload, VersionedEventPayload,
};
use surge_core::run_state::RunMemory;
use surge_persistence::runs::run_writer::RunWriter;
use tokio::sync::oneshot;

/// Parameters for executing a single `NodeKind::HumanGate` stage.
pub struct HumanGateStageParams<'a> {
    /// Key of the human-gate node being executed.
    pub node: &'a NodeKey,
    /// Identity minted when this resolver was registered.
    pub request_id: surge_core::id::GateRequestId,
    /// Gate configuration: delivery channels, timeout, options.
    pub gate_config: &'a HumanGateConfig,
    /// Run writer for persisting `HumanInputRequested` / `HumanInputResolved` events.
    pub writer: &'a RunWriter,
    /// Accumulated run memory (currently unused in M5 rendering).
    pub run_memory: &'a RunMemory,
    /// Receiver fed by `Engine::resolve_human_input`. `None` ⇒ test path (timeout immediately).
    pub resolution_rx: Option<oneshot::Receiver<HumanGateResolution>>,
    /// Cancel only the decision wait, never a decision's persistence sequence.
    pub cancel: &'a tokio_util::sync::CancellationToken,
    /// Default timeout sourced from `EngineRunConfig` if the gate doesn't override.
    pub default_timeout: Duration,
    /// Bootstrap edit-loop cap from `EngineRunConfig.bootstrap.edit_loop_cap`.
    /// `0` disables the cap. Only consulted when the gate is in
    /// `HumanGateMode::Bootstrap` and the operator chose `edit`.
    pub bootstrap_edit_loop_cap: u32,
}

/// Resolution provided by an external caller (operator or automated test).
#[derive(Debug, Clone)]
pub struct HumanGateResolution {
    /// Actual durable response event, committed by the owning engine before API success.
    /// Direct stage fixtures use `None` and let the stage commit their response.
    pub committed_seq: Option<u64>,
    /// The outcome key chosen by the operator.
    pub outcome: OutcomeKey,
    /// Full JSON response payload (must contain an `"outcome"` field).
    pub response: serde_json::Value,
}

/// Node-keyed decision registry `Engine::resolve_human_input` drains.
///
/// Shared (behind an `Arc`) by `RunTaskParams::gate_resolutions` and any
/// stage that pauses on an operator decision routed through the generic
/// `HumanInputRequested`/`HumanInputResolved` pair — today `HumanGate`
/// nodes and the skill-trust prompt
/// (`engine::stage::skill_binding::bind_skills`). A caller registers a
/// sender for its node immediately before requesting the decision (never
/// earlier) so a stale, unread entry can never sit in the map — see
/// `docs/adr/0015-skill-binding-trust-via-content-hash.md`.
/// An exact request and its owned response channel.
pub struct PendingGate {
    /// Append-only capability from the same run writer owning this waiter.
    pub(crate) recorder: surge_persistence::runs::run_writer::RunEventRecorder,
    /// Original allowed options, checked before durable acceptance.
    pub(crate) allowed_outcomes: Vec<OutcomeKey>,
    pub(crate) allow_freetext: bool,
    /// Uniquely identifies this registration across visits and restarts.
    pub request_id: surge_core::id::GateRequestId,
    /// Consumed only after the request identity matches.
    pub sender: oneshot::Sender<HumanGateResolution>,
}

/// Pending node requests; request identity and sender share one lock.
pub type GateResolutions = tokio::sync::Mutex<std::collections::HashMap<NodeKey, PendingGate>>;

/// Execute a single `NodeKind::HumanGate` stage.
///
/// Emits `HumanInputRequested`, then waits for either an external
/// `Engine::resolve_human_input` call or the configured timeout.
///
/// When `gate_config.mode` is `HumanGateMode::Bootstrap { stage }` the handler
/// additionally emits a `BootstrapApprovalRequested` event before the operator
/// card is sent and a `BootstrapApprovalDecided` event after the operator
/// replies. An `edit` outcome additionally appends `BootstrapEditRequested`
/// carrying the operator's free-text feedback so downstream
/// `ArtifactSource::EditFeedback` bindings (Task 6 / Task 8) can resolve to
/// the most recent feedback for that stage.
pub async fn execute_human_gate_stage(p: HumanGateStageParams<'_>) -> StageResult {
    execute_gate(p, None).await
}

/// Rehydrate the original durable request with a fresh local resolution receiver.
pub(crate) async fn restore_human_gate_stage(
    p: HumanGateStageParams<'_>,
    record: &surge_core::run_state::RecoveredGateDecision,
) -> StageResult {
    execute_gate(p, Some(record)).await
}

#[allow(clippy::too_many_lines)]
async fn execute_gate(
    p: HumanGateStageParams<'_>,
    restored: Option<&surge_core::run_state::RecoveredGateDecision>,
) -> StageResult {
    if matches!(p.gate_config.mode, HumanGateMode::Bootstrap { .. })
        && p.run_memory.bootstrap_edit_loop_cap != Some(p.bootstrap_edit_loop_cap)
    {
        return Err(StageError::RecoveryRequired(
            "bootstrap gate lacks its immutable startup edit policy".into(),
        ));
    }
    let summary = restored.map_or_else(
        || render_summary(&p.gate_config.summary, p.run_memory),
        |record| record.prompt.clone(),
    );
    // A bootstrap approval (description / roadmap / flow review) with no
    // explicit timeout waits for the operator: the run is durable and shows
    // as "needs you", and reading a plan routinely takes longer than the
    // generic default. Rejecting on that default discarded finished planning
    // work. `None` = no deadline.
    let timeout: Option<Duration> = match (p.gate_config.timeout_seconds, &p.gate_config.mode) {
        (Some(s), _) => Some(Duration::from_secs(u64::from(s))),
        (None, HumanGateMode::Bootstrap { .. }) => None,
        (None, HumanGateMode::Generic) => Some(p.default_timeout),
    };
    let timeout = if let Some(record) = restored {
        match record
            .schema
            .as_ref()
            .and_then(|schema| schema.get("x-surge-timeout-ms"))
        {
            Some(serde_json::Value::Null) => None,
            Some(value) => Some(Duration::from_millis(value.as_u64().ok_or_else(|| {
                StageError::RecoveryRequired("human gate has an invalid original deadline".into())
            })?)),
            None => {
                return Err(StageError::RecoveryRequired(
                    "human gate has no original deadline contract".into(),
                ));
            },
        }
    } else {
        timeout
    };
    let remaining_timeout = timeout.map(|duration| {
        let elapsed = restored
            .and_then(|record| (chrono::Utc::now() - record.requested_at).to_std().ok())
            .unwrap_or_default();
        duration.saturating_sub(elapsed)
    });
    let deadline = async move {
        match remaining_timeout {
            Some(timeout) => tokio::time::sleep(timeout).await,
            None => std::future::pending::<()>().await,
        }
    };
    tokio::pin!(deadline);

    let mut schema = build_options_schema(&p.gate_config.options, p.gate_config.allow_freetext);
    schema["x-surge-timeout-ms"] = timeout
        .map(|timeout| u64::try_from(timeout.as_millis()).map(serde_json::Value::from))
        .transpose()
        .map_err(|error| StageError::RecoveryRequired(error.to_string()))?
        .unwrap_or(serde_json::Value::Null);

    // Bootstrap dispatch: when the gate guards a bootstrap stage, mirror the
    // generic request with lifecycle metadata for bootstrap observers.
    // The actionable request carries stage metadata itself; renderers must not
    // create a second approval card from BootstrapApprovalRequested.
    let bootstrap_stage = match &p.gate_config.mode {
        HumanGateMode::Generic => None,
        HumanGateMode::Bootstrap { stage } => {
            tracing::debug!(
                target: "engine::bootstrap::stage",
                node = %p.node,
                stage = ?stage,
                "human gate dispatched in Bootstrap mode"
            );
            let channel = p.gate_config.delivery_channels.first().cloned().unwrap_or(
                ApprovalChannel::Desktop {
                    duration: surge_core::approvals::ApprovalDuration::Transient,
                },
            );
            if restored.is_none() {
                p.writer
                    .append_event(VersionedEventPayload::new(
                        EventPayload::BootstrapApprovalRequested {
                            stage: *stage,
                            channel,
                        },
                    ))
                    .await
                    .map_err(|e| StageError::Storage(e.to_string()))?;
            }
            Some(*stage)
        },
    };

    if let Some(stage) = bootstrap_stage {
        schema["x-surge-bootstrap-stage"] = serde_json::json!(stage);
    }
    let requested_seq = if let Some(record) = restored {
        if record.conflicting
            || record.node != *p.node
            || record.request_id != p.request_id
            || record.schema.as_ref() != Some(&schema)
        {
            return Err(StageError::RecoveryRequired(
                "durable gate occurrence contradicts its current contract".into(),
            ));
        }
        record.requested_seq
    } else {
        p.writer
            .append_event(VersionedEventPayload::new(
                EventPayload::HumanInputRequested {
                    node: p.node.clone(),
                    session: None,
                    call_id: Some(p.request_id.to_string()),
                    prompt: summary,
                    schema: Some(schema),
                },
            ))
            .await
            .map_err(|e| StageError::Storage(e.to_string()))?
            .as_u64()
    };

    // Holds the operator's freeform `comment` when present; carried into the
    // BootstrapApprovalDecided / BootstrapEditRequested events below.
    let mut decided_comment: Option<String> = None;
    let mut accepted_answer = None;

    let outcome = if let Some(response) = restored.and_then(|record| record.response.as_ref()) {
        accepted_answer = Some((record_response_seq(restored)?, response.clone()));
        decided_comment = response
            .get("comment")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned);
        let outcome = response
            .get("outcome")
            .and_then(serde_json::Value::as_str)
            .and_then(|value| OutcomeKey::try_from(value).ok())
            .filter(|outcome| {
                p.gate_config.allow_freetext
                    || p.gate_config
                        .options
                        .iter()
                        .any(|option| &option.outcome == outcome)
            })
            .ok_or_else(|| {
                StageError::RecoveryRequired(
                    "accepted gate response contradicts its original options".into(),
                )
            })?;
        Some(outcome)
    } else if let Some(rx) = p.resolution_rx {
        tokio::select! {
            biased;
            () = p.cancel.cancelled() => return Err(StageError::Cancelled),
            resolved = rx => match resolved {
                Ok(res) => {
                    decided_comment = res
                        .response
                        .get("comment")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned);
                    let seq = persist_gate_resolution(p.writer, p.node, p.request_id, &res).await?;
                    accepted_answer = Some((seq, res.response.clone()));
                    Some(res.outcome)
                }
                Err(_) => None,
            },
            () = &mut deadline => None,
        }
    } else {
        tokio::select! {
            biased;
            () = p.cancel.cancelled() => return Err(StageError::Cancelled),
            () = &mut deadline => {},
        }
        None
    };

    let Some(final_outcome) = outcome else {
        p.writer
            .append_event(VersionedEventPayload::new(
                EventPayload::HumanInputTimedOut {
                    node: p.node.clone(),
                    call_id: Some(p.request_id.to_string()),
                    elapsed_seconds: timeout
                        .map_or(u32::MAX, |t| u32::try_from(t.as_secs()).unwrap_or(u32::MAX)),
                },
            ))
            .await
            .map_err(|e| StageError::Storage(e.to_string()))?;
        match p.gate_config.on_timeout {
            TimeoutAction::Reject | TimeoutAction::Escalate => {
                return Err(StageError::HumanGateRejected);
            },
            TimeoutAction::Continue => {
                // M5 has no default_outcome on HumanGateConfig; documented gap.
                return Err(StageError::HumanGateContinueWithoutDefault);
            },
        }
    };

    let mut effects = Vec::new();
    let mut failure = None;
    let mut summary = "human gate decision".to_owned();
    if let Some(stage) = bootstrap_stage {
        let decision = bootstrap_decision_from_outcome(&final_outcome)?;
        effects.push(VersionedEventPayload::new(
            EventPayload::BootstrapApprovalDecided {
                stage,
                decision,
                comment: decided_comment.clone(),
            },
        ));
        match decision {
            BootstrapDecision::Edit => {
                let cap = p.bootstrap_edit_loop_cap;
                let prior_edits = p
                    .run_memory
                    .bootstrap_edit_counts
                    .get(&stage)
                    .copied()
                    .unwrap_or(0);
                if cap > 0 && prior_edits >= cap {
                    summary = format!(
                        "bootstrap edit-loop cap exceeded for stage {stage:?} (cap = {cap}, prior_edits = {prior_edits})"
                    );
                    effects.push(VersionedEventPayload::new(
                        EventPayload::EscalationRequested {
                            stage: Some(stage),
                            reason: summary.clone(),
                            cause: EscalationCause::BootstrapEditLoopExhausted,
                        },
                    ));
                    failure = Some(StageError::EditLoopCapExceeded { stage, cap });
                } else {
                    if cap > 0 && prior_edits + 1 == cap {
                        tracing::warn!(target: "engine::bootstrap::stage", node = %p.node, ?stage, cap, "approaching bootstrap edit-loop cap");
                    }
                    effects.push(VersionedEventPayload::new(
                        EventPayload::BootstrapEditRequested {
                            stage,
                            feedback: decided_comment.unwrap_or_default(),
                        },
                    ));
                }
            },
            BootstrapDecision::Reject => {
                summary = "bootstrap rejected".into();
                failure = Some(StageError::HumanGateRejected);
            },
            BootstrapDecision::Approve => {},
        }
    }
    effects.push(VersionedEventPayload::new(EventPayload::OutcomeReported {
        node: p.node.clone(),
        outcome: final_outcome.clone(),
        summary,
    }));
    // Required outcome and bootstrap side effects form one accepted batch.
    // The independently accepted human answer survives a rejected effect batch.
    let (resolved_seq, response) = accepted_answer.ok_or_else(|| {
        StageError::RecoveryRequired("gate effects lack an accepted answer".into())
    })?;
    let entry = restored
        .map(|record| record.stage_entry_seq)
        .or_else(|| p.run_memory.stage_occurrences.get(p.node).copied())
        .ok_or_else(|| {
            StageError::RecoveryRequired("gate effects lack their durable stage occurrence".into())
        })?;
    let request = GateCommitRequest::new(p.node.clone(), p.request_id, entry, requested_seq)
        .map_err(|error| StageError::RecoveryRequired(error.to_string()))?;
    let response_hash = gate_response_hash(&response)
        .map_err(|error| StageError::RecoveryRequired(error.to_string()))?;
    let answer = GateCommitAnswer::new(request, resolved_seq, response_hash, final_outcome.clone())
        .map_err(|error| StageError::RecoveryRequired(error.to_string()))?;
    let disposition = match failure.as_ref() {
        Some(StageError::EditLoopCapExceeded { .. }) => GateCommitDisposition::BootstrapEscalated,
        Some(StageError::HumanGateRejected) => GateCommitDisposition::BootstrapRejected,
        None => GateCommitDisposition::Route,
        Some(_) => {
            return Err(StageError::RecoveryRequired(
                "unexpected gate commit disposition".into(),
            ));
        },
    };
    let digest = gate_effects_hash(&effects)
        .map_err(|error| StageError::RecoveryRequired(error.to_string()))?;
    let count = u32::try_from(effects.len())
        .map_err(|error| StageError::RecoveryRequired(error.to_string()))?;
    let commit = GateStageCommit::new(answer, disposition, count, digest)
        .map_err(|error| StageError::RecoveryRequired(error.to_string()))?;
    effects.push(VersionedEventPayload::new(
        EventPayload::GateStageOutcomeCommitted { commit },
    ));
    p.writer
        .append_events(effects)
        .await
        .map_err(|error| StageError::Storage(error.to_string()))?;
    if let Some(failure) = failure {
        return Err(failure);
    }

    Ok(final_outcome)
}

/// Translate the operator's `OutcomeKey` choice into a `BootstrapDecision`.
///
/// Recognized canonical keys are `approve`, `edit`, and `reject`.
fn bootstrap_decision_from_outcome(outcome: &OutcomeKey) -> Result<BootstrapDecision, StageError> {
    match outcome.as_ref() {
        "approve" => Ok(BootstrapDecision::Approve),
        "edit" => Ok(BootstrapDecision::Edit),
        "reject" => Ok(BootstrapDecision::Reject),
        other => Err(StageError::Internal(format!(
            "unsupported bootstrap outcome: {other}"
        ))),
    }
}

/// Validate an engine-committed answer or commit a direct stage-fixture answer.
pub(crate) async fn persist_gate_resolution(
    writer: &RunWriter,
    node: &NodeKey,
    request_id: surge_core::id::GateRequestId,
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
        if !matches!(events.as_slice(), [event] if matches!(event.payload.payload(), EventPayload::HumanInputResolved { node: accepted_node, call_id: Some(call_id), response } if accepted_node==node && call_id==&request_id.to_string() && response==&resolution.response))
        {
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

fn record_response_seq(
    record: Option<&surge_core::run_state::RecoveredGateDecision>,
) -> Result<u64, StageError> {
    record
        .and_then(|record| record.resolved_seq)
        .ok_or_else(|| {
            StageError::RecoveryRequired("restored gate answer lacks its durable receipt".into())
        })
}

fn render_summary(
    template: &surge_core::human_gate_config::SummaryTemplate,
    _memory: &RunMemory,
) -> String {
    // M5 rendering: just title + body, no template substitution. Future M6
    // adds template var resolution against memory.artifacts.
    format!("{}\n\n{}", template.title, template.body)
}

/// Build the JSON schema for human gate response options.
///
/// When `allow_freetext` is `false`, the `outcome` field is restricted to the
/// declared enum values. When `true`, the `enum` constraint is omitted so the
/// operator can supply any string (e.g. a free-text approval note).
fn build_options_schema(
    options: &[surge_core::human_gate_config::ApprovalOption],
    allow_freetext: bool,
) -> serde_json::Value {
    let outcomes: Vec<&str> = options.iter().map(|o| o.outcome.as_ref()).collect();
    if allow_freetext {
        // No enum constraint: any string is accepted as outcome.
        serde_json::json!({
            "type": "object",
            "properties": {
                "outcome": { "type": "string" },
                "comment": { "type": "string" },
            },
            "required": ["outcome"],
        })
    } else {
        serde_json::json!({
            "type": "object",
            "properties": {
                "outcome": {
                    "type": "string",
                    "enum": outcomes,
                },
                "comment": { "type": "string" },
            },
            "required": ["outcome"],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use surge_core::approvals::ApprovalChannel;
    use surge_core::human_gate_config::{ApprovalOption, OptionStyle, SummaryTemplate};
    use surge_core::run_event::BootstrapStage;
    use surge_persistence::runs::Storage;

    fn minimal_gate_config(timeout: Option<u32>, on_timeout: TimeoutAction) -> HumanGateConfig {
        HumanGateConfig {
            delivery_channels: vec![ApprovalChannel::Telegram {
                chat_id_ref: "$DEFAULT".into(),
            }],
            timeout_seconds: timeout,
            on_timeout,
            summary: SummaryTemplate {
                title: "Approve?".into(),
                body: "Do it?".into(),
                show_artifacts: vec![],
            },
            options: vec![
                ApprovalOption {
                    outcome: OutcomeKey::try_from("approve").unwrap(),
                    label: "Approve".into(),
                    style: OptionStyle::Primary,
                },
                ApprovalOption {
                    outcome: OutcomeKey::try_from("reject").unwrap(),
                    label: "Reject".into(),
                    style: OptionStyle::Danger,
                },
            ],
            allow_freetext: false,
            mode: HumanGateMode::default(),
        }
    }

    fn bootstrap_gate_config(stage: BootstrapStage) -> HumanGateConfig {
        HumanGateConfig {
            delivery_channels: vec![ApprovalChannel::Telegram {
                chat_id_ref: "$DEFAULT".into(),
            }],
            timeout_seconds: Some(60),
            on_timeout: TimeoutAction::Reject,
            summary: SummaryTemplate {
                title: "Approve bootstrap stage?".into(),
                body: "Review the agent output.".into(),
                show_artifacts: vec![],
            },
            options: vec![
                ApprovalOption {
                    outcome: OutcomeKey::try_from("approve").unwrap(),
                    label: "Approve".into(),
                    style: OptionStyle::Primary,
                },
                ApprovalOption {
                    outcome: OutcomeKey::try_from("edit").unwrap(),
                    label: "Edit".into(),
                    style: OptionStyle::Warn,
                },
                ApprovalOption {
                    outcome: OutcomeKey::try_from("reject").unwrap(),
                    label: "Reject".into(),
                    style: OptionStyle::Danger,
                },
            ],
            allow_freetext: true,
            mode: HumanGateMode::Bootstrap { stage },
        }
    }

    async fn collect_payload_kinds(
        storage: &std::sync::Arc<Storage>,
        run_id: surge_core::id::RunId,
    ) -> Vec<&'static str> {
        let reader = storage.open_run_reader(run_id).await.unwrap();
        let events = reader
            .read_events(
                surge_persistence::runs::EventSeq(0)..surge_persistence::runs::EventSeq(64),
            )
            .await
            .unwrap();
        events
            .into_iter()
            .map(|re| re.payload.payload.discriminant_str())
            .collect()
    }

    async fn start_test_gate_occurrence(
        writer: &RunWriter,
        memory: &mut RunMemory,
        node: &NodeKey,
        edit_loop_cap: u32,
    ) {
        let seq = writer
            .append_event(VersionedEventPayload::new(EventPayload::StageEntered {
                node: node.clone(),
                attempt: 1,
            }))
            .await
            .unwrap()
            .as_u64();
        memory.stage_occurrences.insert(node.clone(), seq);
        memory.bootstrap_edit_loop_cap = Some(edit_loop_cap);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn bootstrap_accepted_effects_rollback_when_outcome_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();
        let run_id = surge_core::id::RunId::new();
        let writer = storage.create_run(run_id, dir.path(), None).await.unwrap();

        let journal = dir
            .path()
            .join("runs")
            .join(run_id.to_string())
            .join("events.sqlite");
        let output = std::process::Command::new("python3").args([
            "-c", "import sqlite3,sys; c=sqlite3.connect(sys.argv[1]); c.execute(\"CREATE TRIGGER fixture_reject_gate_outcome BEFORE INSERT ON events WHEN NEW.kind='OutcomeReported' BEGIN SELECT RAISE(ABORT, 'fixture rejects gate outcome'); END\"); c.commit()",
        ]).arg(journal).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let cfg = bootstrap_gate_config(BootstrapStage::Description);
        let mut mem = RunMemory::default();
        let node = NodeKey::try_from("description_gate").unwrap();
        start_test_gate_occurrence(&writer, &mut mem, &node, 3).await;

        let (tx, rx) = oneshot::channel();
        tx.send(HumanGateResolution {
            committed_seq: None,
            outcome: OutcomeKey::try_from("edit").unwrap(),
            response: serde_json::json!({"outcome": "edit", "comment": "revise this"}),
        })
        .unwrap();

        let outcome = execute_human_gate_stage(HumanGateStageParams {
            request_id: surge_core::id::GateRequestId::new(),
            node: &node,
            gate_config: &cfg,
            writer: &writer,
            run_memory: &mem,
            cancel: &tokio_util::sync::CancellationToken::new(),
            resolution_rx: Some(rx),
            default_timeout: Duration::from_secs(60),
            bootstrap_edit_loop_cap: 3,
        })
        .await;
        assert!(
            matches!(outcome, Err(StageError::Storage(_))),
            "unexpected gate failure: {outcome:?}"
        );

        let kinds = collect_payload_kinds(&storage, run_id).await;
        assert!(
            kinds.contains(&"HumanInputResolved"),
            "accepted operator answer remains durable"
        );
        assert!(
            !kinds.contains(&"BootstrapApprovalDecided"),
            "rejected required outcome must roll back bootstrap decision: {kinds:?}"
        );
        assert!(
            !kinds.contains(&"BootstrapEditRequested"),
            "rejected required outcome must not increment an edit cycle: {kinds:?}"
        );
        assert!(!kinds.contains(&"OutcomeReported"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn precancelled_gate_does_not_invent_a_decision_or_timeout() {
        for with_receiver in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let storage = Storage::open(dir.path()).await.unwrap();
            let id = surge_core::id::RunId::new();
            let writer = storage.create_run(id, dir.path(), None).await.unwrap();
            let cfg = bootstrap_gate_config(BootstrapStage::Flow);
            let node = NodeKey::try_from("gate").unwrap();
            let memory = RunMemory {
                bootstrap_edit_loop_cap: Some(3),
                ..RunMemory::default()
            };
            let cancel = tokio_util::sync::CancellationToken::new();
            cancel.cancel();
            let (tx, rx) = oneshot::channel();
            tx.send(HumanGateResolution {
                committed_seq: None,
                outcome: "approve".try_into().unwrap(),
                response: serde_json::json!({"outcome": "approve"}),
            })
            .unwrap();
            let result = tokio::time::timeout(
                Duration::from_secs(2),
                execute_human_gate_stage(HumanGateStageParams {
                    request_id: surge_core::id::GateRequestId::new(),
                    node: &node,
                    gate_config: &cfg,
                    writer: &writer,
                    run_memory: &memory,
                    resolution_rx: with_receiver.then_some(rx),
                    cancel: &cancel,
                    default_timeout: Duration::from_secs(60),
                    bootstrap_edit_loop_cap: 3,
                }),
            )
            .await
            .unwrap();
            assert!(matches!(result, Err(StageError::Cancelled)));
            assert_eq!(
                collect_payload_kinds(&storage, id).await,
                ["BootstrapApprovalRequested", "HumanInputRequested"]
            );
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn selected_decision_finishes_persistence_despite_later_cancellation() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();
        let id = surge_core::id::RunId::new();
        let writer = storage.create_run(id, dir.path(), None).await.unwrap();
        let cfg = bootstrap_gate_config(BootstrapStage::Flow);
        let node = NodeKey::try_from("gate").unwrap();
        let mut memory = RunMemory::default();
        start_test_gate_occurrence(&writer, &mut memory, &node, 3).await;
        let cancel = tokio_util::sync::CancellationToken::new();
        let (tx, rx) = oneshot::channel();
        tx.send(HumanGateResolution {
            committed_seq: None,
            outcome: "edit".try_into().unwrap(),
            response: serde_json::json!({"outcome": "edit", "comment": "revise"}),
        })
        .unwrap();
        let stage = execute_human_gate_stage(HumanGateStageParams {
            request_id: surge_core::id::GateRequestId::new(),
            node: &node,
            gate_config: &cfg,
            writer: &writer,
            run_memory: &memory,
            resolution_rx: Some(rx),
            cancel: &cancel,
            default_timeout: Duration::from_secs(60),
            bootstrap_edit_loop_cap: 3,
        });
        let cancellation = async {
            loop {
                if collect_payload_kinds(&storage, id)
                    .await
                    .contains(&"HumanInputResolved")
                {
                    cancel.cancel();
                    break;
                }
                tokio::task::yield_now().await;
            }
        };
        let (outcome, ()) = tokio::time::timeout(Duration::from_secs(3), async {
            tokio::join!(stage, cancellation)
        })
        .await
        .unwrap();
        assert_eq!(outcome.unwrap().as_str(), "edit");
        assert_eq!(
            collect_payload_kinds(&storage, id).await,
            [
                "StageEntered",
                "BootstrapApprovalRequested",
                "HumanInputRequested",
                "HumanInputResolved",
                "BootstrapApprovalDecided",
                "BootstrapEditRequested",
                "OutcomeReported",
                "GateStageOutcomeCommitted",
            ]
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn timeout_with_reject_returns_rejected_error() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();
        let writer = storage
            .create_run(surge_core::id::RunId::new(), dir.path(), None)
            .await
            .unwrap();

        let cfg = minimal_gate_config(Some(0), TimeoutAction::Reject);
        let mut mem = RunMemory::default();
        let node = NodeKey::try_from("approve_plan").unwrap();
        start_test_gate_occurrence(&writer, &mut mem, &node, 3).await;

        let result = execute_human_gate_stage(HumanGateStageParams {
            request_id: surge_core::id::GateRequestId::new(),
            node: &node,
            gate_config: &cfg,
            writer: &writer,
            run_memory: &mem,
            cancel: &tokio_util::sync::CancellationToken::new(),
            resolution_rx: None,
            default_timeout: Duration::from_millis(10),
            bootstrap_edit_loop_cap: 3,
        })
        .await;

        assert!(matches!(result, Err(StageError::HumanGateRejected)));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn resolution_returns_outcome() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();
        let writer = storage
            .create_run(surge_core::id::RunId::new(), dir.path(), None)
            .await
            .unwrap();

        let cfg = minimal_gate_config(Some(60), TimeoutAction::Reject);
        let mut mem = RunMemory::default();
        let node = NodeKey::try_from("approve_plan").unwrap();
        start_test_gate_occurrence(&writer, &mut mem, &node, 3).await;

        let (tx, rx) = oneshot::channel();
        tx.send(HumanGateResolution {
            committed_seq: None,
            outcome: OutcomeKey::try_from("approve").unwrap(),
            response: serde_json::json!({"outcome": "approve"}),
        })
        .unwrap();

        let outcome = execute_human_gate_stage(HumanGateStageParams {
            request_id: surge_core::id::GateRequestId::new(),
            node: &node,
            gate_config: &cfg,
            writer: &writer,
            run_memory: &mem,
            cancel: &tokio_util::sync::CancellationToken::new(),
            resolution_rx: Some(rx),
            default_timeout: Duration::from_secs(60),
            bootstrap_edit_loop_cap: 3,
        })
        .await
        .unwrap();
        assert_eq!(outcome.as_ref(), "approve");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn bootstrap_mode_approve_emits_approval_event_pair() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();
        let run_id = surge_core::id::RunId::new();
        let writer = storage.create_run(run_id, dir.path(), None).await.unwrap();

        let cfg = bootstrap_gate_config(BootstrapStage::Description);
        let mut mem = RunMemory::default();
        let node = NodeKey::try_from("description_gate").unwrap();
        start_test_gate_occurrence(&writer, &mut mem, &node, 3).await;

        let (tx, rx) = oneshot::channel();
        tx.send(HumanGateResolution {
            committed_seq: None,
            outcome: OutcomeKey::try_from("approve").unwrap(),
            response: serde_json::json!({"outcome": "approve", "comment": "looks good"}),
        })
        .unwrap();

        let outcome = execute_human_gate_stage(HumanGateStageParams {
            request_id: surge_core::id::GateRequestId::new(),
            node: &node,
            gate_config: &cfg,
            writer: &writer,
            run_memory: &mem,
            cancel: &tokio_util::sync::CancellationToken::new(),
            resolution_rx: Some(rx),
            default_timeout: Duration::from_secs(60),
            bootstrap_edit_loop_cap: 3,
        })
        .await
        .unwrap();
        assert_eq!(outcome.as_ref(), "approve");

        let kinds = collect_payload_kinds(&storage, run_id).await;
        // Order required: BootstrapApprovalRequested → HumanInputRequested →
        // HumanInputResolved → BootstrapApprovalDecided → OutcomeReported.
        assert_eq!(
            kinds,
            vec![
                "StageEntered",
                "BootstrapApprovalRequested",
                "HumanInputRequested",
                "HumanInputResolved",
                "BootstrapApprovalDecided",
                "OutcomeReported",
                "GateStageOutcomeCommitted",
            ],
        );
    }

    /// A plan review outlives the generic run default: the gate keeps
    /// waiting and the operator's later approval still lands.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn bootstrap_gate_without_timeout_outlives_the_run_default() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();
        let run_id = surge_core::id::RunId::new();
        let writer = storage.create_run(run_id, dir.path(), None).await.unwrap();

        // Real bootstrap gates carry no timeout of their own.
        let mut cfg = bootstrap_gate_config(BootstrapStage::Roadmap);
        cfg.timeout_seconds = None;
        let mut mem = RunMemory::default();
        let node = NodeKey::try_from("roadmap_gate").unwrap();
        start_test_gate_occurrence(&writer, &mut mem, &node, 3).await;

        let (tx, rx) = oneshot::channel();
        tokio::spawn(async move {
            // Well past the 10ms run default.
            tokio::time::sleep(Duration::from_millis(300)).await;
            let _ = tx.send(HumanGateResolution {
                committed_seq: None,
                outcome: OutcomeKey::try_from("approve").unwrap(),
                response: serde_json::json!({"outcome": "approve"}),
            });
        });

        let outcome = execute_human_gate_stage(HumanGateStageParams {
            request_id: surge_core::id::GateRequestId::new(),
            node: &node,
            gate_config: &cfg,
            writer: &writer,
            run_memory: &mem,
            cancel: &tokio_util::sync::CancellationToken::new(),
            resolution_rx: Some(rx),
            default_timeout: Duration::from_millis(10),
            bootstrap_edit_loop_cap: 3,
        })
        .await
        .unwrap();
        assert_eq!(outcome.as_ref(), "approve");
        let kinds = collect_payload_kinds(&storage, run_id).await;
        assert!(!kinds.contains(&"HumanInputTimedOut"), "{kinds:?}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn bootstrap_mode_edit_emits_edit_requested_with_feedback() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();
        let run_id = surge_core::id::RunId::new();
        let writer = storage.create_run(run_id, dir.path(), None).await.unwrap();

        let cfg = bootstrap_gate_config(BootstrapStage::Roadmap);
        let mut mem = RunMemory::default();
        let node = NodeKey::try_from("roadmap_gate").unwrap();
        start_test_gate_occurrence(&writer, &mut mem, &node, 3).await;

        let (tx, rx) = oneshot::channel();
        let feedback = "tighten the M3 milestone scope";
        tx.send(HumanGateResolution {
            committed_seq: None,
            outcome: OutcomeKey::try_from("edit").unwrap(),
            response: serde_json::json!({"outcome": "edit", "comment": feedback}),
        })
        .unwrap();

        let outcome = execute_human_gate_stage(HumanGateStageParams {
            request_id: surge_core::id::GateRequestId::new(),
            node: &node,
            gate_config: &cfg,
            writer: &writer,
            run_memory: &mem,
            cancel: &tokio_util::sync::CancellationToken::new(),
            resolution_rx: Some(rx),
            default_timeout: Duration::from_secs(60),
            bootstrap_edit_loop_cap: 3,
        })
        .await
        .unwrap();
        assert_eq!(outcome.as_ref(), "edit");

        let reader = storage.open_run_reader(run_id).await.unwrap();
        let events = reader
            .read_events(
                surge_persistence::runs::EventSeq(0)..surge_persistence::runs::EventSeq(64),
            )
            .await
            .unwrap();
        let kinds: Vec<&str> = events
            .iter()
            .map(|re| re.payload.payload.discriminant_str())
            .collect();
        assert_eq!(
            kinds,
            vec![
                "StageEntered",
                "BootstrapApprovalRequested",
                "HumanInputRequested",
                "HumanInputResolved",
                "BootstrapApprovalDecided",
                "BootstrapEditRequested",
                "OutcomeReported",
                "GateStageOutcomeCommitted",
            ],
        );

        // The BootstrapEditRequested event must carry the operator's
        // freeform feedback verbatim so EditFeedback bindings (Task 8)
        // can resolve it for the next agent stage.
        let edit_event = events
            .iter()
            .find_map(|re| match &re.payload.payload {
                EventPayload::BootstrapEditRequested { stage, feedback } => {
                    Some((*stage, feedback.clone()))
                },
                _ => None,
            })
            .expect("BootstrapEditRequested missing");
        assert_eq!(edit_event.0, BootstrapStage::Roadmap);
        assert_eq!(edit_event.1, feedback);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn bootstrap_mode_reject_returns_rejected_error_with_decided_event() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();
        let run_id = surge_core::id::RunId::new();
        let writer = storage.create_run(run_id, dir.path(), None).await.unwrap();

        let cfg = bootstrap_gate_config(BootstrapStage::Flow);
        let mut mem = RunMemory::default();
        let node = NodeKey::try_from("flow_gate").unwrap();
        start_test_gate_occurrence(&writer, &mut mem, &node, 3).await;

        let (tx, rx) = oneshot::channel();
        tx.send(HumanGateResolution {
            committed_seq: None,
            outcome: OutcomeKey::try_from("reject").unwrap(),
            response: serde_json::json!({"outcome": "reject", "comment": "off-track"}),
        })
        .unwrap();

        let result = execute_human_gate_stage(HumanGateStageParams {
            request_id: surge_core::id::GateRequestId::new(),
            node: &node,
            gate_config: &cfg,
            writer: &writer,
            run_memory: &mem,
            cancel: &tokio_util::sync::CancellationToken::new(),
            resolution_rx: Some(rx),
            default_timeout: Duration::from_secs(60),
            bootstrap_edit_loop_cap: 3,
        })
        .await;
        assert!(matches!(result, Err(StageError::HumanGateRejected)));

        // Reject must still persist Approval → Decided → Outcome before bailing
        // — the bootstrap driver inspects the event log to decide its return code.
        let kinds = collect_payload_kinds(&storage, run_id).await;
        assert_eq!(
            kinds,
            vec![
                "StageEntered",
                "BootstrapApprovalRequested",
                "HumanInputRequested",
                "HumanInputResolved",
                "BootstrapApprovalDecided",
                "OutcomeReported",
                "GateStageOutcomeCommitted",
            ],
        );
    }

    #[test]
    fn bootstrap_decision_mapping_canonical_outcomes() {
        assert_eq!(
            bootstrap_decision_from_outcome(&OutcomeKey::try_from("approve").unwrap()).unwrap(),
            BootstrapDecision::Approve,
        );
        assert_eq!(
            bootstrap_decision_from_outcome(&OutcomeKey::try_from("edit").unwrap()).unwrap(),
            BootstrapDecision::Edit,
        );
        assert_eq!(
            bootstrap_decision_from_outcome(&OutcomeKey::try_from("reject").unwrap()).unwrap(),
            BootstrapDecision::Reject,
        );
        assert!(
            matches!(
                bootstrap_decision_from_outcome(&OutcomeKey::try_from("defer").unwrap()),
                Err(StageError::Internal(_))
            ),
            "non-canonical bootstrap outcomes must fail closed",
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn bootstrap_edit_cycle_below_cap_emits_edit_requested() {
        // Sanity baseline for the cap path: with prior_edits = 0 and cap = 3,
        // an `edit` outcome MUST emit BootstrapEditRequested and return the
        // outcome (no error). Captures the "happy" branch that the cap
        // arithmetic must not break.
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();
        let run_id = surge_core::id::RunId::new();
        let writer = storage.create_run(run_id, dir.path(), None).await.unwrap();

        let cfg = bootstrap_gate_config(BootstrapStage::Roadmap);
        let mut mem = RunMemory::default();
        let node = NodeKey::try_from("roadmap_gate").unwrap();
        start_test_gate_occurrence(&writer, &mut mem, &node, 3).await;

        let (tx, rx) = oneshot::channel();
        tx.send(HumanGateResolution {
            committed_seq: None,
            outcome: OutcomeKey::try_from("edit").unwrap(),
            response: serde_json::json!({"outcome": "edit", "comment": "tighten"}),
        })
        .unwrap();

        let outcome = execute_human_gate_stage(HumanGateStageParams {
            request_id: surge_core::id::GateRequestId::new(),
            node: &node,
            gate_config: &cfg,
            writer: &writer,
            run_memory: &mem,
            cancel: &tokio_util::sync::CancellationToken::new(),
            resolution_rx: Some(rx),
            default_timeout: Duration::from_secs(60),
            bootstrap_edit_loop_cap: 3,
        })
        .await
        .unwrap();
        assert_eq!(outcome.as_ref(), "edit");
        let kinds = collect_payload_kinds(&storage, run_id).await;
        assert!(
            kinds.contains(&"BootstrapEditRequested"),
            "expected BootstrapEditRequested when below the cap, got {kinds:?}",
        );
        assert!(
            !kinds.contains(&"EscalationRequested"),
            "EscalationRequested must NOT appear before the cap is hit, got {kinds:?}",
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn bootstrap_edit_cycle_at_cap_returns_cap_exceeded_with_escalation() {
        // RunMemory carries `bootstrap_edit_counts[Description] = 3` —
        // three prior edit cycles already happened. The fourth `edit`
        // outcome must hit the cap and abort the run with a clear error
        // plus the EscalationRequested event.
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();
        let run_id = surge_core::id::RunId::new();
        let writer = storage.create_run(run_id, dir.path(), None).await.unwrap();

        let cfg = bootstrap_gate_config(BootstrapStage::Description);
        let mut mem = RunMemory::default();
        mem.bootstrap_edit_counts
            .insert(BootstrapStage::Description, 3);
        let node = NodeKey::try_from("description_gate").unwrap();
        start_test_gate_occurrence(&writer, &mut mem, &node, 3).await;

        let (tx, rx) = oneshot::channel();
        tx.send(HumanGateResolution {
            committed_seq: None,
            outcome: OutcomeKey::try_from("edit").unwrap(),
            response: serde_json::json!({"outcome": "edit", "comment": "again"}),
        })
        .unwrap();

        let result = execute_human_gate_stage(HumanGateStageParams {
            request_id: surge_core::id::GateRequestId::new(),
            node: &node,
            gate_config: &cfg,
            writer: &writer,
            run_memory: &mem,
            cancel: &tokio_util::sync::CancellationToken::new(),
            resolution_rx: Some(rx),
            default_timeout: Duration::from_secs(60),
            bootstrap_edit_loop_cap: 3,
        })
        .await;

        match result {
            Err(StageError::EditLoopCapExceeded { stage, cap }) => {
                assert_eq!(stage, BootstrapStage::Description);
                assert_eq!(cap, 3);
            },
            other => panic!("expected EditLoopCapExceeded, got {other:?}"),
        }

        let kinds = collect_payload_kinds(&storage, run_id).await;
        // The cap-exceeded path persists Approval → HumanInput → Decided →
        // Escalation → Outcome (no BootstrapEditRequested — that's the
        // very emission we refused).
        assert_eq!(
            kinds,
            vec![
                "StageEntered",
                "BootstrapApprovalRequested",
                "HumanInputRequested",
                "HumanInputResolved",
                "BootstrapApprovalDecided",
                "EscalationRequested",
                "OutcomeReported",
                "GateStageOutcomeCommitted",
            ],
        );
        assert!(
            !kinds.contains(&"BootstrapEditRequested"),
            "BootstrapEditRequested must NOT be emitted on the cap-exceeded edit attempt",
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn bootstrap_edit_cap_zero_disables_cap_check() {
        // cap = 0 disables the limit — the integration test path used by
        // mock-agent harnesses that need to drive arbitrarily long edit
        // loops. Even with a high prior count, the edit cycle proceeds.
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();
        let run_id = surge_core::id::RunId::new();
        let writer = storage.create_run(run_id, dir.path(), None).await.unwrap();

        let cfg = bootstrap_gate_config(BootstrapStage::Flow);
        let mut mem = RunMemory::default();
        mem.bootstrap_edit_counts.insert(BootstrapStage::Flow, 99);
        let node = NodeKey::try_from("flow_gate").unwrap();
        start_test_gate_occurrence(&writer, &mut mem, &node, 0).await;

        let (tx, rx) = oneshot::channel();
        tx.send(HumanGateResolution {
            committed_seq: None,
            outcome: OutcomeKey::try_from("edit").unwrap(),
            response: serde_json::json!({"outcome": "edit", "comment": "again"}),
        })
        .unwrap();

        let outcome = execute_human_gate_stage(HumanGateStageParams {
            request_id: surge_core::id::GateRequestId::new(),
            node: &node,
            gate_config: &cfg,
            writer: &writer,
            run_memory: &mem,
            cancel: &tokio_util::sync::CancellationToken::new(),
            resolution_rx: Some(rx),
            default_timeout: Duration::from_secs(60),
            bootstrap_edit_loop_cap: 0,
        })
        .await
        .unwrap();
        assert_eq!(outcome.as_ref(), "edit");
        let kinds = collect_payload_kinds(&storage, run_id).await;
        assert!(kinds.contains(&"BootstrapEditRequested"));
        assert!(!kinds.contains(&"EscalationRequested"));
    }
}
