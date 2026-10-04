//! Private stable-v1 SDK adapter. Local callback state never enters SDK handlers.
use agent_client_protocol::schema::v1::*;
use agent_client_protocol::{Agent, ConnectionTo, Responder};
mod transport;
use serde_json::Value;
use std::{rc::Rc, sync::Arc};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc, oneshot, watch};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

const CALLBACK_LIMIT: usize = 32;
/// Shared bound for EOF dispatch drain and explicit legacy driver cleanup.
pub(crate) const DRIVER_CLEANUP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
// Transport invariant: at most 1 MiB of JSON bytes before each newline.
const MAX_INCOMING_LINE: usize = 1024 * 1024;
#[derive(Default)]
struct LineLimit(usize);
impl LineLimit {
    fn accept(&mut self, bytes: &[u8]) -> Result<()> {
        for byte in bytes {
            if *byte == b'\n' {
                self.0 = 0;
            } else {
                self.0 += 1;
                if self.0 > MAX_INCOMING_LINE {
                    return Err(Error::new(-32000, "ACP incoming frame exceeds 1 MiB limit"));
                }
            }
        }
        Ok(())
    }
}

#[async_trait::async_trait(?Send)]
pub(crate) trait ClientCallbacks {
    async fn request(&self, request: AgentRequest) -> Result<Value>;
    async fn notification(&self, notification: AgentNotification) -> Result<()>;
}

struct PendingRequest {
    request: AgentRequest,
    responder: Responder<Value>,
    _permit: OwnedSemaphorePermit,
}
struct PendingNotification {
    notification: AgentNotification,
    processed: oneshot::Sender<Result<()>>,
}

/// Private distinction between host authority refusal and a provider protocol error.
#[derive(thiserror::Error)]
pub(crate) enum SdkCallError {
    #[error(transparent)]
    HostEffectRefused(#[from] crate::bridge::effect_fence::HostEffectRefused),
    #[error(transparent)]
    Protocol(#[from] Error),
}
impl std::fmt::Debug for SdkCallError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::HostEffectRefused(error) => std::fmt::Debug::fmt(error, formatter),
            Self::Protocol(error) => std::fmt::Debug::fmt(error, formatter),
        }
    }
}
pub(crate) type CallResult<T> = std::result::Result<T, SdkCallError>;

