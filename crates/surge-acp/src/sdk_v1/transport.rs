//! Bounded producer for the SDK's public Channel. No SDK Lines actor is used.
use super::{Error, LineLimit, Result};
use agent_client_protocol::{Channel, TransportFrame};
use futures::{AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, StreamExt};

/// Up to31 queued frames, one ordered SDK dispatch, and one capped lookahead.
const QUEUED_FRAMES: usize = 31;

pub(super) fn connect<W, R>(
    writer: W,
    reader: R,
) -> (
    Channel,
    impl Future<Output = Result<()>>,
    impl Future<Output = Result<()>>,
)
where
    W: AsyncWrite + Send + 'static,
    R: AsyncRead + Send + 'static,
{
    let (sdk, peer) = Channel::duplex();
    let Channel { mut rx, tx } = peer;
    let incoming = read_frames(reader, tx);
    let outgoing = async move {
        let mut writer = std::pin::pin!(writer);
        while let Some(frame) = rx.next().await {
            let line = frame
                .to_json()
                .map_err(|_| Error::new(-32000, "ACP outgoing frame invalid"))?;
            writer
                .write_all(line.as_bytes())
                .await
                .map_err(|_| Error::new(-32000, "ACP transport write failed"))?;
            writer
                .write_all(b"\n")
                .await
                .map_err(|_| Error::new(-32000, "ACP transport write failed"))?;
            writer
                .flush()
                .await
                .map_err(|_| Error::new(-32000, "ACP transport flush failed"))?;
        }
        Ok(())
    };
    (sdk, incoming, outgoing)
}

async fn read_frames(
    reader: impl AsyncRead,
    sender: futures::channel::mpsc::UnboundedSender<TransportFrame>,
) -> Result<()> {
    let mut reader = std::pin::pin!(futures::io::BufReader::with_capacity(8192, reader));
    let mut line = Vec::with_capacity(super::MAX_INCOMING_LINE + 1);
    let mut limit = LineLimit::default();
    loop {
        let buffer = reader
            .fill_buf()
            .await
            .map_err(|_| Error::new(-32000, "ACP transport read failed"))?;
        if buffer.is_empty() {
            if !line.is_empty() {
                send_frame(&line, &sender).await?;
            }
            return Ok(());
        }
        let count = buffer
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(buffer.len(), |position| position + 1);
        limit.accept(&buffer[..count])?;
        line.extend_from_slice(&buffer[..count]);
        let complete = buffer[count - 1] == b'\n';
        reader.as_mut().consume(count);
        if complete {
            send_frame(&line, &sender).await?;
            line.clear();
        }
    }
}

async fn send_frame(
    bytes: &[u8],
    sender: &futures::channel::mpsc::UnboundedSender<TransportFrame>,
) -> Result<()> {
    // This is the sole producer. The SDK can only decrease len between check/send.
    while sender.len() >= QUEUED_FRAMES {
        if sender.is_closed() {
            return Err(Error::new(-32000, "ACP transport owner ended"));
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    let message: agent_client_protocol::RawJsonRpcMessage =
        serde_json::from_slice(bytes).map_err(|_| {
            Error::new(
                -32000,
                "ACP incoming frame must be a single valid JSON-RPC message",
            )
        })?;
    let frame = TransportFrame::Single(message);
    sender
        .unbounded_send(frame)
        .map_err(|_| Error::new(-32000, "ACP transport owner ended"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn malformed_and_batch_frames_never_enter_sdk_channel() {
        for bytes in [b"not-json\n".as_slice(), b"[]\n", b"[{}]\n", b"{}\n"] {
            let (sdk, peer) = Channel::duplex();
            let error = read_frames(futures::io::Cursor::new(bytes), peer.tx.clone())
                .await
                .unwrap_err();
            assert!(error.to_string().contains("single valid JSON-RPC"));
            assert_eq!(peer.tx.len(), 0);
            drop(sdk);
        }
    }

    #[tokio::test]
    async fn closed_channel_ends_producer() {
        let (sdk, peer) = Channel::duplex();
        drop(sdk);
        let input = br#"{"jsonrpc":"2.0","id":1,"result":{}}"#;
        let error = read_frames(futures::io::Cursor::new(input), peer.tx)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("owner ended"));
    }
}
