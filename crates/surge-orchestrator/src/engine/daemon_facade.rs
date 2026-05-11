//! `DaemonEngineFacade` — out-of-process [`crate::engine::facade::EngineFacade`]
//! impl that forwards every method as an IPC request to a `surge-daemon`.
//!
//! Wire format: line-delimited JSON over a local socket. The client
//! task spawned by [`DaemonClient::connect`] reads inbound frames and
//! dispatches them: responses go to the matching oneshot in
//! `pending`; events go to the [`EventDispatcher`] for per-run
//! fan-out.

use crate::engine::config::EngineRunConfig;
use crate::engine::error::EngineError;
use crate::engine::facade::EngineFacade;
use crate::engine::handle::{EngineRunEvent, RunHandle, RunOutcome, RunSummary};
use crate::engine::ipc::{
    DaemonEvent, DaemonRequest, DaemonResponse, ErrorCode, GlobalDaemonEvent, InboundServerFrame,
    RequestId, read_inbound_server_frame, write_frame,
};
use crate::roadmap_amendment::ActiveRunAmendmentOutcome;
use async_trait::async_trait;
use interprocess::local_socket::tokio::prelude::*;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use surge_core::graph::Graph;
use surge_core::id::RunId;
use surge_core::roadmap_patch::{RoadmapPatchApplyResult, RoadmapPatchId, RoadmapPatchTarget};
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::sync::{Mutex, broadcast, oneshot};

/// IPC client wrapping a connected local socket. One client supports
/// multiplexed requests via [`RequestId`] correlation. Exposes
/// [`EngineFacade`] via [`DaemonEngineFacade`].
pub struct DaemonClient {
    next_id: AtomicU64,
    write_half: Mutex<interprocess::local_socket::tokio::SendHalf>,
    pending: Arc<Mutex<HashMap<RequestId, oneshot::Sender<DaemonResponse>>>>,
    event_dispatcher: Arc<EventDispatcher>,
}

/// Internal: per-run + global broadcast channel routing for the
/// background read loop.
struct EventDispatcher {
    per_run: Mutex<HashMap<RunId, broadcast::Sender<EngineRunEvent>>>,
    global: broadcast::Sender<GlobalDaemonEvent>,
    /// Tracks completion oneshots so `start_run` / `resume_run` can
    /// fabricate a `JoinHandle<RunOutcome>` for the returned
    /// [`RunHandle`].
    completion: Mutex<HashMap<RunId, oneshot::Sender<RunOutcome>>>,
}

