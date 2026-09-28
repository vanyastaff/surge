//! Controlled SDK-v1 peer: delayed request handlers never hold SDK dispatch.
use super::{MockAgent, NotifItem, PermissionItem, acp};
use agent_client_protocol::{ByteStreams, Responder};
use futures::{StreamExt, stream::FuturesUnordered};
use serde_json::Value;
use std::{rc::Rc, sync::Arc};
use tokio::sync::{Semaphore, mpsc, oneshot};
use tokio::task::JoinSet;

pub async fn run<W, R>(
    agent: MockAgent,
    writer: W,
    reader: R,
    mut outbound: mpsc::UnboundedReceiver<NotifItem>,
    mut permissions: mpsc::UnboundedReceiver<PermissionItem>,
) -> acp::Result<()>
where
    W: futures::AsyncWrite + Send + 'static,
    R: futures::AsyncRead + Send + 'static,
{
    let agent = Rc::new(agent);
    let (requests_tx, mut requests) = mpsc::channel(32);
    let (notifications_tx, mut notifications) = mpsc::channel(32);
    let permits = Arc::new(Semaphore::new(32));
    let builder = agent_client_protocol::Agent
        .builder()
        .on_receive_request(
            async move |request: acp::ClientRequest, responder: Responder<Value>, _cx| {
                let Ok(permit) = permits.clone().try_acquire_owned() else {
                    return responder.respond_with_error(acp::Error::new(
                        -32000,
                        "mock request capacity exceeded",
                    ));
                };
                requests_tx
                    .try_send((request, responder, permit))
                    .map_err(|_| acp::Error::internal_error())?;
                Ok(())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_notification(
            async move |notification: acp::ClientNotification, _cx| {
                let (ack, received) = oneshot::channel();
                notifications_tx
                    .try_send((notification, ack))
                    .map_err(|_| acp::Error::internal_error())?;
                received.await.map_err(|_| acp::Error::internal_error())?
            },
            agent_client_protocol::on_receive_notification!(),
        );
    let driver = builder.connect_with(ByteStreams::new(writer, reader), async move |connection| {
        let send_notifications = async {
            while let Some((notification, ack)) = outbound.recv().await {
                connection.send_notification(notification)?;
                let _ = ack.send(());
            }
            Ok::<(), acp::Error>(())
        };
        let send_permissions = async {
            let mut pending = FuturesUnordered::new();
            loop {
                tokio::select! {
                    Some((request, reply)) = permissions.recv(), if pending.len() < 64 => {
                        let connection = connection.clone();
                        pending.push(async move {
                            let response = connection.send_request(request).block_task().await;
                            let _ = reply.send(response);
                        });
                    },
                    Some(()) = pending.next(), if !pending.is_empty() => {},
                    else => return Ok::<(), acp::Error>(()),
                }
            }
        };
        tokio::select! {
            () = connection.incoming_closed() => Ok(()),
            result = send_notifications => result,
            result = send_permissions => result,
        }
    });
    let mut tasks = JoinSet::new();
    tokio::pin!(driver);
    let result = loop {
        tokio::select! {
            result = &mut driver => break result,
            Some((request, responder, permit)) = requests.recv() => {
                let agent = agent.clone();
                tasks.spawn_local(async move {
                    let _ = responder.respond_with_result(dispatch(&agent, request).await);
                    drop(permit);
                });
            },
            Some((notification, ack)) = notifications.recv() => {
                let result = match notification {
                    acp::ClientNotification::CancelNotification(request) => agent.cancel(request).await,
                    _ => Ok(()),
                };
                let _ = ack.send(result);
            },
            Some(result) = tasks.join_next(), if !tasks.is_empty() => {
                if result.is_err() { break Err(acp::Error::internal_error()); }
            },
        }
    };
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
    result
}

async fn dispatch(agent: &MockAgent, request: acp::ClientRequest) -> acp::Result<Value> {
    let response = match request {
        acp::ClientRequest::InitializeRequest(request) => {
            serde_json::to_value(agent.initialize(request).await?)
        },
        acp::ClientRequest::AuthenticateRequest(request) => {
            serde_json::to_value(agent.authenticate(request).await?)
        },
        acp::ClientRequest::NewSessionRequest(request) => {
            serde_json::to_value(agent.new_session(request).await?)
        },
        acp::ClientRequest::PromptRequest(request) => {
            serde_json::to_value(agent.prompt(request).await?)
        },
        _ => return Err(acp::Error::method_not_found()),
    };
    response.map_err(|_| acp::Error::internal_error())
}
