//! Standalone resource ownership, independent of engine writer evidence.

use std::sync::Arc;

use surge_acp::bridge::error::CloseSessionError;
use surge_acp::bridge::facade::BridgeFacade;
use surge_core::id::SessionId;
use surge_mcp::stage::{StageEndpoint, StageTransportError};

use super::InvocationFailure;

/// Why an endpoint could not be confirmed settled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndpointSettlementFailureKind {
    /// Local transport I/O failed.
    Io,
    /// Transport framing failed.
    Json,
    /// The transport rejected an operation.
    Rejected,
    /// The endpoint task did not confirm cleanup.
    CleanupUnconfirmed,
}

/// Why a bridge could not confirm session settlement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionSettlementFailureKind {
    /// Missing bridge state is not evidence that the writer was reaped.
    SessionNotFound,
    /// The close timed out without confirming a forced reap.
    GracefulTimedOut,
    /// Bridge communication failed.
    Bridge,
}

/// One observed cleanup failure; diagnostic text never controls retry policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvocationCleanupFailure {
    /// Endpoint cleanup failed.
    Endpoint {
        /// Typed transport failure.
        kind: EndpointSettlementFailureKind,
        /// Diagnostic detail.
        detail: String,
    },
    /// Session cleanup was not confirmed.
    Session {
        /// The owned session.
        session: SessionId,
        /// Typed close failure.
        kind: SessionSettlementFailureKind,
        /// Diagnostic detail.
        detail: String,
    },
    /// The independent cleanup task failed before returning its observations.
    Task {
        /// Diagnostic detail; neither resource is assumed settled.
        detail: String,
    },
}

/// Terminal settlement failure, optionally preserving the body's original error.
/// Only the invocation owner constructs this, with at least one cleanup failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub struct InvocationSettlementFailure {
    failures: Vec<InvocationCleanupFailure>,
    #[source]
    original: Option<Box<InvocationFailure>>,
}

impl std::fmt::Display for InvocationSettlementFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "invocation settlement failed: {:?}",
            self.failures
        )?;
        if let Some(original) = &self.original {
            write!(formatter, "; original failure: {original}")?;
        }
        Ok(())
    }
}

impl InvocationSettlementFailure {
    /// Every observed cleanup failure, in endpoint-then-session order.
    #[must_use]
    pub fn failures(&self) -> &[InvocationCleanupFailure] {
        &self.failures
    }

    /// The original body or opening failure, if the invocation already failed.
    #[must_use]
    pub fn original(&self) -> Option<&InvocationFailure> {
        self.original.as_deref()
    }
}

/// Own the endpoint before opening, then the session immediately after opening.
pub(super) struct InvocationResources {
    bridge: Arc<dyn BridgeFacade>,
    endpoint: Option<StageEndpoint>,
    session: Option<SessionId>,
}

impl InvocationResources {
    pub(super) fn new(bridge: &Arc<dyn BridgeFacade>, endpoint: StageEndpoint) -> Self {
        Self {
            bridge: Arc::clone(bridge),
            endpoint: Some(endpoint),
            session: None,
        }
    }

    pub(super) fn opened(&mut self, session: SessionId) {
        self.session = Some(session);
    }

    pub(super) async fn settle<T>(
        mut self,
        body: Result<T, InvocationFailure>,
    ) -> Result<T, InvocationFailure> {
        // Transfer ownership before awaiting. Dropping this future detaches the
        // cleanup task; it cannot abandon a close or issue a duplicate attempt.
        let bridge = Arc::clone(&self.bridge);
        let endpoint = self.endpoint.take();
        let session = self.session.take();
        let task = tokio::spawn(cleanup(bridge, endpoint, session));
        let failures = task.await.unwrap_or_else(|error| {
            vec![InvocationCleanupFailure::Task {
                detail: error.to_string(),
            }]
        });
        finish(body, failures)
    }
}

impl Drop for InvocationResources {
    fn drop(&mut self) {
        if self.endpoint.is_none() && self.session.is_none() {
            return;
        }
        tracing::error!(target: "agent_invocation", session = ?self.session,
            "invocation resources dropped without settlement; cleanup is best-effort");
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            tracing::error!(target: "agent_invocation", "no runtime for detached invocation cleanup");
            return;
        };
        runtime.spawn(cleanup(
            Arc::clone(&self.bridge),
            self.endpoint.take(),
            self.session.take(),
        ));
    }
}

