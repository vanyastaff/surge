//! Internal command channel payload. Public for tests; production callers
//! use the `AcpBridge` methods rather than constructing commands directly.

use surge_core::SessionId;
use tokio::sync::oneshot;

use super::error::{BridgeError, CloseSessionError, OpenSessionError, SendMessageError};
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
        config: SessionConfig,
        reply: oneshot::Sender<Result<SessionId, OpenSessionError>>,
    },
    /// Send a message to an open session — see `AcpBridge::send_message`.
    SendMessage {
        session: SessionId,
        content: MessageContent,
        reply: oneshot::Sender<Result<(), SendMessageError>>,
    },
    /// Read a session's bridge-observable state — see `AcpBridge::session_state`.
    GetSessionState {
        session: SessionId,
        reply: oneshot::Sender<Result<SessionState, BridgeError>>,
    },
    /// Close a session — see `AcpBridge::close_session`.
    CloseSession {
        session: SessionId,
        reply: oneshot::Sender<Result<(), CloseSessionError>>,
    },
    /// Drain pending commands and exit the worker — see `AcpBridge::shutdown`.
    Shutdown {
        reply: oneshot::Sender<()>,
    },
    /// Test-only: inject a panic into the worker thread to exercise the
    /// `WorkerDead` recovery path. Gated by `#[cfg(any(test, feature = "test-helpers"))]`
    /// to keep production builds clean while remaining accessible to integration
    /// tests (which compile as a separate crate and don't trigger `cfg(test)` of
    /// the parent crate).
    #[cfg(any(test, feature = "test-helpers"))]
    TestPanic,
}