impl DaemonClient {
    /// Open a connection to the daemon at `socket_path` and start
    /// the background read loop. On Unix this is a Unix-domain
    /// socket path; on Windows it is the named-pipe path the daemon
    /// recorded in `~/.surge/daemon/daemon.sock` (the discovery
    /// helper resolving the file→pipe-name lives in PR 3).
    pub async fn connect(socket_path: PathBuf) -> Result<Arc<Self>, EngineError> {
        let name = crate::engine::ipc::local_socket_name_from_path(&socket_path)
            .map_err(|e| EngineError::Internal(format!("socket name: {e}")))?;
        let stream = LocalSocketStream::connect(name).await.map_err(|e| {
            EngineError::Internal(format!("connect {}: {e}", socket_path.display()))
        })?;
        // Use interprocess's own split — tokio::io::split does not work here.
        let (read_half, write_half) = stream.split();

        let (global_tx, _) = broadcast::channel(64);
        let event_dispatcher = Arc::new(EventDispatcher {
            per_run: Mutex::new(HashMap::new()),
            global: global_tx,
            completion: Mutex::new(HashMap::new()),
        });
        let pending: Arc<Mutex<HashMap<RequestId, oneshot::Sender<DaemonResponse>>>> =
            Arc::new(Mutex::new(HashMap::new()));

        let pending_for_task = pending.clone();
        let dispatcher_for_task = event_dispatcher.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(read_half);
            loop {
                match read_inbound_server_frame(&mut reader).await {
                    Ok(Some(InboundServerFrame::Response(resp))) => {
                        let id = resp.request_id();
                        if let Some(tx) = pending_for_task.lock().await.remove(&id) {
                            let _ = tx.send(resp);
                        }
                    },
                    Ok(Some(InboundServerFrame::Event(ev))) => match *ev {
                        DaemonEvent::PerRun { run_id, event } => {
                            let event = *event;
                            let is_terminal = matches!(&event, EngineRunEvent::Terminal { .. });
                            if is_terminal {
                                if let EngineRunEvent::Terminal { outcome } = &event {
                                    if let Some(tx) =
                                        dispatcher_for_task.completion.lock().await.remove(&run_id)
                                    {
                                        let _ = tx.send(outcome.clone());
                                    }
                                }
                            }
                            // Forward event to per-run broadcast (if subscribed).
                            {
                                let map = dispatcher_for_task.per_run.lock().await;
                                if let Some(tx) = map.get(&run_id) {
                                    let _ = tx.send(event);
                                }
                            }
                            // After Terminal: remove the per_run sender so subscribers see
                            // a closed channel and the map doesn't accumulate stale entries.
                            if is_terminal {
                                dispatcher_for_task.per_run.lock().await.remove(&run_id);
                            }
                        },
                        DaemonEvent::Global(g) => {
                            let _ = dispatcher_for_task.global.send(g);
                        },
                    },
                    Ok(None) => break, // EOF — daemon closed connection
                    Err(e) => {
                        tracing::error!(err = %e, "daemon-client read loop");
                        break;
                    },
                }
            }
            // Fix 1: drain all pending maps so in-flight futures resolve
            // gracefully (via channel-closed error) instead of hanging forever.
            tracing::info!("daemon-client read loop ended; draining pending+completion maps");
            {
                let mut p = pending_for_task.lock().await;
                p.clear();
            }
            {
                let mut c = dispatcher_for_task.completion.lock().await;
                c.clear();
            }
            {
                let mut r = dispatcher_for_task.per_run.lock().await;
                r.clear();
            }
        });

        Ok(Arc::new(Self {
            next_id: AtomicU64::new(1),
            write_half: Mutex::new(write_half),
            pending,
            event_dispatcher,
        }))
    }

    /// Allocate a fresh request id.
    fn next_request_id(&self) -> RequestId {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Send a request and await the matching response.
    async fn rpc(
        &self,
        build: impl FnOnce(RequestId) -> DaemonRequest,
    ) -> Result<DaemonResponse, EngineError> {
        let id = self.next_request_id();
        let req = build(id);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);

        let write_result = async {
            let mut w = self.write_half.lock().await;
            write_frame(&mut *w, &req)
                .await
                .map_err(|e| EngineError::Internal(format!("write_frame: {e}")))?;
            w.flush()
                .await
                .map_err(|e| EngineError::Internal(format!("flush: {e}")))?;
            Ok::<(), EngineError>(())
        }
        .await;

        if let Err(e) = write_result {
            // Remove the orphaned pending entry — receiver would otherwise hang.
            self.pending.lock().await.remove(&id);
            return Err(e);
        }

        rx.await
            .map_err(|_| EngineError::Internal("daemon dropped before response".into()))
    }
}

/// Public [`EngineFacade`] surface — wraps the [`DaemonClient`].
pub struct DaemonEngineFacade {
    inner: Arc<DaemonClient>,
}

impl DaemonEngineFacade {
    /// Open an IPC connection and return a facade.
    pub async fn connect(socket_path: PathBuf) -> Result<Self, EngineError> {
        Ok(Self {
            inner: DaemonClient::connect(socket_path).await?,
        })
    }
}

#[async_trait]
impl EngineFacade for DaemonEngineFacade {
    async fn start_run(
        &self,
        run_id: RunId,
        graph: Graph,
        worktree_path: PathBuf,
        run_config: EngineRunConfig,
    ) -> Result<RunHandle, EngineError> {
        // Reserve completion + per-run channel BEFORE sending Subscribe so
        // we don't lose early events.
        let (completion_tx, completion_rx) = oneshot::channel();
        self.inner
            .event_dispatcher
            .completion
            .lock()
            .await
            .insert(run_id, completion_tx);
        let (event_tx, event_rx) = broadcast::channel(256);
        self.inner
            .event_dispatcher
            .per_run
            .lock()
            .await
            .insert(run_id, event_tx);

        let resp = self
            .inner
            .rpc(|request_id| DaemonRequest::StartRun {
                request_id,
                run_id,
                graph: Box::new(graph),
                worktree_path,
                run_config: Box::new(run_config),
            })
            .await?;
        match resp {
            DaemonResponse::StartRunOk { .. } | DaemonResponse::StartRunQueued { .. } => {},
            DaemonResponse::Error { code, message, .. } => {
                self.cleanup_run_channels(run_id).await;
                return Err(map_error(code, &message));
            },
            other => {
                self.cleanup_run_channels(run_id).await;
                return Err(EngineError::Internal(format!(
                    "unexpected response: {other:?}"
                )));
            },
        }

        // Subscribe to per-run events.
        let sub = self
            .inner
            .rpc(|request_id| DaemonRequest::Subscribe { request_id, run_id })
            .await?;
        match sub {
            DaemonResponse::SubscribeOk { .. } => {},
            DaemonResponse::Error { code, message, .. } => {
                self.cleanup_run_channels(run_id).await;
                return Err(map_error(code, &message));
            },
            other => {
                self.cleanup_run_channels(run_id).await;
                return Err(EngineError::Internal(format!(
                    "unexpected response to Subscribe: {other:?}"
                )));
            },
        }

        let join: tokio::task::JoinHandle<RunOutcome> = tokio::spawn(async move {
            completion_rx.await.unwrap_or(RunOutcome::Aborted {
                reason: "daemon connection lost".into(),
            })
        });

        Ok(RunHandle {
            run_id,
            events: event_rx,
            completion: join,
        })
    }

