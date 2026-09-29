//! `AcpBridge` — owned by callers, hides the worker thread + LocalSet.
//!
//! See spec §5.1 for the spawn machinery rationale, §11.6 for per-process
//! count guidance, §11.8 for the lagged-subscriber contract.

use std::{sync::Arc, time::Duration};
use surge_core::SessionId;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use super::command::BridgeCommand;
use super::error::{
    BridgeError, CloseSessionError, OpenSessionError, ReplyToPermissionError, ReplyToToolError,
    SendMessageError,
};
use super::event::BridgeEvent;
use super::session::{MessageContent, SessionConfig, SessionState};
/// Deadlines for session setup and verified worker shutdown. Prompts are unbounded.
#[derive(Clone, Copy, Debug)]
pub struct BridgeTimeouts {
    /// Total initialize plus new-session deadline.
    pub handshake: Duration,
    /// Maximum wait for verified cleanup before returning an explicit error.
    pub shutdown: Duration,
}
impl Default for BridgeTimeouts {
    fn default() -> Self {
        Self {
            // `npx`-launched adapters resolve the package before they speak
            // ACP; on a cold cache or a loaded machine that alone exceeded
            // 30s and failed runs before their first stage. The deadline
            // only bounds startup — a dead adapter still fails, just later.
            handshake: Duration::from_secs(120),
            shutdown: Duration::from_secs(8),
        }
    }
}

/// Public handle to the ACP bridge worker thread.
///
/// Spawn one per process (see spec §11.6); methods can be called from any
/// tokio context. All work funnels through a dedicated OS thread that runs
/// a current-thread tokio runtime + `LocalSet` for the SDK's `!Send` futures.
///
/// `Drop` signals cleanup without blocking. Call `shutdown().await` to verify
/// that children were reaped and the dedicated worker exited.
pub struct AcpBridge {
    /// Command channel sender — bounded mpsc.
    cmd_tx: mpsc::Sender<BridgeCommand>,
    /// Broadcast sender for `BridgeEvent`s. Subscribers obtain receivers via
    /// `subscribe()`. Best-effort observability per spec §11.8.
    event_tx: broadcast::Sender<BridgeEvent>,
    /// Worker thread handle; joined only after verified thread exit.
    worker: Option<std::thread::JoinHandle<Result<(), BridgeError>>>,
    shutdown: CancellationToken,
    timeouts: BridgeTimeouts,
    open_slots: Arc<tokio::sync::Semaphore>,
}

impl AcpBridge {
    /// Spawn the bridge worker thread with explicit channel capacities.
    ///
    /// `cmd_capacity` bounds the mpsc command channel; producers block on
    /// `send().await` if the worker can't drain fast enough. `event_capacity`
    /// bounds the broadcast channel; subscribers that lag past this silently
    /// drop oldest events (see spec §11.8 for the durable-consumer pattern).
    pub fn spawn(cmd_capacity: usize, event_capacity: usize) -> Result<Self, BridgeError> {
        Self::spawn_with_timeouts(cmd_capacity, event_capacity, BridgeTimeouts::default())
    }

