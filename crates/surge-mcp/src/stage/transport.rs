//! Restricted local transport with bounded framing and explicit session lifetime.

use super::{
    StageToolCall, StageToolContext, StageToolDefinition, StageToolReply, StageTransportError,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

const FRAME_LIMIT: usize = 256 * 1024;
/// Environment variable inherited by the provider's MCP helper. Never log its value.
pub const AUTH_ENV: &str = "SURGE_STAGE_MCP_AUTH";
/// Endpoint locator; it does not grant authority without authentication.
pub const ENDPOINT_ENV: &str = "SURGE_STAGE_MCP_ENDPOINT";

#[derive(Serialize, Deserialize)]
pub(super) struct Request {
    pub auth: String,
    pub operation: Operation,
}

#[derive(Serialize, Deserialize)]
pub(super) enum Operation {
    Catalog,
    Monitor,
    Call { tool: String, arguments: Value },
}

/// Owns endpoint lifetime. Dropping revokes authority; `close` verifies task cleanup.
pub struct StageEndpoint {
    address: String,
    auth: String,
    stop: CancellationToken,
    task: Option<tokio::task::JoinHandle<Result<(), StageTransportError>>>,
}

impl StageEndpoint {
    /// Create a unique local endpoint for one pinned stage generation.
    pub fn open(
        context: StageToolContext,
        catalog: Vec<StageToolDefinition>,
    ) -> Result<(Self, mpsc::Receiver<StageToolCall>), StageTransportError> {
        let auth = surge_core::content_hash::ContentHash::from_bytes(rand::random()).to_hex();
        let listener = super::local::Listener::bind()?;
        let address = listener.address().to_owned();
        let stop = CancellationToken::new();
        let (send, receive) = mpsc::channel(8);
        let task = tokio::spawn(serve(
            listener,
            context,
            catalog,
            auth.clone(),
            send,
            stop.clone(),
        ));
        Ok((
            Self {
                address,
                auth,
                stop,
                task: Some(task),
            },
            receive,
        ))
    }

    /// Nonsecret local locator.
    #[must_use]
    pub fn address(&self) -> &str {
        &self.address
    }

    /// Transient helper environment credential. Do not persist, display, or log this value.
    #[must_use]
    pub fn credential(&self) -> &str {
        &self.auth
    }

    /// Revoke all connections and confirm the endpoint task has settled.
    pub async fn close(mut self) -> Result<(), StageTransportError> {
        self.stop.cancel();
        if let Some(task) = self.task.take() {
            tokio::time::timeout(std::time::Duration::from_secs(2), task)
                .await
                .map_err(|_| StageTransportError::CleanupUnconfirmed)?
                .map_err(|_| StageTransportError::CleanupUnconfirmed)??;
        }
        Ok(())
    }
}

impl Drop for StageEndpoint {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}

async fn serve(
    listener: super::local::Listener,
    context: StageToolContext,
    catalog: Vec<StageToolDefinition>,
    auth: String,
    send: mpsc::Sender<StageToolCall>,
    stop: CancellationToken,
) -> Result<(), StageTransportError> {
    let mut tasks = tokio::task::JoinSet::new();
    let result = loop {
        tokio::select! {
            biased;
            () = stop.cancelled() => break Ok(()),
            Some(_) = tasks.join_next(), if !tasks.is_empty() => {},
            accepted = listener.accept(), if tasks.len() < 16 => {
                let stream = match accepted { Ok(stream) => stream, Err(e) => break Err(e.into()) };
                let context = context.clone();
                let catalog = catalog.clone();
                let auth = auth.clone();
                let send = send.clone();
                let stop = stop.clone();
                tasks.spawn(async move {
                    tokio::select! {
                        biased;
                        () = stop.cancelled() => Ok(()),
                        result = connection(stream, context, catalog, auth, send) => result,
                    }
                });
            },
        }
    };
    stop.cancel();
    while tasks.join_next().await.is_some() {}
    result
}

async fn connection(
    mut stream: super::local::Stream,
    context: StageToolContext,
    catalog: Vec<StageToolDefinition>,
    auth: String,
    send: mpsc::Sender<StageToolCall>,
) -> Result<(), StageTransportError> {
    let request: Request =
        tokio::time::timeout(std::time::Duration::from_secs(5), read_frame(&mut stream))
            .await
            .map_err(|_| StageTransportError::Rejected("authentication deadline".into()))??;
    if !same_secret(request.auth.as_bytes(), auth.as_bytes()) {
        return Err(StageTransportError::Rejected(
            "authentication failed".into(),
        ));
    }
    match request.operation {
        Operation::Catalog => write_frame(&mut stream, &catalog).await,
        Operation::Monitor => {
            write_frame(&mut stream, &true).await?;
            let mut byte = [0];
            let _ = stream.read(&mut byte).await?;
            Ok(())
        },
        Operation::Call { tool, arguments } => {
            if arguments.to_string().contains(&auth) {
                return write_frame(
                    &mut stream,
                    &StageToolReply::rejected("authentication material is not a tool argument"),
                )
                .await;
            }
            if !catalog.iter().any(|entry| entry.name == tool) {
                return write_frame(
                    &mut stream,
                    &StageToolReply::rejected("tool is not visible in this session"),
                )
                .await;
            }
            let (reply, result) = oneshot::channel();
            let call = StageToolCall {
                context,
                tool,
                arguments,
                reply,
            };
            let mut byte = [0];
            let response = tokio::select! {
                _ = stream.read(&mut byte) => return Ok(()),
                response = async {
                    send.send(call).await.map_err(|_| StageTransportError::Rejected("stage ended".into()))?;
                    result.await.map_err(|_| StageTransportError::Rejected("stage ended before durable reply".into()))
                } => response?,
            };
            write_frame(&mut stream, &response).await
        },
    }
}

fn same_secret(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |diff, (x, y)| diff | (x ^ y)) == 0
}

