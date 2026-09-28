//! Single-owner control loop. Only this loop changes the session map.
use super::acp_bridge::BridgeTimeouts;
use super::worker::{self, AcpSession, SessionMap};
use super::{
    BridgeCommand, BridgeError, BridgeEvent, CloseSessionError, OpenSessionError, SendMessageError,
    SessionEndReason,
};

use futures::{StreamExt, stream::FuturesUnordered};
use std::{rc::Rc, time::Duration};
use surge_core::SessionId;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio_util::sync::CancellationToken;

type OpenReply = oneshot::Sender<Result<SessionId, OpenSessionError>>;
type PromptReply = oneshot::Sender<Result<(), SendMessageError>>;
type CloseReply = oneshot::Sender<Result<(), CloseSessionError>>;

struct OpenDone {
    result: Result<AcpSession, OpenSessionError>,
    reply: OpenReply,
}
struct PromptDone {
    session: SessionId,
    result: Result<(), SendMessageError>,
    reply: PromptReply,
}
struct CloseDone {
    session: SessionId,
    reason: SessionEndReason,
    result: Result<(), CloseSessionError>,
    reply: Option<CloseReply>,
}

/// Owns child exit evidence, independently of command handling.
pub(crate) struct ProcessExit {
    pub status: Result<std::process::ExitStatus, String>,
    pub kill_error: Option<String>,
}

