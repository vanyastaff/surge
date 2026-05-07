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
use surge_core::human_gate_config::{HumanGateConfig, HumanGateMode, TimeoutAction};
use surge_core::keys::{NodeKey, OutcomeKey};
use surge_core::run_event::{BootstrapDecision, EventPayload, VersionedEventPayload};
use surge_core::run_state::RunMemory;
use surge_persistence::runs::run_writer::RunWriter;
use tokio::sync::oneshot;

/// Parameters for executing a single `NodeKind::HumanGate` stage.
pub struct HumanGateStageParams<'a> {
    /// Key of the human-gate node being executed.
    pub node: &'a NodeKey,
    /// Gate configuration: delivery channels, timeout, options.
    pub gate_config: &'a HumanGateConfig,
    /// Run writer for persisting `HumanInputRequested` / `HumanInputResolved` events.
    pub writer: &'a RunWriter,
    /// Accumulated run memory (currently unused in M5 rendering).
    pub run_memory: &'a RunMemory,
    /// Receiver fed by `Engine::resolve_human_input`. `None` ⇒ test path (timeout immediately).
    pub resolution_rx: Option<oneshot::Receiver<HumanGateResolution>>,
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
    /// The outcome key chosen by the operator.
    pub outcome: OutcomeKey,
    /// Full JSON response payload (must contain an `"outcome"` field).
    pub response: serde_json::Value,
}

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
#[allow(clippy::too_many_lines)]
pub async fn execute_human_gate_stage(p: HumanGateStageParams<'_>) -> StageResult {
    let summary = render_summary(&p.gate_config.summary, p.run_memory);
    let timeout = p
        .gate_config
        .timeout_seconds
        .map_or(p.default_timeout, |s| Duration::from_secs(u64::from(s)));

    let schema = build_options_schema(&p.gate_config.options, p.gate_config.allow_freetext);

    // Bootstrap dispatch: when the gate guards a bootstrap stage, mirror the
    // generic HumanInputRequested with a BootstrapApprovalRequested so the
    // bootstrap driver / Telegram cockpit / inbox can render a stage-aware
    // card. The same event log carries both for downstream observers.
    let bootstrap_stage = match &p.gate_config.mode {
        HumanGateMode::Generic => None,
        HumanGateMode::Bootstrap { stage } => {
            tracing::debug!(
                target: "engine::bootstrap::stage",
                node = %p.node,
                stage = ?stage,
                "human gate dispatched in Bootstrap mode"
            );
            let channel = p
                .gate_config
                .delivery_channels
                .first()
                .cloned()
                .unwrap_or(ApprovalChannel::Desktop {
                    duration: surge_core::approvals::ApprovalDuration::Transient,
                });
            p.writer
                .append_event(VersionedEventPayload::new(
                    EventPayload::BootstrapApprovalRequested {
                        stage: *stage,
                        channel,
                    },
                ))
                .await
                .map_err(|e| StageError::Storage(e.to_string()))?;
            Some(*stage)
        },
    };

    p.writer
        .append_event(VersionedEventPayload::new(
            EventPayload::HumanInputRequested {
                node: p.node.clone(),
                session: None,
                call_id: None,
                prompt: summary,
                schema: Some(schema),
            },
        ))
        .await
        .map_err(|e| StageError::Storage(e.to_string()))?;

    // Holds the operator's freeform `comment` when present; carried into the
    // BootstrapApprovalDecided / BootstrapEditRequested events below.
    let mut decided_comment: Option<String> = None;

    let outcome = if let Some(rx) = p.resolution_rx {
        tokio::select! {
            resolved = rx => match resolved {
                Ok(res) => {
                    decided_comment = res
                        .response
                        .get("comment")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned);
                    p.writer
                        .append_event(VersionedEventPayload::new(EventPayload::HumanInputResolved {
                            node: p.node.clone(),
                            call_id: None,
                            response: res.response.clone(),
                        }))
                        .await
                        .map_err(|e| StageError::Storage(e.to_string()))?;
                    Some(res.outcome)
                }
                Err(_) => None,
            },
            () = tokio::time::sleep(timeout) => None,
        }
    } else {
        tokio::time::sleep(timeout).await;
        None
    };

    let Some(final_outcome) = outcome else {
        p.writer
            .append_event(VersionedEventPayload::new(
                EventPayload::HumanInputTimedOut {
                    node: p.node.clone(),
                    call_id: None,
                    elapsed_seconds: u32::try_from(timeout.as_secs()).unwrap_or(u32::MAX),
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

    // Bootstrap mode: mirror the operator's decision into stage-aware events
    // BEFORE the OutcomeReported is appended, so the event log preserves the
    // sequence Approval → Decided (+ EditRequested for `edit`) → Outcome.
    // A Bootstrap-mode gate also rejects the run on `reject`: the bootstrap
    // driver treats this as a terminal abort signal.
    if let Some(stage) = bootstrap_stage {
        let decision = bootstrap_decision_from_outcome(&final_outcome);
        tracing::info!(
            target: "engine::bootstrap::stage",
            node = %p.node,
            stage = ?stage,
            decision = ?decision,
            "bootstrap decision emitted"
        );
        p.writer
            .append_event(VersionedEventPayload::new(
                EventPayload::BootstrapApprovalDecided {
                    stage,
                    decision,
                    comment: decided_comment.clone(),
                },
            ))
            .await
            .map_err(|e| StageError::Storage(e.to_string()))?;
        if decision == BootstrapDecision::Edit {
            // Edit-loop cap: if the operator has already requested
            // `cap` edits for this stage, the engine bails out instead
            // of routing back to the agent. `bootstrap_edit_counts`
            // reflects the count of PRIOR edits (the not-yet-emitted
            // BootstrapEditRequested would push it to count + 1), so
            // the comparison is `>=`. Cap == 0 disables the limit.
            let cap = p.bootstrap_edit_loop_cap;
            let prior_edits = p
                .run_memory
                .bootstrap_edit_counts
                .get(&stage)
                .copied()
                .unwrap_or(0);
            if cap > 0 && prior_edits >= cap {
                tracing::error!(
                    target: "engine::bootstrap::stage",
                    node = %p.node,
                    stage = ?stage,
                    cap,
                    prior_edits,
                    "EditLoopCapExceeded — bootstrap edit-loop cap exceeded"
                );
                let reason = format!(
                    "bootstrap edit-loop cap exceeded for stage {stage:?} \
                     (cap = {cap}, prior_edits = {prior_edits})"
                );
                p.writer
                    .append_event(VersionedEventPayload::new(
                        EventPayload::EscalationRequested {
                            stage: Some(stage),
                            reason: reason.clone(),
                        },
                    ))
                    .await
                    .map_err(|e| StageError::Storage(e.to_string()))?;
                p.writer
                    .append_event(VersionedEventPayload::new(EventPayload::OutcomeReported {
                        node: p.node.clone(),
                        outcome: final_outcome.clone(),
                        summary: reason,
                    }))
                    .await
                    .map_err(|e| StageError::Storage(e.to_string()))?;
                return Err(StageError::EditLoopCapExceeded { stage, cap });
            }
            // Approaching-cap WARN one cycle before the limit so operators
            // see something coming in the logs.
            if cap > 0 && prior_edits + 1 == cap {
                tracing::warn!(
                    target: "engine::bootstrap::stage",
                    node = %p.node,
                    stage = ?stage,
                    cap,
                    prior_edits,
                    "approaching bootstrap edit-loop cap"
                );
            } else {
                tracing::info!(
                    target: "engine::bootstrap::stage",
                    node = %p.node,
                    stage = ?stage,
                    attempt = prior_edits + 1,
                    "bootstrap edit cycle"
                );
            }
            // Feedback prefers the operator's freeform `comment`; falls back
            // to the empty string so downstream EditFeedback bindings still
            // resolve (the absence of feedback is itself a signal).
            let feedback = decided_comment.unwrap_or_default();
            p.writer
                .append_event(VersionedEventPayload::new(
                    EventPayload::BootstrapEditRequested { stage, feedback },
                ))
                .await
                .map_err(|e| StageError::Storage(e.to_string()))?;
        } else if decision == BootstrapDecision::Reject {
            // Persist the OutcomeReported for replay symmetry, then signal
            // a terminal failure to the engine. The bootstrap driver maps
            // this to a `BootstrapError::Rejected` for the outer caller.
            p.writer
                .append_event(VersionedEventPayload::new(EventPayload::OutcomeReported {
                    node: p.node.clone(),
                    outcome: final_outcome.clone(),
                    summary: "bootstrap rejected".into(),
                }))
                .await
                .map_err(|e| StageError::Storage(e.to_string()))?;
            return Err(StageError::HumanGateRejected);
        }
    }

    p.writer
        .append_event(VersionedEventPayload::new(EventPayload::OutcomeReported {
            node: p.node.clone(),
            outcome: final_outcome.clone(),
            summary: "human gate decision".into(),
        }))
        .await
        .map_err(|e| StageError::Storage(e.to_string()))?;

    Ok(final_outcome)
}

/// Translate the operator's `OutcomeKey` choice into a `BootstrapDecision`.
///
/// Recognized canonical keys are `approve`, `edit`, and `reject` (`snake_case`
/// to match the gate's declared `ApprovalOption.outcome` values used by the
/// bundled bootstrap profiles in Task 12). Any other outcome is treated as
/// `Approve` — an opt-in extension for future archetypes that introduce
/// non-canonical approval keys (e.g., a `defer` outcome).
fn bootstrap_decision_from_outcome(outcome: &OutcomeKey) -> BootstrapDecision {
    match outcome.as_ref() {
        "edit" => BootstrapDecision::Edit,
        "reject" => BootstrapDecision::Reject,
        _ => BootstrapDecision::Approve,
    }
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
            .read_events(surge_persistence::runs::EventSeq(0)..surge_persistence::runs::EventSeq(64))
            .await
            .unwrap();
        events
            .into_iter()
            .map(|re| re.payload.payload.discriminant_str())
            .collect()
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
        let mem = RunMemory::default();
        let node = NodeKey::try_from("approve_plan").unwrap();

        let result = execute_human_gate_stage(HumanGateStageParams {
            node: &node,
            gate_config: &cfg,
            writer: &writer,
            run_memory: &mem,
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
        let mem = RunMemory::default();
        let node = NodeKey::try_from("approve_plan").unwrap();

        let (tx, rx) = oneshot::channel();
        tx.send(HumanGateResolution {
            outcome: OutcomeKey::try_from("approve").unwrap(),
            response: serde_json::json!({"outcome": "approve"}),
        })
        .unwrap();

        let outcome = execute_human_gate_stage(HumanGateStageParams {
            node: &node,
            gate_config: &cfg,
            writer: &writer,
            run_memory: &mem,
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
        let mem = RunMemory::default();
        let node = NodeKey::try_from("description_gate").unwrap();

        let (tx, rx) = oneshot::channel();
        tx.send(HumanGateResolution {
            outcome: OutcomeKey::try_from("approve").unwrap(),
            response: serde_json::json!({"outcome": "approve", "comment": "looks good"}),
        })
        .unwrap();

        let outcome = execute_human_gate_stage(HumanGateStageParams {
            node: &node,
            gate_config: &cfg,
            writer: &writer,
            run_memory: &mem,
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
                "BootstrapApprovalRequested",
                "HumanInputRequested",
                "HumanInputResolved",
                "BootstrapApprovalDecided",
                "OutcomeReported",
            ],
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn bootstrap_mode_edit_emits_edit_requested_with_feedback() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();
        let run_id = surge_core::id::RunId::new();
        let writer = storage.create_run(run_id, dir.path(), None).await.unwrap();

        let cfg = bootstrap_gate_config(BootstrapStage::Roadmap);
        let mem = RunMemory::default();
        let node = NodeKey::try_from("roadmap_gate").unwrap();

        let (tx, rx) = oneshot::channel();
        let feedback = "tighten the M3 milestone scope";
        tx.send(HumanGateResolution {
            outcome: OutcomeKey::try_from("edit").unwrap(),
            response: serde_json::json!({"outcome": "edit", "comment": feedback}),
        })
        .unwrap();

        let outcome = execute_human_gate_stage(HumanGateStageParams {
            node: &node,
            gate_config: &cfg,
            writer: &writer,
            run_memory: &mem,
            resolution_rx: Some(rx),
            default_timeout: Duration::from_secs(60),
            bootstrap_edit_loop_cap: 3,
        })
        .await
        .unwrap();
        assert_eq!(outcome.as_ref(), "edit");

        let reader = storage.open_run_reader(run_id).await.unwrap();
        let events = reader
            .read_events(surge_persistence::runs::EventSeq(0)..surge_persistence::runs::EventSeq(64))
            .await
            .unwrap();
        let kinds: Vec<&str> = events.iter().map(|re| re.payload.payload.discriminant_str()).collect();
        assert_eq!(
            kinds,
            vec![
                "BootstrapApprovalRequested",
                "HumanInputRequested",
                "HumanInputResolved",
                "BootstrapApprovalDecided",
                "BootstrapEditRequested",
                "OutcomeReported",
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
        let mem = RunMemory::default();
        let node = NodeKey::try_from("flow_gate").unwrap();

        let (tx, rx) = oneshot::channel();
        tx.send(HumanGateResolution {
            outcome: OutcomeKey::try_from("reject").unwrap(),
            response: serde_json::json!({"outcome": "reject", "comment": "off-track"}),
        })
        .unwrap();

        let result = execute_human_gate_stage(HumanGateStageParams {
            node: &node,
            gate_config: &cfg,
            writer: &writer,
            run_memory: &mem,
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
                "BootstrapApprovalRequested",
                "HumanInputRequested",
                "HumanInputResolved",
                "BootstrapApprovalDecided",
                "OutcomeReported",
            ],
        );
    }

    #[test]
    fn bootstrap_decision_mapping_canonical_outcomes() {
        assert_eq!(
            bootstrap_decision_from_outcome(&OutcomeKey::try_from("approve").unwrap()),
            BootstrapDecision::Approve,
        );
        assert_eq!(
            bootstrap_decision_from_outcome(&OutcomeKey::try_from("edit").unwrap()),
            BootstrapDecision::Edit,
        );
        assert_eq!(
            bootstrap_decision_from_outcome(&OutcomeKey::try_from("reject").unwrap()),
            BootstrapDecision::Reject,
        );
        // Non-canonical outcomes fall back to Approve so future archetypes
        // that introduce extra approval keys do not silently break.
        assert_eq!(
            bootstrap_decision_from_outcome(&OutcomeKey::try_from("defer").unwrap()),
            BootstrapDecision::Approve,
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
        let mem = RunMemory::default();
        let node = NodeKey::try_from("roadmap_gate").unwrap();

        let (tx, rx) = oneshot::channel();
        tx.send(HumanGateResolution {
            outcome: OutcomeKey::try_from("edit").unwrap(),
            response: serde_json::json!({"outcome": "edit", "comment": "tighten"}),
        })
        .unwrap();

        let outcome = execute_human_gate_stage(HumanGateStageParams {
            node: &node,
            gate_config: &cfg,
            writer: &writer,
            run_memory: &mem,
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
        mem.bootstrap_edit_counts.insert(BootstrapStage::Description, 3);
        let node = NodeKey::try_from("description_gate").unwrap();

        let (tx, rx) = oneshot::channel();
        tx.send(HumanGateResolution {
            outcome: OutcomeKey::try_from("edit").unwrap(),
            response: serde_json::json!({"outcome": "edit", "comment": "again"}),
        })
        .unwrap();

        let result = execute_human_gate_stage(HumanGateStageParams {
            node: &node,
            gate_config: &cfg,
            writer: &writer,
            run_memory: &mem,
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
                "BootstrapApprovalRequested",
                "HumanInputRequested",
                "HumanInputResolved",
                "BootstrapApprovalDecided",
                "EscalationRequested",
                "OutcomeReported",
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

        let (tx, rx) = oneshot::channel();
        tx.send(HumanGateResolution {
            outcome: OutcomeKey::try_from("edit").unwrap(),
            response: serde_json::json!({"outcome": "edit", "comment": "again"}),
        })
        .unwrap();

        let outcome = execute_human_gate_stage(HumanGateStageParams {
            node: &node,
            gate_config: &cfg,
            writer: &writer,
            run_memory: &mem,
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