    /// Spawn with explicit setup and cleanup deadlines.
    pub fn spawn_with_timeouts(
        cmd_capacity: usize,
        event_capacity: usize,
        timeouts: BridgeTimeouts,
    ) -> Result<Self, BridgeError> {
        if cmd_capacity == 0
            || event_capacity == 0
            || timeouts.handshake.is_zero()
            || timeouts.shutdown.is_zero()
        {
            return Err(BridgeError::InvalidConfig);
        }
        let (cmd_tx, cmd_rx) = mpsc::channel(cmd_capacity);
        let (event_tx, _) = broadcast::channel(event_capacity);
        let events = event_tx.clone();
        let shutdown = CancellationToken::new();
        let signal = shutdown.clone();
        let worker = std::thread::Builder::new()
            .name("surge-acp-bridge".into())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|_| BridgeError::WorkerDead)?;
                tokio::task::LocalSet::new().block_on(
                    &runtime,
                    super::lifecycle::run(cmd_rx, events, signal, timeouts),
                )
            })
            .map_err(|_| BridgeError::WorkerDead)?;
        Ok(Self {
            cmd_tx,
            event_tx,
            worker: Some(worker),
            shutdown,
            timeouts,
            open_slots: Arc::new(tokio::sync::Semaphore::new(cmd_capacity)),
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
    pub async fn open_session(&self, config: SessionConfig) -> Result<SessionId, OpenSessionError> {
        let (tx, rx) = oneshot::channel();
        let permit = tokio::select! {
            biased;
            () = self.shutdown.cancelled() => return Err(OpenSessionError::Cancelled),
            permit = tokio::time::timeout(self.timeouts.handshake + self.timeouts.shutdown, self.open_slots.clone().acquire_owned()) => {
                permit.map_err(|_| OpenSessionError::Bridge(BridgeError::CleanupUnconfirmed))?
                    .map_err(|_| OpenSessionError::Cancelled)?
            },
        };
        self.cmd_tx
            .send(BridgeCommand::OpenSession {
                config,
                reply: tx,
                permit,
            })
            .await
            .map_err(|e| OpenSessionError::Bridge(BridgeError::CommandSendFailed(e.to_string())))?;
        tokio::time::timeout(self.timeouts.handshake + self.timeouts.shutdown, rx)
            .await
            .map_err(|_| OpenSessionError::Bridge(BridgeError::CleanupUnconfirmed))?
            .map_err(|_| OpenSessionError::Bridge(BridgeError::ReplyDropped))?
    }

    /// Send a user message and await the authoritative prompt result.
    /// Streaming events and permission requests arrive while this call is pending.
    /// A second concurrent prompt for the same session is rejected.
    pub async fn send_message(
        &self,
        session: SessionId,
        content: MessageContent,
    ) -> Result<(), SendMessageError> {
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .send(BridgeCommand::SendMessage {
                session,
                content,
                reply: tx,
            })
            .await
            .map_err(|e| SendMessageError::Bridge(BridgeError::CommandSendFailed(e.to_string())))?;
        rx.await
            .map_err(|_| SendMessageError::Bridge(BridgeError::ReplyDropped))?
    }

    /// Read a session's bridge-observable state (open / closed / crashed).
    pub async fn session_state(&self, session: SessionId) -> Result<SessionState, BridgeError> {
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .send(BridgeCommand::GetSessionState { session, reply: tx })
            .await
            .map_err(|e| BridgeError::CommandSendFailed(e.to_string()))?;
        rx.await.map_err(|_| BridgeError::ReplyDropped)?
    }

    /// Close a session and verify child cleanup. For an active prompt, attempt
    /// ACP `session/cancel` and wait up to 500 ms for its prompt RPC to settle.
    /// Then tear down transport, allow a five-second process-exit grace period,
    /// and kill/reap the child if needed. ACP cancellation is not process shutdown.
    /// A concurrent close during cleanup returns `CleanupUnconfirmed`;
    /// success confirms cleanup has settled.
    pub async fn close_session(&self, session: SessionId) -> Result<(), CloseSessionError> {
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .send(BridgeCommand::CloseSession { session, reply: tx })
            .await
            .map_err(|e| {
                CloseSessionError::Bridge(BridgeError::CommandSendFailed(e.to_string()))
            })?;
        tokio::time::timeout(self.timeouts.shutdown, rx)
            .await
            .map_err(|_| CloseSessionError::Bridge(BridgeError::CleanupUnconfirmed))?
            .map_err(|_| CloseSessionError::Bridge(BridgeError::ReplyDropped))?
    }

    /// Send a reply to an outstanding tool call. Used by M5 engine to:
    /// - Reply to dispatcher tool calls (`read_file`, `write_file`, `shell_exec`, …)
    /// - Reply to `request_human_input` with the human's actual answer
    /// - Reply to `report_stage_outcome` with `Ok` after persisting the outcome
    ///
    /// On success: removes the `call_id` from the session's pending-replies
    /// map and broadcasts `BridgeEvent::ToolResult { session, call_id, payload }`
    /// for observability subscribers (engine event-log persisters, telemetry).
    ///
    /// **ACP wire-level caveat.** ACP v1 (SDK 0.10.4) has no client→agent
    /// "tool result" RPC method; `SessionUpdate::ToolCall` is a one-way
    /// agent→client notification. This method therefore does NOT deliver the
    /// payload to the agent subprocess at the wire level — it's purely
    /// internal Surge bookkeeping that closes the call-id loop and surfaces
    /// the engine's result to observers. If a future Surge milestone needs
    /// out-of-band tool delivery to the agent, the natural extension point
    /// is `connection.ext_notification(...)` with a vendor-specific method.
    ///
    /// # Errors
    /// - [`ReplyToToolError::SessionGone`] — no session with this id is
    ///   currently open in the bridge.
    /// - [`ReplyToToolError::UnknownCallId`] — the session is open but the
    ///   `call_id` does not match any pending tool call (e.g. already replied,
    ///   or the agent never fired this id).
    /// - [`ReplyToToolError::Bridge`] — the worker thread is dead or the
    ///   command channel is closed.
    pub async fn reply_to_tool(
        &self,
        session: SessionId,
        call_id: String,
        payload: crate::bridge::event::ToolResultPayload,
    ) -> Result<(), ReplyToToolError> {
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .send(BridgeCommand::ReplyToTool {
                session,
                call_id,
                payload,
                reply: tx,
            })
            .await
            .map_err(|e| ReplyToToolError::Bridge(BridgeError::CommandSendFailed(e.to_string())))?;
        rx.await
            .map_err(|_| ReplyToToolError::Bridge(BridgeError::ReplyDropped))?
    }

    /// Resolve an outstanding ACP permission request.
    ///
    /// Pass the `request_id` from the matching `BridgeEvent::PermissionRequested`
    /// event plus the operator's decision; the bridge fulfils the parked
    /// oneshot and the agent receives `response`. Pairs with the elevation
    /// roundtrip wired in `BridgeClient::request_permission`.
    ///
    /// # Errors
    ///
    /// - [`ReplyToPermissionError::SessionGone`] — the session is no longer
    ///   open in the bridge.
    /// - [`ReplyToPermissionError::UnknownRequestId`] — the session is open
    ///   but the `request_id` does not match a pending request (already
    ///   resolved, timed out, or never issued).
    /// - [`ReplyToPermissionError::Bridge`] — the worker thread is dead or
    ///   the command channel is closed.
    pub async fn reply_to_permission(
        &self,
        session: SessionId,
        request_id: String,
        response: agent_client_protocol::schema::v1::RequestPermissionResponse,
    ) -> Result<(), ReplyToPermissionError> {
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .send(BridgeCommand::ReplyToPermission {
                session,
                request_id,
                response,
                reply: tx,
            })
            .await
            .map_err(|e| {
                ReplyToPermissionError::Bridge(BridgeError::CommandSendFailed(e.to_string()))
            })?;
        rx.await
            .map_err(|_| ReplyToPermissionError::Bridge(BridgeError::ReplyDropped))?
    }

    /// Test-only: inject a panic into the worker thread to verify that
    /// subsequent commands fail with `BridgeError::CommandSendFailed` or
    /// `ReplyDropped`. Gated by `#[cfg(any(test, feature = "test-helpers"))]`
    /// so production builds cannot accidentally call it.
    #[cfg(any(test, feature = "test-helpers"))]
    pub fn __test_panic_now(&self) {
        // Best-effort send; channel may already be closed.
        let cmd_tx = self.cmd_tx.clone();
        tokio::spawn(async move {
            let _ = cmd_tx.send(BridgeCommand::TestPanic).await;
        });
    }

    /// Signal shutdown out of band and verify worker exit and child cleanup.
    /// Timeout is an error; the detached worker continues owning cleanup.
    pub async fn shutdown(mut self) -> Result<(), BridgeError> {
        self.shutdown.cancel();
        let deadline = tokio::time::Instant::now() + self.timeouts.shutdown;
        while self
            .worker
            .as_ref()
            .is_some_and(|worker| !worker.is_finished())
        {
            if tokio::time::Instant::now() >= deadline {
                return Err(BridgeError::CleanupUnconfirmed);
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        self.worker
            .take()
            .ok_or(BridgeError::WorkerDead)?
            .join()
            .map_err(|_| BridgeError::WorkerDead)?
    }
}

impl Drop for AcpBridge {
    fn drop(&mut self) {
        self.shutdown.cancel();
        // Never wait on the caller thread. The worker owns its runtime and children.
        if let Some(worker) = self.worker.take()
            && worker.is_finished()
        {
            let _ = worker.join();
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

    /// Regression test for a `Drop` deadlock that surfaced under nextest as
    /// "three integration tests hang for >120s" on PR #40.
    ///
    /// The bug: custom `Drop::drop(&mut self)` runs BEFORE the struct's fields
    /// are dropped, so calling `worker.join()` directly inside `Drop` blocks
    /// forever — the worker thread is parked on `cmd_rx.recv().await`, and
    /// `cmd_tx` (a field of the same struct) is still alive at that point.
    /// The hang only manifested when a test panicked before reaching
    /// `shutdown().await`, which is exactly what happens when an integration
    /// test fails an assertion or hits `AgentSpawnFailed`.
    ///
    /// This test exercises the panic-path drop directly: no `shutdown()`,
    /// just create a bridge and drop it. Pre-fix this hangs forever; post-fix
    /// it returns within milliseconds.
    #[tokio::test(flavor = "multi_thread")]
    async fn drop_without_shutdown_does_not_deadlock() {
        let start = std::time::Instant::now();
        {
            let _bridge = AcpBridge::with_defaults().unwrap();
            // Intentionally NO `shutdown().await` — exercises the bare-Drop path.
        }
        let elapsed = start.elapsed();
        assert!(
            elapsed < std::time::Duration::from_secs(5),
            "AcpBridge::Drop took {elapsed:?} (>5s) — drop deadlock regression. \
             See `Drop for AcpBridge` for the cmd_tx-before-join ordering fix."
        );
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
            config_selections: Vec::new(),
            stage_mcp: None,
            agent_kind: AgentKind::Mock { args: vec![] },
            working_dir: std::path::PathBuf::from("/tmp/wt"),
            system_prompt: "sys".into(),
            declared_outcomes: vec![OutcomeKey::from_str("done").unwrap()],
            allows_escalation: false,
            tools: vec![],
            sandbox: Box::new(AlwaysAllowSandbox),
            permission_policy: PermissionPolicy::default(),
            bindings: Default::default(),
            env: Default::default(),
        };
        let err = bridge.open_session(cfg).await.unwrap_err();
        // Phase 8.1 replaces the Phase 6 HandshakeFailed stub with the real
        // open_session_impl. The mock_acp_agent binary does not exist yet
        // (Phase 9), so the spawn fails here. Once Phase 9 lands the binary,
        // this test should be replaced by a full `SessionEstablished` lifecycle
        // test in `tests/bridge_session_lifecycle.rs`.
        assert!(matches!(err, OpenSessionError::AgentSpawnFailed { .. }));
        bridge.shutdown().await.unwrap();
    }
}
