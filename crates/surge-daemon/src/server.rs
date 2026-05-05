//! IPC server: accept-and-dispatch loop. Each accepted connection
//! gets a per-connection task that reads [`DaemonRequest`] frames
//! from the socket and forwards them to the engine via the
//! [`EngineFacade`]. Per-run subscriptions spawn forwarder
//! tasks that pump [`EngineRunEvent`]s into the wire as
//! [`DaemonEvent::PerRun`].

use crate::admission::{AdmissionController, AdmissionDecision};
use crate::broadcast::BroadcastRegistry;
use crate::error::DaemonError;
use interprocess::local_socket::tokio::prelude::*;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use surge_core::id::RunId;
use surge_orchestrator::engine::facade::EngineFacade;
use surge_orchestrator::engine::handle::EngineRunEvent;
use surge_orchestrator::engine::ipc::{
    DaemonEvent, DaemonRequest, DaemonResponse, ErrorCode, GlobalDaemonEvent, read_request_frame,
    write_frame,
};
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

/// Top-level daemon-server config.
pub struct ServerConfig {
    /// Maximum concurrent active runs.
    pub max_active: usize,
    /// Path of the local socket to bind.
    pub socket_path: PathBuf,
}

/// Wires together the engine facade, admission, broadcast registry,
/// and the IPC listener. Called by `main.rs` (Phase 6.3).
pub async fn run(
    cfg: ServerConfig,
    facade: Arc<dyn EngineFacade>,
    shutdown: CancellationToken,
) -> Result<(), DaemonError> {
    use interprocess::local_socket::ListenerOptions;

    let admission = Arc::new(AdmissionController::new(cfg.max_active));
    let broadcast = Arc::new(BroadcastRegistry::new());

    let name = path_to_socket_name(&cfg.socket_path)?;
    let listener = ListenerOptions::new()
        .name(name)
        .create_tokio()
        .map_err(DaemonError::Io)?;

    tracing::info!(socket = %cfg.socket_path.display(), "daemon listening");

    loop {
        tokio::select! {
            () = shutdown.cancelled() => {
                tracing::info!("shutdown signal received; closing listener");
                break;
            }
            conn = listener.accept() => {
                match conn {
                    Ok(stream) => {
                        let facade = facade.clone();
                        let admission = admission.clone();
                        let broadcast = broadcast.clone();
                        let shutdown_for_conn = shutdown.clone();
                        tokio::spawn(async move {
                            if let Err(e) = handle_connection(
                                stream,
                                facade,
                                admission,
                                broadcast,
                                shutdown_for_conn,
                            )
                            .await
                            {
                                tracing::warn!(err = %e, "connection ended with error");
                            }
                        });
                    }
                    Err(e) => {
                        tracing::warn!(err = %e, "accept failed");
                    }
                }
            }
        }
    }
    Ok(())
}

fn path_to_socket_name(path: &Path) -> Result<interprocess::local_socket::Name<'_>, DaemonError> {
    use interprocess::local_socket::ToFsName;
    path.to_fs_name::<interprocess::local_socket::GenericFilePath>()
        .map_err(DaemonError::Io)
}

/// Per-connection state: tracks which runs the client subscribed to,
/// so we can identify subscriptions on disconnect (per-run forwarders
/// terminate naturally when their write target closes).
struct ConnState {
    subscriptions: HashSet<RunId>,
}