    async fn resume_run(
        &self,
        run_id: RunId,
        worktree_path: PathBuf,
    ) -> Result<RunHandle, EngineError> {
        let (completion_tx, completion_rx) = oneshot::channel();
        self.inner
            .event_dispatcher
            .completion
            .lock()
            .await
            .insert(run_id, completion_tx);
        let (event_tx, event_rx) = broadcast::channel(256);
        self.inner
            .event_dispatcher
            .per_run
            .lock()
            .await
            .insert(run_id, event_tx);

        let resp = self
            .inner
            .rpc(|request_id| DaemonRequest::ResumeRun {
                request_id,
                run_id,
                worktree_path,
            })
            .await?;
        match resp {
            DaemonResponse::ResumeRunOk { .. } => {},
            DaemonResponse::Error { code, message, .. } => {
                self.cleanup_run_channels(run_id).await;
                return Err(map_error(code, &message));
            },
            other => {
                self.cleanup_run_channels(run_id).await;
                return Err(EngineError::Internal(format!(
                    "unexpected response: {other:?}"
                )));
            },
        }

        let sub = self
            .inner
            .rpc(|request_id| DaemonRequest::Subscribe { request_id, run_id })
            .await?;
        match sub {
            DaemonResponse::SubscribeOk { .. } => {},
            DaemonResponse::Error { code, message, .. } => {
                self.cleanup_run_channels(run_id).await;
                return Err(map_error(code, &message));
            },
            other => {
                self.cleanup_run_channels(run_id).await;
                return Err(EngineError::Internal(format!(
                    "unexpected response to Subscribe: {other:?}"
                )));
            },
        }

        let join: tokio::task::JoinHandle<RunOutcome> = tokio::spawn(async move {
            completion_rx.await.unwrap_or(RunOutcome::Aborted {
                reason: "daemon connection lost".into(),
            })
        });

        Ok(RunHandle {
            run_id,
            events: event_rx,
            completion: join,
        })
    }

    async fn stop_run(&self, run_id: RunId, reason: String) -> Result<(), EngineError> {
        match self
            .inner
            .rpc(|request_id| DaemonRequest::StopRun {
                request_id,
                run_id,
                reason,
            })
            .await?
        {
            DaemonResponse::StopRunOk { .. } => Ok(()),
            DaemonResponse::Error { code, message, .. } => Err(map_error(code, &message)),
            other => Err(EngineError::Internal(format!("unexpected: {other:?}"))),
        }
    }

    async fn submit_roadmap_amendment(
        &self,
        run_id: RunId,
        patch_id: RoadmapPatchId,
        target: RoadmapPatchTarget,
        patch_result: RoadmapPatchApplyResult,
    ) -> Result<ActiveRunAmendmentOutcome, EngineError> {
        match self
            .inner
            .rpc(|request_id| DaemonRequest::SubmitRoadmapAmendment {
                request_id,
                run_id,
                patch_id,
                target,
                patch_result: Box::new(patch_result),
            })
            .await?
        {
            DaemonResponse::SubmitRoadmapAmendmentOk { outcome, .. } => Ok(*outcome),
            DaemonResponse::Error { code, message, .. } => Err(map_error(code, &message)),
            other => Err(EngineError::Internal(format!("unexpected: {other:?}"))),
        }
    }

