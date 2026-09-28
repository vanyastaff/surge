//! Admission happens before RMCP can spawn a request handler.
use futures::{SinkExt, StreamExt};
use rmcp::{
    RoleServer,
    model::{ClientNotification, ErrorCode, ErrorData, GetExtensions, JsonRpcMessage, RequestId},
    service::{RxJsonRpcMessage, TxJsonRpcMessage},
    transport::worker::{Worker, WorkerContext, WorkerQuitReason},
};
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::{
    codec::{FramedRead, FramedWrite, LinesCodec},
    sync::CancellationToken,
};

const FRAME_LIMIT: usize = 256 * 1024;
const ACTIVE_LIMIT: usize = 8;
const REQUEST_LIMIT: usize = 128;
const WRITE_DEADLINE: Duration = Duration::from_secs(2);

#[derive(Clone)]
pub(super) struct RequestCancellation(pub CancellationToken);

#[derive(Debug, thiserror::Error)]
pub(super) enum TransportError {
    #[error("stage stdio transport closed")]
    Closed,
    #[error("stage stdio frame rejected")]
    Invalid,
    #[error("stage stdio resource limit reached")]
    Exhausted,
    #[error("stage stdio write failed")]
    Write,
    #[error("stage stdio worker failed")]
    Join,
}

