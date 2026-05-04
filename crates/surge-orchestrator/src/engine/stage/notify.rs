//! `NodeKind::Notify` — M5 stub: log-only, advances with the fixed
//! `delivered` outcome. Real channel delivery is M6+.

use crate::engine::stage::{StageError, StageResult};
use surge_core::keys::{NodeKey, OutcomeKey};
use surge_core::notify_config::NotifyConfig;
use surge_core::run_event::{EventPayload, VersionedEventPayload};
use surge_persistence::runs::run_writer::RunWriter;

/// Parameters for executing a single `NodeKind::Notify` stage.
pub struct NotifyStageParams<'a> {
    /// Key of the notify node being executed.
    pub node: &'a NodeKey,
    /// Notify node configuration (channel, template, on-failure action).
    pub notify_config: &'a NotifyConfig,
    /// Run writer for persisting the `OutcomeReported` event.
    pub writer: &'a RunWriter,
}

/// Execute a single `NodeKind::Notify` stage (M5 stub: log-only).
///
/// Always returns the `"delivered"` outcome. Real channel delivery is M6+.
pub async fn execute_notify_stage(p: NotifyStageParams<'_>) -> StageResult {
    tracing::info!(node = %p.node, "notify stage (M5 stub: log-only)");
    let outcome = OutcomeKey::try_from("delivered")
        .map_err(|e| StageError::Internal(format!("'delivered' outcome key: {e}")))?;
    p.writer
        .append_event(VersionedEventPayload::new(EventPayload::OutcomeReported {
            node: p.node.clone(),
            outcome: outcome.clone(),
            summary: "notify stub".into(),
        }))
        .await
        .map_err(|e| StageError::Storage(e.to_string()))?;
    let _ = p.notify_config; // unused in stub
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use surge_core::notify_config::{
        NotifyChannel, NotifyFailureAction, NotifySeverity, NotifyTemplate,
    };
    use surge_persistence::runs::Storage;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn notify_stub_returns_delivered_outcome() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();
        let writer = storage
            .create_run(surge_core::id::RunId::new(), dir.path(), None)
            .await
            .unwrap();

        let cfg = NotifyConfig {
            channel: NotifyChannel::Desktop,
            template: NotifyTemplate {
                severity: NotifySeverity::Info,
                title: "test".into(),
                body: "test body".into(),
                artifacts: vec![],
            },
            on_failure: NotifyFailureAction::Continue,
        };

        let node = NodeKey::try_from("ping").unwrap();
        let outcome = execute_notify_stage(NotifyStageParams {
            node: &node,
            notify_config: &cfg,
            writer: &writer,
        })
        .await
        .unwrap();
        assert_eq!(outcome.as_ref(), "delivered");
    }
}