pub(crate) struct ClientConnection {
    ready: watch::Receiver<Option<ConnectionTo<Agent>>>,
    stop: CancellationToken,
    effect_fence: Option<Arc<dyn crate::bridge::effect_fence::HostEffectFence>>,
}
impl Drop for ClientConnection {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}
impl ClientConnection {
    pub(crate) fn new<C, W, R>(
        client: C,
        writer: W,
        reader: R,
    ) -> (Self, impl Future<Output = Result<()>>)
    where
        C: ClientCallbacks + 'static,
        W: futures::AsyncWrite + Send + 'static,
        R: futures::AsyncRead + Send + 'static,
    {
        let (ready_tx, ready) = watch::channel(None);
        let stop = CancellationToken::new();
        let run_stop = stop.clone();
        let (request_tx, requests) = mpsc::channel(CALLBACK_LIMIT);
        let (notification_tx, notifications) = mpsc::channel(CALLBACK_LIMIT);
        let permits = Arc::new(Semaphore::new(CALLBACK_LIMIT));
        let builder = agent_client_protocol::Client
            .builder()
            .on_receive_request(
                async move |request: AgentRequest, responder: Responder<Value>, _cx| {
                    let Ok(permit) = permits.clone().try_acquire_owned() else {
                        return responder.respond_with_error(Error::new(
                            -32000,
                            "client callback capacity exceeded",
                        ));
                    };
                    let pending = PendingRequest {
                        request,
                        responder,
                        _permit: permit,
                    };
                    if let Err(error) = request_tx.try_send(pending) {
                        return error
                            .into_inner()
                            .responder
                            .respond_with_error(Error::new(-32000, "client callback unavailable"));
                    }
                    Ok(())
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_notification(
                async move |notification: AgentNotification, _cx| {
                    let (processed, done) = oneshot::channel();
                    notification_tx
                        .try_send(PendingNotification {
                            notification,
                            processed,
                        })
                        .map_err(|_| Error::new(-32000, "client notification capacity exceeded"))?;
                    // Preserve wire ordering: acknowledgement means processed, not enqueued.
                    done.await
                        .map_err(|_| Error::new(-32000, "client notification owner ended"))?
                },
                agent_client_protocol::on_receive_notification!(),
            );
        let future = async move {
            let (channel, pump, outgoing) = transport::connect(writer, reader);
            let driver = builder.connect_with(channel, async move |cx| {
                ready_tx
                    .send(Some(cx.clone()))
                    .map_err(|_| Error::internal_error())?;
                tokio::select! { () = run_stop.cancelled() => {}, () = cx.incoming_closed() => {} }
                Ok(())
            });
            let driven = async move {
                tokio::select! { result = driver => result, result = outgoing => result }
            };
            drive(client, requests, notifications, driven, pump).await
        };
        (
            Self {
                ready,
                stop,
                effect_fence: None,
            },
            future,
        )
    }
    pub(crate) fn set_effect_fence(
        &mut self,
        fence: Option<Arc<dyn crate::bridge::effect_fence::HostEffectFence>>,
    ) {
        self.effect_fence = fence;
    }
    pub(crate) fn stop(&self) {
        self.stop.cancel();
    }
    async fn connection(&self) -> Result<ConnectionTo<Agent>> {
        let mut ready = self.ready.clone();
        loop {
            if let Some(connection) = ready.borrow().clone() {
                return Ok(connection);
            }
            ready
                .changed()
                .await
                .map_err(|_| Error::new(-32000, "ACP connection driver ended"))?;
        }
    }
    async fn effect_connection(&self) -> CallResult<ConnectionTo<Agent>> {
        let connection = self.connection().await?;
        // Final Surge boundary: callers immediately enqueue with send_request in
        // this same poll, before awaiting the SDK response task.
        crate::bridge::effect_fence::check(self.effect_fence.as_ref())?;
        Ok(connection)
    }
    pub(crate) async fn initialize(
        &self,
        request: InitializeRequest,
    ) -> CallResult<InitializeResponse> {
        self.effect_connection()
            .await?
            .send_request(request)
            .block_task()
            .await
            .map_err(SdkCallError::Protocol)
    }
    pub(crate) async fn new_session(
        &self,
        request: NewSessionRequest,
    ) -> CallResult<NewSessionResponse> {
        self.effect_connection()
            .await?
            .send_request(request)
            .block_task()
            .await
            .map_err(SdkCallError::Protocol)
    }
    pub(crate) async fn load_session(
        &self,
        request: agent_client_protocol::schema::v1::LoadSessionRequest,
    ) -> CallResult<agent_client_protocol::schema::v1::LoadSessionResponse> {
        self.effect_connection()
            .await?
            .send_request(request)
            .block_task()
            .await
            .map_err(SdkCallError::Protocol)
    }
    pub(crate) async fn resume_session(
        &self,
        request: agent_client_protocol::schema::v1::ResumeSessionRequest,
    ) -> CallResult<agent_client_protocol::schema::v1::ResumeSessionResponse> {
        self.effect_connection()
            .await?
            .send_request(request)
            .block_task()
            .await
            .map_err(SdkCallError::Protocol)
    }
    pub(crate) async fn set_session_mode(
        &self,
        request: SetSessionModeRequest,
    ) -> CallResult<SetSessionModeResponse> {
        self.effect_connection()
            .await?
            .send_request(request)
            .block_task()
            .await
            .map_err(SdkCallError::Protocol)
    }
    pub(crate) async fn set_session_config_option(
        &self,
        request: SetSessionConfigOptionRequest,
    ) -> CallResult<SetSessionConfigOptionResponse> {
        self.effect_connection()
            .await?
            .send_request(request)
            .block_task()
            .await
            .map_err(SdkCallError::Protocol)
    }
    pub(crate) async fn prompt(&self, request: PromptRequest) -> CallResult<PromptResponse> {
        self.effect_connection()
            .await?
            .send_request(request)
            .block_task()
            .await
            .map_err(SdkCallError::Protocol)
    }
    pub(crate) async fn cancel(&self, request: CancelNotification) -> Result<()> {
        self.connection().await?.send_notification(request)
    }
}

async fn drive<C: ClientCallbacks + 'static>(
    client: C,
    mut requests: mpsc::Receiver<PendingRequest>,
    mut notifications: mpsc::Receiver<PendingNotification>,
    driver: impl Future<Output = Result<()>>,
    pump: impl Future<Output = Result<()>>,
) -> Result<()> {
    let client = Rc::new(client);
    let mut tasks = JoinSet::new();
    tokio::pin!(driver, pump);
    let mut eof = false;
    let mut eof_deadline = tokio::time::Instant::now();
    let result = 'drive: loop {
        tokio::select! {
            result = &mut driver => break result,
            result = &mut pump, if !eof => {
                if let Err(error) = result { break Err(error); }
                eof = true;
                eof_deadline = tokio::time::Instant::now() + DRIVER_CLEANUP_TIMEOUT;
            },
            () = tokio::time::sleep_until(eof_deadline), if eof => {
                // Completed tasks can still occupy JoinSet slots. Drain them before
                // classifying an EOF stall; a successful callback is not an error.
                while let Some(result) = tasks.try_join_next() {
                    match result {
                        Ok(Ok(())) => {},
                        _ => break 'drive Err(Error::new(-32000, "client callback task failed")),
                    }
                }
                break Err(Error::new(-32000, "ACP transport closed before dispatch settled"));
            },
            Some(pending) = notifications.recv() => {
                let client = client.clone();
                tasks.spawn_local(async move {
                    let result = client.notification(pending.notification).await;
                    let failed = result.is_err();
                    let _ = pending.processed.send(result);
                    if failed { Err(Error::new(-32000, "client notification processing failed")) } else { Ok(()) }
                });
            },
            Some(pending) = requests.recv() => {
                let client = client.clone();
                tasks.spawn_local(async move {
                    let result = client.request(pending.request).await;
                    let _ = pending.responder.respond_with_result(result);
                    drop(pending._permit);
                    Ok(())
                });
            },
            Some(result) = tasks.join_next(), if !tasks.is_empty() => {
                match result {
                    Ok(Ok(())) => {},
                    Ok(Err(error)) => break Err(error),
                    Err(_) => break Err(Error::new(-32000, "client callback task failed")),
                }
            },
        }
    };
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
    result
}