pub(super) struct BoundedStdio<R, W> {
    read: FramedRead<R, LinesCodec>,
    write: FramedWrite<W, LinesCodec>,
    active: HashMap<RequestId, CancellationToken>,
    requests: usize,
    initialized: bool,
    failed: Arc<AtomicBool>,
}
impl<R: AsyncRead + Unpin, W: AsyncWrite + Unpin> BoundedStdio<R, W> {
    pub(super) fn new(reader: R, writer: W, failed: Arc<AtomicBool>) -> Self {
        Self {
            read: FramedRead::new(reader, LinesCodec::new_with_max_length(FRAME_LIMIT)),
            write: FramedWrite::new(writer, LinesCodec::new()),
            active: HashMap::new(),
            requests: 0,
            initialized: false,
            failed,
        }
    }
}
impl<R, W> Worker for BoundedStdio<R, W>
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    type Error = TransportError;
    type Role = RoleServer;
    fn err_closed() -> Self::Error {
        TransportError::Closed
    }
    fn err_join(_: tokio::task::JoinError) -> Self::Error {
        TransportError::Join
    }
    async fn run(
        mut self,
        mut context: WorkerContext<Self>,
    ) -> Result<(), WorkerQuitReason<Self::Error>> {
        let result = self.run_loop(&mut context).await;
        for token in self.active.values() {
            token.cancel();
        }
        if context.cancellation_token.is_cancelled() {
            return Ok(());
        }
        if result.is_err() {
            self.failed.store(true, Ordering::SeqCst);
        }
        result.map_err(|error| WorkerQuitReason::fatal(error, "dedicated stage stdio"))
    }
}
impl<R, W> BoundedStdio<R, W>
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    async fn run_loop(&mut self, context: &mut WorkerContext<Self>) -> Result<(), TransportError> {
        loop {
            tokio::select! {
                biased;
                () = context.cancellation_token.cancelled() => return Ok(()),
                outgoing = context.from_handler_rx.recv() => {
                    let Some(outgoing) = outgoing else { return Ok(()); };
                    let result = self.send_response(outgoing.message, &context.cancellation_token).await;
                    let failed = result.is_err();
                    let _ = outgoing.responder.send(result);
                    if failed { return Err(TransportError::Write); }
                },
                incoming = self.read.next() => {
                    let Some(incoming) = incoming else { return Ok(()); };
                    let line = incoming.map_err(|_| TransportError::Invalid)?;
                    let message: RxJsonRpcMessage<RoleServer> = serde_json::from_str(&line).map_err(|_| TransportError::Invalid)?;
                    self.receive(message, context).await?;
                },
            }
        }
    }

    async fn receive(
        &mut self,
        message: RxJsonRpcMessage<RoleServer>,
        context: &mut WorkerContext<Self>,
    ) -> Result<(), TransportError> {
        match message {
            JsonRpcMessage::Request(mut request) => {
                if self.active.contains_key(&request.id) {
                    // Two responses sharing an active wire ID would be ambiguous.
                    return Err(TransportError::Invalid);
                }
                self.requests += 1;
                if self.requests > REQUEST_LIMIT {
                    self.reject(
                        request.id,
                        "stage helper request limit reached",
                        &context.cancellation_token,
                    )
                    .await?;
                    return Err(TransportError::Exhausted);
                }
                if self.active.len() >= ACTIVE_LIMIT {
                    return self
                        .reject(
                            request.id,
                            "stage helper request capacity exceeded",
                            &context.cancellation_token,
                        )
                        .await;
                }
                let token = context.cancellation_token.child_token();
                request
                    .request
                    .extensions_mut()
                    .insert(RequestCancellation(token.clone()));
                self.active.insert(request.id.clone(), token);
                context
                    .to_handler_tx
                    .try_send(JsonRpcMessage::Request(request))
                    .map_err(|_| TransportError::Exhausted)?;
                Ok(())
            },
            JsonRpcMessage::Notification(notification) => match &notification.notification {
                ClientNotification::CancelledNotification(cancelled) => {
                    if let Some(token) = self.active.get(&cancelled.params.request_id) {
                        token.cancel();
                    }
                    Ok(())
                },
                ClientNotification::InitializedNotification(_) if !self.initialized => {
                    self.initialized = true;
                    context
                        .to_handler_tx
                        .try_send(JsonRpcMessage::Notification(notification))
                        .map_err(|_| TransportError::Exhausted)
                },
                _ => Err(TransportError::Invalid),
            },
            _ => Err(TransportError::Invalid),
        }
    }

    async fn reject(
        &mut self,
        id: RequestId,
        reason: &'static str,
        stop: &CancellationToken,
    ) -> Result<(), TransportError> {
        // This path never enters RMCP, opens a local stage call, or releases an active ID.
        self.write_message(
            JsonRpcMessage::error(ErrorData::new(ErrorCode(-32000), reason, None), id),
            stop,
        )
        .await
    }

    async fn send_response(
        &mut self,
        message: TxJsonRpcMessage<RoleServer>,
        stop: &CancellationToken,
    ) -> Result<(), TransportError> {
        let id = match &message {
            JsonRpcMessage::Response(response) => Some(response.id.clone()),
            JsonRpcMessage::Error(error) => Some(error.id.clone()),
            _ => None,
        };
        self.write_message(message, stop).await?;
        if let Some(id) = id {
            self.active.remove(&id);
        }
        Ok(())
    }

    async fn write_message(
        &mut self,
        message: TxJsonRpcMessage<RoleServer>,
        stop: &CancellationToken,
    ) -> Result<(), TransportError> {
        let line = serde_json::to_string(&message).map_err(|_| TransportError::Write)?;
        tokio::select! {
            biased;
            () = stop.cancelled() => Err(TransportError::Closed),
            result = tokio::time::timeout(WRITE_DEADLINE, self.write.send(line)) => result.map_err(|_| TransportError::Write)?.map_err(|_| TransportError::Write),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_util::codec::Decoder;

    #[tokio::test]
    async fn admission_slot_is_held_until_response_flush() {
        let (writer, _unread_peer) = tokio::io::duplex(32);
        let mut worker =
            BoundedStdio::new(tokio::io::empty(), writer, Arc::new(AtomicBool::new(false)));
        let id = RequestId::Number(1);
        worker.active.insert(id.clone(), CancellationToken::new());
        let response = JsonRpcMessage::error(
            ErrorData::new(ErrorCode(-32000), "x".repeat(4096), None),
            id.clone(),
        );
        let stop = CancellationToken::new();
        assert!(
            tokio::time::timeout(
                Duration::from_millis(20),
                worker.send_response(response, &stop)
            )
            .await
            .is_err()
        );
        assert!(
            worker.active.contains_key(&id),
            "handler completion is not response flush"
        );
        let mut worker = BoundedStdio::new(
            tokio::io::empty(),
            tokio::io::sink(),
            Arc::new(AtomicBool::new(false)),
        );
        worker.active.insert(id.clone(), CancellationToken::new());
        worker
            .send_response(
                JsonRpcMessage::error(ErrorData::new(ErrorCode(-32000), "done", None), id),
                &stop,
            )
            .await
            .unwrap();
        assert!(worker.active.is_empty());
    }

    #[test]
    fn bounded_line_codec_checks_fragments_and_delimiters() {
        let mut codec = LinesCodec::new_with_max_length(FRAME_LIMIT);
        let mut bytes = tokio_util::bytes::BytesMut::from(vec![b'x'; FRAME_LIMIT].as_slice());
        assert!(codec.decode(&mut bytes).unwrap().is_none());
        bytes.extend_from_slice(b"\nfirst\nsecond\n");
        assert_eq!(
            codec.decode(&mut bytes).unwrap().unwrap().len(),
            FRAME_LIMIT
        );
        assert_eq!(codec.decode(&mut bytes).unwrap().unwrap(), "first");
        assert_eq!(codec.decode(&mut bytes).unwrap().unwrap(), "second");
        bytes.extend_from_slice(&vec![b'x'; FRAME_LIMIT + 1]);
        assert!(codec.decode(&mut bytes).is_err());
    }
}
