//! Host-supplied writer ownership; MCP transport never invents cleanup proof.

use surge_core::id::ExecutionWriterId;

/// A durable ownership observation failed before a child side effect.
#[derive(Debug, thiserror::Error)]
#[error("MCP writer ownership observation failed: {0}")]
pub struct WriterObservationError(pub String);

/// Narrow host boundary injected by the engine without importing storage or ACP.
#[async_trait::async_trait]
pub trait HostWriterObserver: Send + Sync {
    /// Admit exactly the immediately following physical MCP effect.
    /// Cleanup remains available regardless of this synchronous permission.
    fn before_effect(&self, _server: &str) -> Result<(), WriterObservationError> {
        Ok(())
    }

    /// Commit a distinct ownership intent before each child launch, including respawns.
    async fn before_child(&self, server: &str)
    -> Result<ExecutionWriterId, WriterObservationError>;
    /// Commit the actual spawned identity before MCP initialization may write.
    async fn child_started(
        &self,
        writer: ExecutionWriterId,
        pid: Option<u32>,
    ) -> Result<(), WriterObservationError>;
    /// Commit an actual launch identity, including a native Windows job when present.
    /// Existing observers retain their PID callback; durable hosts can validate the job.
    async fn child_started_observed(
        &self,
        writer: ExecutionWriterId,
        pid: Option<u32>,
        _job_name: Option<&str>,
    ) -> Result<(), WriterObservationError> {
        self.child_started(writer, pid).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct LegacyObserver(Mutex<Option<(ExecutionWriterId, Option<u32>)>>);

    #[async_trait::async_trait]
    impl HostWriterObserver for LegacyObserver {
        async fn before_child(&self, _: &str) -> Result<ExecutionWriterId, WriterObservationError> {
            Ok(ExecutionWriterId::new())
        }
        async fn child_started(
            &self,
            writer: ExecutionWriterId,
            pid: Option<u32>,
        ) -> Result<(), WriterObservationError> {
            *self.0.lock().unwrap() = Some((writer, pid));
            Ok(())
        }
    }

    #[tokio::test]
    async fn native_observation_preserves_legacy_callback_identity() {
        let observer = LegacyObserver::default();
        let writer = ExecutionWriterId::new();
        observer
            .child_started_observed(writer, Some(42), Some("native-job-fixture"))
            .await
            .unwrap();
        assert_eq!(*observer.0.lock().unwrap(), Some((writer, Some(42))));
    }
}
