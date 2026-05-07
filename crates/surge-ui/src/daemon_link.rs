//! Daemon connection lifecycle for the runtime UI.
//!
//! The runtime UI (this crate) is a daemon client per
//! `docs/ARCHITECTURE.md` — it watches runs that
//! the daemon hosts rather than running them in-process. This module
//! owns the connection state machine and the `try_connect` helper used
//! by `SurgeApp::new` on startup.
//!
//! Per-run event subscription lives in a follow-up phase and will live
//! in its own module; this one stops at connect + global event
//! subscription.

use std::sync::Arc;

use surge_orchestrator::engine::EngineError;
use surge_orchestrator::engine::daemon_facade::DaemonEngineFacade;

/// Where the runtime UI thinks the daemon is.
///
/// `Disconnected` is the initial / "user dismissed" state.
/// `Connecting` is in-flight — the connect task hasn't finished yet.
/// `Connected` carries the live facade for subsequent IPC.
/// `Failed` carries a one-line reason; surfaced as a "click to retry"
/// affordance in the UI.
#[derive(Clone, Default)]
pub enum ConnectionState {
    #[default]
    Disconnected,
    Connecting,
    Connected(Arc<DaemonEngineFacade>),
    Failed(String),
}

impl std::fmt::Debug for ConnectionState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Disconnected => write!(f, "Disconnected"),
            Self::Connecting => write!(f, "Connecting"),
            Self::Connected(_) => write!(f, "Connected(<facade>)"),
            Self::Failed(e) => write!(f, "Failed({e})"),
        }
    }
}

impl ConnectionState {
    /// Convenience for the top-bar indicator.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::Disconnected => "Daemon: Disconnected",
            Self::Connecting => "Daemon: Connecting…",
            Self::Connected(_) => "Daemon: Connected",
            Self::Failed(_) => "Daemon: Connection failed",
        }
    }

    /// Hands the live facade to callers that need IPC. `None` for any
    /// other state.
    #[must_use]
    pub fn facade(&self) -> Option<Arc<DaemonEngineFacade>> {
        match self {
            Self::Connected(f) => Some(f.clone()),
            _ => None,
        }
    }
}

/// Resolve the daemon socket path and open a connection. Does NOT
/// auto-start the daemon (yet) — that's a UX choice we'll surface as
/// an explicit button rather than launching a background process
/// silently on UI startup.
pub async fn try_connect() -> Result<Arc<DaemonEngineFacade>, EngineError> {
    let socket = surge_daemon::pidfile::socket_path()
        .map_err(|e| EngineError::Internal(format!("daemon socket path: {e}")))?;
    let facade = DaemonEngineFacade::connect(socket).await?;
    Ok(Arc::new(facade))
}