async fn handle_connection(
    stream: LocalSocketStream,
    facade: Arc<dyn EngineFacade>,
    admission: Arc<AdmissionController>,
    broadcast: Arc<BroadcastRegistry>,
    shutdown: CancellationToken,
) -> Result<(), DaemonError> {
    let (read_half, write_half) = stream.split();
    let mut reader = BufReader::new(read_half);
    let writer: Arc<Mutex<_>> = Arc::new(Mutex::new(write_half));
    let state = Arc::new(Mutex::new(ConnState {
        subscriptions: HashSet::new(),
    }));

    loop {
        tokio::select! {
            () = shutdown.cancelled() => {
                let err = DaemonResponse::Error {
                    request_id: 0,
                    code: ErrorCode::ShuttingDown,
                    message: "daemon shutting down".into(),
                };
                let mut w = writer.lock().await;
                let _ = write_frame(&mut *w, &err).await;
                let _ = w.flush().await;
                break;
            }
            frame = read_request_frame(&mut reader) => {
                match frame {
                    Ok(Some(req)) => {
                        let resp = dispatch(
                            req,
                            &*facade,
                            &admission,
                            &broadcast,
                            &state,
                            &writer,
                        )
                        .await;
                        if let Some(r) = resp {
                            let mut w = writer.lock().await;
                            if let Err(e) = write_frame(&mut *w, &r).await {
                                tracing::warn!(err = %e, "write_frame failed; closing connection");
                                break;
                            }
                        }
                    }
                    Ok(None) => break, // EOF
                    Err(e) => {
                        tracing::warn!(err = %e, "read_request_frame failed; closing connection");
                        break;
                    }
                }
            }
        }
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
async fn dispatch(
    req: DaemonRequest,
    facade: &dyn EngineFacade,
    admission: &Arc<AdmissionController>,
    broadcast: &Arc<BroadcastRegistry>,
    state: &Arc<Mutex<ConnState>>,
    writer: &Arc<Mutex<interprocess::local_socket::tokio::SendHalf>>,
) -> Option<DaemonResponse> {
    match req {
        DaemonRequest::Ping { request_id } => Some(DaemonResponse::PingOk {
            request_id,
            version: env!("CARGO_PKG_VERSION").into(),
        }),

        DaemonRequest::StartRun {
            request_id,
            run_id,
            graph,
            worktree_path,
            run_config,
        } => {
            let decision = admission.try_admit(run_id).await;
            match decision {
                AdmissionDecision::Admitted => {
                    broadcast.publish_global(GlobalDaemonEvent::RunAccepted { run_id });
                    let publisher = broadcast.register(run_id).await;
                    let admission_for_completion = admission.clone();
                    let broadcast_for_completion = broadcast.clone();
                    match facade
                        .start_run(run_id, *graph, worktree_path, run_config)
                        .await
                    {
                        Ok(handle) => {
                            spawn_forward_task(
                                run_id,
                                handle,
                                publisher,
                                admission_for_completion,
                                broadcast_for_completion,
                            );
                            Some(DaemonResponse::StartRunOk { request_id, run_id })
                        },
                        Err(e) => {
                            broadcast.deregister(run_id).await;
                            admission.notify_completed(run_id).await;
                            Some(DaemonResponse::Error {
                                request_id,
                                code: ErrorCode::EngineError,
                                message: format!("{e}"),
                            })
                        },
                    }
                },
                AdmissionDecision::Queued { position } => {
                    // TODO M7: drain-queue task lands with Phase 9 or 10 polish.
                    // Until that task exists, a queued run sits in the FIFO queue
                    // and will only be admitted when an active run finishes AND
                    // pop_queued() is called externally (e.g., a Phase 9 drain loop).
                    // The client receives StartRunQueued and must poll or rely on
                    // GlobalDaemonEvent::RunAccepted to know when its run eventually
                    // starts.
                    Some(DaemonResponse::StartRunQueued {
                        request_id,
                        run_id,
                        position,
                    })
                },
            }
        },

        DaemonRequest::ResumeRun {
            request_id,
            run_id,
            worktree_path,
        } => {
            let publisher = broadcast.register(run_id).await;
            let admission_for_completion = admission.clone();
            let broadcast_for_completion = broadcast.clone();
            match facade.resume_run(run_id, worktree_path).await {
                Ok(handle) => {
                    spawn_forward_task(
                        run_id,
                        handle,
                        publisher,
                        admission_for_completion,
                        broadcast_for_completion,
                    );
                    Some(DaemonResponse::ResumeRunOk { request_id })
                },
                Err(e) => {
                    broadcast.deregister(run_id).await;
                    Some(DaemonResponse::Error {
                        request_id,
                        code: ErrorCode::EngineError,
                        message: format!("{e}"),
                    })
                },
            }
        },

        DaemonRequest::StopRun {
            request_id,
            run_id,
            reason,
        } => match facade.stop_run(run_id, reason).await {
            Ok(()) => Some(DaemonResponse::StopRunOk { request_id }),
            Err(e) => Some(DaemonResponse::Error {
                request_id,
                code: ErrorCode::EngineError,
                message: format!("{e}"),
            }),
        },

        DaemonRequest::ResolveHumanInput {
            request_id,
            run_id,
            call_id,
            response,
        } => match facade.resolve_human_input(run_id, call_id, response).await {
            Ok(()) => Some(DaemonResponse::ResolveHumanInputOk { request_id }),
            Err(e) => Some(DaemonResponse::Error {
                request_id,
                code: ErrorCode::EngineError,
                message: format!("{e}"),
            }),
        },

        DaemonRequest::ListRuns { request_id } => match facade.list_runs().await {
            Ok(runs) => Some(DaemonResponse::ListRunsOk { request_id, runs }),
            Err(e) => Some(DaemonResponse::Error {
                request_id,
                code: ErrorCode::EngineError,
                message: format!("{e}"),
            }),
        },

        DaemonRequest::Subscribe { request_id, run_id } => {
            let rx = broadcast.subscribe(run_id).await;
            match rx {
                Some(rx) => {
                    {
                        let mut s = state.lock().await;
                        s.subscriptions.insert(run_id);
                    }
                    let writer_for_task = writer.clone();
                    tokio::spawn(forward_per_run_to_client(run_id, rx, writer_for_task));
                    Some(DaemonResponse::SubscribeOk { request_id })
                },
                None => Some(DaemonResponse::Error {
                    request_id,
                    code: ErrorCode::RunNotActive,
                    message: format!("run {run_id} not active in this daemon"),
                }),
            }
        },

        DaemonRequest::Unsubscribe { request_id, run_id } => {
            let mut s = state.lock().await;
            s.subscriptions.remove(&run_id);
            Some(DaemonResponse::UnsubscribeOk { request_id })
        },

        DaemonRequest::Shutdown { request_id } => {
            // Lifecycle (Phase 6.2) handles the actual signal cancel;
            // we just acknowledge here. The lifecycle drain closes
            // connections shortly after.
            Some(DaemonResponse::ShutdownOk { request_id })
        },

        // `DaemonRequest` is #[non_exhaustive]; new variants added in
        // future milestones will fall here with a generic error until
        // this server is updated.
        _ => {
            tracing::warn!("unrecognised DaemonRequest variant; returning error");
            None
        },
    }
}

/// Spawn the task that forwards engine events into the broadcast
/// registry and on terminal: publishes `RunFinished` + frees the
/// admission slot.
fn spawn_forward_task(
    run_id: RunId,
    handle: surge_orchestrator::engine::handle::RunHandle,
    publisher: tokio::sync::broadcast::Sender<EngineRunEvent>,
    admission: Arc<AdmissionController>,
    broadcast: Arc<BroadcastRegistry>,
) {
    tokio::spawn(async move {
        let mut rx = handle.events;
        loop {
            match rx.recv().await {
                Ok(ev) => {
                    let _ = publisher.send(ev);
                },
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {},
            }
        }
        // The run task finished (engine event stream closed). Wait for
        // the run-task JoinHandle to read the outcome, then notify.
        let outcome = handle.completion.await.unwrap_or(
            surge_orchestrator::engine::handle::RunOutcome::Aborted {
                reason: "run task panicked".into(),
            },
        );
        broadcast.publish_global(GlobalDaemonEvent::RunFinished { run_id, outcome });
        broadcast.deregister(run_id).await;
        admission.notify_completed(run_id).await;
    });
}

/// Per-subscriber forwarder: pumps per-run broadcast → wire as
/// [`DaemonEvent::PerRun`]. Exits when the broadcast closes, the
/// writer fails, or the receiver lags too much.
async fn forward_per_run_to_client(
    run_id: RunId,
    mut rx: tokio::sync::broadcast::Receiver<EngineRunEvent>,
    writer: Arc<Mutex<interprocess::local_socket::tokio::SendHalf>>,
) {
    loop {
        match rx.recv().await {
            Ok(event) => {
                let frame = DaemonEvent::PerRun { run_id, event };
                let mut w = writer.lock().await;
                if let Err(e) = write_frame(&mut *w, &frame).await {
                    tracing::debug!(
                        err = %e,
                        run_id = %run_id,
                        "subscriber write failed; ending forwarder"
                    );
                    break;
                }
            },
            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                tracing::warn!(
                    run_id = %run_id,
                    dropped = n,
                    "per-run forwarder lagged"
                );
            },
        }
    }
}