    async fn resolve_human_input(
        &self,
        run_id: RunId,
        call_id: Option<String>,
        response: serde_json::Value,
    ) -> Result<(), EngineError> {
        match self
            .inner
            .rpc(|request_id| DaemonRequest::ResolveHumanInput {
                request_id,
                run_id,
                call_id,
                response,
            })
            .await?
        {
            DaemonResponse::ResolveHumanInputOk { .. } => Ok(()),
            DaemonResponse::Error { code, message, .. } => Err(map_error(code, &message)),
            other => Err(EngineError::Internal(format!("unexpected: {other:?}"))),
        }
    }

    async fn list_runs(&self) -> Result<Vec<RunSummary>, EngineError> {
        match self
            .inner
            .rpc(|request_id| DaemonRequest::ListRuns { request_id })
            .await?
        {
            DaemonResponse::ListRunsOk { runs, .. } => Ok(runs),
            DaemonResponse::Error { code, message, .. } => Err(map_error(code, &message)),
            other => Err(EngineError::Internal(format!("unexpected: {other:?}"))),
        }
    }
}

impl DaemonEngineFacade {
    /// Subscribe to per-run events for an EXISTING active run. Returns
    /// a `broadcast::Receiver<EngineRunEvent>` fed by the daemon's
    /// per-run forwarder. Used by `surge engine watch <run_id> --daemon`.
    ///
    /// Errors with `EngineError::Internal` if:
    /// - the daemon doesn't recognize `run_id` (returns `RunNotActive`)
    /// - the IPC connection fails
    /// - an unexpected response variant arrives
    ///
    /// Note: per-run broadcast doesn't replay history. Events emitted
    /// before this call returns are not seen by the caller. This is
    /// the same semantics as the explicit `Subscribe` path inside
    /// `start_run` / `resume_run`.
    pub async fn subscribe_to_run(
        &self,
        run_id: RunId,
    ) -> Result<broadcast::Receiver<EngineRunEvent>, EngineError> {
        // Register a local per-run broadcast channel BEFORE sending
        // Subscribe so the read loop can dispatch any events arriving
        // mid-handshake.
        let (event_tx, event_rx) = broadcast::channel(256);
        self.inner
            .event_dispatcher
            .per_run
            .lock()
            .await
            .insert(run_id, event_tx);

        let resp = match self
            .inner
            .rpc(|request_id| DaemonRequest::Subscribe { request_id, run_id })
            .await
        {
            Ok(r) => r,
            Err(e) => {
                // RPC failed (write error, daemon dropped, etc.). Remove the
                // orphaned per_run entry so subsequent event dispatch doesn't
                // target a channel nobody listens to.
                self.inner
                    .event_dispatcher
                    .per_run
                    .lock()
                    .await
                    .remove(&run_id);
                return Err(e);
            },
        };
        match resp {
            DaemonResponse::SubscribeOk { .. } => Ok(event_rx),
            DaemonResponse::Error {
                code: ErrorCode::RunNotActive,
                ..
            } => {
                // Clean up the locally-registered channel since we never got Ok.
                self.inner
                    .event_dispatcher
                    .per_run
                    .lock()
                    .await
                    .remove(&run_id);
                Err(EngineError::RunNotActive(run_id))
            },
            DaemonResponse::Error { code, message, .. } => {
                self.inner
                    .event_dispatcher
                    .per_run
                    .lock()
                    .await
                    .remove(&run_id);
                Err(map_error(code, &message))
            },
            other => {
                self.inner
                    .event_dispatcher
                    .per_run
                    .lock()
                    .await
                    .remove(&run_id);
                Err(EngineError::Internal(format!(
                    "unexpected response to Subscribe: {other:?}"
                )))
            },
        }
    }

    /// Subscribe to daemon-level [`GlobalDaemonEvent`] notifications.
    /// Returns a `broadcast::Receiver<GlobalDaemonEvent>` that yields
    /// events the daemon publishes (e.g. `RunAccepted`, `RunFinished`,
    /// `DaemonShuttingDown`) for the lifetime of this connection.
    ///
    /// Past events are NOT replayed — only events fired AFTER the
    /// daemon registers the subscription arrive. The local broadcast
    /// channel was created up-front in `DaemonClient::connect`; this
    /// method simply hands out a fresh receiver and tells the daemon
    /// to start fanning out global events to this connection's wire.
    ///
    /// Errors with `EngineError::Internal` if the IPC fails or an
    /// unexpected response variant arrives.
    pub async fn subscribe_global(
        &self,
    ) -> Result<broadcast::Receiver<GlobalDaemonEvent>, EngineError> {
        // Subscribe to the local channel BEFORE sending the IPC so any
        // global events arriving immediately after the daemon registers
        // the subscription are delivered to the caller.
        let rx = self.inner.event_dispatcher.global.subscribe();

        let resp = self
            .inner
            .rpc(|request_id| DaemonRequest::SubscribeGlobal { request_id })
            .await?;
        match resp {
            DaemonResponse::SubscribeGlobalOk { .. } => Ok(rx),
            DaemonResponse::Error { code, message, .. } => Err(map_error(code, &message)),
            other => Err(EngineError::Internal(format!(
                "unexpected response to SubscribeGlobal: {other:?}"
            ))),
        }
    }

