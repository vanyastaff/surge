//! Agent session lease (#101): the one owner of an agent stage's provider
//! session, from opening through cleanup.
//!
//! [`SessionLease::open`] opens the session, checks the provider's writer
//! identity against the durable intent and writes `SessionOpened`. The live
//! lease runs the stage's prompt turns. [`SessionLease::settle`] consumes the
//! lease together with the stage body's result and is the only way to turn
//! that result into the stage's final one, so every failure after opening —
//! including those before the first prompt — goes through one cleanup path.
//!
//! # Ordering guarantees
//!
//! Opening, in order:
//! 1. subscribe to bridge events (nothing of the session can be missed);
//! 2. `open_session` — on failure only the stage endpoint is released, and
//!    the error keeps its [`OpenFailure`] classification;
//! 3. writer identity check — a provider whose observed writer differs from
//!    the durable intent is closed and **no** `SessionOpened` is written;
//! 4. `SessionOpened` — on write failure the session is closed.
//!
//! Settling, in order:
//! 1. release the stage MCP endpoint;
//! 2. close the bridge session — exactly once per lease;
//! 3. if the close failed, abort the in-flight turn, otherwise join it;
//! 4. persist usage observations still queued for the session;
//! 5. classify the close: confirmed (including a confirmed forced reap) or
//!    unconfirmed;
//! 6. write `SessionClosed` with the provider-reported disposition, else
//!    `Normal` for a clean success, else `ForcedClose` — only when the close
//!    was confirmed, because `SessionClosed` is the durable evidence that the
//!    provider writer is gone; an unconfirmed close leaves `SessionOpened`
//!    unpaired, which is exactly the "may still be live" signal recovery
//!    reads;
//! 7. combine the body's result with every cleanup failure into one error.
//!
//! A confirmed forced reap keeps a successful body result. Any cleanup
//! failure — an unconfirmed close above all — fails the stage: it is never
//! reported as success. A body that needs operator recovery or was cancelled
//! keeps that classification, with the cleanup failures in its message.
//!
//! A lease dropped without settling (its future was dropped) logs at error
//! level and schedules a detached best-effort close. It writes no events.

use std::collections::VecDeque;
use std::fmt;
use std::sync::Arc;

use surge_acp::bridge::error::{CloseSessionError, SendMessageError};
use surge_acp::bridge::event::{BridgeEvent, SessionEndReason};
use surge_acp::bridge::facade::BridgeFacade;
use surge_acp::bridge::session::{MessageContent, SessionConfig};
use surge_core::id::SessionId;
use surge_core::keys::NodeKey;
use surge_core::run_event::{EventPayload, SessionDisposition, VersionedEventPayload};
use surge_persistence::runs::run_writer::RunWriter;
use tokio::sync::broadcast;
use tokio::task::{JoinError, JoinHandle};
use tokio_util::sync::CancellationToken;

use crate::engine::stage::StageError;

/// How an `open_session` failure is reported. A reserved continuation that
/// cannot reopen its provider session needs an explicit recovery choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OpenFailure {
    /// An ordinary bridge failure.
    Bridge,
    /// Recovery is required before the stage can run again.
    RecoveryRequired,
}

/// The facts `SessionOpened` records besides the opened session itself.
#[derive(Debug, Clone)]
pub(crate) struct SessionOpenedFacts {
    /// Once-admitted quota opening epoch, if this is a quota handoff.
    pub handoff: Option<surge_core::id::WorkItemOperationId>,
    /// The node the session serves.
    pub node: NodeKey,
    /// The flow-authored profile reference.
    pub agent: String,
    /// The normalized runtime identity.
    pub agent_id: Option<String>,
}

/// Everything opening a lease needs.
pub(crate) struct LeaseRequest<'a> {
    /// The bridge the session is opened through.
    pub bridge: &'a Arc<dyn BridgeFacade>,
    /// The run writer `SessionOpened` / `SessionClosed` go to.
    pub writer: &'a RunWriter,
    /// The stage MCP endpoint, released when the lease settles.
    pub endpoint: surge_mcp::stage::StageEndpoint,
    /// The session to open; its `writer_id` is the durable writer intent.
    pub config: SessionConfig,
    /// The facts recorded with `SessionOpened`.
    pub opened: SessionOpenedFacts,
    /// How an `open_session` failure is classified.
    pub open_failure: OpenFailure,
}

/// A freshly opened lease with the stage's event stream.
pub(crate) struct OpenedLease<'a> {
    /// The live lease.
    pub lease: SessionLease<'a>,
    /// Bridge events, subscribed before the session was opened.
    pub events: broadcast::Receiver<BridgeEvent>,
    /// The sequence of the `SessionOpened` event.
    pub opened_seq: u64,
}

