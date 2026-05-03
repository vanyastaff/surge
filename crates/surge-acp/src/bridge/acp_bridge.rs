//! `AcpBridge` — owned by callers, hides the worker thread + LocalSet.
//!
//! See spec §5.1 for the spawn machinery rationale, §11.6 for per-process
//! count guidance, §11.8 for the lagged-subscriber contract.

use surge_core::SessionId;
use tokio::sync::{broadcast, mpsc, oneshot};
use tracing::warn;

use super::command::BridgeCommand;
use super::error::{BridgeError, CloseSessionError, OpenSessionError, SendMessageError};
use super::event::BridgeEvent;
use super::session::{MessageContent, SessionConfig, SessionState};
use super::worker::bridge_loop;

/// Public handle to the ACP bridge worker thread.
///
/// Spawn one per process (see spec §11.6); methods can be called from any
/// tokio context. All work funnels through a dedicated OS thread that runs
/// a current-thread tokio runtime + `LocalSet` for the SDK's `!Send` futures.
///
/// `Drop` lets the worker thread terminate when the last command sender
/// goes out of scope. For deterministic shutdown, prefer `shutdown().await`.
pub struct AcpBridge {
    /// Command channel sender — bounded mpsc.
    cmd_tx: mpsc::Sender<BridgeCommand>,
    /// Broadcast sender for `BridgeEvent`s. Subscribers obtain receivers via
    /// `subscribe()`. Best-effort observability per spec §11.8.
    event_tx: broadcast::Sender<BridgeEvent>,
    /// Worker thread handle. `Some` until `shutdown()` consumes it; `Drop`
    /// joins it best-effort if `shutdown()` was not called.
    worker: Option<std::thread::JoinHandle<()>>,
}

impl AcpBridge {
    /// Spawn the bridge worker thread with explicit channel capacities.
    ///
    /// `cmd_capacity` bounds the mpsc command channel; producers block on
    /// `send().await` if the worker can't drain fast enough. `event_capacity`
    /// bounds the broadcast channel; subscribers that lag past this silently
    /// drop oldest events (see spec §11.8 for the durable-consumer pattern).
    pub fn spawn(cmd_capacity: usize, event_capacity: usize) -> Result<Self, BridgeError> {
        let (cmd_tx, cmd_rx) = mpsc::channel(cmd_capacity);
        let (event_tx, _) = broadcast::channel(event_capacity);
        let event_tx_for_worker = event_tx.clone();

        let thread = std::thread::Builder::new()
            .name("surge-acp-bridge".into())
            .spawn(move || {
                let rt = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(e) => {
                        warn!("bridge worker failed to build runtime: {e}");
                        return;
                    }
                };
                let local = tokio::task::LocalSet::new();
                local.block_on(&rt, bridge_loop(cmd_rx, event_tx_for_worker));
            })
            .map_err(|_| BridgeError::WorkerDead)?;

