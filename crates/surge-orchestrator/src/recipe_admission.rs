//! Registry admission precedes provider effects, including sessions without quota policy.
use agent_client_protocol::schema::v1::RequestPermissionResponse;
use async_trait::async_trait;
use std::sync::Arc;
use surge_acp::bridge::error::{
    BridgeError, CloseSessionError, OpenSessionError, ReplyToPermissionError, ReplyToToolError,
    SendMessageError,
};
use surge_acp::bridge::facade::BridgeFacade;
use surge_acp::bridge::{
    BridgeEvent, MessageContent, SessionConfig, SessionState, ToolResultPayload,
};
use surge_core::{SessionId, execution_recovery::OpenedSession};
use surge_persistence::work_items::WorkItemStore;

/// One owner wraps its raw bridge once. This records ordering, not launch authority.
pub struct RecipeAdmissionBridge {
    inner: Arc<dyn BridgeFacade>,
    store: WorkItemStore,
}
impl RecipeAdmissionBridge {
    #[must_use]
    pub fn new(inner: Arc<dyn BridgeFacade>, store: WorkItemStore) -> Self {
        Self { inner, store }
    }
}
#[async_trait]
impl BridgeFacade for RecipeAdmissionBridge {
    fn legacy_stage_event_adapter(&self) -> bool {
        self.inner.legacy_stage_event_adapter()
    }
    async fn open_session(&self, config: SessionConfig) -> Result<OpenedSession, OpenSessionError> {
        let writer = config.writer_id;
        let invocation = config.invocation;
        let runtime = config.runtime.clone();
        let hash = surge_core::ContentHash::compute(format!("{:?}", config.agent_kind).as_bytes());
        self.store
            .admit_recipe_opening(writer, invocation, &runtime, &hash)
            .map_err(|error| OpenSessionError::HandshakeFailed {
                reason: format!("recipe admission refused: {error}"),
            })?;
        let opened = self.inner.open_session(config).await?;
        if opened
            .execution_writer
            .as_ref()
            .map(|observation| observation.writer())
            != Some(writer)
            || opened.descriptor.invocation() != invocation
            || opened.descriptor.runtime() != runtime
            || opened.descriptor.launch_hash() != &hash
        {
            let cleanup = self.inner.close_session(opened.session).await;
            return Err(OpenSessionError::HandshakeFailed {
                reason: format!(
                    "provider opening differs from recipe admission; cleanup: {cleanup:?}"
                ),
            });
        }
        Ok(opened)
    }
    async fn send_message(
        &self,
        session: SessionId,
        content: MessageContent,
    ) -> Result<(), SendMessageError> {
        self.inner.send_message(session, content).await
    }
    async fn session_state(&self, session: SessionId) -> Result<SessionState, BridgeError> {
        self.inner.session_state(session).await
    }
    async fn close_session(&self, session: SessionId) -> Result<(), CloseSessionError> {
        self.inner.close_session(session).await
    }
    async fn reply_to_tool(
        &self,
        session: SessionId,
        call_id: String,
        payload: ToolResultPayload,
    ) -> Result<(), ReplyToToolError> {
        self.inner.reply_to_tool(session, call_id, payload).await
    }
    async fn reply_to_permission(
        &self,
        session: SessionId,
        request_id: String,
        response: RequestPermissionResponse,
    ) -> Result<(), ReplyToPermissionError> {
        self.inner
            .reply_to_permission(session, request_id, response)
            .await
    }
    fn subscribe(&self) -> tokio::sync::broadcast::Receiver<BridgeEvent> {
        self.inner.subscribe()
    }
}