/// Bridge events the stage set aside, and the stream they came from; settling
/// persists the session's queued usage from both.
pub(crate) struct UsageQueue<'q> {
    /// The stage's bridge event stream.
    pub events: &'q mut broadcast::Receiver<BridgeEvent>,
    /// Events read ahead of the stage's event loop.
    pub deferred: &'q mut VecDeque<BridgeEvent>,
}

/// One prompt turn: the spawned send and its finished signal.
struct Turn {
    task: JoinHandle<Result<(), SendMessageError>>,
    joined: bool,
}

/// What a joined turn produced: the send's result, or the task's failure.
pub(crate) type TurnResult = Result<Result<(), SendMessageError>, JoinError>;

/// A live agent session. Settle it with [`SessionLease::settle`]; dropping it
/// unsettled only schedules a detached close.
#[must_use = "a session lease must be settled, or its session is only closed best-effort"]
pub(crate) struct SessionLease<'a> {
    bridge: Arc<dyn BridgeFacade>,
    writer: &'a RunWriter,
    session: SessionId,
    endpoint: Option<surge_mcp::stage::StageEndpoint>,
    turn: Option<Turn>,
    turn_finished: CancellationToken,
    ended: Option<SessionDisposition>,
    settled: bool,
}

impl<'a> SessionLease<'a> {
    /// Open the session: subscribe, `open_session`, the writer identity
    /// check, then `SessionOpened` (see the module docs for the order).
    ///
    /// # Errors
    /// The open failure classified by [`OpenFailure`]; a writer identity
    /// mismatch as [`StageError::Bridge`]; a `SessionOpened` write failure as
    /// [`StageError::Storage`]. Each names both cleanup results.
    #[must_use = "the opened lease must be settled"]
    pub(crate) async fn open(request: LeaseRequest<'a>) -> Result<OpenedLease<'a>, StageError> {
        let LeaseRequest {
            bridge,
            writer,
            endpoint,
            config,
            opened: facts,
            open_failure,
        } = request;
        let events = bridge.subscribe();
        let intent = config.writer_id;
        let opened = match bridge.open_session(config).await {
            Ok(opened) => opened,
            Err(error) => {
                let endpoint = endpoint.close().await.map_err(|error| error.to_string());
                let message = format!("open_session: {error}; stage endpoint: {endpoint:?}");
                return Err(match open_failure {
                    OpenFailure::Bridge => StageError::Bridge(message),
                    OpenFailure::RecoveryRequired => StageError::RecoveryRequired(message),
                });
            },
        };
        let session = opened.session;
        if opened
            .execution_writer
            .as_ref()
            .is_some_and(|observed| observed.writer() != intent)
        {
            let cleanup = close_unrecorded(bridge, session, endpoint).await;
            return Err(StageError::Bridge(format!(
                "provider writer identity mismatched its durable intent; {cleanup}"
            )));
        }
        let recorded = writer
            .append_event(VersionedEventPayload::new(EventPayload::SessionOpened {
                handoff: facts.handoff,
                opened: Some(opened),
                node: facts.node,
                session,
                agent: facts.agent,
                agent_id: facts.agent_id,
            }))
            .await;
        let opened_seq = match recorded {
            Ok(seq) => seq.as_u64(),
            Err(error) => {
                let cleanup = close_unrecorded(bridge, session, endpoint).await;
                return Err(StageError::Storage(format!(
                    "SessionOpened: {error}; {cleanup}"
                )));
            },
        };
        Ok(OpenedLease {
            lease: Self {
                bridge: Arc::clone(bridge),
                writer,
                session,
                endpoint: Some(endpoint),
                turn: None,
                turn_finished: CancellationToken::new(),
                ended: None,
                settled: false,
            },
            events,
            opened_seq,
        })
    }

    /// The leased session.
    #[must_use]
    pub(crate) fn session(&self) -> SessionId {
        self.session
    }

    /// Start a prompt turn: spawn the send and arm a fresh turn-finished
    /// signal. One turn is in flight at a time.
    ///
    /// # Errors
    /// [`StageError::Internal`] when the previous turn was not joined.
    pub(crate) fn start_turn(&mut self, message: MessageContent) -> Result<(), StageError> {
        if !self.turn_joined() {
            return Err(StageError::Internal(
                "a prompt turn started while the previous one was in flight".into(),
            ));
        }
        self.turn_finished = CancellationToken::new();
        let finished = self.turn_finished.clone();
        let bridge = Arc::clone(&self.bridge);
        let session = self.session;
        self.turn = Some(Turn {
            task: tokio::spawn(async move {
                let _finished = finished.drop_guard();
                bridge.send_message(session, message).await
            }),
            joined: false,
        });
        Ok(())
    }

    /// Cancelled when the current turn's send returns (or before the first
    /// turn, never). Permission and human-decision waits abandon on it.
    #[must_use]
    pub(crate) fn turn_finished(&self) -> &CancellationToken {
        &self.turn_finished
    }

    /// Whether no turn is in flight (none started, or the last one joined).
    #[must_use]
    pub(crate) fn turn_joined(&self) -> bool {
        self.turn.as_ref().is_none_or(|turn| turn.joined)
    }

    /// Wait for the in-flight turn. Cancel-safe: dropping the future before
    /// it completes leaves the turn in flight. Poll it only while
    /// [`Self::turn_joined`] is `false`.
    ///
    /// # Errors
    /// The task's failure; the send's own result is the `Ok` value.
    pub(crate) async fn join_turn(&mut self) -> TurnResult {
        let Some(turn) = self.turn.as_mut().filter(|turn| !turn.joined) else {
            return Ok(Ok(()));
        };
        let joined = (&mut turn.task).await;
        turn.joined = true;
        joined
    }

    /// Record how the provider reported the session ended; `SessionClosed`
    /// carries it.
    pub(crate) fn note_session_ended(&mut self, reason: &SessionEndReason) {
        self.ended = Some(match reason {
            SessionEndReason::Normal => SessionDisposition::Normal,
            SessionEndReason::AgentCrashed { .. } => SessionDisposition::AgentCrashed,
            SessionEndReason::Timeout { .. } => SessionDisposition::Timeout,
            SessionEndReason::ForcedClose => SessionDisposition::ForcedClose,
        });
    }

    /// Settle the lease with the stage body's result: the single cleanup
    /// path (see the module docs for the order).
    ///
    /// # Errors
    /// The body's error when cleanup was clean; otherwise one error naming
    /// every cleanup failure, both cleanup results and the body's result.
    #[must_use = "the settled result is the stage's result"]
    pub(crate) async fn settle<T: fmt::Debug>(
        mut self,
        body: Result<T, StageError>,
        usage: UsageQueue<'_>,
    ) -> Result<T, StageError> {
        let endpoint = match self.endpoint.take() {
            Some(endpoint) => endpoint.close().await.map_err(|error| error.to_string()),
            None => Ok(()),
        };
        let closed = self.bridge.close_session(self.session).await;
        // The bridge close has run: from here the backstop has nothing to do.
        // Dropped before this point, it still schedules the close.
        self.settled = true;

        let mut failures = Vec::new();
        if let Some(turn) = self.turn.take().filter(|turn| !turn.joined) {
            // A failed close leaves resource ownership in the bridge. Stop
            // only this caller task so an unconfirmed cleanup cannot hang the
            // engine driver.
            if closed.is_err() {
                turn.task.abort();
            }
            if let Err(error) = turn.task.await
                && (!error.is_cancelled() || closed.is_ok())
            {
                failures.push(Failure::bridge(format!("prompt task: {error}")));
            }
        }
        if let Err(error) =
            persist_queued_usage(usage.events, usage.deferred, self.session, self.writer).await
        {
            failures.push(Failure::from(error));
        }

        let forced_reap = matches!(
            &closed,
            Err(CloseSessionError::GracefulTimedOut { killed: true, .. })
        );
        if forced_reap {
            tracing::warn!(session = %self.session, "stage session was forcibly closed and reaped");
        }
        let confirmed = closed.is_ok() || forced_reap;
        if confirmed {
            let disposition = self.ended.unwrap_or(if body.is_ok() && !forced_reap {
                SessionDisposition::Normal
            } else {
                SessionDisposition::ForcedClose
            });
            if let Err(error) = self
                .writer
                .append_event(VersionedEventPayload::new(EventPayload::SessionClosed {
                    session: self.session,
                    disposition,
                }))
                .await
            {
                failures.push(Failure::storage(format!("SessionClosed: {error}")));
            }
        } else if let Err(error) = &closed {
            failures.push(Failure::bridge(format!("session cleanup failed: {error}")));
        }
        if let Err(error) = &endpoint {
            failures.push(Failure::bridge(format!("stage endpoint cleanup: {error}")));
        }

        if failures.is_empty() {
            return body;
        }
        let storage_only = failures.iter().all(|failure| failure.storage);
        let mut message = failures
            .into_iter()
            .map(|failure| failure.message)
            .collect::<Vec<_>>()
            .join("; ");
        let stage = match &body {
            Ok(outcome) => format!("outcome {outcome:?}"),
            Err(error) => format!("failed: {error}"),
        };
        message = format!(
            "{message}; {}; stage {stage}",
            Cleanup {
                session: &closed,
                endpoint: &endpoint,
            }
        );
        Err(match body {
            // Routing-significant outcomes survive a cleanup failure: an
            // operator-recovery stop must not become a retryable or
            // hook-suppressible failure, and a cancelled run stays cancelled.
            Err(StageError::RecoveryRequired(reason)) => {
                StageError::RecoveryRequired(format!("{reason}; {message}"))
            },
            Err(StageError::Cancelled) => {
                tracing::error!(session = %self.session, %message, "cleanup failed for a cancelled stage");
                StageError::Cancelled
            },
            _ if storage_only => StageError::Storage(message),
            _ => StageError::Bridge(message),
        })
    }
}

impl Drop for SessionLease<'_> {
    fn drop(&mut self) {
        if self.settled {
            return;
        }
        tracing::error!(
            session = %self.session,
            "agent session lease dropped without settling; scheduling a detached close \
             (no SessionClosed is recorded)"
        );
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            tracing::error!(session = %self.session, "no runtime to close the dropped session");
            return;
        };
        let bridge = Arc::clone(&self.bridge);
        let session = self.session;
        runtime.spawn(async move {
            if let Err(error) = bridge.close_session(session).await {
                tracing::error!(%session, %error, "detached close of a dropped session failed");
            }
        });
    }
}

