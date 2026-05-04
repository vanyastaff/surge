//! IPC protocol types shared between the daemon (`surge-daemon`) and
//! the daemon-facing client (`DaemonEngineFacade`). Wire format is
//! line-delimited JSON; one frame per line, no embedded newlines
//! (compact `serde_json`).

use crate::engine::config::EngineRunConfig;
use crate::engine::handle::{EngineRunEvent, RunOutcome, RunSummary};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use surge_core::graph::Graph;
use surge_core::id::RunId;

/// Monotonically-increasing client-side request identifier. Echoed
/// in the matching [`DaemonResponse`] so the client can multiplex
/// requests over a single socket.
pub type RequestId = u64;

/// Stable error codes for IPC error responses.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// The request is malformed or missing required fields.
    BadRequest,
    /// `start_run` for an already-active run id.
    RunAlreadyActive,
    /// Lookup of a run id that the daemon never saw.
    RunNotFound,
    /// Lookup of a run id that the daemon knows but is not currently active.
    RunNotActive,
    /// `AdmissionController` queue overflow.
    AdmissionFull,
    /// Underlying storage error (sqlite I/O, etc.).
    StorageError,
    /// Engine-level error (graph validation, run lifecycle, etc.).
    EngineError,
    /// Unexpected internal failure not in the above buckets.
    Internal,
    /// The daemon is in graceful shutdown and refusing new work.
    ShuttingDown,
}

/// Request frames sent from CLI to daemon.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "snake_case")]
pub enum DaemonRequest {
    /// Health check + version handshake.
    Ping {
        /// Client-assigned identifier echoed in the response.
        request_id: RequestId,
    },
    /// Begin a new run.
    StartRun {
        /// Client-assigned identifier echoed in the response.
        request_id: RequestId,
        /// Identifier for the run being started.
        run_id: RunId,
        /// The frozen pipeline graph to execute. Boxed to keep the enum
        /// variant size small (graphs can be hundreds of KB).
        graph: Box<Graph>,
        /// Path to the isolated git worktree the engine will operate in.
        worktree_path: PathBuf,
        /// Per-run engine configuration knobs.
        run_config: EngineRunConfig,
    },
    /// Resume an existing run from its latest snapshot.
    ResumeRun {
        /// Client-assigned identifier echoed in the response.
        request_id: RequestId,
        /// Identifier of the run to resume.
        run_id: RunId,
        /// Path to the isolated git worktree the engine will operate in.
        worktree_path: PathBuf,
    },
    /// Cancel an in-flight run.
    StopRun {
        /// Client-assigned identifier echoed in the response.
        request_id: RequestId,
        /// Identifier of the run to cancel.
        run_id: RunId,
        /// Human-readable reason for the cancellation.
        reason: String,
    },
    /// Provide an answer to a paused run waiting on human input.
    ResolveHumanInput {
        /// Client-assigned identifier echoed in the response.
        request_id: RequestId,
        /// Identifier of the run that is waiting for input.
        run_id: RunId,
        /// Optional ACP call id that ties this response to the pending tool call.
        call_id: Option<String>,
        /// The human's response value, forwarded to the engine as-is.
        response: serde_json::Value,
    },
    /// List all runs the daemon knows about.
    ListRuns {
        /// Client-assigned identifier echoed in the response.
        request_id: RequestId,
    },
    /// Subscribe to per-run events. The daemon will start sending
    /// [`DaemonEvent::PerRun`] notifications until [`DaemonRequest::Unsubscribe`]
    /// or the connection closes.
    Subscribe {
        /// Client-assigned identifier echoed in the response.
        request_id: RequestId,
        /// Identifier of the run to subscribe to.
        run_id: RunId,
    },
    /// Cancel a previous subscription.
    Unsubscribe {
        /// Client-assigned identifier echoed in the response.
        request_id: RequestId,
        /// Identifier of the run to unsubscribe from.
        run_id: RunId,
    },
    /// Begin a graceful daemon shutdown.
    Shutdown {
        /// Client-assigned identifier echoed in the response.
        request_id: RequestId,
    },
}

