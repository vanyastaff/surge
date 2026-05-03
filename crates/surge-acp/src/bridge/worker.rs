//! Bridge worker — owns the session map, dispatches commands.
//! Runs on the dedicated bridge thread inside a `LocalSet`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use surge_core::SessionId;
use tokio::sync::{broadcast, mpsc};
use tracing::{debug, info};

use super::command::BridgeCommand;
use super::event::{BridgeEvent, SessionEndReason};

/// Per-session state held by the worker. Filled in by Phase 7.
pub(crate) struct AcpSession {
    pub session_id: SessionId,
    pub agent_label: String,
    // ACP-side connection, child handle, observer/waiter task handles, etc.
    // are added in Phase 7.
}

pub(crate) type SessionMap = Rc<RefCell<HashMap<SessionId, AcpSession>>>;

/// Main worker loop. Drains commands from `cmd_rx`, dispatches them, and
/// emits `BridgeEvent`s to subscribers. Returns when `Shutdown` is processed
/// or the channel closes.
///
/// Phase 6 ships a skeleton: most commands return immediate stub errors;
/// Phase 7+ replaces those arms with real handlers (`open_session_impl` etc).
pub async fn bridge_loop(
    mut cmd_rx: mpsc::Receiver<BridgeCommand>,
    event_tx: broadcast::Sender<BridgeEvent>,
) {
    info!("bridge worker entering main loop");
    let sessions: SessionMap = Rc::default();

    while let Some(cmd) = cmd_rx.recv().await {
        match cmd {
            BridgeCommand::OpenSession { reply, .. } => {
                // Phase 7 lands the real impl. For now, refuse cleanly so the
                // skeleton still tests the dispatch path.
                let _ = reply.send(Err(super::error::OpenSessionError::HandshakeFailed {
                    reason: "open_session not implemented in M3 skeleton".into(),
                }));
            }
            BridgeCommand::SendMessage { session, reply, .. } => {
                let _ = reply.send(Err(super::error::SendMessageError::SessionNotFound { session }));
            }
            BridgeCommand::GetSessionState { session, reply } => {
                // Phase 6 stub: returns the bridge-observable state if the session
                // exists, else `BridgeError::ReplyDropped` as a stand-in. Phase 7
                // replaces this with proper not-found semantics once `BridgeError`
                // gains a `SessionNotFound` variant or `session_state` switches to
                // `Result<Option<SessionState>, _>`.
                let state = sessions
                    .borrow()
                    .get(&session)
                    .map(|s| super::session::SessionState {
                        session_id: s.session_id.clone(),
                        agent_label: s.agent_label.clone(),
                        status: super::session::SessionStatus::Open,
                        bindings: Default::default(),
                    });
                let _ = reply.send(state.ok_or(super::error::BridgeError::ReplyDropped));
            }
            BridgeCommand::CloseSession { session, reply } => {
                let _ = reply.send(Err(super::error::CloseSessionError::SessionNotFound { session }));
            }
            BridgeCommand::Shutdown { reply } => {
                close_all_sessions(&sessions, &event_tx, SessionEndReason::ForcedClose).await;
                let _ = reply.send(());
                info!("bridge worker shutting down");
                return;
            }
            #[cfg(test)]
            BridgeCommand::TestPanic => {
                panic!("bridge worker test-panic injected");
            }
        }
    }

    debug!("command channel closed; bridge worker exiting");
}

/// Emit `SessionEnded` for every open session and drop them from the map.
/// Used by `Shutdown` and (later) by failure paths in Phase 7+.
pub(crate) async fn close_all_sessions(
    sessions: &SessionMap,
    event_tx: &broadcast::Sender<BridgeEvent>,
    reason: SessionEndReason,
) {
    let to_close: Vec<SessionId> = sessions.borrow().keys().cloned().collect();
    for sid in to_close {
        // Best-effort emit; broadcast::send returns Err only when no
        // subscribers exist, which is acceptable during shutdown.
        let _ = event_tx.send(BridgeEvent::SessionEnded {
            session: sid.clone(),
            reason: reason.clone(),
        });
        sessions.borrow_mut().remove(&sid);
    }
}