/// One cleanup failure while settling.
struct Failure {
    message: String,
    storage: bool,
}

impl Failure {
    fn bridge(message: String) -> Self {
        Self {
            message,
            storage: false,
        }
    }

    fn storage(message: String) -> Self {
        Self {
            message,
            storage: true,
        }
    }
}

impl From<StageError> for Failure {
    fn from(error: StageError) -> Self {
        let storage = matches!(error, StageError::Storage(_));
        Self {
            message: error.to_string(),
            storage,
        }
    }
}

/// Both cleanup results, in the one shape every lease error uses.
struct Cleanup<'r> {
    session: &'r Result<(), CloseSessionError>,
    endpoint: &'r Result<(), String>,
}

impl fmt::Display for Cleanup<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "session cleanup: {:?}; stage endpoint: {:?}",
            self.session, self.endpoint
        )
    }
}

/// Close a session that was never recorded as opened, and the endpoint.
async fn close_unrecorded(
    bridge: &Arc<dyn BridgeFacade>,
    session: SessionId,
    endpoint: surge_mcp::stage::StageEndpoint,
) -> String {
    let endpoint = endpoint.close().await.map_err(|error| error.to_string());
    let closed = bridge.close_session(session).await;
    Cleanup {
        session: &closed,
        endpoint: &endpoint,
    }
    .to_string()
}