impl DaemonRequest {
    /// Returns the `request_id` of the carried request.
    #[must_use]
    pub fn request_id(&self) -> RequestId {
        match self {
            Self::Ping { request_id }
            | Self::StartRun { request_id, .. }
            | Self::ResumeRun { request_id, .. }
            | Self::StopRun { request_id, .. }
            | Self::ResolveHumanInput { request_id, .. }
            | Self::ListRuns { request_id }
            | Self::Subscribe { request_id, .. }
            | Self::Unsubscribe { request_id, .. }
            | Self::Shutdown { request_id } => *request_id,
        }
    }
}

/// Response frames sent from daemon to CLI.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "snake_case")]
pub enum DaemonResponse {
    /// Reply to [`DaemonRequest::Ping`]. Carries the daemon binary version string.
    PingOk {
        /// Echoed `request_id` from the originating [`DaemonRequest::Ping`].
        request_id: RequestId,
        /// Semver version string of the running daemon binary.
        version: String,
    },
    /// [`DaemonRequest::StartRun`] accepted; engine has started executing the run.
    StartRunOk {
        /// Echoed `request_id` from the originating [`DaemonRequest::StartRun`].
        request_id: RequestId,
        /// Identifier of the run that was started.
        run_id: RunId,
    },
    /// [`DaemonRequest::StartRun`] accepted but queued behind the admission cap.
    /// The run will start when an active slot frees up.
    StartRunQueued {
        /// Echoed `request_id` from the originating [`DaemonRequest::StartRun`].
        request_id: RequestId,
        /// Identifier of the run that was queued.
        run_id: RunId,
        /// One-based position in the admission queue.
        position: usize,
    },
    /// [`DaemonRequest::ResumeRun`] accepted; engine has begun resuming the run.
    ResumeRunOk {
        /// Echoed `request_id` from the originating [`DaemonRequest::ResumeRun`].
        request_id: RequestId,
    },
    /// [`DaemonRequest::StopRun`] accepted; cancellation is in flight.
    StopRunOk {
        /// Echoed `request_id` from the originating [`DaemonRequest::StopRun`].
        request_id: RequestId,
    },
    /// [`DaemonRequest::ResolveHumanInput`] accepted; the run will resume.
    ResolveHumanInputOk {
        /// Echoed `request_id` from the originating [`DaemonRequest::ResolveHumanInput`].
        request_id: RequestId,
    },
    /// [`DaemonRequest::ListRuns`] reply.
    ListRunsOk {
        /// Echoed `request_id` from the originating [`DaemonRequest::ListRuns`].
        request_id: RequestId,
        /// Snapshot of all runs the daemon currently knows about.
        runs: Vec<RunSummary>,
    },
    /// [`DaemonRequest::Subscribe`] accepted; per-run events will follow.
    SubscribeOk {
        /// Echoed `request_id` from the originating [`DaemonRequest::Subscribe`].
        request_id: RequestId,
    },
    /// [`DaemonRequest::Unsubscribe`] accepted.
    UnsubscribeOk {
        /// Echoed `request_id` from the originating [`DaemonRequest::Unsubscribe`].
        request_id: RequestId,
    },
    /// [`DaemonRequest::Shutdown`] accepted; daemon is draining.
    ShutdownOk {
        /// Echoed `request_id` from the originating [`DaemonRequest::Shutdown`].
        request_id: RequestId,
    },
    /// Generic error response. `code` is stable enough for clients
    /// to react programmatically; `message` is operator-facing.
    Error {
        /// Echoed `request_id` from the originating request.
        request_id: RequestId,
        /// Machine-readable error classification.
        code: ErrorCode,
        /// Human-readable error description.
        message: String,
    },
}

impl DaemonResponse {
    /// Returns the `request_id` this response correlates to.
    #[must_use]
    pub fn request_id(&self) -> RequestId {
        match self {
            Self::PingOk { request_id, .. }
            | Self::StartRunOk { request_id, .. }
            | Self::StartRunQueued { request_id, .. }
            | Self::ResumeRunOk { request_id }
            | Self::StopRunOk { request_id }
            | Self::ResolveHumanInputOk { request_id }
            | Self::ListRunsOk { request_id, .. }
            | Self::SubscribeOk { request_id }
            | Self::UnsubscribeOk { request_id }
            | Self::ShutdownOk { request_id }
            | Self::Error { request_id, .. } => *request_id,
        }
    }
}

