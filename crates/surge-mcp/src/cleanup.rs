//! Transport settlement evidence, separate from host process/effect disappearance.

/// MCP service teardown could not be confirmed.
#[derive(Debug, thiserror::Error)]
pub enum CleanupError {
    /// A call still owns the service; dropping the registry is not settlement.
    #[error("MCP server {server} still has an outstanding service handle")]
    OutstandingHandle {
        /// Configured server whose service remains owned by an in-flight call.
        server: String,
    },
    /// The service task failed while cancellation was awaited.
    #[error("MCP server {server} cancellation failed: {reason}")]
    ServiceJoin {
        /// Configured server whose cancellation task failed.
        server: String,
        /// Fixed opaque task failure category.
        reason: String,
    },
    /// The bounded host wait ended without a confirmed result.
    #[error("MCP server {server} cleanup timed out")]
    Timeout {
        /// Configured server whose cleanup budget expired.
        server: String,
    },
    /// A teardown worker failed instead of returning a cleanup result.
    #[error("MCP cleanup worker failed: {reason}")]
    WorkerJoin {
        /// Fixed opaque join failure category retained by registry teardown.
        reason: String,
    },
}

/// All service-level failures observed during bounded registry teardown.
#[derive(Debug, thiserror::Error)]
#[error("MCP transport cleanup is unconfirmed: {failures:?}")]
pub struct RegistryCleanupError {
    /// Child-level failures; a successful report still does not prove writer containment.
    pub failures: Vec<CleanupError>,
}
