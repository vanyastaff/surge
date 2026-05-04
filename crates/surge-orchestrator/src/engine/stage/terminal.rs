//! `NodeKind::Terminal` execution.

use crate::engine::stage::StageError;
use surge_core::keys::NodeKey;
use surge_core::run_event::{EventPayload, VersionedEventPayload};
use surge_core::terminal_config::{TerminalConfig, TerminalKind};
use surge_persistence::runs::run_writer::RunWriter;

/// Outcome produced by a `NodeKind::Terminal` stage.
#[derive(Debug, Clone, PartialEq)]
pub enum TerminalOutcome {
    /// The run completed successfully at this terminal node.
    Completed {
        /// Key of the terminal node that ended the run.
        node: NodeKey,
    },
    /// The run terminated with an error (reached a `TerminalKind::Failure` node).
    Failed {
        /// Human-readable description of the failure.
        error: String,
    },
    /// The run halted gracefully (reached a `TerminalKind::Aborted` node).
    ///
    /// Distinct from `Failed`: aborted is an intentional graceful halt (e.g.
    /// a user-requested cancellation baked into the graph), whereas `Failed`
    /// is an unexpected error path. Emits `EventPayload::RunAborted` rather
    /// than `RunFailed`.
    Aborted {
        /// Reason message from the terminal node's `message` field, or a default.
        reason: String,
    },
}

/// Parameters for executing a single `NodeKind::Terminal` stage.
pub struct TerminalStageParams<'a> {
    /// Key of the terminal node being executed.
    pub node: &'a NodeKey,
    /// Terminal node configuration (kind + optional message).
    pub terminal_config: &'a TerminalConfig,
    /// Run writer for persisting the run-level terminal event.
    pub writer: &'a RunWriter,
}

/// Execute a single `NodeKind::Terminal` stage.
///
/// Emits `RunCompleted` for `TerminalKind::Success`,
/// `RunFailed` for `TerminalKind::Failure`, and
/// `RunAborted` for `TerminalKind::Aborted` (graceful halt).
///
/// `Aborted` is the "graceful halt" path — an intentional stop baked into
/// the pipeline graph. `Failure` is the "error" path for unexpected terminal
/// conditions. Both return distinct `TerminalOutcome` variants so the run-task
/// loop can emit the correct `RunOutcome`.
pub async fn execute_terminal_stage(
    p: TerminalStageParams<'_>,
) -> Result<TerminalOutcome, StageError> {
    match &p.terminal_config.kind {
        TerminalKind::Success => {
            p.writer
                .append_event(VersionedEventPayload::new(EventPayload::RunCompleted {
                    terminal_node: p.node.clone(),
                }))
                .await
                .map_err(|e| StageError::Storage(e.to_string()))?;
            Ok(TerminalOutcome::Completed {
                node: p.node.clone(),
            })
        },
        TerminalKind::Failure { .. } => {
            let error = p
                .terminal_config
                .message
                .clone()
                .unwrap_or_else(|| "terminal failure node".into());
            p.writer
                .append_event(VersionedEventPayload::new(EventPayload::RunFailed {
                    error: error.clone(),
                }))
                .await
                .map_err(|e| StageError::Storage(e.to_string()))?;
            Ok(TerminalOutcome::Failed { error })
        },
        TerminalKind::Aborted => {
            let reason = p
                .terminal_config
                .message
                .clone()
                .unwrap_or_else(|| "terminal aborted node".into());
            p.writer
                .append_event(VersionedEventPayload::new(EventPayload::RunAborted {
                    reason: reason.clone(),
                }))
                .await
                .map_err(|e| StageError::Storage(e.to_string()))?;
            Ok(TerminalOutcome::Aborted { reason })
        },
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

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn aborted_terminal_emits_run_aborted() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();
        let writer = storage
            .create_run(surge_core::id::RunId::new(), dir.path(), None)
            .await
            .unwrap();

        let cfg = TerminalConfig {
            kind: TerminalKind::Aborted,
            message: Some("user cancelled".into()),
        };
        let node = NodeKey::try_from("abort").unwrap();
        let outcome = execute_terminal_stage(TerminalStageParams {
            node: &node,
            terminal_config: &cfg,
            writer: &writer,
        })
        .await
        .unwrap();
        match outcome {
            TerminalOutcome::Aborted { reason } => assert_eq!(reason, "user cancelled"),
            other => panic!("expected Aborted, got {other:?}"),
        }
    }
}