pub(super) async fn run(
    mut commands: mpsc::Receiver<BridgeCommand>,
    events: broadcast::Sender<BridgeEvent>,
    shutdown: CancellationToken,
    timeouts: BridgeTimeouts,
) -> Result<(), BridgeError> {
    let sessions: SessionMap = Rc::default();
    let mut openings = FuturesUnordered::new();
    let mut prompts = FuturesUnordered::new();
    let mut closings = FuturesUnordered::new();
    let mut closing_ids = std::collections::HashSet::new();
    let (exits, mut exited) = mpsc::unbounded_channel();
    let mut stopping = false;
    let mut cleanup_failed = false;
    let mut shutdown_replies: Vec<oneshot::Sender<()>> = Vec::new();
    loop {
        if stopping
            && sessions.borrow().is_empty()
            && openings.is_empty()
            && prompts.is_empty()
            && closings.is_empty()
        {
            if !cleanup_failed {
                for reply in shutdown_replies {
                    let _ = reply.send(());
                }
            }
            return if cleanup_failed {
                Err(BridgeError::CleanupUnconfirmed)
            } else {
                Ok(())
            };
        }
        tokio::select! {
            biased;
            () = shutdown.cancelled(), if !stopping => {
                stopping = true;
                commands.close();
                while commands.try_recv().is_ok() {}
                let owned: Vec<_> = sessions.borrow_mut().drain().map(|(_, value)| value).collect();
                for session in owned { closing_ids.insert(session.session_id); closings.push(tokio::task::spawn_local(close(session, true, None))); }
            },
            Some(done) = openings.next(), if !openings.is_empty() => {
                match done {
                    Ok(OpenDone { result: Ok(mut session), reply }) => {
                        let id = session.session_id;
                        let exited_before_delivery = match session.child.as_mut().map(tokio::process::Child::try_wait) {
                            Some(Ok(None)) => false,
                            Some(Ok(Some(_))) | Some(Err(_)) | None => true,
                        };
                        install_waiter(&mut session, exits.clone());
                        if exited_before_delivery {
                            closing_ids.insert(id);
                            closings.push(tokio::task::spawn_local(close(session, true, None)));
                            let _ = reply.send(Err(OpenSessionError::HandshakeFailed { reason: "agent exited or process state unavailable before session delivery".into() }));
                        } else if stopping || reply.is_closed() {
                            closing_ids.insert(session.session_id); closings.push(tokio::task::spawn_local(close(session, true, None)));
                        } else {
                            let established = session.established.take();
                            sessions.borrow_mut().insert(id, session);
                            if reply.send(Ok(id)).is_err() {
                                if let Some(session) = sessions.borrow_mut().remove(&id) {
                                    closing_ids.insert(session.session_id); closings.push(tokio::task::spawn_local(close(session, true, None)));
                                }
                            } else if let Some(event) = established { let _ = events.send(event); }
                        }
                    },
                    Ok(OpenDone { result: Err(error), reply }) => { let _ = reply.send(Err(error)); },
                    Err(error) => { tracing::error!(%error, "ACP opening task failed"); cleanup_failed = true; shutdown.cancel(); },
                }
            },
            Some(done) = prompts.next(), if !prompts.is_empty() => {
                if let Ok(PromptDone { session, result, reply }) = done {
                    if let Some(entry) = sessions.borrow_mut().get_mut(&session) { entry.prompt_running = false; }
                    let _ = reply.send(result);
                } else { cleanup_failed = true; shutdown.cancel(); }
            },
            Some(done) = closings.next(), if !closings.is_empty() => {
                match done {
                    Ok(done) => {
                        closing_ids.remove(&done.session);
                        cleanup_failed |= matches!(&done.result, Err(CloseSessionError::Bridge(_)));
                        let _ = events.send(BridgeEvent::SessionEnded { session: done.session, reason: done.reason });
                        if let Some(reply) = done.reply { let _ = reply.send(done.result); }
                    },
                    Err(error) => { tracing::error!(%error, "ACP cleanup task failed"); cleanup_failed = true; shutdown.cancel(); },
                }
            },
            Some(id) = exited.recv() => {
                if let Some(session) = sessions.borrow_mut().remove(&id) {
                    closing_ids.insert(session.session_id); closings.push(tokio::task::spawn_local(close(session, false, None)));
                }
            },
            command = commands.recv(), if !stopping => {
                let Some(command) = command else { shutdown.cancel(); continue; };
                match command {
                    BridgeCommand::OpenSession { config, mut reply, permit } => {
                        let token = shutdown.clone();
                        let tx = events.clone();
                        openings.push(tokio::task::spawn_local(async move {
                            let _permit = permit;
                            let result = worker::open_session_impl(&tx, config, &token, &mut reply, timeouts.handshake).await;
                            OpenDone { result, reply }
                        }));
                    },
                    BridgeCommand::SendMessage { session, content, reply } => {
                        let ready = {
                            let mut map = sessions.borrow_mut();
                            match map.get_mut(&session) {
                                None => Err(SendMessageError::SessionNotFound { session }),
                                Some(entry) if entry.prompt_running => Err(SendMessageError::PromptAlreadyRunning { session }),
                                Some(entry) => {
                                    entry.prompt_running = true;
                                    entry.prompt_done = CancellationToken::new();
                                    Ok((entry.connection.clone(), entry.inner.borrow().acp_session_id.clone(), entry.cancel.clone(), entry.prompt_done.clone(), entry.secrets.clone()))
                                },
                            }
                        };
                        match ready {
                            Err(error) => { let _ = reply.send(Err(error)); },
                            Ok((connection, acp_id, cancel, finished, secrets)) => {
                                prompts.push(tokio::task::spawn_local(async move {
                                    let _finished = finished.drop_guard();
                                    let result = tokio::select! {
                                        biased;
                                        () = cancel.cancelled() => Err(SendMessageError::SessionEnded { session, reason: SessionEndReason::ForcedClose }),
                                        result = worker::send_message_impl(connection, acp_id, session, content, secrets) => result,
                                    };
                                    PromptDone { session, result, reply }
                                }));
                            },
                        }
                    },
                    BridgeCommand::CloseSession { session, reply } => {
                        let owned = sessions.borrow_mut().remove(&session);
                        if let Some(owned) = owned { closing_ids.insert(session); closings.push(tokio::task::spawn_local(close(owned, false, Some(reply)))); }
                        else if closing_ids.contains(&session) || cleanup_failed {
                            let _ = reply.send(Err(CloseSessionError::Bridge(BridgeError::CleanupUnconfirmed)));
                        } else { let _ = reply.send(Ok(())); }
                    },
                    BridgeCommand::GetSessionState { session, reply } => {
                        let state = sessions.borrow().get(&session).map(|entry| super::SessionState {
                            session_id: session, agent_label: entry.agent_label.clone(), status: super::SessionStatus::Open, bindings: Default::default(),
                        });
                        let _ = reply.send(state.ok_or(BridgeError::ReplyDropped));
                    },
                    BridgeCommand::ReplyToTool { session, call_id, payload, reply } => {
                        let _ = reply.send(worker::reply_to_tool_impl(&sessions, &events, session, call_id, payload));
                    },
                    BridgeCommand::ReplyToPermission { session, request_id, response, reply } => {
                        let _ = reply.send(worker::reply_to_permission_impl(&sessions, session, request_id, response));
                    },
                    BridgeCommand::Shutdown { reply } => { shutdown_replies.push(reply); shutdown.cancel(); },
                    #[cfg(any(test, feature = "test-helpers"))]
                    BridgeCommand::TestPanic => panic!("bridge worker test-panic injected"),
                }
            },
        }
    }
}