async fn cleanup(
    bridge: Arc<dyn BridgeFacade>,
    endpoint: Option<StageEndpoint>,
    session: Option<SessionId>,
) -> Vec<InvocationCleanupFailure> {
    let endpoint = match endpoint {
        Some(endpoint) => endpoint.close().await,
        None => Ok(()),
    };
    // Endpoint failure must never skip the session close.
    let closed = match session {
        Some(session) => Some((session, bridge.close_session(session).await)),
        None => None,
    };
    let failures = cleanup_failures(endpoint, closed);
    for failure in &failures {
        tracing::error!(target: "agent_invocation", ?failure, "invocation cleanup unconfirmed");
    }
    failures
}

fn cleanup_failures(
    endpoint: Result<(), StageTransportError>,
    closed: Option<(SessionId, Result<(), CloseSessionError>)>,
) -> Vec<InvocationCleanupFailure> {
    let mut failures = Vec::new();
    if let Err(error) = endpoint {
        let kind = match &error {
            StageTransportError::Io(_) => EndpointSettlementFailureKind::Io,
            StageTransportError::Json(_) => EndpointSettlementFailureKind::Json,
            StageTransportError::Rejected(_) => EndpointSettlementFailureKind::Rejected,
            StageTransportError::CleanupUnconfirmed => {
                EndpointSettlementFailureKind::CleanupUnconfirmed
            },
        };
        failures.push(InvocationCleanupFailure::Endpoint {
            kind,
            detail: error.to_string(),
        });
    }
    if let Some((session, Err(error))) = closed {
        let kind = match &error {
            CloseSessionError::GracefulTimedOut {
                killed: true,
                session,
            } => {
                tracing::warn!(target: "agent_invocation", %session,
                    "invocation session was forcibly closed and reaped");
                return failures;
            },
            CloseSessionError::SessionNotFound { .. } => {
                SessionSettlementFailureKind::SessionNotFound
            },
            CloseSessionError::GracefulTimedOut { .. } => {
                SessionSettlementFailureKind::GracefulTimedOut
            },
            CloseSessionError::Bridge(_) => SessionSettlementFailureKind::Bridge,
        };
        failures.push(InvocationCleanupFailure::Session {
            session,
            kind,
            detail: error.to_string(),
        });
    }
    failures
}

fn finish<T>(
    body: Result<T, InvocationFailure>,
    failures: Vec<InvocationCleanupFailure>,
) -> Result<T, InvocationFailure> {
    if failures.is_empty() {
        body
    } else {
        Err(InvocationFailure::Settlement(InvocationSettlementFailure {
            failures,
            original: body.err().map(Box::new),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_failure_alone_is_terminal_and_cannot_construct_an_empty_failure() {
        let failures = cleanup_failures(Err(StageTransportError::CleanupUnconfirmed), None);
        let failure = finish(Ok(()), failures).unwrap_err();
        assert!(!failure.is_retryable());
        let InvocationFailure::Settlement(cleanup) = failure else {
            panic!("{failure:?}")
        };
        assert!(cleanup.original().is_none());
        assert!(matches!(
            cleanup.failures(),
            [InvocationCleanupFailure::Endpoint {
                kind: EndpointSettlementFailureKind::CleanupUnconfirmed,
                ..
            }]
        ));
        assert_eq!(finish(Ok(()), vec![]), Ok(()));
    }

    #[test]
    fn endpoint_and_session_failures_are_both_retained_with_the_original_source() {
        let session = SessionId::new();
        let failures = cleanup_failures(
            Err(StageTransportError::CleanupUnconfirmed),
            Some((
                session,
                Err(CloseSessionError::GracefulTimedOut {
                    session,
                    killed: false,
                }),
            )),
        );
        let failure = finish::<()>(Err(InvocationFailure::Cancelled), failures).unwrap_err();
        let InvocationFailure::Settlement(cleanup) = failure else {
            panic!("{failure:?}")
        };
        assert!(matches!(
            cleanup.failures(),
            [
                InvocationCleanupFailure::Endpoint { .. },
                InvocationCleanupFailure::Session {
                    kind: SessionSettlementFailureKind::GracefulTimedOut,
                    ..
                }
            ]
        ));
        assert_eq!(cleanup.original(), Some(&InvocationFailure::Cancelled));
        assert_eq!(
            std::error::Error::source(&cleanup).unwrap().to_string(),
            "cancelled"
        );
    }
}
