//! `NodeKind::Terminal` execution.

use crate::engine::stage::StageError;
use surge_core::keys::NodeKey;
use surge_core::run_event::{EventPayload, VersionedEventPayload};
use surge_core::terminal_config::{TerminalConfig, TerminalKind};
use surge_persistence::runs::run_writer::RunWriter;

#[derive(Debug, Clone, PartialEq)]
pub enum TerminalOutcome {
    Completed { node: NodeKey },
    Failed { error: String },
}

pub struct TerminalStageParams<'a> {
    pub node: &'a NodeKey,
    pub terminal_config: &'a TerminalConfig,
    pub writer: &'a RunWriter,
}

pub async fn execute_terminal_stage(p: TerminalStageParams<'_>) -> Result<TerminalOutcome, StageError> {
    match &p.terminal_config.kind {
        TerminalKind::Success => {
            p.writer
                .append_event(VersionedEventPayload::new(EventPayload::RunCompleted {
                    terminal_node: p.node.clone(),
                }))
                .await
                .map_err(|e| StageError::Storage(e.to_string()))?;
            Ok(TerminalOutcome::Completed { node: p.node.clone() })
        }
        TerminalKind::Failure { .. } | TerminalKind::Aborted => {
            let reason = p
                .terminal_config
                .message
                .clone()
                .unwrap_or_else(|| "terminal failure node".into());
            p.writer
                .append_event(VersionedEventPayload::new(EventPayload::RunFailed {
                    error: reason.clone(),
                }))
                .await
                .map_err(|e| StageError::Storage(e.to_string()))?;
            Ok(TerminalOutcome::Failed { error: reason })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use surge_persistence::runs::Storage;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn success_terminal_emits_run_completed() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();
        let writer = storage
            .create_run(surge_core::id::RunId::new(), dir.path(), None)
            .await
            .unwrap();

        let cfg = TerminalConfig {
            kind: TerminalKind::Success,
            message: None,
        };
        let node = NodeKey::try_from("end").unwrap();
        let outcome = execute_terminal_stage(TerminalStageParams {
            node: &node,
            terminal_config: &cfg,
            writer: &writer,
        })
        .await
        .unwrap();
        match outcome {
            TerminalOutcome::Completed { node: n } => assert_eq!(n.as_ref(), "end"),
            other => panic!("expected Completed, got {other:?}"),
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn failure_terminal_emits_run_failed() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();
        let writer = storage
            .create_run(surge_core::id::RunId::new(), dir.path(), None)
            .await
            .unwrap();

        let cfg = TerminalConfig {
            kind: TerminalKind::Failure { exit_code: 1 },
            message: Some("oops".into()),
        };
        let node = NodeKey::try_from("fail").unwrap();
        let outcome = execute_terminal_stage(TerminalStageParams {
            node: &node,
            terminal_config: &cfg,
            writer: &writer,
        })
        .await
        .unwrap();
        match outcome {
            TerminalOutcome::Failed { error } => assert_eq!(error, "oops"),
            other => panic!("expected Failed, got {other:?}"),
        }
    }
}