/// Notification frames pushed from daemon to CLI (no `request_id` —
/// these are fire-and-forget broadcasts).
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DaemonEvent {
    /// Per-run event delivered to subscribers via [`DaemonRequest::Subscribe`].
    PerRun {
        /// Identifier of the run this event belongs to.
        run_id: RunId,
        /// The engine event payload.
        event: EngineRunEvent,
    },
    /// Daemon-level event delivered to all connected clients.
    Global(GlobalDaemonEvent),
}

/// Daemon-level events broadcast to every connected client.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum GlobalDaemonEvent {
    /// A run was admitted (left the queue, engine picked it up).
    RunAccepted {
        /// Identifier of the run that was admitted.
        run_id: RunId,
    },
    /// A run terminated. The outcome carries success / failure / abort.
    RunFinished {
        /// Identifier of the run that finished.
        run_id: RunId,
        /// Terminal outcome of the run.
        outcome: RunOutcome,
    },
    /// The daemon is beginning graceful shutdown.
    DaemonShuttingDown,
}

// ============================================================================
// Framing helpers
// ============================================================================

use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

/// Errors produced by the IPC framing layer.
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum FramingError {
    /// Underlying I/O error reading or writing the socket.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// Failed to encode or decode a frame as JSON.
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    /// A frame exceeded the maximum allowed size.
    #[error("frame too large ({0} bytes; cap is {1})")]
    FrameTooLarge(usize, usize),
}

/// Maximum size of a single IPC frame in bytes. Larger frames (e.g.,
/// a Graph TOML payload over 8 MB) are rejected. Adjust if real
/// workloads bump up against this.
pub const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;

/// Serialize and write a single frame followed by `\n`. Compact JSON
/// (no embedded newlines).
pub async fn write_frame<W, T>(writer: &mut W, frame: &T) -> Result<(), FramingError>
where
    W: AsyncWrite + Unpin,
    T: serde::Serialize,
{
    let mut bytes = serde_json::to_vec(frame)?;
    if bytes.len() > MAX_FRAME_BYTES {
        return Err(FramingError::FrameTooLarge(bytes.len(), MAX_FRAME_BYTES));
    }
    bytes.push(b'\n');
    writer.write_all(&bytes).await?;
    writer.flush().await?;
    Ok(())
}

/// Read one line from `reader` and parse it as a [`DaemonRequest`].
/// Returns `Ok(None)` on EOF (clean disconnect).
pub async fn read_request_frame<R>(
    reader: &mut BufReader<R>,
) -> Result<Option<DaemonRequest>, FramingError>
where
    R: AsyncRead + Unpin,
{
    let mut line = String::new();
    let n = reader.read_line(&mut line).await?;
    if n == 0 {
        return Ok(None);
    }
    if line.len() > MAX_FRAME_BYTES {
        return Err(FramingError::FrameTooLarge(line.len(), MAX_FRAME_BYTES));
    }
    Ok(Some(serde_json::from_str(line.trim_end())?))
}

/// Read one line and parse as a [`DaemonResponse`].
pub async fn read_response_frame<R>(
    reader: &mut BufReader<R>,
) -> Result<Option<DaemonResponse>, FramingError>
where
    R: AsyncRead + Unpin,
{
    let mut line = String::new();
    let n = reader.read_line(&mut line).await?;
    if n == 0 {
        return Ok(None);
    }
    if line.len() > MAX_FRAME_BYTES {
        return Err(FramingError::FrameTooLarge(line.len(), MAX_FRAME_BYTES));
    }
    Ok(Some(serde_json::from_str(line.trim_end())?))
}

/// Read one line and parse as a [`DaemonEvent`] (notification frame).
pub async fn read_event_frame<R>(
    reader: &mut BufReader<R>,
) -> Result<Option<DaemonEvent>, FramingError>
where
    R: AsyncRead + Unpin,
{
    let mut line = String::new();
    let n = reader.read_line(&mut line).await?;
    if n == 0 {
        return Ok(None);
    }
    if line.len() > MAX_FRAME_BYTES {
        return Err(FramingError::FrameTooLarge(line.len(), MAX_FRAME_BYTES));
    }
    Ok(Some(serde_json::from_str(line.trim_end())?))
}