pub(super) async fn read_frame<T: serde::de::DeserializeOwned>(
    stream: &mut (impl AsyncRead + Unpin),
) -> Result<T, StageTransportError> {
    let length = stream.read_u32().await? as usize;
    if length > FRAME_LIMIT {
        return Err(StageTransportError::Rejected("frame too large".into()));
    }
    let mut bytes = vec![0; length];
    stream.read_exact(&mut bytes).await?;
    Ok(serde_json::from_slice(&bytes)?)
}

pub(super) async fn write_frame<T: Serialize>(
    stream: &mut (impl AsyncWrite + Unpin),
    value: &T,
) -> Result<(), StageTransportError> {
    let bytes = serde_json::to_vec(value)?;
    if bytes.len() > FRAME_LIMIT {
        return Err(StageTransportError::Rejected("frame too large".into()));
    }
    let length = u32::try_from(bytes.len())
        .map_err(|_| StageTransportError::Rejected("frame too large".into()))?;
    stream.write_u32(length).await?;
    stream.write_all(&bytes).await?;
    stream.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use surge_core::id::{RunId, SessionId, StageGenerationId};
    use surge_core::keys::NodeKey;

    fn context() -> StageToolContext {
        StageToolContext {
            run: RunId::new(),
            node: NodeKey::try_from("impl_1").unwrap(),
            session: SessionId::new(),
            generation: StageGenerationId::new(),
        }
    }

    #[tokio::test]
    async fn authenticated_call_waits_for_owner_reply_and_close_revokes_connections() {
        let expected = context();
        let (endpoint, mut calls) = StageEndpoint::open(
            expected.clone(),
            vec![StageToolDefinition {
                name: "report_stage_outcome".into(),
                description: "candidate".into(),
                input_schema: serde_json::json!({"type":"object"}),
            }],
        )
        .unwrap();
        let mut stream = super::super::local::connect(endpoint.address())
            .await
            .unwrap();
        write_frame(
            &mut stream,
            &Request {
                auth: endpoint.credential().into(),
                operation: Operation::Call {
                    tool: "report_stage_outcome".into(),
                    arguments: serde_json::json!({"call_id":"x"}),
                },
            },
        )
        .await
        .unwrap();
        let call = tokio::time::timeout(std::time::Duration::from_secs(1), calls.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(call.context, expected);
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(20),
                read_frame::<StageToolReply>(&mut stream)
            )
            .await
            .is_err()
        );
        call.reply
            .send(StageToolReply {
                is_error: false,
                value: serde_json::json!({"accepted":true}),
            })
            .unwrap();
        let reply: StageToolReply = read_frame(&mut stream).await.unwrap();
        assert_eq!(reply.value, serde_json::json!({"accepted":true}));
        let address = endpoint.address().to_owned();
        endpoint.close().await.unwrap();
        assert!(super::super::local::connect(&address).await.is_err());
    }

    #[tokio::test]
    async fn invalid_auth_and_hidden_tools_never_reach_owner() {
        let (endpoint, mut calls) = StageEndpoint::open(context(), vec![]).unwrap();
        let mut stream = super::super::local::connect(endpoint.address())
            .await
            .unwrap();
        write_frame(
            &mut stream,
            &Request {
                auth: "wrong".into(),
                operation: Operation::Catalog,
            },
        )
        .await
        .unwrap();
        assert!(read_frame::<Value>(&mut stream).await.is_err());
        let mut stream = super::super::local::connect(endpoint.address())
            .await
            .unwrap();
        write_frame(
            &mut stream,
            &Request {
                auth: endpoint.credential().into(),
                operation: Operation::Call {
                    tool: "denied_tool".into(),
                    arguments: Value::Null,
                },
            },
        )
        .await
        .unwrap();
        let reply: StageToolReply = read_frame(&mut stream).await.unwrap();
        assert!(reply.is_error);
        assert!(calls.try_recv().is_err());
        endpoint.close().await.unwrap();
    }
}
