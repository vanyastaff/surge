//! Internal command channel payload. Public for tests; production callers
//! use the `AcpBridge` methods rather than constructing commands directly.

use surge_core::SessionId;
use tokio::sync::oneshot;

use super::error::{BridgeError, CloseSessionError, OpenSessionError, ReplyToToolError, SendMessageError};
use super::session::{MessageContent, SessionConfig, SessionState};

/// Internal command payload sent over the mpsc channel from
/// `AcpBridge::*` methods to the worker task. Each variant carries a
/// `oneshot` sender for the reply.
///
/// Public visibility is for tests; production callers go through
/// `AcpBridge`'s typed API.
pub enum BridgeCommand {
    /// Open a new session — see `AcpBridge::open_session`.
    OpenSession {
        /// Open-session parameters.
        config: SessionConfig,
        /// Reply channel carrying the new `SessionId` or an `OpenSessionError`.
        reply: oneshot::Sender<Result<SessionId, OpenSessionError>>,
    },
    /// Send a message to an open session — see `AcpBridge::send_message`.
    SendMessage {
        /// Target session.
        session: SessionId,
        /// Message payload.
        content: MessageContent,
        /// Reply channel carrying `()` on success or a `SendMessageError`.
        reply: oneshot::Sender<Result<(), SendMessageError>>,
    },
    /// Read a session's bridge-observable state — see `AcpBridge::session_state`.
    GetSessionState {
        /// Target session.
        session: SessionId,
        /// Reply channel carrying the `SessionState` or a `BridgeError`.
        reply: oneshot::Sender<Result<SessionState, BridgeError>>,
    },
    /// Close a session — see `AcpBridge::close_session`.
    CloseSession {
        /// Target session.
        session: SessionId,
        /// Reply channel carrying `()` on clean close or a `CloseSessionError`.
        reply: oneshot::Sender<Result<(), CloseSessionError>>,
    },
    /// Drain pending commands and exit the worker — see `AcpBridge::shutdown`.
    Shutdown {
        /// Reply channel; worker sends `()` once the shutdown is complete.
        reply: oneshot::Sender<()>,
    },
    /// Send a reply to an outstanding tool call — see `AcpBridge::reply_to_tool`.
    ReplyToTool {
        /// Target session.
        session: SessionId,
        /// ACP-supplied call id from the matching `BridgeEvent::ToolCall`,
        /// `OutcomeReported`, or `HumanInputRequested` event.
        call_id: String,
        /// Result payload to send back to the agent.
        payload: super::event::ToolResultPayload,
        /// Reply channel carrying `()` on success or a `ReplyToToolError`.
        reply: oneshot::Sender<Result<(), ReplyToToolError>>,
    },
    /// Test-only: inject a panic into the worker thread to exercise the
    /// `WorkerDead` recovery path. Gated by `#[cfg(any(test, feature = "test-helpers"))]`
    /// to keep production builds clean while remaining accessible to integration
    /// tests (which compile as a separate crate and don't trigger `cfg(test)` of
    /// the parent crate).
    #[cfg(any(test, feature = "test-helpers"))]
    TestPanic,
}
