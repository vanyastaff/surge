//! Bridge-level error types.
//!
//! Five enums by API surface, no `From` between them and `crate::SurgeError`
//! (legacy domain) per spec §4.7. The bridge speaks its own error vocabulary.

use surge_core::SessionId;
use thiserror::Error;

use super::event::SessionEndReason;

/// Worker-level errors. Surfaced to API callers when the bridge worker thread
/// has died or refuses commands.
#[derive(Debug, Error)]
pub enum BridgeError {
    /// Worker thread panicked or exited unexpectedly. The bridge is dead;
    /// callers should drop the `AcpBridge` and respawn if they want to recover.
    #[error("bridge worker thread is dead")]
    WorkerDead,

    /// Command channel `send().await` failed (worker is shutting down or the
    /// thread already exited).
    #[error("command channel send failed: {0}")]
    CommandSendFailed(String),

    /// `oneshot` reply was dropped before sending (worker died mid-command).
    #[error("oneshot reply dropped before sending")]
    ReplyDropped,
}

/// Errors from `AcpBridge::open_session`.
#[derive(Debug, Error)]
pub enum OpenSessionError {
    /// Agent subprocess could not be started (binary not found, bad working dir, etc.).
    #[error("agent subprocess spawn failed for kind '{kind}': {source}")]
    AgentSpawnFailed {
        /// Agent kind label (e.g. `"claude-code"`, `"mock"`).
        kind: String,
        /// Underlying OS error from the spawn attempt.
        #[source]
        source: std::io::Error,
    },

    /// ACP `initialize` or `new_session` handshake exchange failed.
    #[error("ACP handshake failed: {reason}")]
    HandshakeFailed {
        /// Human-readable ACP protocol error.
        reason: String,
    },

    /// `SessionConfig::declared_outcomes` is empty; at least one outcome is required.
    #[error("declared_outcomes is empty — `report_stage_outcome` cannot be constructed")]
    NoDeclaredOutcomes,

    /// Caller-supplied tool list failed validation (e.g. duplicate names, reserved names).
    #[error("invalid tool definitions: {0}")]
    InvalidToolDefs(String),

    /// Bindings map failed validation (too many entries or values too long).
    #[error("invalid bindings: {0}")]
    InvalidBindings(String),

    /// Bridge worker communication failed before the session could open.
    #[error("bridge: {0}")]
    Bridge(#[source] BridgeError),
}

/// Errors from `AcpBridge::send_message`.
#[derive(Debug, Error)]
pub enum SendMessageError {
    /// No session with this id exists in the bridge's session map.
    #[error("session {session} not found")]
    SessionNotFound {
        /// The session id that was not found.
        session: SessionId,
    },

    /// The session ended (normally or via crash) before the message could be delivered.
    #[error("session {session} ended ({reason:?})")]
    SessionEnded {
        /// The session that ended.
        session: SessionId,
        /// Why it ended.
        reason: SessionEndReason,
    },

    /// Bridge worker communication failed.
    #[error("bridge: {0}")]
    Bridge(#[source] BridgeError),
}

/// Errors from `AcpBridge::close_session`.
#[derive(Debug, Error)]
pub enum CloseSessionError {
    /// No session with this id exists in the bridge's session map.
    #[error("session {session} not found")]
    SessionNotFound {
        /// The session id that was not found.
        session: SessionId,
    },

    /// Graceful shutdown timed out; the child was killed and the session is gone,
    /// but the closure was not clean.
    #[error("session {session} graceful close timed out (killed = {killed})")]
    GracefulTimedOut {
        /// The session that timed out.
        session: SessionId,
        /// Whether a SIGKILL was successfully delivered to the subprocess.
        killed: bool,
    },

    /// Bridge worker communication failed.
    #[error("bridge: {0}")]
    Bridge(#[source] BridgeError),
}

/// Wrapper for errors originating in the underlying ACP SDK.
#[derive(Debug, Error)]
pub enum AcpError {
    /// ACP protocol-level error (framing, serialization, handshake violation).
    #[error("ACP protocol error: {0}")]
    Protocol(#[source] agent_client_protocol::Error),

    /// OS I/O error on the subprocess stdio pipes.
    #[error("io: {0}")]
    Io(#[source] std::io::Error),

    /// Agent subprocess exited before the handshake completed.
    #[error("agent subprocess exited mid-handshake (exit_code = {exit_code:?})")]
    AgentExited {
        /// OS exit code, or `None` if killed by signal.
        exit_code: Option<i32>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_no_outcomes_renders() {
        let e = OpenSessionError::NoDeclaredOutcomes;
        assert!(format!("{e}").contains("declared_outcomes is empty"));
    }

    #[test]
    fn close_graceful_timeout_renders_with_killed_flag() {
        let s = SessionId::new();
        let e = CloseSessionError::GracefulTimedOut {
            session: s,
            killed: true,
        };
        let rendered = format!("{e}");
        assert!(rendered.contains(&s.to_string()));
        assert!(rendered.contains("killed = true"));
    }

    #[test]
    fn bridge_error_is_send_sync() {
        // Compile-time bound check — bridge errors must be Send + Sync to cross
        // tokio task boundaries via oneshot replies.
        fn bound<T: Send + Sync>() {}
        bound::<BridgeError>();
        bound::<OpenSessionError>();
        bound::<SendMessageError>();
        bound::<CloseSessionError>();
        bound::<AcpError>();
    }
}
