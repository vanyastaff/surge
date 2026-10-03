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

pub(crate) struct ClientConnection {
    ready: watch::Receiver<Option<ConnectionTo<Agent>>>,
    stop: CancellationToken,
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
        (Self { ready, stop }, future)
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
    pub(crate) async fn initialize(
        &self,
        request: InitializeRequest,
    ) -> Result<InitializeResponse> {
        self.connection()
            .await?
            .send_request(request)
            .block_task()
            .await
    }
    pub(crate) async fn new_session(
        &self,
        request: NewSessionRequest,
    ) -> Result<NewSessionResponse> {
        self.connection()
            .await?
            .send_request(request)
            .block_task()
            .await
    }
    pub(crate) async fn load_session(
        &self,
        request: agent_client_protocol::schema::v1::LoadSessionRequest,
    ) -> Result<agent_client_protocol::schema::v1::LoadSessionResponse> {
        self.connection()
            .await?
            .send_request(request)
            .block_task()
            .await
    }
    pub(crate) async fn resume_session(
        &self,
        request: agent_client_protocol::schema::v1::ResumeSessionRequest,
    ) -> Result<agent_client_protocol::schema::v1::ResumeSessionResponse> {
        self.connection()
            .await?
            .send_request(request)
            .block_task()
            .await
    }
    pub(crate) async fn set_session_mode(
        &self,
        request: SetSessionModeRequest,
    ) -> Result<SetSessionModeResponse> {
        self.connection()
            .await?
            .send_request(request)
            .block_task()
            .await
    }
    pub(crate) async fn set_session_config_option(
        &self,
        request: SetSessionConfigOptionRequest,
    ) -> Result<SetSessionConfigOptionResponse> {
        self.connection()
            .await?
            .send_request(request)
            .block_task()
            .await
    }
    pub(crate) async fn prompt(&self, request: PromptRequest) -> Result<PromptResponse> {
        self.connection()
            .await?
            .send_request(request)
            .block_task()
            .await
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