fn usage_stream_lag(skipped: u64) -> StageError {
    StageError::Bridge(format!("usage evidence stream lagged by {skipped} events"))
}

/// Record that usage evidence was lost to a lagging stream.
///
/// # Errors
/// [`StageError::Storage`] when the record cannot be written; otherwise the
/// lag error to return.
pub(super) async fn record_usage_stream_lag(
    writer: &RunWriter,
    session: SessionId,
    skipped: u64,
) -> Result<StageError, StageError> {
    writer
        .append_event(VersionedEventPayload::new(
            EventPayload::ProviderUsageObserved {
                session,
                usage: surge_core::TokenUsageObservation::Unavailable {
                    reason: surge_core::UsageUnavailableReason::Missing,
                    reported: None,
                },
            },
        ))
        .await
        .map_err(|error| StageError::Storage(error.to_string()))?;
    Ok(usage_stream_lag(skipped))
}

/// A successful prompt join is a publication barrier for final provider usage.
/// Persist those queued observations before accepting a buffered stage outcome.
/// Other events retain their relative order for the normal stage event loop.
///
/// # Errors
/// A storage failure, or the recorded lag of a lagging stream.
pub(super) async fn persist_queued_usage(
    events: &mut broadcast::Receiver<BridgeEvent>,
    deferred: &mut VecDeque<BridgeEvent>,
    session: SessionId,
    writer: &RunWriter,
) -> Result<(), StageError> {
    for _ in 0..events.len() {
        let event = match events.try_recv() {
            Ok(event) => event,
            Err(broadcast::error::TryRecvError::Lagged(skipped)) => {
                return Err(record_usage_stream_lag(writer, session, skipped).await?);
            },
            Err(_) => break,
        };
        let payload = match event {
            BridgeEvent::ProviderUsageObserved {
                session: actual,
                usage,
            } if actual == session => EventPayload::ProviderUsageObserved { session, usage },
            BridgeEvent::ContextUsageObserved {
                session: actual,
                usage,
            } if actual == session => EventPayload::ContextUsageObserved { session, usage },
            other => {
                deferred.push_back(other);
                continue;
            },
        };
        writer
            .append_event(VersionedEventPayload::new(payload))
            .await
            .map_err(|error| StageError::Storage(error.to_string()))?;
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests;
