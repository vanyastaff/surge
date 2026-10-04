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