        Ok(Self {
            cmd_tx,
            event_tx,
            worker: Some(thread),
        })
    }

    /// Spawn with sane default capacities (64 commands queued, 1024 events buffered).
    /// Defaults chosen per spec §5.1 — high enough to absorb burst traffic from
    /// open_session bootstrapping, low enough to surface backpressure quickly.
    pub fn with_defaults() -> Result<Self, BridgeError> {
        Self::spawn(64, 1024)
    }

    /// Subscribe to the bridge's event stream.
    ///
    /// **Important:** broadcast is best-effort observability. Lagging
    /// subscribers silently drop the oldest events. Consumers that need
    /// durable delivery (M5 engine event-log persistence) MUST add their own
    /// backpressure. See spec §11.8.
    pub fn subscribe(&self) -> broadcast::Receiver<BridgeEvent> {
        self.event_tx.subscribe()
    }

    /// Open a new ACP session. The bridge spawns the agent subprocess,
    /// performs the ACP handshake, declares the sandbox-filtered tool list,
    /// and returns the freshly-allocated `SessionId`.
    pub async fn open_session(
        &self,
        config: SessionConfig,
    ) -> Result<SessionId, OpenSessionError> {
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .send(BridgeCommand::OpenSession { config, reply: tx })
            .await
            .map_err(|e| OpenSessionError::Bridge(BridgeError::CommandSendFailed(e.to_string())))?;
        rx.await
            .map_err(|_| OpenSessionError::Bridge(BridgeError::ReplyDropped))?
    }

    /// Send a user message to an open session. Returns once the bridge has
    /// queued the message; the agent's response surfaces via subsequent
    /// `BridgeEvent::AgentMessage` events.
    pub async fn send_message(
        &self,
        session: SessionId,
        content: MessageContent,
    ) -> Result<(), SendMessageError> {
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .send(BridgeCommand::SendMessage { session, content, reply: tx })
            .await
            .map_err(|e| SendMessageError::Bridge(BridgeError::CommandSendFailed(e.to_string())))?;
        rx.await
            .map_err(|_| SendMessageError::Bridge(BridgeError::ReplyDropped))?
    }

    /// Read a session's bridge-observable state (open / closed / crashed).
    pub async fn session_state(
        &self,
        session: SessionId,
    ) -> Result<SessionState, BridgeError> {
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .send(BridgeCommand::GetSessionState { session, reply: tx })
            .await
            .map_err(|e| BridgeError::CommandSendFailed(e.to_string()))?;
        rx.await.map_err(|_| BridgeError::ReplyDropped)?
    }

    /// Close a session gracefully. The bridge sends ACP shutdown to the
    /// agent and waits up to a grace period before forcibly killing the
    /// child (see Phase 8.3 close_session_impl for the timeout details).
    pub async fn close_session(
        &self,
        session: SessionId,
    ) -> Result<(), CloseSessionError> {
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .send(BridgeCommand::CloseSession { session, reply: tx })
            .await
            .map_err(|e| CloseSessionError::Bridge(BridgeError::CommandSendFailed(e.to_string())))?;
        rx.await
            .map_err(|_| CloseSessionError::Bridge(BridgeError::ReplyDropped))?
    }

    /// Drain pending commands and shut down the worker. Open sessions emit
    /// `SessionEnded { reason: ForcedClose }`. Joins the worker thread.
    /// Consumes self — call exactly once.
    pub async fn shutdown(mut self) -> Result<(), BridgeError> {
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .send(BridgeCommand::Shutdown { reply: tx })
            .await
            .map_err(|e| BridgeError::CommandSendFailed(e.to_string()))?;
        rx.await.map_err(|_| BridgeError::ReplyDropped)?;
        if let Some(t) = self.worker.take() {
            t.join().map_err(|_| BridgeError::WorkerDead)?;
        }
        Ok(())
    }
}

impl Drop for AcpBridge {
    fn drop(&mut self) {
        // No await possible in Drop. Dropping the only owned cmd_tx
        // (when `self` is dropped) closes the channel, causing the worker's
        // `cmd_rx.recv()` to return `None` and the loop to exit. We then
        // best-effort join the thread.
        if let Some(handle) = self.worker.take() {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn spawn_then_shutdown_clean() {
        let bridge = AcpBridge::with_defaults().unwrap();
        bridge.shutdown().await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn subscribe_yields_no_events_on_idle_bridge() {
        let bridge = AcpBridge::with_defaults().unwrap();
        let mut rx = bridge.subscribe();
        // No events expected — spawn does not emit anything on its own.
        let r = tokio::time::timeout(std::time::Duration::from_millis(50), rx.recv()).await;
        assert!(r.is_err(), "unexpected event on idle bridge");
        bridge.shutdown().await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn open_session_returns_skeleton_error() {
        use crate::bridge::sandbox::AlwaysAllowSandbox;
        use crate::bridge::session::AgentKind;
        use crate::client::PermissionPolicy;
        use std::str::FromStr;
        use surge_core::OutcomeKey;

        let bridge = AcpBridge::with_defaults().unwrap();
        let cfg = SessionConfig {
            agent_kind: AgentKind::Mock { args: vec![] },
            working_dir: std::path::PathBuf::from("/tmp/wt"),
            system_prompt: "sys".into(),
            declared_outcomes: vec![OutcomeKey::from_str("done").unwrap()],
            allows_escalation: false,
            tools: vec![],
            sandbox: Box::new(AlwaysAllowSandbox),
            permission_policy: PermissionPolicy::default(),
            bindings: Default::default(),
        };
        let err = bridge.open_session(cfg).await.unwrap_err();
        // Phase 6 stub returns HandshakeFailed; Phase 7 replaces with real impl.
        assert!(matches!(err, OpenSessionError::HandshakeFailed { .. }));
        bridge.shutdown().await.unwrap();
    }
}
