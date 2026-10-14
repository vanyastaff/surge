//! Observe real endpoint revocation and close calls through the public invoke seam.

use super::*;
use surge_acp::bridge::error::{
    BridgeError, CloseSessionError, OpenSessionError, ReplyToPermissionError, ReplyToToolError,
    SendMessageError,
};
use surge_acp::bridge::event::ToolResultPayload;
use surge_acp::bridge::session::{MessageContent, SessionConfig, SessionState};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{Mutex, Semaphore};

struct CapturedEndpoint {
    address: String,
    auth: String,
}

impl CapturedEndpoint {
    async fn catalog(&self) -> std::io::Result<serde_json::Value> {
        #[cfg(unix)]
        let mut stream = tokio::net::UnixStream::connect(&self.address).await?;
        #[cfg(windows)]
        let mut stream =
            tokio::net::windows::named_pipe::ClientOptions::new().open(&self.address)?;
        let request = serde_json::to_vec(&serde_json::json!({
            "auth": self.auth,
            "operation": "Catalog",
        }))?;
        stream
            .write_u32(u32::try_from(request.len()).unwrap())
            .await?;
        stream.write_all(&request).await?;
        stream.flush().await?;
        let length = stream.read_u32().await? as usize;
        if length > 256 * 1024 {
            return Err(std::io::Error::other(
                "catalog frame exceeds transport limit",
            ));
        }
        let mut response = vec![0; length];
        stream.read_exact(&mut response).await?;
        Ok(serde_json::from_slice(&response)?)
    }
}

struct ObservedBridge {
    inner: Arc<MockBridge>,
    open_fails: bool,
    hold_close: bool,
    endpoint: Mutex<Option<CapturedEndpoint>>,
    close_entered: Semaphore,
    release_close: Semaphore,
}

impl ObservedBridge {
    fn new(inner: Arc<MockBridge>, open_fails: bool, hold_close: bool) -> Self {
        Self {
            inner,
            open_fails,
            hold_close,
            endpoint: Mutex::new(None),
            close_entered: Semaphore::new(0),
            release_close: Semaphore::new(0),
        }
    }

    async fn wait_for_close(&self) {
        tokio::time::timeout(Duration::from_secs(2), self.close_entered.acquire())
            .await
            .unwrap()
            .unwrap()
            .forget();
    }

    async fn assert_endpoint_revoked(&self) {
        let endpoint = self.endpoint.lock().await;
        // Windows IOCP may retain a cancelled listener handle after the owner
        // task has joined. A raw open can still succeed, but the authenticated
        // endpoint must fail the roundtrip rather than serve any authority.
        let result =
            tokio::time::timeout(Duration::from_secs(2), endpoint.as_ref().unwrap().catalog())
                .await
                .expect("revoked endpoint must terminate the protocol roundtrip");
        assert!(
            result.is_err(),
            "revoked endpoint still serves an authenticated catalog"
        );
    }
}

#[async_trait::async_trait]
impl BridgeFacade for ObservedBridge {
    fn legacy_stage_event_adapter(&self) -> bool {
        true
    }

    async fn open_session(
        &self,
        config: SessionConfig,
    ) -> Result<surge_core::execution_recovery::OpenedSession, OpenSessionError> {
        let env = &config.stage_mcp.as_ref().unwrap().server.env;
        let address = env
            .iter()
            .find(|entry| entry.name == surge_mcp::stage::ENDPOINT_ENV)
            .unwrap()
            .value
            .clone();
        let auth = env
            .iter()
            .find(|entry| entry.name == surge_mcp::stage::AUTH_ENV)
            .unwrap()
            .value
            .clone();
        let endpoint = CapturedEndpoint { address, auth };
        let catalog = tokio::time::timeout(Duration::from_secs(2), endpoint.catalog())
            .await
            .expect("live endpoint must answer the protocol roundtrip")
            .unwrap();
        assert!(catalog.as_array().is_some_and(|tools| !tools.is_empty()));
        *self.endpoint.lock().await = Some(endpoint);
        if self.open_fails {
            return Err(OpenSessionError::HandshakeTimedOut {
                phase: "fixture",
                timeout: Duration::from_millis(1),
            });
        }
        self.inner.open_session(config).await
    }

    async fn send_message(
        &self,
        session: SessionId,
        content: MessageContent,
    ) -> Result<(), SendMessageError> {
        self.inner.send_message(session, content).await
    }

    async fn session_state(&self, session: SessionId) -> Result<SessionState, BridgeError> {
        self.inner.session_state(session).await
    }

    async fn close_session(&self, session: SessionId) -> Result<(), CloseSessionError> {
        self.assert_endpoint_revoked().await;
        self.close_entered.add_permits(1);
        if self.hold_close {
            self.release_close.acquire().await.unwrap().forget();
        }
        self.inner.close_session(session).await
    }

    async fn reply_to_tool(
        &self,
        session: SessionId,
        call_id: String,
        payload: ToolResultPayload,
    ) -> Result<(), ReplyToToolError> {
        self.inner.reply_to_tool(session, call_id, payload).await
    }

    async fn reply_to_permission(
        &self,
        session: SessionId,
        request_id: String,
        response: agent_client_protocol::schema::v1::RequestPermissionResponse,
    ) -> Result<(), ReplyToPermissionError> {
        self.inner
            .reply_to_permission(session, request_id, response)
            .await
    }