    /// Unsubscribe from daemon-level events. Sends `UnsubscribeGlobal`
    /// IPC; the local `broadcast::Receiver` returned by
    /// [`Self::subscribe_global`] naturally drops on the caller's
    /// side and will see `Closed` on subsequent `recv` calls once the
    /// last sender clone goes away (the local sender lives until the
    /// connection drops, so existing receivers keep yielding past
    /// events the daemon already pushed).
    pub async fn unsubscribe_global(&self) -> Result<(), EngineError> {
        let resp = self
            .inner
            .rpc(|request_id| DaemonRequest::UnsubscribeGlobal { request_id })
            .await?;
        match resp {
            DaemonResponse::UnsubscribeGlobalOk { .. } => Ok(()),
            DaemonResponse::Error { code, message, .. } => Err(map_error(code, &message)),
            other => Err(EngineError::Internal(format!(
                "unexpected response to UnsubscribeGlobal: {other:?}"
            ))),
        }
    }

    /// Unsubscribe from a run's events. Drops the per-run broadcast
    /// channel locally (subscribers see Closed); sends Unsubscribe
    /// IPC to the daemon so it stops pumping events to this connection.
    pub async fn unsubscribe_from_run(&self, run_id: RunId) -> Result<(), EngineError> {
        // Drop the local channel FIRST so subscribers see Closed immediately
        // and we don't leak the entry if the IPC fails.
        self.inner
            .event_dispatcher
            .per_run
            .lock()
            .await
            .remove(&run_id);

        let resp = self
            .inner
            .rpc(|request_id| DaemonRequest::Unsubscribe { request_id, run_id })
            .await?;
        match resp {
            DaemonResponse::UnsubscribeOk { .. } => Ok(()),
            DaemonResponse::Error { code, message, .. } => Err(map_error(code, &message)),
            other => Err(EngineError::Internal(format!(
                "unexpected response to Unsubscribe: {other:?}"
            ))),
        }
    }

    async fn cleanup_run_channels(&self, run_id: RunId) {
        self.inner
            .event_dispatcher
            .per_run
            .lock()
            .await
            .remove(&run_id);
        self.inner
            .event_dispatcher
            .completion
            .lock()
            .await
            .remove(&run_id);
    }
}

fn map_error(code: ErrorCode, message: &str) -> EngineError {
    match code {
        ErrorCode::RunNotFound => {
            EngineError::Internal(format!("daemon: run not found ({message})"))
        },
        ErrorCode::RunNotActive => {
            EngineError::Internal(format!("daemon: run not active ({message})"))
        },
        ErrorCode::RunAlreadyActive => {
            EngineError::Internal(format!("daemon: run already active ({message})"))
        },
        ErrorCode::AdmissionFull => {
            EngineError::Internal(format!("daemon: admission full ({message})"))
        },
        ErrorCode::QueueFull => EngineError::QueueFull(message.to_string()),
        ErrorCode::ShuttingDown => {
            EngineError::Internal(format!("daemon: shutting down ({message})"))
        },
        _ => EngineError::Internal(format!("daemon error [{code:?}]: {message}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_error_admission_full() {
        let e = map_error(ErrorCode::AdmissionFull, "8/8 active");
        assert!(format!("{e}").contains("admission full"));
    }

    #[test]
    fn map_error_queue_full_returns_typed_variant() {
        let e = map_error(ErrorCode::QueueFull, "queue is full (4/4)");
        assert!(
            matches!(e, EngineError::QueueFull(ref m) if m == "queue is full (4/4)"),
            "QueueFull must surface as a typed variant carrying the daemon message; got {e:?}"
        );
        assert!(format!("{e}").contains("queue is full (4/4)"));
    }
}