macro_rules! client_callbacks {
    ($client:ty) => {
        #[async_trait::async_trait(?Send)]
        impl $crate::sdk_v1::ClientCallbacks for $client {
            async fn request(
                &self,
                request: agent_client_protocol::schema::v1::AgentRequest,
            ) -> agent_client_protocol::Result<serde_json::Value> {
                use agent_client_protocol::schema::v1::AgentRequest as Request;
                let response = match request {
                    Request::RequestPermissionRequest(req) => {
                        serde_json::to_value(self.request_permission(req).await?)
                    },
                    Request::WriteTextFileRequest(req) => {
                        serde_json::to_value(self.write_text_file(req).await?)
                    },
                    Request::ReadTextFileRequest(req) => {
                        serde_json::to_value(self.read_text_file(req).await?)
                    },
                    Request::CreateTerminalRequest(req) => {
                        serde_json::to_value(self.create_terminal(req).await?)
                    },
                    Request::TerminalOutputRequest(req) => {
                        serde_json::to_value(self.terminal_output(req).await?)
                    },
                    Request::WaitForTerminalExitRequest(req) => {
                        serde_json::to_value(self.wait_for_terminal_exit(req).await?)
                    },
                    Request::KillTerminalRequest(req) => {
                        serde_json::to_value(self.kill_terminal(req).await?)
                    },
                    Request::ReleaseTerminalRequest(req) => {
                        serde_json::to_value(self.release_terminal(req).await?)
                    },
                    Request::ExtMethodRequest(req) => {
                        serde_json::to_value(self.ext_method(req).await?)
                    },
                    _ => return Err(agent_client_protocol::Error::method_not_found()),
                };
                response.map_err(|_| agent_client_protocol::Error::internal_error())
            }
            async fn notification(
                &self,
                notification: agent_client_protocol::schema::v1::AgentNotification,
            ) -> agent_client_protocol::Result<()> {
                use agent_client_protocol::schema::v1::AgentNotification as Notification;
                match notification {
                    Notification::SessionNotification(notification) => {
                        self.session_notification(notification).await
                    },
                    Notification::ExtNotification(notification) => {
                        self.ext_notification(notification).await
                    },
                    _ => Ok(()),
                }
            }
        }
    };
}
pub(crate) use client_callbacks;

