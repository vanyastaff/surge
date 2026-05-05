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
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use surge_core::id::RunId;
use surge_orchestrator::engine::EngineRunConfig;
use surge_orchestrator::engine::facade::EngineFacade;
use surge_orchestrator::engine::handle::EngineRunEvent;
use surge_orchestrator::engine::ipc::{
    DaemonEvent, DaemonRequest, DaemonResponse, ErrorCode, GlobalDaemonEvent, RequestId,
    read_request_frame, write_frame,
};
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

/// Stashed `StartRun` parameters held by the daemon while admission has
/// queued the run. The original IPC connection's writer is intentionally
/// NOT kept — by the time the drain task admits this run the connection
/// may already be gone, and clients that care about events for the
/// eventually-admitted run should re-subscribe via `surge engine watch
/// <run_id> --daemon`.
struct PendingStartRun {
    graph: Box<surge_core::graph::Graph>,
    worktree_path: PathBuf,
    run_config: EngineRunConfig,
    /// Original `request_id` from the queued `StartRun`. Diagnostics only;
    /// the `StartRunQueued` reply has already been written and no further
    /// response is sent on that request.
    #[allow(dead_code)]
    request_id: RequestId,
    /// Wall-clock time the run was added to the admission queue.
    /// Surfaced as `RunSummary::started_at` for queued runs in
    /// `dispatch::ListRuns` so callers see a stable timestamp instead
    /// of one that drifts on each call.
    queued_at: chrono::DateTime<chrono::Utc>,
    /// `true` once admission has confirmed the run is queued (i.e.,
    /// `try_admit` returned `Queued`). The entry is inserted into
    /// `pending_starts` BEFORE `try_admit` runs (see the comment in
    /// `dispatch::StartRun` for why); on the `Admitted` path it is
    /// removed immediately, but during the brief window between
    /// `insert` and `remove` a concurrent `ListRuns` would otherwise
    /// see the entry. `dispatch::ListRuns` filters by this flag so
    /// admitted-but-not-yet-removed entries don't get reported as
    /// `Awaiting`. Set only in the `AdmissionDecision::Queued` arm.
    was_queued: bool,
}

/// Map from queued `RunId` → its stashed `StartRun` params. Populated by
/// `dispatch::StartRun` when admission queues; drained by the
/// drain-queue task in [`run`].
type PendingStarts = Arc<Mutex<HashMap<RunId, PendingStartRun>>>;

/// Top-level daemon-server config.
pub struct ServerConfig {
    /// Maximum concurrent active runs.
    pub max_active: usize,
    /// Maximum runs allowed to wait in the FIFO admission queue. When
    /// both `max_active` and this cap are hit, further `StartRun`
    /// requests are rejected with [`ErrorCode::QueueFull`] instead of
    /// growing the daemon's pending-start map without bound.
    pub max_queue: usize,
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

    let admission = Arc::new(AdmissionController::new(cfg.max_active, cfg.max_queue));
    let broadcast = Arc::new(BroadcastRegistry::new());
    let pending_starts: PendingStarts = Arc::new(Mutex::new(HashMap::new()));

    // F2: Unlink any stale socket file from a previous unclean exit.
    // On Windows, the named pipe doesn't live on the filesystem so this is a no-op.
    #[cfg(unix)]
    {
        if cfg.socket_path.exists() {
            let _ = std::fs::remove_file(&cfg.socket_path);
        }
    }

    let name = surge_orchestrator::engine::ipc::local_socket_name_from_path(&cfg.socket_path)
        .map_err(DaemonError::Io)?;
    let listener = ListenerOptions::new()
        .name(name)
        .create_tokio()
        .map_err(DaemonError::Io)?;

    tracing::info!(socket = %cfg.socket_path.display(), "daemon listening");

