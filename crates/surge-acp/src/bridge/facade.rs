//! `BridgeFacade` — abstraction over `AcpBridge` for engine consumers.
//!
//! Promised in the M3 design (§2.4): "if M5 engine accumulates real test
//! pain, introduce traits then." M5 is that point. Without this trait every
//! engine unit test would have to spawn `mock_acp_agent` as a subprocess,
//! adding ~200ms per test and flaking on slow CI shards.
//!
//! `AcpBridge` (the M3 type) implements this trait via straight delegation;
//! engine code holds an `Arc<dyn BridgeFacade>` so the same engine instance
//! can be wired against the real bridge or a `MockBridge` test double.

use async_trait::async_trait;
use tokio::sync::broadcast;

use agent_client_protocol::RequestPermissionResponse;

use crate::bridge::acp_bridge::AcpBridge;
use crate::bridge::error::{
    BridgeError, CloseSessionError, OpenSessionError, ReplyToPermissionError, ReplyToToolError,
    SendMessageError,
};
use crate::bridge::event::BridgeEvent;
use crate::bridge::event::ToolResultPayload;
use crate::bridge::session::{MessageContent, SessionConfig, SessionState};
use surge_core::SessionId;

/// Engine-facing surface of an ACP bridge. All futures are `Send`.
#[async_trait]
pub trait BridgeFacade: Send + Sync {
    /// Open a new ACP session with the given configuration.
    async fn open_session(&self, config: SessionConfig) -> Result<SessionId, OpenSessionError>;

    /// Send a user-role message to an open session.
    async fn send_message(
        &self,
        session: SessionId,
        content: MessageContent,
    ) -> Result<(), SendMessageError>;

    /// Read a session's bridge-observable state (open / closed / crashed).
    async fn session_state(&self, session: SessionId) -> Result<SessionState, BridgeError>;

    /// Close a session gracefully.
    async fn close_session(&self, session: SessionId) -> Result<(), CloseSessionError>;

    /// Reply to an outstanding tool call. M5 engine uses this to dispatch
    /// non-injected tools and to deliver the human's answer to
    /// `request_human_input` requests.
    async fn reply_to_tool(
        &self,
        session: SessionId,
        call_id: String,
        payload: ToolResultPayload,
    ) -> Result<(), ReplyToToolError>;

    /// Reply to an outstanding ACP permission request. Pair with the
    /// matching `BridgeEvent::PermissionRequested` event's `request_id`.
    async fn reply_to_permission(
        &self,
        session: SessionId,
        request_id: String,
        response: RequestPermissionResponse,
    ) -> Result<(), ReplyToPermissionError>;

    /// Subscribe to the broadcast event stream. Each subscriber receives
    /// every event from every active session.
    fn subscribe(&self) -> broadcast::Receiver<BridgeEvent>;
}

#[async_trait]
impl BridgeFacade for AcpBridge {
    async fn open_session(&self, config: SessionConfig) -> Result<SessionId, OpenSessionError> {
        AcpBridge::open_session(self, config).await
    }

    async fn send_message(
        &self,
        session: SessionId,
        content: MessageContent,
    ) -> Result<(), SendMessageError> {
        AcpBridge::send_message(self, session, content).await
    }

    async fn session_state(&self, session: SessionId) -> Result<SessionState, BridgeError> {
        AcpBridge::session_state(self, session).await
    }

    async fn close_session(&self, session: SessionId) -> Result<(), CloseSessionError> {
        AcpBridge::close_session(self, session).await
    }

    async fn reply_to_tool(
        &self,
        session: SessionId,
        call_id: String,
        payload: ToolResultPayload,
    ) -> Result<(), ReplyToToolError> {
        AcpBridge::reply_to_tool(self, session, call_id, payload).await
    }

    async fn reply_to_permission(
        &self,
        session: SessionId,
        request_id: String,
        response: RequestPermissionResponse,
    ) -> Result<(), ReplyToPermissionError> {
        AcpBridge::reply_to_permission(self, session, request_id, response).await
    }

    fn subscribe(&self) -> broadcast::Receiver<BridgeEvent> {
        AcpBridge::subscribe(self)
    }
}