fn install_waiter(session: &mut AcpSession, exited: mpsc::UnboundedSender<SessionId>) {
    let Some(mut child) = session.child.take() else {
        return;
    };
    let (kill, mut killed) = oneshot::channel();
    session.kill_tx = Some(kill);
    let id = session.session_id;
    session.waiter = Some(tokio::task::spawn_local(async move {
        let mut kill_error = None;
        let result = tokio::select! {
            status = child.wait() => status,
            _ = &mut killed => {
                if let Err(error) = worker::kill_owned_child(&mut child) { kill_error = Some(error.to_string()); }
                child.wait().await
            },
        };
        let status = match result {
            Ok(status) => status,
            Err(error) => {
                tracing::error!(%error, "child wait failed; retaining cleanup ownership");
                loop {
                    let _ = worker::kill_owned_child(&mut child);
                    match child.wait().await {
                        Ok(status) => break status,
                        Err(error) => {
                            tracing::error!(%error, "child reap remains unconfirmed");
                            tokio::time::sleep(Duration::from_millis(100)).await;
                        },
                    }
                }
            },
        };
        let _ = exited.send(id);
        ProcessExit {
            status: Ok(status),
            kill_error,
        }
    }));
}

async fn close(mut entry: AcpSession, force: bool, reply: Option<CloseReply>) -> CloseDone {
    let id = entry.session_id;
    entry.inner.borrow_mut().closing = true;
    entry.inner.borrow_mut().pending_permissions.clear();
    if entry.prompt_running
        && let Some(connection) = entry.connection.as_ref()
    {
        let acp_id = entry.inner.borrow().acp_session_id.clone();
        let graceful_cancel = async {
            connection
                .cancel(agent_client_protocol::schema::v1::CancelNotification::new(
                    agent_client_protocol::schema::v1::SessionId::new(acp_id),
                ))
                .await?;
            // Notification enqueue alone is not receipt. Keep the prompt RPC
            // alive until the agent responds, bounded by this cancel budget.
            entry.prompt_done.cancelled().await;
            Ok::<(), agent_client_protocol::schema::v1::Error>(())
        };
        match tokio::time::timeout(Duration::from_millis(500), graceful_cancel).await {
            Ok(Ok(())) => {},
            result => {
                tracing::warn!(session = %id, failed = result.is_err(), "wire cancellation unconfirmed; continuing owned process cleanup")
            },
        }
    }
    entry.cancel.cancel();
    if let Some(connection) = entry.connection.as_ref() {
        connection.stop();
    }
    if let Some(io) = entry.io_task_handle.take() {
        let _ = io.await;
    }
    entry.connection.take();
    let mut forced = force;
    if force && let Some(kill) = entry.kill_tx.take() {
        let _ = kill.send(());
    }
    let exit = if let Some(mut waiter) = entry.waiter.take() {
        match tokio::time::timeout(Duration::from_millis(worker::GRACE_MS), &mut waiter).await {
            Ok(result) => result,
            Err(_) => {
                forced = true;
                if let Some(kill) = entry.kill_tx.take() {
                    let _ = kill.send(());
                }
                // Keep owning the waiter until actual reap. Public API timeout
                // reports unconfirmed cleanup if the OS cannot settle this.
                waiter.await
            },
        }
    } else {
        return CloseDone {
            session: id,
            reason: SessionEndReason::ForcedClose,
            result: Err(CloseSessionError::Bridge(BridgeError::WorkerDead)),
            reply,
        };
    };
    for task in entry.task_handles.drain(..) {
        task.abort();
        let _ = task.await;
    }
    let mut result = Ok(());
    let reason = match exit {
        Ok(report) if report.kill_error.is_none() && report.status.is_ok() => {
            if force {
                SessionEndReason::ForcedClose
            } else if forced {
                result = Err(CloseSessionError::GracefulTimedOut {
                    session: id,
                    killed: true,
                });
                SessionEndReason::Timeout {
                    duration_ms: worker::GRACE_MS,
                }
            } else if report
                .status
                .as_ref()
                .is_ok_and(std::process::ExitStatus::success)
            {
                SessionEndReason::Normal
            } else {
                SessionEndReason::AgentCrashed {
                    exit_code: report.status.ok().and_then(|status| status.code()),
                    stderr_tail: worker::read_stderr_tail(&entry.tail),
                }
            }
        },
        _ => {
            result = Err(CloseSessionError::Bridge(BridgeError::CleanupUnconfirmed));
            SessionEndReason::AgentCrashed {
                exit_code: None,
                stderr_tail: worker::read_stderr_tail(&entry.tail),
            }
        },
    };
    worker::flush_pending_token_usage(&entry.events, &entry.inner, &id);
    entry.inner.borrow_mut().end_emitted = Some(reason.clone());
    CloseDone {
        session: id,
        reason,
        result,
        reply,
    }
}
