//! `NodeKind::Notify` — real channel delivery via `surge-notify`.

use crate::engine::stage::{StageError, StageResult};
use std::sync::Arc;
use surge_core::keys::{NodeKey, OutcomeKey};
use surge_core::node::OutcomeDecl;
use surge_core::notify_config::{NotifyConfig, NotifyFailureAction};
use surge_core::run_event::{EventPayload, VersionedEventPayload};
use surge_core::run_state::RunMemory;
use surge_notify::{NotifyDeliverer, NotifyDeliveryContext, RenderContext, render};
use surge_persistence::runs::run_writer::RunWriter;

/// Parameters for executing a Notify stage.
pub struct NotifyStageParams<'a> {
    /// `NodeKey` of the Notify node.
    pub node: &'a NodeKey,
    /// Notify configuration from the node.
    pub notify_config: &'a NotifyConfig,
    /// Declared outcomes — used to decide whether `undeliverable` is configured.
    pub declared_outcomes: &'a [OutcomeDecl],
    /// Run writer for events.
    pub writer: &'a RunWriter,
    /// Run memory for template rendering.
    pub run_memory: &'a RunMemory,
    /// Run id (used in delivery context + template).
    pub run_id: surge_core::id::RunId,
    /// Pluggable notification deliverer.
    pub deliverer: Arc<dyn NotifyDeliverer>,
}

/// Execute a Notify stage: render template → deliver via channel →
/// emit `NotifyDelivered` + `OutcomeReported` → return outcome.
pub async fn execute_notify_stage(p: NotifyStageParams<'_>) -> StageResult {
    let render_ctx = RenderContext {
        run_id: p.run_id,
        node: p.node,
        run_memory: p.run_memory,
    };
    let rendered = render(&p.notify_config.template, &render_ctx)
        .map_err(|e| StageError::NotifyDelivery(format!("render: {e}")))?;

    let delivery_ctx = NotifyDeliveryContext {
        run_id: p.run_id,
        node: p.node,
    };

    let result = p
        .deliverer
        .deliver(&delivery_ctx, &p.notify_config.channel, &rendered)
        .await;

    let outcome = compute_outcome(&result, p.notify_config.on_failure, p.declared_outcomes)?;

    p.writer
        .append_event(VersionedEventPayload::new(EventPayload::NotifyDelivered {
            node: p.node.clone(),
            channel_kind: p.notify_config.channel.kind(),
            success: result.is_ok(),
            error: result.as_ref().err().map(ToString::to_string),
        }))
        .await
        .map_err(|e| StageError::Storage(e.to_string()))?;

    let summary = match &result {
        Ok(()) => "delivered".to_string(),
        Err(e) => format!("delivery error: {e}"),
    };
    p.writer
        .append_event(VersionedEventPayload::new(EventPayload::OutcomeReported {
            node: p.node.clone(),
            outcome: outcome.clone(),
            summary,
        }))
        .await
        .map_err(|e| StageError::Storage(e.to_string()))?;

    Ok(outcome)
}

/// Compute the routing outcome based on delivery result and `on_failure` policy.
pub(crate) fn compute_outcome(
    result: &Result<(), surge_notify::NotifyError>,
    on_failure: NotifyFailureAction,
    declared: &[OutcomeDecl],
) -> Result<OutcomeKey, StageError> {
    let delivered = OutcomeKey::try_from("delivered")
        .map_err(|e| StageError::Internal(format!("'delivered' outcome key: {e}")))?;
    match (result, on_failure) {
        (Ok(()), _) | (Err(_), NotifyFailureAction::Continue) => Ok(delivered),
        (Err(e), NotifyFailureAction::Fail) => {
            let undeliverable = OutcomeKey::try_from("undeliverable")
                .map_err(|e| StageError::Internal(format!("'undeliverable' outcome key: {e}")))?;
            if declared.iter().any(|o| o.id == undeliverable) {
                Ok(undeliverable)
            } else {
                Err(StageError::NotifyDelivery(e.to_string()))
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use surge_core::edge::EdgeKind;

    fn outcome_decl(id: &str) -> OutcomeDecl {
        OutcomeDecl {
            id: OutcomeKey::try_from(id).unwrap(),
            description: id.into(),
            edge_kind_hint: EdgeKind::Forward,
            is_terminal: false,
        }
    }

    #[test]
    fn ok_returns_delivered() {
        let r: Result<(), surge_notify::NotifyError> = Ok(());
        let declared = vec![outcome_decl("delivered")];
        let outcome = compute_outcome(&r, NotifyFailureAction::Continue, &declared).unwrap();
        assert_eq!(outcome.as_ref(), "delivered");
    }

    #[test]
    fn err_continue_returns_delivered() {
        let r: Result<(), _> = Err(surge_notify::NotifyError::ChannelNotConfigured);
        let declared = vec![outcome_decl("delivered")];
        let outcome = compute_outcome(&r, NotifyFailureAction::Continue, &declared).unwrap();
        assert_eq!(outcome.as_ref(), "delivered");
    }

    #[test]
    fn err_fail_with_undeliverable_returns_undeliverable() {
        let r: Result<(), _> = Err(surge_notify::NotifyError::ChannelNotConfigured);
        let declared = vec![outcome_decl("delivered"), outcome_decl("undeliverable")];
        let outcome = compute_outcome(&r, NotifyFailureAction::Fail, &declared).unwrap();
        assert_eq!(outcome.as_ref(), "undeliverable");
    }

    #[test]
    fn err_fail_without_undeliverable_errors() {
        let r: Result<(), _> = Err(surge_notify::NotifyError::ChannelNotConfigured);
        let declared = vec![outcome_decl("delivered")];
        let result = compute_outcome(&r, NotifyFailureAction::Fail, &declared);
        assert!(matches!(result, Err(StageError::NotifyDelivery(_))));
    }
}