#[cfg(test)]
mod lifecycle_tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::io::AsyncWriteExt;
    use tokio::sync::Notify;
    use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

    struct ReadinessFence {
        permitted: AtomicBool,
        checks: std::sync::atomic::AtomicUsize,
    }
    impl crate::bridge::effect_fence::HostEffectFence for ReadinessFence {
        fn check(&self) -> std::result::Result<(), crate::bridge::effect_fence::HostEffectRefused> {
            self.checks.fetch_add(1, Ordering::SeqCst);
            if self.permitted.load(Ordering::SeqCst) {
                Ok(())
            } else {
                Err(crate::bridge::effect_fence::HostEffectRefused)
            }
        }
    }

    #[tokio::test]
    async fn initialize_rechecks_host_after_driver_readiness_before_wire_admission() {
        use agent_client_protocol::schema::ProtocolVersion;
        use tokio::io::{AsyncBufReadExt, BufReader};
        tokio::task::LocalSet::new().run_until(async {
            for permitted_after_ready in [true,false] {
                let (local,peer)=tokio::io::duplex(4096);
                let (read,write)=tokio::io::split(local);
                let (peer_read,mut peer_write)=tokio::io::split(peer);
                let callbacks=HeldNotification { entered:Arc::new(Notify::new()),release:Arc::new(Notify::new()),dropped:Arc::new(AtomicBool::new(false)) };
                let (mut connection,driver)=ClientConnection::new(callbacks,write.compat_write(),read.compat());
                let fence=Arc::new(ReadinessFence { permitted:AtomicBool::new(true),checks:std::sync::atomic::AtomicUsize::new(0) });
                let host:Arc<dyn crate::bridge::effect_fence::HostEffectFence>=fence.clone();
                connection.set_effect_fence(Some(host.clone()));
                let mut opening=Box::pin(crate::bridge::effect_fence::admitted(Some(&host),connection.initialize(InitializeRequest::new(ProtocolVersion::V1))));
                let mut context=std::task::Context::from_waker(std::task::Waker::noop());
                assert!(std::future::Future::poll(opening.as_mut(),&mut context).is_pending());
                assert_eq!(fence.checks.load(Ordering::SeqCst),1,"initial admission must precede readiness wait");
                fence.permitted.store(permitted_after_ready,Ordering::SeqCst);
                let task=tokio::task::spawn_local(driver);
                let mut reader=BufReader::new(peer_read);
                let mut line=String::new();
                let (sent,result)=tokio::time::timeout(std::time::Duration::from_secs(2),async {
                    tokio::select! {
                        result=&mut opening=>(false,result),
                        count=reader.read_line(&mut line)=>{
                            assert!(count.unwrap()>0,"actual peer closed before admission observation");
                            let request:Value=serde_json::from_str(&line).unwrap();
                            assert_eq!(request["method"],"initialize");
                            let response=serde_json::json!({"jsonrpc":"2.0","id":request["id"],"result":InitializeResponse::new(ProtocolVersion::V1)});
                            let mut bytes=serde_json::to_vec(&response).unwrap();bytes.push(b'\n');
                            peer_write.write_all(&bytes).await.unwrap();
                            (true,opening.await)
                        }
                    }
                }).await.unwrap();
                let final_checks=fence.checks.load(Ordering::SeqCst);
                let late_request=if permitted_after_ready { false } else {
                    let mut late=String::new();
                    tokio::time::timeout(std::time::Duration::from_millis(100),reader.read_line(&mut late)).await.is_ok()
                };
                connection.stop();
                drop(reader);drop(peer_write);
                tokio::time::timeout(std::time::Duration::from_secs(2),task).await.unwrap().unwrap().unwrap();
                if permitted_after_ready {
                    assert!(sent && result.is_ok_and(|response|response.is_ok()),"positive readiness transition must initialize exactly");
                } else {
                    assert!(!sent,"Stop during Surge-owned readiness wait admitted actual initialize bytes");
                    assert!(!late_request,"refused initialize was queued after the result completed");
                    assert!(matches!(result,Ok(Err(SdkCallError::HostEffectRefused(_)))),"final readiness refusal lost its exact host type");
                }
                assert_eq!(final_checks,2,"final callback must follow driver readiness");
            }
        }).await;
    }

    async fn effect_method(connection: &ClientConnection, method: usize) -> CallResult<()> {
        use agent_client_protocol::schema::ProtocolVersion;
        match method {
            0 => connection
                .initialize(InitializeRequest::new(ProtocolVersion::V1))
                .await
                .map(|_| ()),
            1 => connection
                .new_session(NewSessionRequest::new("/workspace"))
                .await
                .map(|_| ()),
            2 => connection
                .load_session(LoadSessionRequest::new("provider-session", "/workspace"))
                .await
                .map(|_| ()),
            3 => connection
                .resume_session(ResumeSessionRequest::new("provider-session", "/workspace"))
                .await
                .map(|_| ()),
            4 => connection
                .set_session_mode(SetSessionModeRequest::new("provider-session", "safe-mode"))
                .await
                .map(|_| ()),
            5 => connection
                .set_session_config_option(SetSessionConfigOptionRequest::new(
                    "provider-session",
                    "mode",
                    "safe",
                ))
                .await
                .map(|_| ()),
            6 => connection
                .prompt(PromptRequest::new("provider-session", Vec::new()))
                .await
                .map(|_| ()),
            _ => panic!("unknown test effect method"),
        }
    }

    #[tokio::test]
    async fn all_effect_methods_refuse_after_readiness_without_retry_or_cleanup_blocking() {
        use tokio::io::{AsyncBufReadExt, BufReader};
        tokio::task::LocalSet::new()
            .run_until(async {
                for method in 0..7 {
                    let (local, peer) = tokio::io::duplex(4096);
                    let (read, write) = tokio::io::split(local);
                    let (peer_read, _peer_write) = tokio::io::split(peer);
                    let callbacks = HeldNotification {
                        entered: Arc::new(Notify::new()),
                        release: Arc::new(Notify::new()),
                        dropped: Arc::new(AtomicBool::new(false)),
                    };
                    let (mut connection, driver) =
                        ClientConnection::new(callbacks, write.compat_write(), read.compat());
                    let fence = Arc::new(ReadinessFence {
                        permitted: AtomicBool::new(true),
                        checks: std::sync::atomic::AtomicUsize::new(0),
                    });
                    connection.set_effect_fence(Some(fence.clone()));
                    let mut request = Box::pin(effect_method(&connection, method));
                    let mut context = std::task::Context::from_waker(std::task::Waker::noop());
                    assert!(std::future::Future::poll(request.as_mut(), &mut context).is_pending());
                    assert_eq!(fence.checks.load(Ordering::SeqCst), 0);
                    fence.permitted.store(false, Ordering::SeqCst);
                    let task = tokio::task::spawn_local(driver);
                    assert!(matches!(
                        tokio::time::timeout(std::time::Duration::from_secs(2), request)
                            .await
                            .unwrap(),
                        Err(SdkCallError::HostEffectRefused(_))
                    ));
                    assert!(matches!(
                        effect_method(&connection, method).await,
                        Err(SdkCallError::HostEffectRefused(_))
                    ));
                    assert_eq!(fence.checks.load(Ordering::SeqCst), 2);
                    let mut reader = BufReader::new(peer_read);
                    let mut line = String::new();
                    assert!(
                        tokio::time::timeout(
                            std::time::Duration::from_millis(100),
                            reader.read_line(&mut line)
                        )
                        .await
                        .is_err(),
                        "refused effect or retry reached the actual peer"
                    );
                    connection
                        .cancel(CancelNotification::new("provider-session"))
                        .await
                        .unwrap();
                    tokio::time::timeout(
                        std::time::Duration::from_secs(2),
                        reader.read_line(&mut line),
                    )
                    .await
                    .unwrap()
                    .unwrap();
                    let cleanup: Value = serde_json::from_str(&line).unwrap();
                    assert_eq!(cleanup["method"], "session/cancel");
                    assert!(
                        cleanup.get("id").is_none(),
                        "cleanup must remain a notification"
                    );
                    assert_eq!(
                        fence.checks.load(Ordering::SeqCst),
                        2,
                        "cleanup must not require effect admission"
                    );
                    connection.stop();
                    tokio::time::timeout(std::time::Duration::from_secs(2), task)
                        .await
                        .unwrap()
                        .unwrap()
                        .unwrap();
                }
            })
            .await;
    }

    #[tokio::test]
    async fn all_admitted_effect_methods_reach_the_actual_peer_and_typed_response() {
        use tokio::io::{AsyncBufReadExt, BufReader};
        let methods = [
            "initialize",
            "session/new",
            "session/load",
            "session/resume",
            "session/set_mode",
            "session/set_config_option",
            "session/prompt",
        ];
        let responses = [
            serde_json::json!({"protocolVersion":1,"agentCapabilities":{}}),
            serde_json::json!({"sessionId":"provider-session"}),
            serde_json::json!({}),
            serde_json::json!({}),
            serde_json::json!({}),
            serde_json::json!({"configOptions":[]}),
            serde_json::json!({"stopReason":"end_turn"}),
        ];
        tokio::task::LocalSet::new().run_until(async {
            for (method, expected) in methods.into_iter().enumerate() {
                let (local, peer) = tokio::io::duplex(4096);
                let (read, write) = tokio::io::split(local);
                let (peer_read, mut peer_write) = tokio::io::split(peer);
                let callbacks = HeldNotification { entered: Arc::new(Notify::new()), release: Arc::new(Notify::new()), dropped: Arc::new(AtomicBool::new(false)) };
                let (mut connection, driver) = ClientConnection::new(callbacks, write.compat_write(), read.compat());
                let fence = Arc::new(ReadinessFence { permitted: AtomicBool::new(true), checks: std::sync::atomic::AtomicUsize::new(0) });
                connection.set_effect_fence(Some(fence.clone()));
                let task = tokio::task::spawn_local(driver);
                let mut reader = BufReader::new(peer_read);
                let peer_response = async {
                    let mut line = String::new();
                    assert!(reader.read_line(&mut line).await.unwrap() > 0);
                    let request: Value = serde_json::from_str(&line).unwrap();
                    assert_eq!(request["method"], expected);
                    assert!(request["id"].is_number() || request["id"].is_string(), "actual JSON-RPC request ID is absent");
                    let response = serde_json::json!({"jsonrpc":"2.0","id":request["id"],"result":responses[method]});
                    let mut bytes = serde_json::to_vec(&response).unwrap(); bytes.push(b'\n');
                    peer_write.write_all(&bytes).await.unwrap();
                };
                let (result, ()) = tokio::time::timeout(std::time::Duration::from_secs(2), async { tokio::join!(effect_method(&connection, method), peer_response) }).await.unwrap();
                assert!(result.is_ok(), "admitted method failed its actual typed response: {expected}");
                assert_eq!(fence.checks.load(Ordering::SeqCst), 1);
                connection.stop();
                tokio::time::timeout(std::time::Duration::from_secs(2), task).await.unwrap().unwrap().unwrap();
            }
        }).await;
    }

    struct HeldNotification {
        entered: Arc<Notify>,
        release: Arc<Notify>,
        dropped: Arc<AtomicBool>,
    }
    impl Drop for HeldNotification {
        fn drop(&mut self) {
            self.dropped.store(true, Ordering::SeqCst);
        }
    }
    #[async_trait::async_trait(?Send)]
    impl ClientCallbacks for HeldNotification {
        async fn request(&self, _: AgentRequest) -> Result<Value> {
            std::future::pending().await
        }
        async fn notification(&self, _: AgentNotification) -> Result<()> {
            self.entered.notify_one();
            self.release.notified().await;
            Ok(())
        }
    }

    #[tokio::test]
    async fn stop_joins_outstanding_notification_callback() {
        tokio::task::LocalSet::new().run_until(async {
            let (local, mut peer) = tokio::io::duplex(4096);
            let (read, write) = tokio::io::split(local);
            let entered = Arc::new(Notify::new());
            let dropped = Arc::new(AtomicBool::new(false));
            let callbacks = HeldNotification { entered: entered.clone(), release: Arc::new(Notify::new()), dropped: dropped.clone() };
            let (connection, driver) = ClientConnection::new(callbacks, write.compat_write(), read.compat());
            let task = tokio::task::spawn_local(driver);
            peer.write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"session/update\",\"params\":{\"sessionId\":\"s\",\"update\":{\"sessionUpdate\":\"agent_message_chunk\",\"content\":{\"type\":\"text\",\"text\":\"hello\"}}}}\n").await.unwrap();
            tokio::time::timeout(std::time::Duration::from_secs(1), entered.notified()).await.unwrap();
            connection.stop();
            tokio::time::timeout(std::time::Duration::from_secs(1), task).await.expect("driver must terminate while notification is outstanding").unwrap().unwrap();
            assert!(dropped.load(Ordering::SeqCst));
        }).await;
    }
    #[tokio::test]
    async fn prompt_response_waits_for_notification_processing() {
        use tokio::io::{AsyncBufReadExt, BufReader};
        tokio::task::LocalSet::new().run_until(async {
            let (local, peer) = tokio::io::duplex(4096);
            let (read, write) = tokio::io::split(local);
            let (peer_read, mut peer_write) = tokio::io::split(peer);
            let entered = Arc::new(Notify::new());
            let release = Arc::new(Notify::new());
            let callbacks = HeldNotification { entered: entered.clone(), release: release.clone(), dropped: Arc::new(AtomicBool::new(false)) };
            let (connection, driver) = ClientConnection::new(callbacks, write.compat_write(), read.compat());
            let connection = Rc::new(connection);
            let task = tokio::task::spawn_local(driver);
            let prompt_connection = connection.clone();
            let mut prompt = tokio::task::spawn_local(async move { prompt_connection.prompt(PromptRequest::new("s", vec![])).await });
            let mut reader = BufReader::new(peer_read);
            let mut line = String::new();
            reader.read_line(&mut line).await.unwrap();
            let request: Value = serde_json::from_str(&line).unwrap();
            let notification = serde_json::json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"hello"}}}});
            let response = serde_json::json!({"jsonrpc":"2.0","id":request["id"],"result":{"stopReason":"end_turn"}});
            peer_write.write_all(format!("{notification}\n{response}\n").as_bytes()).await.unwrap();
            tokio::time::timeout(std::time::Duration::from_secs(1), entered.notified()).await.unwrap();
            assert!(tokio::time::timeout(std::time::Duration::from_millis(50), &mut prompt).await.is_err(), "wire response must not overtake owner processing");
            drop(reader);
            drop(peer_write);
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            release.notify_one();
            tokio::time::timeout(std::time::Duration::from_secs(1), prompt).await.unwrap().unwrap().unwrap();
            tokio::time::timeout(std::time::Duration::from_secs(1), task).await.unwrap().unwrap().unwrap();
        }).await;
    }

    #[tokio::test]
    async fn eof_joins_outstanding_callback() {
        tokio::task::LocalSet::new().run_until(async {
            let (local, mut peer) = tokio::io::duplex(4096);
            let (read, write) = tokio::io::split(local);
            let entered = Arc::new(Notify::new());
            let dropped = Arc::new(AtomicBool::new(false));
            let callbacks = HeldNotification { entered: entered.clone(), release: Arc::new(Notify::new()), dropped: dropped.clone() };
            let (_connection, driver) = ClientConnection::new(callbacks, write.compat_write(), read.compat());
            let task = tokio::task::spawn_local(driver);
            peer.write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"session/update\",\"params\":{\"sessionId\":\"s\",\"update\":{\"sessionUpdate\":\"agent_message_chunk\",\"content\":{\"type\":\"text\",\"text\":\"hello\"}}}}\n").await.unwrap();
            tokio::time::timeout(std::time::Duration::from_secs(1), entered.notified()).await.unwrap();
            drop(peer);
            let _ = tokio::time::timeout(std::time::Duration::from_secs(3), task).await.expect("EOF must terminate driver and callbacks").unwrap();
            assert!(dropped.load(Ordering::SeqCst));
        }).await;
    }
    struct BrokenReader;
    impl futures::AsyncRead for BrokenReader {
        fn poll_read(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
            _: &mut [u8],
        ) -> std::task::Poll<std::io::Result<usize>> {
            std::task::Poll::Ready(Err(std::io::Error::other("fixture")))
        }
    }
    #[tokio::test]
    async fn pump_read_failure_reaches_driver() {
        tokio::task::LocalSet::new()
            .run_until(async {
                let callbacks = HeldNotification {
                    entered: Arc::new(Notify::new()),
                    release: Arc::new(Notify::new()),
                    dropped: Arc::new(AtomicBool::new(false)),
                };
                let (_connection, driver) =
                    ClientConnection::new(callbacks, futures::io::sink(), BrokenReader);
                let error = driver.await.unwrap_err();
                assert!(error.to_string().contains("transport read failed"));
            })
            .await;
    }

    #[tokio::test]
    async fn blocked_owner_applies_transport_backpressure() {
        tokio::task::LocalSet::new().run_until(async {
            let (local, mut peer) = tokio::io::duplex(1024);
            let (read, write) = tokio::io::split(local);
            let entered = Arc::new(Notify::new());
            let callbacks = HeldNotification { entered: entered.clone(), release: Arc::new(Notify::new()), dropped: Arc::new(AtomicBool::new(false)) };
            let (connection, driver) = ClientConnection::new(callbacks, write.compat_write(), read.compat());
            let task = tokio::task::spawn_local(driver);
            let notification = serde_json::json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"hello"}}}}).to_string() + "\n";
            peer.write_all(notification.as_bytes()).await.unwrap();
            entered.notified().await;
            // A large unfinished frame cannot flow through while the owner is blocked.
            //
            // The write and `driver` (which reads it back via `read_frames`) must
            // race as peer tasks in the same `LocalSet`, not as a `spawn_local`
            // task racing a future driven directly inside `run_until`'s own
            // future: the latter starves the reader under tokio's cooperative
            // scheduler (the un-spawned future keeps repolling itself as the
            // duplex buffer drains and refills, without yielding a turn back to
            // `driver`), which made this assertion flaky — sometimes passing
            // in well under a second, sometimes not resolving in 1s at all.
            //
            // The write is not time-boxed: with a 100ms cap a loaded machine
            // wrote less than the line limit, so there was nothing to reject
            // and the reader waited forever. Wait for the rejection instead,
            // then check the frame was never accepted in full.
            let oversized = vec![b'x'; 4 * 1024 * 1024];
            let write_task = tokio::task::spawn_local(async move { peer.write_all(&oversized).await });
            let error = tokio::time::timeout(std::time::Duration::from_secs(10), task).await.unwrap().unwrap().unwrap_err();
            assert!(error.to_string().contains("frame exceeds"));
            if write_task.is_finished() {
                let write = write_task.await.unwrap();
                assert!(write.is_err(), "oversized frame must not be fully accepted");
            } else {
                write_task.abort();
            }
            connection.stop();
        }).await;
    }
    #[test]
    fn incoming_line_limit_counts_fragmented_and_multiple_frames() {
        let mut limit = LineLimit::default();
        limit.accept(&vec![b'x'; MAX_INCOMING_LINE - 1]).unwrap();
        limit.accept(b"x").unwrap();
        assert!(limit.accept(b"x").is_err());
        let mut limit = LineLimit::default();
        limit.accept(&vec![b'x'; MAX_INCOMING_LINE]).unwrap();
        limit.accept(b"\nfirst\nsecond\n").unwrap();
        limit.accept(&vec![b'x'; MAX_INCOMING_LINE]).unwrap();
        assert!(limit.accept(b"x\n").is_err());
    }
    struct BurstCallbacks {
        entered: Arc<Notify>,
        release: Arc<Notify>,
        count: Arc<std::sync::atomic::AtomicUsize>,
        hold_first: bool,
    }
    #[async_trait::async_trait(?Send)]
    impl ClientCallbacks for BurstCallbacks {
        async fn request(&self, _: AgentRequest) -> Result<Value> {
            Err(Error::method_not_found())
        }
        async fn notification(&self, notification: AgentNotification) -> Result<()> {
            if self.hold_first && self.count.load(Ordering::SeqCst) == 0 {
                self.entered.notify_one();
                self.release.notified().await;
            }
            if self.hold_first
                && let AgentNotification::SessionNotification(notification) = notification
                && let SessionUpdate::AgentMessageChunk(chunk) = notification.update
                && let ContentBlock::Text(text) = chunk.content
            {
                let expected = self.count.load(Ordering::SeqCst);
                if expected == 0 {
                    assert_eq!(text.text, "first");
                } else {
                    assert_eq!(
                        text.text
                            .split(':')
                            .next()
                            .unwrap()
                            .parse::<usize>()
                            .unwrap(),
                        expected
                    );
                }
            }
            self.count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }
    fn raw_notification(text: &str) -> String {
        serde_json::json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":text}}}}).to_string() + "\n"
    }

    #[tokio::test]
    async fn valid_frame_burst_is_backpressured_until_owner_processes() {
        tokio::task::LocalSet::new()
            .run_until(async {
                let (local, mut peer) = tokio::io::duplex(1024);
                let (read, write) = tokio::io::split(local);
                let entered = Arc::new(Notify::new());
                let release = Arc::new(Notify::new());
                let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
                let callbacks = BurstCallbacks {
                    entered: entered.clone(),
                    release: release.clone(),
                    count: count.clone(),
                    hold_first: true,
                };
                let (connection, driver) =
                    ClientConnection::new(callbacks, write.compat_write(), read.compat());
                let task = tokio::task::spawn_local(driver);
                peer.write_all(raw_notification("first").as_bytes())
                    .await
                    .unwrap();
                entered.notified().await;
                let burst: String = (1..=80)
                    .map(|index| raw_notification(&format!("{index}:{}", "x".repeat(4096))))
                    .collect();
                {
                    let send = peer.write_all(burst.as_bytes());
                    tokio::pin!(send);
                    assert!(
                        tokio::time::timeout(std::time::Duration::from_millis(100), &mut send)
                            .await
                            .is_err(),
                        "valid frames must not accumulate unboundedly in SDK actor"
                    );
                    release.notify_one();
                    tokio::time::timeout(std::time::Duration::from_secs(2), send)
                        .await
                        .unwrap()
                        .unwrap();
                }
                tokio::time::timeout(std::time::Duration::from_secs(2), async {
                    while count.load(Ordering::SeqCst) != 81 {
                        tokio::task::yield_now().await;
                    }
                })
                .await
                .unwrap();
                drop(peer);
                tokio::time::timeout(std::time::Duration::from_secs(3), task)
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap();
                drop(connection);
            })
            .await;
    }

    #[tokio::test]
    async fn completed_notification_and_prompt_response_survive_immediate_eof() {
        use tokio::io::{AsyncBufReadExt, BufReader};
        tokio::task::LocalSet::new().run_until(async {
            for _ in 0..64 {
                let (local, peer) = tokio::io::duplex(4096);
                let (read, write) = tokio::io::split(local);
                let (peer_read, mut peer_write) = tokio::io::split(peer);
                let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
                let callbacks = BurstCallbacks { entered: Arc::new(Notify::new()), release: Arc::new(Notify::new()), count: count.clone(), hold_first: false };
                let (connection, driver) = ClientConnection::new(callbacks, write.compat_write(), read.compat());
                let connection = Rc::new(connection);
                let task = tokio::task::spawn_local(driver);
                let prompt_connection = connection.clone();
                let prompt = tokio::task::spawn_local(async move { prompt_connection.prompt(PromptRequest::new("s", vec![])).await });
                let mut reader = BufReader::new(peer_read);
                let mut request = String::new();
                reader.read_line(&mut request).await.unwrap();
                let request: Value = serde_json::from_str(&request).unwrap();
                let response = serde_json::json!({"jsonrpc":"2.0","id":request["id"],"result":{"stopReason":"end_turn"}});
                peer_write.write_all(format!("{}{response}\n", raw_notification("last")).as_bytes()).await.unwrap();
                drop(peer_write);
                drop(reader);
                tokio::time::timeout(std::time::Duration::from_secs(1), prompt).await.unwrap().unwrap().expect("completed notification must not cause false EOF failure");
                assert_eq!(count.load(Ordering::SeqCst), 1);
                task.await.unwrap().unwrap();
            }
        }).await;
    }

    #[tokio::test]
    async fn stop_remains_responsive_when_valid_frame_queue_is_saturated() {
        tokio::task::LocalSet::new()
            .run_until(async {
                let (local, mut peer) = tokio::io::duplex(1024);
                let (read, write) = tokio::io::split(local);
                let entered = Arc::new(Notify::new());
                let dropped = Arc::new(AtomicBool::new(false));
                let callbacks = HeldNotification {
                    entered: entered.clone(),
                    release: Arc::new(Notify::new()),
                    dropped: dropped.clone(),
                };
                let (connection, driver) =
                    ClientConnection::new(callbacks, write.compat_write(), read.compat());
                let task = tokio::task::spawn_local(driver);
                peer.write_all(raw_notification("first").as_bytes())
                    .await
                    .unwrap();
                entered.notified().await;
                let burst = raw_notification(&"x".repeat(4096)).repeat(80);
                assert!(
                    tokio::time::timeout(
                        std::time::Duration::from_millis(100),
                        peer.write_all(burst.as_bytes())
                    )
                    .await
                    .is_err()
                );
                connection.stop();
                tokio::time::timeout(std::time::Duration::from_secs(1), task)
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap();
                assert!(dropped.load(Ordering::SeqCst));
            })
            .await;
    }
}