/// Either a [`DaemonResponse`] or a [`DaemonEvent`], since they share
/// a wire stream (server → client). Used by the daemon-side client
/// task that has to route both kinds.
#[non_exhaustive]
#[derive(Debug)]
pub enum InboundServerFrame {
    /// A response to an earlier client request, identified by `request_id`.
    Response(DaemonResponse),
    /// A daemon-pushed notification (no `request_id`).
    Event(DaemonEvent),
}

/// Read one server-bound frame and discriminate. Tries
/// [`DaemonResponse`] first; on parse failure, falls back to
/// [`DaemonEvent`].
pub async fn read_inbound_server_frame<R>(
    reader: &mut BufReader<R>,
) -> Result<Option<InboundServerFrame>, FramingError>
where
    R: AsyncRead + Unpin,
{
    let mut line = String::new();
    let n = reader.read_line(&mut line).await?;
    if n == 0 {
        return Ok(None);
    }
    if line.len() > MAX_FRAME_BYTES {
        return Err(FramingError::FrameTooLarge(line.len(), MAX_FRAME_BYTES));
    }
    let trimmed = line.trim_end();
    // Both share serde discriminator tags — try response first.
    if let Ok(r) = serde_json::from_str::<DaemonResponse>(trimmed) {
        return Ok(Some(InboundServerFrame::Response(r)));
    }
    let ev: DaemonEvent = serde_json::from_str(trimmed)?;
    Ok(Some(InboundServerFrame::Event(ev)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ping_request_serde_roundtrips() {
        let req = DaemonRequest::Ping { request_id: 42 };
        let s = serde_json::to_string(&req).unwrap();
        assert!(!s.contains('\n'), "compact json must not have newlines");
        let parsed: DaemonRequest = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed.request_id(), 42);
    }

    #[test]
    fn error_response_carries_code() {
        let r = DaemonResponse::Error {
            request_id: 7,
            code: ErrorCode::RunNotFound,
            message: "no such run".into(),
        };
        let s = serde_json::to_string(&r).unwrap();
        let parsed: DaemonResponse = serde_json::from_str(&s).unwrap();
        match parsed {
            DaemonResponse::Error {
                code, request_id, ..
            } => {
                assert_eq!(code, ErrorCode::RunNotFound);
                assert_eq!(request_id, 7);
            }
            _ => panic!("expected Error variant"),
        }
    }

    #[test]
    fn shutting_down_event_serializes() {
        let ev = DaemonEvent::Global(GlobalDaemonEvent::DaemonShuttingDown);
        let s = serde_json::to_string(&ev).unwrap();
        let parsed: DaemonEvent = serde_json::from_str(&s).unwrap();
        match parsed {
            DaemonEvent::Global(GlobalDaemonEvent::DaemonShuttingDown) => {}
            _ => panic!("roundtrip failed"),
        }
    }

    #[tokio::test]
    async fn write_and_read_frame_roundtrip() {
        use tokio::io::{BufReader, BufWriter};

        // Write a frame into an in-memory Vec<u8> buffer.
        let buf: Vec<u8> = Vec::new();
        let mut writer = BufWriter::new(buf);
        let req = DaemonRequest::Ping { request_id: 100 };
        crate::engine::ipc::write_frame(&mut writer, &req)
            .await
            .unwrap();
        // BufWriter wraps Vec<u8>; flush was already called inside write_frame.
        let bytes = writer.into_inner();
        let text = std::str::from_utf8(&bytes).unwrap();
        assert!(text.ends_with('\n'), "frame must end with newline");

        // Round-trip: parse it back via read_request_frame.
        let cursor = std::io::Cursor::new(bytes);
        let mut cursor_reader = BufReader::new(cursor);
        let parsed = crate::engine::ipc::read_request_frame(&mut cursor_reader)
            .await
            .unwrap();
        assert!(matches!(
            parsed,
            Some(DaemonRequest::Ping { request_id: 100 })
        ));
    }

    #[tokio::test]
    async fn read_request_frame_returns_none_on_eof() {
        use tokio::io::BufReader;
        let empty: &[u8] = &[];
        let mut reader = BufReader::new(empty);
        let parsed = crate::engine::ipc::read_request_frame(&mut reader).await.unwrap();
        assert!(parsed.is_none());
    }
}
