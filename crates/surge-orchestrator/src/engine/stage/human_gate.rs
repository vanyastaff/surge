//! `NodeKind::HumanGate` execution.
//!
//! M5 model: pause the run, emit `HumanInputRequested`, wait for either an
//! external `Engine::resolve_human_input` call or the configured timeout.
//! On timeout, apply `HumanGateConfig::on_timeout` (Reject / Escalate /
//! Continue). M5 treats Escalate as Reject (no escalation channels) and
//! Continue without a default outcome as a configuration error.

use crate::engine::stage::{StageError, StageResult};
use std::time::Duration;
use surge_core::human_gate_config::{HumanGateConfig, TimeoutAction};
use surge_core::keys::{NodeKey, OutcomeKey};
use surge_core::run_event::{EventPayload, VersionedEventPayload};
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
pub async fn execute_human_gate_stage(p: HumanGateStageParams<'_>) -> StageResult {
    let summary = render_summary(&p.gate_config.summary, p.run_memory);
    let timeout = p
        .gate_config
        .timeout_seconds
        .map_or(p.default_timeout, |s| Duration::from_secs(u64::from(s)));

    let schema = build_options_schema(&p.gate_config.options);

    p.writer
        .append_event(VersionedEventPayload::new(EventPayload::HumanInputRequested {
            node: p.node.clone(),
            session: None,
            call_id: None,
            prompt: summary,
            schema: Some(schema),
        }))
        .await
        .map_err(|e| StageError::Storage(e.to_string()))?;

    let outcome = if let Some(rx) = p.resolution_rx {
        tokio::select! {
            resolved = rx => match resolved {
                Ok(res) => {
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
            .append_event(VersionedEventPayload::new(EventPayload::HumanInputTimedOut {
                node: p.node.clone(),
                call_id: None,
                elapsed_seconds: u32::try_from(timeout.as_secs()).unwrap_or(u32::MAX),
            }))
            .await
            .map_err(|e| StageError::Storage(e.to_string()))?;
        match p.gate_config.on_timeout {
            TimeoutAction::Reject | TimeoutAction::Escalate => {
                return Err(StageError::HumanGateRejected);
            }
            TimeoutAction::Continue => {
                // M5 has no default_outcome on HumanGateConfig; documented gap.
                return Err(StageError::HumanGateContinueWithoutDefault);
            }
        }
    };

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

fn render_summary(template: &surge_core::human_gate_config::SummaryTemplate, _memory: &RunMemory) -> String {
    // M5 rendering: just title + body, no template substitution. Future M6
    // adds template var resolution against memory.artifacts.
    format!("{}\n\n{}", template.title, template.body)
}

fn build_options_schema(options: &[surge_core::human_gate_config::ApprovalOption]) -> serde_json::Value {
    let outcomes: Vec<&str> = options.iter().map(|o| o.outcome.as_ref()).collect();
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

#[cfg(test)]
mod tests {
    use super::*;
    use surge_core::approvals::ApprovalChannel;
    use surge_core::human_gate_config::{ApprovalOption, OptionStyle, SummaryTemplate};
    use surge_persistence::runs::Storage;

    fn minimal_gate_config(timeout: Option<u32>, on_timeout: TimeoutAction) -> HumanGateConfig {
        HumanGateConfig {
            delivery_channels: vec![ApprovalChannel::Telegram { chat_id_ref: "$DEFAULT".into() }],
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
        }
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
        })
        .await
        .unwrap();
        assert_eq!(outcome.as_ref(), "approve");
    }
}
