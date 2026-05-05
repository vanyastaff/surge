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
use async_trait::async_trait;
use interprocess::local_socket::tokio::prelude::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use surge_core::graph::Graph;
use surge_core::id::RunId;
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
        let name = socket_name_from_path(&socket_path)
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
                    Ok(Some(InboundServerFrame::Event(ev))) => match ev {
                        DaemonEvent::PerRun { run_id, event } => {
                            let is_terminal = matches!(event, EngineRunEvent::Terminal(_));
                            if is_terminal {
                                if let EngineRunEvent::Terminal(outcome) = &event {
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

/// Helper to convert a filesystem path into the platform-specific
/// [`interprocess::local_socket::Name`] that interprocess expects. Uses the
/// `GenericFilePath` namespace (i.e., the path is interpreted as a
/// filesystem entry on every platform).
fn socket_name_from_path(
    path: &Path,
) -> Result<interprocess::local_socket::Name<'_>, std::io::Error> {
    use interprocess::local_socket::ToFsName;
    path.to_fs_name::<interprocess::local_socket::GenericFilePath>()
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
                run_config,
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
        ErrorCode::RunAlreadyActive => {
            EngineError::Internal(format!("daemon: run already active ({message})"))
        },
        ErrorCode::AdmissionFull => {
            EngineError::Internal(format!("daemon: admission full ({message})"))
        },
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
}
