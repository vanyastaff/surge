//! Host-only effect admission retained by ACP sessions and MCP tasks.

use std::sync::Arc;
use surge_acp::bridge::{BridgeFacade, HostEffectFence, HostEffectRefused};
use surge_persistence::work_items::{WorkItemLaunchClaim, WorkItemStore};

pub(crate) struct OwnedFlowEffectFence {
    store: WorkItemStore,
    claim: WorkItemLaunchClaim,
}

impl OwnedFlowEffectFence {
    pub(crate) fn new(store: WorkItemStore, claim: WorkItemLaunchClaim) -> Self {
        Self { store, claim }
    }
}

impl HostEffectFence for OwnedFlowEffectFence {
    fn check(&self) -> Result<(), HostEffectRefused> {
        self.store
            .validate_owned_flow_effect(&self.claim)
            .map_err(|_| HostEffectRefused)
    }
}

pub(crate) fn bridge(
    inner: Arc<dyn BridgeFacade>,
    fence: Option<Arc<OwnedFlowEffectFence>>,
) -> Arc<dyn BridgeFacade> {
    match fence {
        Some(fence) => Arc::new(OwnedBridge { inner, fence }),
        None => inner,
    }
}

struct OwnedBridge {
    inner: Arc<dyn BridgeFacade>,
    fence: Arc<OwnedFlowEffectFence>,
}

#[async_trait::async_trait]
impl BridgeFacade for OwnedBridge {
    fn legacy_stage_event_adapter(&self) -> bool {
        self.inner.legacy_stage_event_adapter()
    }

    async fn open_session(
        &self,
        mut config: surge_acp::bridge::SessionConfig,
    ) -> Result<surge_core::execution_recovery::OpenedSession, surge_acp::bridge::OpenSessionError>
    {
        self.fence.check()?;
        config.effect_fence = Some(self.fence.clone());
        self.inner.open_session(config).await
    }

    async fn send_message(
        &self,
        session: surge_core::SessionId,
        content: surge_acp::bridge::MessageContent,
    ) -> Result<(), surge_acp::bridge::SendMessageError> {
        self.fence.check()?;
        self.inner.send_message(session, content).await
    }

    async fn session_state(
        &self,
        session: surge_core::SessionId,
    ) -> Result<surge_acp::bridge::SessionState, surge_acp::bridge::BridgeError> {
        self.inner.session_state(session).await
    }

    async fn close_session(
        &self,
        session: surge_core::SessionId,
    ) -> Result<(), surge_acp::bridge::CloseSessionError> {
        self.inner.close_session(session).await
    }

    async fn reply_to_tool(
        &self,
        session: surge_core::SessionId,
        call_id: String,
        payload: surge_acp::bridge::ToolResultPayload,
    ) -> Result<(), surge_acp::bridge::ReplyToToolError> {
        self.inner.reply_to_tool(session, call_id, payload).await
    }

    async fn reply_to_permission(
        &self,
        session: surge_core::SessionId,
        request_id: String,
        response: surge_acp::bridge::RequestPermissionResponse,
    ) -> Result<(), surge_acp::bridge::ReplyToPermissionError> {
        // The actual client checks the offered option's semantics after its
        // approval await. Denial/cancellation remains available as cleanup.
        self.inner
            .reply_to_permission(session, request_id, response)
            .await
    }

    fn subscribe(&self) -> tokio::sync::broadcast::Receiver<surge_acp::bridge::BridgeEvent> {
        self.inner.subscribe()
    }
}
