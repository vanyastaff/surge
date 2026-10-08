//! Host-owned pre-dispatch audit authority for executable writer boundaries.

use surge_core::execution_recovery::process::{ExecutionWriterIntent, ExecutionWriterKind};
use surge_core::id::{ExecutionWriterId, StageInvocationId};
use surge_core::{EventPayload, VersionedEventPayload};
use surge_persistence::runs::run_writer::RunEventRecorder;

/// Run-owned observer injected into the lower-level MCP transport.
pub(crate) struct McpWriterObserver {
    pub(crate) recorder: RunEventRecorder,
    pub(crate) invocation: StageInvocationId,
    pub(crate) effect_fence: Option<std::sync::Arc<super::owned_effects::OwnedFlowEffectFence>>,
}

#[async_trait::async_trait]
impl surge_mcp::writer_observer::HostWriterObserver for McpWriterObserver {
    fn before_effect(
        &self,
        _server: &str,
    ) -> Result<(), surge_mcp::writer_observer::WriterObservationError> {
        use surge_acp::bridge::HostEffectFence;
        self.effect_fence.as_ref().map_or(Ok(()), |fence| {
            fence.check().map_err(|_| {
                surge_mcp::writer_observer::WriterObservationError("host-effect-refused".into())
            })
        })
    }

    async fn before_child(
        &self,
        server: &str,
    ) -> Result<ExecutionWriterId, surge_mcp::writer_observer::WriterObservationError> {
        surge_mcp::writer_observer::HostWriterObserver::before_effect(self, server)?;
        begin(
            &self.recorder,
            self.invocation,
            ExecutionWriterKind::HostTool {
                call_id: format!("mcp-child:{server}"),
            },
            false,
        )
        .await
        .map_err(|error| surge_mcp::writer_observer::WriterObservationError(error.to_string()))
    }

    async fn child_started(
        &self,
        writer: ExecutionWriterId,
        pid: Option<u32>,
    ) -> Result<(), surge_mcp::writer_observer::WriterObservationError> {
        let pid = pid.ok_or_else(|| {
            surge_mcp::writer_observer::WriterObservationError(
                "spawned MCP writer has no observable process identity".into(),
            )
        })?;
        let container = surge_acp::process_evidence::observe_container(
            pid,
            surge_core::execution_recovery::process::WriterCoverage::GroupOnly,
        )
        .map_err(|error| surge_mcp::writer_observer::WriterObservationError(error.to_string()))?;
        self.recorder
            .append_event(VersionedEventPayload::new(
                EventPayload::ExecutionWriterEstablished { writer, container },
            ))
            .await
            .map_err(|error| {
                surge_mcp::writer_observer::WriterObservationError(error.to_string())
            })?;
        Ok(())
    }
}

/// Commit ownership before launching a local writer or permitting an external effect.
pub(crate) async fn begin(
    recorder: &RunEventRecorder,
    invocation: StageInvocationId,
    kind: ExecutionWriterKind,
    local_effects: bool,
) -> Result<ExecutionWriterId, std::io::Error> {
    let writer = ExecutionWriterId::new();
    let owner = surge_acp::process_evidence::observe(std::process::id())
        .ok()
        .map(|(identity, _)| identity);
    recorder
        .append_event(VersionedEventPayload::new(
            EventPayload::ExecutionWriterIntent {
                intent: ExecutionWriterIntent {
                    writer,
                    invocation,
                    kind,
                    owner,
                    local_effects,
                },
            },
        ))
        .await
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    Ok(writer)
}

fn is_mcp_writer(kind: &ExecutionWriterKind) -> bool {
    matches!(kind, ExecutionWriterKind::HostTool { call_id } if call_id.starts_with("mcp-child:"))
}

/// Assess prior MCP writers under ADR-0021.
///
/// Returns the writers whose recorded process group is now observed empty and
/// still lacks a best-effort cleanup record, or the reason resume must refuse:
/// conflicting ownership, no established identity, or a group that is still
/// occupied. An empty `GroupOnly` group is best-effort cleanup, never confirmed
/// closure; escaped descendants are an accepted residual risk.
pub(crate) fn assess_mcp_cleanup(
    memory: &surge_core::run_state::RunMemory,
) -> Result<Vec<ExecutionWriterId>, String> {
    use surge_acp::process_evidence::GroupState;
    let mut stopped = Vec::new();
    for record in memory.execution_writers.values() {
        if !is_mcp_writer(&record.intent.kind) {
            continue;
        }
        let writer = record.intent.writer;
        if record.conflicting_observation {
            return Err(format!(
                "MCP writer {writer} has conflicting ownership evidence"
            ));
        }
        let Some(container) = record.container.as_ref() else {
            return Err(format!(
                "MCP writer {writer} has no established process identity"
            ));
        };
        if record.cleanup_confirmed || record.group_stopped {
            continue;
        }
        match surge_acp::process_evidence::group_state(container) {
            GroupState::Empty => stopped.push(writer),
            state => {
                return Err(format!(
                    "MCP writer {writer} process group is not stopped ({state:?})"
                ));
            },
        }
    }
    Ok(stopped)
}

/// Grace per signal when stopping a prior MCP group on recovery.
const MCP_GROUP_STOP_GRACE: std::time::Duration = std::time::Duration::from_secs(2);

/// Stop prior MCP groups whose recorded leader is still running, then assess.
///
/// Reads fresh journal evidence (observer events are appended outside a stage's
/// memory). Signals only groups whose leader identity still matches.
pub(crate) async fn stop_and_assess_mcp_cleanup(
    storage: &std::sync::Arc<surge_persistence::runs::Storage>,
    run: surge_core::id::RunId,
) -> Result<Result<Vec<ExecutionWriterId>, String>, crate::engine::error::EngineError> {
    use surge_acp::process_evidence::GroupState;
    let reader = storage
        .open_run_reader(run)
        .await
        .map_err(|error| crate::engine::error::EngineError::Storage(error.to_string()))?;
    let memory = super::replay::replay(&reader).await?.memory;
    for record in memory.execution_writers.values() {
        if !is_mcp_writer(&record.intent.kind)
            || record.cleanup_confirmed
            || record.group_stopped
            || record.conflicting_observation
        {
            continue;
        }
        let Some(container) = record.container.clone() else {
            continue;
        };
        if surge_acp::process_evidence::group_state(&container) != GroupState::LeaderAlive {
            continue;
        }
        let writer = record.intent.writer;
        let state = tokio::task::spawn_blocking(move || {
            surge_acp::process_evidence::stop_group(&container, MCP_GROUP_STOP_GRACE)
        })
        .await
        .map_err(|error| {
            crate::engine::error::EngineError::Storage(format!("join MCP group stop: {error}"))
        })?;
        tracing::info!(target: "mcp::recovery", %run, %writer, ?state, "stopped prior MCP process group");
    }
    Ok(assess_mcp_cleanup(&memory))
}

/// Durably record best-effort cleanup for each writer (ADR-0021).
pub(crate) async fn record_mcp_groups_stopped(
    writer: &surge_persistence::runs::run_writer::RunWriter,
    stopped: &[ExecutionWriterId],
) -> Result<(), String> {
    for id in stopped {
        writer
            .append_event(VersionedEventPayload::new(
                EventPayload::ExecutionWriterGroupStopped { writer: *id },
            ))
            .await
            .map_err(|error| format!("record MCP group cleanup for {id}: {error}"))?;
        tracing::info!(target: "mcp::recovery", writer = %id, "recorded best-effort MCP group cleanup");
    }
    Ok(())
}