    // Drain task: when an active run completes (or any other admission
    // state change happens) and there is a queued run, pop it from the
    // FIFO and trigger the same Admitted-arm logic that
    // `dispatch::StartRun` runs (broadcast.register, facade.start_run,
    // spawn_forward_task, RunAccepted publication).
    spawn_drain_task(
        admission.clone(),
        broadcast.clone(),
        facade.clone(),
        pending_starts.clone(),
        shutdown.clone(),
    );

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
                        let pending_starts = pending_starts.clone();
                        let shutdown_for_conn = shutdown.clone();
                        tokio::spawn(async move {
                            if let Err(e) = handle_connection(
                                stream,
                                facade,
                                admission,
                                broadcast,
                                pending_starts,
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

/// Per-connection state: tracks which runs the client subscribed to
/// and the `JoinHandle` of each per-run forwarder task so that
/// `Unsubscribe` (and disconnect cleanup) can abort them promptly.
struct ConnState {
    subscriptions: HashSet<RunId>,
    /// Per-run forwarder task handles. Populated by `Subscribe`;
    /// removed (and aborted) by `Unsubscribe` or connection teardown.
    forwarders: HashMap<RunId, tokio::task::JoinHandle<()>>,
    /// Global-event forwarder task handle. Populated by `SubscribeGlobal`;
    /// aborted by `UnsubscribeGlobal` or connection teardown. Only one
    /// global subscription per connection — repeating `SubscribeGlobal`
    /// replaces the existing forwarder.
    global_forwarder: Option<tokio::task::JoinHandle<()>>,
}

async fn handle_connection(
    stream: LocalSocketStream,
    facade: Arc<dyn EngineFacade>,
    admission: Arc<AdmissionController>,
    broadcast: Arc<BroadcastRegistry>,
    pending_starts: PendingStarts,
    shutdown: CancellationToken,
) -> Result<(), DaemonError> {
    let (read_half, write_half) = stream.split();
    let mut reader = BufReader::new(read_half);
    let writer: Arc<Mutex<_>> = Arc::new(Mutex::new(write_half));
    let state = Arc::new(Mutex::new(ConnState {
        subscriptions: HashSet::new(),
        forwarders: HashMap::new(),
        global_forwarder: None,
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
                            &pending_starts,
                            &state,
                            &writer,
                            &shutdown,
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

    // F5: Cleanup — abort any forwarder tasks the client left open.
    let mut s = state.lock().await;
    for (_, h) in s.forwarders.drain() {
        h.abort();
    }
    if let Some(h) = s.global_forwarder.take() {
        h.abort();
    }

    Ok(())
}

#[allow(clippy::too_many_lines)]
#[allow(clippy::too_many_arguments)]
async fn dispatch(
    req: DaemonRequest,
    facade: &dyn EngineFacade,
    admission: &Arc<AdmissionController>,
    broadcast: &Arc<BroadcastRegistry>,
    pending_starts: &PendingStarts,
    state: &Arc<Mutex<ConnState>>,
    writer: &Arc<Mutex<interprocess::local_socket::tokio::SendHalf>>,
    shutdown: &CancellationToken,
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
            // Insert into `pending_starts` BEFORE `try_admit`. If we did
            // it after, an unrelated active run could complete in the
            // gap, wake the drain task, which would `pop_queued` our
            // run_id and find no `PendingStartRun` entry — silently
            // dropping the request. By stashing first, the drain task
            // sees a consistent view: any run_id it pops is guaranteed
            // to have its parameters already recorded here. The
            // `Admitted` arm below removes the entry it just inserted
            // (the run is in `active`, not `queue`, so the drain task
            // can never observe it).
            pending_starts.lock().await.insert(
                run_id,
                PendingStartRun {
                    graph,
                    worktree_path,
                    run_config,
                    request_id,
                    queued_at: chrono::Utc::now(),
                    was_queued: false,
                },
            );
            let decision = admission.try_admit(run_id).await;
            match decision {
                AdmissionDecision::Admitted => {
                    let pending = pending_starts
                        .lock()
                        .await
                        .remove(&run_id)
                        .expect("just inserted; only the drain task removes other entries");
                    // Register the per-run broadcast BEFORE publishing
                    // RunAccepted so a client subscribed to global
                    // daemon events that races a Subscribe(run_id)
                    // against this dispatch finds the per-run channel
                    // already in the registry.
                    let publisher = broadcast.register(run_id).await;
                    broadcast.publish_global(GlobalDaemonEvent::RunAccepted { run_id });
                    let admission_for_completion = admission.clone();
                    let broadcast_for_completion = broadcast.clone();
                    match facade
                        .start_run(
                            run_id,
                            *pending.graph,
                            pending.worktree_path,
                            pending.run_config,
                        )
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
                    // `pending_starts` is already populated above; the
                    // drain task in `run` will pick it up via
                    // `pop_queued` once a slot frees, then replay the
                    // same admitted-arm logic (broadcast.register,
                    // facade.start_run, spawn_forward_task).
                    //
                    // Known limitations of observing queued runs from
                    // outside today:
                    //
                    //   * The original IPC connection that received
                    //     `StartRunQueued` is NOT auto-resubscribed
                    //     to the eventually-admitted run's events.
                    //   * `GlobalDaemonEvent` (incl. `RunAccepted`)
                    //     is published into the broadcast registry
                    //     but no server-side code currently forwards
                    //     it to wire clients, so subscribing to
                    //     "global" events is not actually exposed.
                    //   * `surge engine watch <run_id> --daemon`
                    //     also can't help yet: a queued run has no
                    //     per-run DB on disk (the persistence
                    //     scaffolding is created by `Engine::start_run`
                    //     when admission lands), so the disk-replay
                    //     fallback errors out with a not-found
                    //     condition rather than waiting.
                    //
                    // What IS exposed: the queued run shows up in
                    // `dispatch::ListRuns` with `RunStatus::Awaiting`
                    // (synthesised from `pending_starts`). Callers
                    // can poll that to detect admission landing. We
                    // mark `was_queued = true` so ListRuns can
                    // distinguish entries that are actually queued
                    // from ones still mid-flight through the
                    // Admitted-arm's brief insert→remove window.
                    if let Some(entry) = pending_starts.lock().await.get_mut(&run_id) {
                        entry.was_queued = true;
                    }
                    Some(DaemonResponse::StartRunQueued {
                        request_id,
                        run_id,
                        position,
                    })
                },
                AdmissionDecision::QueueFull {
                    queue_len,
                    max_queue,
                } => {
                    // We optimistically inserted into `pending_starts`
                    // before `try_admit` to close a TOCTOU window
                    // against the drain task (see comment on the
                    // insert above). Admission rejected — the run
                    // will never be popped, so we MUST remove the
                    // entry here or the daemon leaks the boxed Graph
                    // and `EngineRunConfig` for every QueueFull
                    // rejection.
                    pending_starts.lock().await.remove(&run_id);
                    Some(DaemonResponse::Error {
                        request_id,
                        code: ErrorCode::QueueFull,
                        message: format!("queue is full ({queue_len}/{max_queue})"),
                    })
                },
            }
        },

        DaemonRequest::ResumeRun {
            request_id,
            run_id,
            worktree_path,
        } => {
            // Resume must consume an admission slot just like a fresh
            // StartRun, otherwise users can exceed max_active by replaying
            // resumes. We do NOT queue resumes (the user expects the run to
            // come back live); if the cap is hit, return AdmissionFull.
            if !admission.try_admit_no_queue(run_id).await {
                return Some(DaemonResponse::Error {
                    request_id,
                    code: ErrorCode::AdmissionFull,
                    message: format!("admission cap reached; cannot resume {run_id} now"),
                });
            }
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
                    admission.notify_completed(run_id).await;
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

        DaemonRequest::ListRuns { request_id } => {
            // Merge two sources: the engine's view of currently
            // active runs (via the facade) plus the daemon's queued
            // runs (held in `pending_starts` while waiting for
            // admission). Queued entries are synthesised with
            // `RunStatus::Awaiting`; without this the daemon's
            // queue would be invisible to clients calling ListRuns.
            //
            // Filter by `was_queued` to skip entries that are still
            // mid-flight through the `dispatch::StartRun` Admitted
            // arm: those are inserted into `pending_starts` BEFORE
            // `try_admit` runs (see the comment there for the
            // drain-task race that justifies that ordering) and
            // removed immediately after admission lands. A
            // concurrent ListRuns observing the entry in that
            // window would otherwise misreport an Active run as
            // `Awaiting`.
            match facade.list_runs().await {
                Ok(mut runs) => {
                    let pending = pending_starts.lock().await;
                    let queued_count = pending.values().filter(|e| e.was_queued).count();
                    runs.reserve(queued_count);
                    for (run_id, entry) in &*pending {
                        if !entry.was_queued {
                            continue;
                        }
                        runs.push(surge_orchestrator::engine::handle::RunSummary::queued(
                            *run_id,
                            entry.queued_at,
                        ));
                    }
                    drop(pending);
                    Some(DaemonResponse::ListRunsOk { request_id, runs })
                },
                Err(e) => Some(DaemonResponse::Error {
                    request_id,
                    code: ErrorCode::EngineError,
                    message: format!("{e}"),
                }),
            }
        },

        DaemonRequest::Subscribe { request_id, run_id } => {
            let rx = broadcast.subscribe(run_id).await;
            match rx {
                Some(rx) => {
                    let writer_for_task = writer.clone();
                    // F5: store the JoinHandle so Unsubscribe can abort it.
                    let handle =
                        tokio::spawn(forward_per_run_to_client(run_id, rx, writer_for_task));
                    {
                        let mut s = state.lock().await;
                        s.subscriptions.insert(run_id);
                        s.forwarders.insert(run_id, handle);
                    }
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
            // F5: abort the per-run forwarder task.
            if let Some(h) = s.forwarders.remove(&run_id) {
                h.abort();
            }
            Some(DaemonResponse::UnsubscribeOk { request_id })
        },

        DaemonRequest::SubscribeGlobal { request_id } => {
            // Idempotent. Spawning a second forwarder while the first
            // is still alive would briefly race it: both receivers
            // would observe the same `GlobalDaemonEvent` from
            // `BroadcastRegistry.global` and each would call
            // `write_frame` (the writer mutex serializes the wire
            // writes, but the duplication is real and observable as
            // two identical `DaemonEvent::Global` frames). Only
            // (re)spawn when there is no live forwarder — i.e. either
            // the slot is empty or the previous task has finished
            // (e.g. writer error broke it out of its loop).
            let mut s = state.lock().await;
            let needs_new = match s.global_forwarder.as_ref() {
                Some(h) => h.is_finished(),
                None => true,
            };
            if needs_new {
                // Drop the dead handle (if any) before spawning the
                // replacement; abort is a no-op on an already-finished
                // task but keeps the call symmetric with teardown.
                if let Some(old) = s.global_forwarder.take() {
                    old.abort();
                }
                let rx = broadcast.subscribe_global();
                let writer_for_task = writer.clone();
                let handle = tokio::spawn(forward_global_to_client(rx, writer_for_task));
                s.global_forwarder = Some(handle);
            }
            Some(DaemonResponse::SubscribeGlobalOk { request_id })
        },

        DaemonRequest::UnsubscribeGlobal { request_id } => {
            let mut s = state.lock().await;
            if let Some(h) = s.global_forwarder.take() {
                h.abort();
            }
            Some(DaemonResponse::UnsubscribeGlobalOk { request_id })
        },

        DaemonRequest::Shutdown { request_id } => {
            // F4: actually cancel the shutdown token so the daemon exits.
            tracing::info!("Shutdown IPC received; cancelling shutdown token");
            shutdown.cancel();
            Some(DaemonResponse::ShutdownOk { request_id })
        },

        // `DaemonRequest` is #[non_exhaustive]; new variants added in
        // future milestones will fall here with a generic error until
        // this server is updated.
        _ => {
            tracing::warn!("unrecognised DaemonRequest variant; returning error");
            Some(DaemonResponse::Error {
                request_id: 0,
                code: ErrorCode::BadRequest,
                message: "unsupported request method".into(),
            })
        },
    }
}

/// Spawn the drain-queue task. Wakes on every admission state change
/// (run completion, etc.) and pops queued runs while a slot is free,
/// triggering the same admitted-arm logic that a fresh `StartRun`
/// would. Exits cleanly on `shutdown.cancelled()`.
fn spawn_drain_task(
    admission: Arc<AdmissionController>,
    broadcast: Arc<BroadcastRegistry>,
    facade: Arc<dyn EngineFacade>,
    pending_starts: PendingStarts,
    shutdown: CancellationToken,
) {
    tokio::spawn(async move {
        loop {
            tokio::select! {
                () = shutdown.cancelled() => {
                    tracing::debug!("drain-queue task exiting (shutdown)");
                    break;
                }
                () = admission.wait_changed() => {
                    drain_one_pass(
                        &admission,
                        &broadcast,
                        facade.as_ref(),
                        &pending_starts,
                    )
                    .await;
                }
            }
        }
    });
}

/// Pop every queued run we currently have a slot for and admit it.
/// Pulled into a helper for readability. Each iteration mirrors the
/// `AdmissionDecision::Admitted` arm in `dispatch`.
async fn drain_one_pass(
    admission: &Arc<AdmissionController>,
    broadcast: &Arc<BroadcastRegistry>,
    facade: &dyn EngineFacade,
    pending_starts: &PendingStarts,
) {
    while let Some(run_id) = admission.pop_queued().await {
        let Some(pending) = pending_starts.lock().await.remove(&run_id) else {
            // pop_queued returned an id we have no PendingStartRun for.
            // Shouldn't happen unless queue/map fell out of sync, but be
            // defensive: free the slot we just claimed via pop_queued
            // (otherwise the active set leaks) and continue draining.
            tracing::warn!(
                run_id = %run_id,
                "drain-queue: pop_queued returned id with no PendingStartRun; releasing slot"
            );
            admission.notify_completed(run_id).await;
            continue;
        };
        // Register the per-run broadcast BEFORE publishing
        // RunAccepted (mirrors the dispatch::StartRun Admitted arm
        // ordering — see the comment there for the race).
        let publisher = broadcast.register(run_id).await;
        broadcast.publish_global(GlobalDaemonEvent::RunAccepted { run_id });
        let admission_for_completion = admission.clone();
        let broadcast_for_completion = broadcast.clone();
        match facade
            .start_run(
                run_id,
                *pending.graph,
                pending.worktree_path,
                pending.run_config,
            )
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
            },
            Err(e) => {
                tracing::error!(
                    run_id = %run_id,
                    err = %e,
                    "drain-queue: queued run failed to start; deregistering"
                );
                broadcast.deregister(run_id).await;
                admission.notify_completed(run_id).await;
                broadcast.publish_global(GlobalDaemonEvent::RunFinished {
                    run_id,
                    outcome: surge_orchestrator::engine::handle::RunOutcome::Aborted {
                        reason: format!("queued start failed: {e}"),
                    },
                });
            },
        }
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

/// Per-connection forwarder for daemon-level events: pumps the global
/// broadcast → wire as [`DaemonEvent::Global`]. Exits when the
/// broadcast closes (daemon shutdown) or the writer fails. Mirrors
/// [`forward_per_run_to_client`] but for [`GlobalDaemonEvent`].
async fn forward_global_to_client(
    mut rx: tokio::sync::broadcast::Receiver<GlobalDaemonEvent>,
    writer: Arc<Mutex<interprocess::local_socket::tokio::SendHalf>>,
) {
    loop {
        match rx.recv().await {
            Ok(event) => {
                let frame = DaemonEvent::Global(event);
                let mut w = writer.lock().await;
                if let Err(e) = write_frame(&mut *w, &frame).await {
                    tracing::debug!(
                        err = %e,
                        "global subscriber write failed; ending forwarder"
                    );
                    break;
                }
            },
            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                tracing::warn!(dropped = n, "global forwarder lagged");
            },
        }
    }
}
