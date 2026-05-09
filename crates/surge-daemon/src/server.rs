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
use tokio::io::{AsyncWrite, AsyncWriteExt, BufReader};
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
///
/// This entry point creates a fresh [`BroadcastRegistry`] internally; for
/// callers that need to subscribe to global daemon events themselves
/// (e.g., the run-completion → tracker-comment hook in `surge-daemon`'s
/// `main.rs`), use [`run_with_registry`] instead.
pub async fn run(
    cfg: ServerConfig,
    facade: Arc<dyn EngineFacade>,
    shutdown: CancellationToken,
) -> Result<(), DaemonError> {
    let broadcast = Arc::new(BroadcastRegistry::new());
    run_with_registry(cfg, facade, broadcast, shutdown).await
}

/// Like [`run`], but accepts a pre-built [`BroadcastRegistry`] so the
/// caller can `subscribe_global()` for daemon-internal listeners. The
/// caller is responsible for keeping its `Arc` clone alive for as long
/// as it wants subscriptions to keep receiving events.
pub async fn run_with_registry(
    cfg: ServerConfig,
    facade: Arc<dyn EngineFacade>,
    broadcast: Arc<BroadcastRegistry>,
    shutdown: CancellationToken,
) -> Result<(), DaemonError> {
    use interprocess::local_socket::ListenerOptions;

    let admission = Arc::new(AdmissionController::new(cfg.max_active, cfg.max_queue));
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
                    // Safe: try_admit returned Admitted, so run_id is
                    // in `active` (not queue). The other paths that
                    // remove from pending_starts — drain.pop_queued
                    // and StopRun.cancel_queued — both gate on the
                    // run being in the FIFO queue, so neither can
                    // touch this entry.
                    let pending = pending_starts
                        .lock()
                        .await
                        .remove(&run_id)
                        .expect("just inserted; Admitted ⇒ no concurrent remover");
                    // Register the per-run broadcast BEFORE publishing
                    // RunAccepted so a client subscribed to global
                    // daemon events that races a Subscribe(run_id)
                    // against this dispatch finds the per-run channel
                    // already in the registry.
                    let publisher = broadcast.register(run_id).await;
                    // Pre-attach a forwarder for THIS connection BEFORE
                    // the engine starts emitting. Without this, fast-
                    // completing flows (e.g. `flow_terminal_only.toml`)
                    // can race the client: the engine fires Terminal
                    // into `publisher` before the client's follow-up
                    // `Subscribe` IPC creates a subscriber, and tokio
                    // broadcast's `send` with no receivers drops the
                    // message — there is no buffer-for-future-receivers
                    // semantic. Subscribing now (off the same publisher
                    // we hand to `spawn_forward_task` below) guarantees
                    // the receiver exists before any send; the
                    // companion `Subscribe` handler is idempotent for
                    // already-attached connections so the client's
                    // post-StartRun Subscribe is a no-op.
                    let pre_subscribe_rx = publisher.subscribe();
                    let writer_for_task = writer.clone();
                    let pre_handle = tokio::spawn(forward_per_run_to_client(
                        run_id,
                        pre_subscribe_rx,
                        writer_for_task,
                    ));
                    {
                        let mut s = state.lock().await;
                        s.subscriptions.insert(run_id);
                        if let Some(old) = s.forwarders.insert(run_id, pre_handle) {
                            // Defensive: should never have a prior
                            // forwarder for a freshly-admitted run,
                            // but mirror the Subscribe handler's
                            // abort-before-replace pattern for
                            // safety.
                            old.abort();
                        }
                    }
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
                            // Clean up the pre-attached forwarder
                            // we optimistically spawned above, so the
                            // connection's state stays consistent and
                            // the task does not hang awaiting events
                            // that never come.
                            let mut s = state.lock().await;
                            s.subscriptions.remove(&run_id);
                            if let Some(h) = s.forwarders.remove(&run_id) {
                                h.abort();
                            }
                            drop(s);
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
        } => {
            // The engine only knows about runs that `Engine::start_run`
            // has admitted; a queued run is invisible to
            // `facade.stop_run`. Without a queue-aware branch, the
            // user's cancellation would be silently lost: facade.stop_run
            // returns RunNotFound, and worse, the drain task would
            // later admit and start the run anyway.
            //
            // We use `admission.cancel_queued` (not pending_starts) as
            // the source of truth. cancel_queued is atomic with the
            // admission state, so a `true` result means the run was
            // definitely in the FIFO at the call site (not in
            // `active`). Once it returns true, the drain task can no
            // longer see this run, so it's safe to also drop its
            // stashed start params from pending_starts.
            //
            // Why NOT check pending_starts first: dispatch::StartRun
            // inserts into pending_starts BEFORE calling try_admit,
            // and only removes in the Admitted arm afterward. A
            // concurrent StopRun that races that window would observe
            // a `Some` entry for a run that admission has already put
            // into `active` (not queue), would call cancel_queued and
            // get `false`, and would leave both StopRun and StartRun
            // in inconsistent state — the StartRun's `expect("just
            // inserted...")` would then panic when it tried to remove
            // the same entry.
            //
            // Races we accept (not panics):
            //   * StopRun fires between drain.pop_queued and
            //     drain.pending_starts.remove: cancel_queued returns
            //     false, falls through to facade.stop_run, which sees
            //     RunNotFound (engine doesn't know the run yet). User
            //     can retry once the run is fully admitted.
            //   * Two concurrent StopRuns for the same queued run:
            //     one wins, the other falls through to facade.stop_run.
            // We DO call `broadcast.deregister` here — even though a
            // queued run never reached `broadcast.register`, a client
            // may have called `Subscribe(run_id)` while the run was
            // queued and parked a waiter via
            // `BroadcastRegistry::subscribe_eventual`. Without
            // deregister, those waiters orphan in the registry's
            // `waiters` map indefinitely (leak) and the per-connection
            // `forward_queued_to_client` task hangs forever awaiting
            // the oneshot. `deregister` removes the per_run entry
            // (no-op if absent) AND drops every parked
            // `oneshot::Sender` for this run_id, which causes each
            // waiter to wake with `Err(RecvError::Closed)` and exit
            // cleanly via the `forward_queued_to_client` Err arm.
            if admission.cancel_queued(run_id).await {
                pending_starts.lock().await.remove(&run_id);
                broadcast.deregister(run_id).await;
                broadcast.publish_global(GlobalDaemonEvent::RunFinished {
                    run_id,
                    outcome: surge_orchestrator::engine::handle::RunOutcome::Aborted { reason },
                });
                return Some(DaemonResponse::StopRunOk { request_id });
            }
            match facade.stop_run(run_id, reason).await {
                Ok(()) => Some(DaemonResponse::StopRunOk { request_id }),
                Err(e) => Some(DaemonResponse::Error {
                    request_id,
                    code: ErrorCode::EngineError,
                    message: format!("{e}"),
                }),
            }
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
            // Idempotent for connections that already have a forwarder
            // for this run_id — alive OR finished. The StartRun
            // Admitted arm pre-attaches a forwarder so fast-completing
            // flows don't lose events to tokio broadcast's
            // no-receivers drop semantic. Two cases:
            //
            //   * Forwarder still running: spawning a second one
            //     would double-send every wire frame. Reply
            //     SubscribeOk and skip.
            //   * Forwarder already finished (run completed during
            //     the StartRun→Subscribe IPC roundtrip — observed on
            //     macOS for `flow_terminal_only.toml`): any events
            //     have already been written to the wire by the
            //     pre-attached forwarder; the publisher has been
            //     deregistered by `spawn_forward_task`'s terminal
            //     cleanup, so the fallback `broadcast.subscribe`
            //     would return None and produce a misleading
            //     RunNotActive error. Reply SubscribeOk: the client
            //     already has whatever it was going to get from
            //     the wire.
            {
                let s = state.lock().await;
                if s.forwarders.contains_key(&run_id) {
                    return Some(DaemonResponse::SubscribeOk { request_id });
                }
            }
            if let Some(rx) = broadcast.subscribe(run_id).await {
                let writer_for_task = writer.clone();
                // F5: store the JoinHandle so Unsubscribe can abort it.
                let handle = tokio::spawn(forward_per_run_to_client(run_id, rx, writer_for_task));
                {
                    let mut s = state.lock().await;
                    s.subscriptions.insert(run_id);
                    // No prior forwarder is possible here: the
                    // idempotency block above returns early for any
                    // existing entry (alive or finished). Plain
                    // insert is correct.
                    s.forwarders.insert(run_id, handle);
                }
                Some(DaemonResponse::SubscribeOk { request_id })
            } else {
                // Run not yet registered in the broadcast registry. If
                // it is queued (`pending_starts` knows it), wait for
                // admission instead of rejecting — the per-run
                // channel is created in `drain_one_pass` BEFORE the
                // engine emits `RunStarted`, so a forwarder spawned
                // now will catch every event once admission lands.
                let is_queued = pending_starts.lock().await.contains_key(&run_id);
                if !is_queued {
                    return Some(DaemonResponse::Error {
                        request_id,
                        code: ErrorCode::RunNotActive,
                        message: format!("run {run_id} not active in this daemon"),
                    });
                }
                let writer_for_task = writer.clone();
                // `subscribe_eventual` parks a waiter inside the
                // registry and is woken atomically by the next
                // `register(run_id)` call (drain_one_pass). The
                // subscriber receiver is attached to the per-run
                // sender BEFORE register() returns, which is
                // strictly before `spawn_forward_task` is
                // spawned and starts pushing events — so no
                // event can land in the publisher before the
                // subscriber is in place.
                let pending_rx = broadcast.subscribe_eventual(run_id).await;
                tracing::debug!(
                    run_id = %run_id,
                    "subscribing to queued run; parked waiter for admission"
                );
                let handle = tokio::spawn(forward_queued_to_client(
                    run_id,
                    pending_rx,
                    writer_for_task,
                ));
                {
                    let mut s = state.lock().await;
                    s.subscriptions.insert(run_id);
                    s.forwarders.insert(run_id, handle);
                }
                Some(DaemonResponse::SubscribeOk { request_id })
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

/// Forwarder for a `Subscribe` issued before the run was admitted.
/// Awaits the oneshot returned by
/// [`BroadcastRegistry::subscribe_eventual`]; the registry attaches
/// a per-run [`broadcast::Receiver`] atomically when the queued run
/// is admitted by `drain_one_pass`, BEFORE the engine begins
/// emitting events into the publisher. Exits cleanly if the run is
/// cancelled while queued (observed as the oneshot resolving with
/// `Err(RecvError)` because `BroadcastRegistry::deregister` cleared
/// the waiter without ever calling `register`).
///
/// Spawned by the `Subscribe` handler when the run is in
/// `pending_starts` but not yet in the per-run registry; lets the
/// CLI's `subscribe_to_run` call survive admission queueing without
/// requiring an explicit re-subscribe after `RunAccepted`.
async fn forward_queued_to_client(
    run_id: RunId,
    pending_rx: tokio::sync::oneshot::Receiver<tokio::sync::broadcast::Receiver<EngineRunEvent>>,
    writer: Arc<Mutex<interprocess::local_socket::tokio::SendHalf>>,
) {
    match pending_rx.await {
        Ok(rx) => {
            tracing::debug!(
                run_id = %run_id,
                "queued subscribe: admission landed; forwarding per-run events"
            );
            forward_per_run_to_client(run_id, rx, writer).await;
        },
        Err(_) => {
            // Sender side dropped without delivering — typically
            // means the queued run was cancelled (StopRun on a
            // queued run, or daemon shutdown clearing waiters).
            tracing::debug!(
                run_id = %run_id,
                "queued subscribe: registry dropped waiter (run cancelled or daemon shutdown)"
            );
        },
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
    forward_global_to_writer(&mut rx, writer).await;
}

async fn forward_global_to_writer<W>(
    rx: &mut tokio::sync::broadcast::Receiver<GlobalDaemonEvent>,
    writer: Arc<Mutex<W>>,
) where
    W: AsyncWrite + Unpin,
{
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
                let frame = DaemonEvent::Global(GlobalDaemonEvent::SubscriberLagged { dropped: n });
                let mut w = writer.lock().await;
                let _ = write_frame(&mut *w, &frame).await;
                break;
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::BufReader;

    #[tokio::test]
    async fn global_forwarder_reports_lag_and_closes() {
        let (tx, _) = tokio::sync::broadcast::channel(2);
        let mut rx = tx.subscribe();
        for _ in 0..5 {
            tx.send(GlobalDaemonEvent::RunAccepted {
                run_id: RunId::new(),
            })
            .expect("send global event");
        }

        let (client, server) = tokio::io::duplex(4096);
        let writer = Arc::new(Mutex::new(server));
        forward_global_to_writer(&mut rx, writer).await;

        let mut reader = BufReader::new(client);
        let frame = surge_orchestrator::engine::ipc::read_inbound_server_frame(&mut reader)
            .await
            .expect("read lag response")
            .expect("lag response frame");

        match frame {
            surge_orchestrator::engine::ipc::InboundServerFrame::Event(DaemonEvent::Global(
                GlobalDaemonEvent::SubscriberLagged { dropped },
            )) => assert!(dropped > 0, "lagged event should report dropped count"),
            other => panic!("expected SubscriberLagged global event, got {other:?}"),
        }
    }
}