    fn subscribe(&self) -> tokio::sync::broadcast::Receiver<BridgeEvent> {
        self.inner.subscribe()
    }
}

#[tokio::test]
async fn failed_open_revokes_the_actual_endpoint_and_preserves_the_open_error() {
    let fx = Fixture::new();
    let observed = Arc::new(ObservedBridge::new(fx.mock.clone(), true, false));
    let bridge: Arc<dyn BridgeFacade> = observed.clone();
    let result = invoke(&bridge, fx.request("invoker@1.0", None)).await;
    assert!(matches!(
        result,
        Err(InvocationFailure::HandshakeTimedOut(_))
    ));
    observed.assert_endpoint_revoked().await;
    assert_eq!(fx.closes().await, 0);
}

#[tokio::test]
async fn dropping_a_pending_invocation_closes_its_opened_session() {
    let fx = Fixture::new();
    let observed = Arc::new(ObservedBridge::new(fx.mock.clone(), false, false));
    let bridge: Arc<dyn BridgeFacade> = observed.clone();
    let mut running = Box::pin(invoke(&bridge, fx.request("invoker@1.0", None)));
    tokio::select! {
        result = &mut running => panic!("unexpected completion: {result:?}"),
        () = fx.wait_for_sends(1) => {},
    }
    assert_eq!(fx.sends().await, 1);
    drop(running);
    observed.wait_for_close().await;
    wait_for_one_close(&fx).await;
}

#[tokio::test]
async fn dropping_during_explicit_settlement_keeps_the_single_close_running() {
    let fx = Fixture::new();
    fx.mock.pin_next_session_id(fx.session).await;
    let observed = Arc::new(ObservedBridge::new(fx.mock.clone(), false, true));
    let bridge: Arc<dyn BridgeFacade> = observed.clone();
    let mut running = Box::pin(invoke(&bridge, fx.request("invoker@1.0", None)));
    let agent = async {
        fx.wait_for_sends(1).await;
        fx.emit(reported(fx.session, "skipped", &[])).await;
    };
    let driver = async {
        tokio::select! {
            result = &mut running => panic!("unexpected completion: {result:?}"),
            () = observed.wait_for_close() => {},
        }
        drop(running);
        observed.release_close.add_permits(1);
        wait_for_one_close(&fx).await;
        assert_eq!(observed.close_entered.available_permits(), 0);
    };
    tokio::join!(driver, agent);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn accepted_evidence_waits_for_the_pending_session_close() {
    use surge_core::{RunId, keys::NodeKey, run_event::EventPayload};
    use surge_orchestrator::agent_invocation::InvocationRecorder;
    use surge_persistence::runs::{EventSeq, Storage};

    let fx = Fixture::new();
    fx.write("description.md", DESCRIPTION);
    fx.mock.pin_next_session_id(fx.session).await;
    let observed = Arc::new(ObservedBridge::new(fx.mock.clone(), false, true));
    let bridge: Arc<dyn BridgeFacade> = observed.clone();
    let home = runtime_home_fixture::FixtureHome::new().unwrap();
    let storage = Storage::open(home.path()).await.unwrap();
    let run = RunId::new();
    let writer = storage.create_run(run, home.path(), None).await.unwrap();
    let reader = storage.open_run_reader(run).await.unwrap();
    let before = reader.current_seq().await.unwrap();
    let artifacts = surge_persistence::artifacts::ArtifactStore::new(home.path().join("runs"));
    let node = NodeKey::try_from("invocation").unwrap();
    let mut request = fx.request("invoker@1.0", None);
    request.recorder = Some(InvocationRecorder {
        writer: &writer,
        node: &node,
        artifact_store: &artifacts,
        run_id: run,
    });
    let mut running = Box::pin(invoke(&bridge, request));
    let agent = async {
        fx.wait_for_sends(1).await;
        fx.emit(reported(fx.session, "done", &["description.md"]))
            .await;
    };
    let driver = async {
        tokio::select! {
            result = &mut running => panic!("completed before cleanup: {result:?}"),
            () = observed.wait_for_close() => {},
        }
        assert_eq!(
            reader.current_seq().await.unwrap(),
            before,
            "OutcomeReported and ArtifactProduced must not be recorded while cleanup is pending"
        );
        observed.release_close.add_permits(1);
        running.await
    };
    let (outcome, ()) = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(driver, agent)
    })
    .await
    .unwrap();
    assert_eq!(outcome.unwrap().outcome.as_str(), "done");
    let after = reader.current_seq().await.unwrap();
    let events = reader
        .read_events(EventSeq(before.as_u64() + 1)..EventSeq(after.as_u64() + 1))
        .await
        .unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(
                event.payload.payload,
                EventPayload::ArtifactProduced { .. } | EventPayload::OutcomeReported { .. }
            ))
            .count(),
        2,
        "confirmed cleanup publishes both artifact and outcome evidence"
    );
    assert_eq!(fx.closes().await, 1);
    drop((reader, artifacts));
    writer.close().await.unwrap();
    drop(storage);
    home.close().unwrap();
}

async fn wait_for_one_close(fx: &Fixture) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while fx.closes().await == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(fx.closes().await, 1);
}
