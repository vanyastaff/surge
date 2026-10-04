//! Synthetic host-budget scheduling oracle. These charges are never measured ACP usage.
use agent_client_protocol::schema::v1::RequestPermissionResponse;
use futures::StreamExt;
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};
use surge_acp::bridge::error::{
    BridgeError, CloseSessionError, OpenSessionError, ReplyToPermissionError, ReplyToToolError,
    SendMessageError,
};
use surge_acp::bridge::facade::BridgeFacade;
use surge_acp::bridge::{
    BridgeEvent, MessageContent, SessionConfig, SessionEndReason, SessionState, ToolResultPayload,
};
use surge_core::{EventPayload, RunId, SessionId};
use surge_persistence::runs::Storage;
use tokio::sync::{Mutex, broadcast};
use tokio_util::sync::CancellationToken;

const ORACLE_MODEL: &str = "host-budget-oracle";
#[derive(Clone)]
struct Designation {
    run: RunId,
    tokens: u32,
}
#[derive(Clone)]
struct Charge {
    designation: Designation,
    cancel: CancellationToken,
    acknowledged: CancellationToken,
}

pub struct HostBudgetBridge {
    inner: Arc<dyn BridgeFacade>,
    storage: Arc<Storage>,
    designated: Mutex<HashMap<PathBuf, Designation>>,
    sessions: Arc<Mutex<HashMap<SessionId, Charge>>>,
    charged: Mutex<HashSet<SessionId>>,
    events: broadcast::Sender<BridgeEvent>,
    cancel: CancellationToken,
    relay: tokio::task::JoinHandle<()>,
}
impl HostBudgetBridge {
    pub fn new(
        inner: Arc<dyn BridgeFacade>,
        storage: Arc<Storage>,
        cancel: CancellationToken,
    ) -> Arc<Self> {
        let cancel = cancel.child_token();
        let mut source = inner.subscribe();
        let (events, _) = broadcast::channel(1024);
        let sessions = Arc::new(Mutex::new(HashMap::<SessionId, Charge>::new()));
        let relay_events = events.clone();
        let relay_cancel = cancel.clone();
        let relay_sessions = sessions.clone();
        let relay = tokio::spawn(async move {
            loop {
                tokio::select! {
                    biased;
                    () = relay_cancel.cancelled() => break,
                    event = source.recv() => match event {
                        Ok(event) => {
                            if let BridgeEvent::SessionEnded { session, .. } = &event
                                && let Some(charge) = relay_sessions.lock().await.get(session) {
                                charge.cancel.cancel();
                            }
                            let _ = relay_events.send(event);
                        }
                        Err(broadcast::error::RecvError::Closed) => { relay_cancel.cancel(); break; }
                        Err(broadcast::error::RecvError::Lagged(_)) => {
                            let _ = relay_events.send(BridgeEvent::Error { session: None, error: "host budget oracle relay lagged".into() });
                            relay_cancel.cancel();
                            break;
                        }
                    }
                }
            }
        });
        Arc::new(Self {
            inner,
            storage,
            designated: Mutex::new(HashMap::new()),
            sessions,
            charged: Mutex::new(HashSet::new()),
            events,
            cancel,
            relay,
        })
    }
    /// Arm only an explicit current task attempt and retained workspace before launch.
    pub async fn designate(
        &self,
        run: RunId,
        working_dir: PathBuf,
        tokens: u32,
    ) -> Result<(), String> {
        let storage = self.storage.clone();
        let expected = working_dir.clone();
        tokio::task::spawn_blocking(move || {
            let store = storage.work_items();
            let attempt = store
                .for_run(run)
                .map_err(|_| "host budget attempt query failed")?
                .ok_or("host budget attempt missing")?;
            let item = store
                .show(attempt.item)
                .map_err(|_| "host budget item query failed")?
                .item;
            if item.active_run != Some(run) || item.workspace.path != expected {
                return Err("host budget designation does not match active attempt workspace");
            }
            Ok(())
        })
        .await
        .map_err(|_| "host budget designation worker failed")?
        .map_err(str::to_owned)?;
        let mut designated = self.designated.lock().await;
        if let Some(previous) = designated.get(&working_dir)
            && (previous.run != run || previous.tokens != tokens)
        {
            return Err("conflicting host budget designation".into());
        }
        designated.insert(working_dir, Designation { run, tokens });
        Ok(())
    }
}
fn oracle_error(message: &str) -> SendMessageError {
    SendMessageError::Bridge(BridgeError::CommandSendFailed(message.to_owned()))
}
impl Drop for HostBudgetBridge {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.relay.abort();
    }
}
#[async_trait::async_trait]
impl BridgeFacade for HostBudgetBridge {
    fn legacy_stage_event_adapter(&self) -> bool {
        self.inner.legacy_stage_event_adapter()
    }
    async fn open_session(
        &self,
        config: SessionConfig,
    ) -> Result<surge_core::execution_recovery::OpenedSession, OpenSessionError> {
        let designation = self
            .designated
            .lock()
            .await
            .get(&config.working_dir)
            .cloned();
        let opened = self.inner.open_session(config).await?;
        if let Some(designation) = designation {
            self.sessions.lock().await.insert(
                opened.session,
                Charge {
                    designation,
                    cancel: self.cancel.child_token(),
                    acknowledged: CancellationToken::new(),
                },
            );
        }
        Ok(opened)
    }
    async fn send_message(
        &self,
        session: SessionId,
        content: MessageContent,
    ) -> Result<(), SendMessageError> {
        let charge = self.sessions.lock().await.get(&session).cloned();
        if let Some(charge) = charge {
            let first = self.charged.lock().await.insert(session);
            let wait = async {
                if !first {
                    charge.acknowledged.cancelled().await;
                    return Ok(());
                }
                // Register the durable stream before publishing the synthetic host charge.
                // Its historical replay also closes races with fast journal consumers.
                let reader = self
                    .storage
                    .open_run_reader(charge.designation.run)
                    .await
                    .map_err(|_| oracle_error("host budget journal unavailable"))?;
                let prefix = reader
                    .current_seq()
                    .await
                    .map_err(|_| oracle_error("host budget journal prefix unavailable"))?;
                let stream = reader.subscribe_events();
                futures::pin_mut!(stream);
                self.events
                    .send(BridgeEvent::TokenUsage {
                        session,
                        prompt_tokens: charge.designation.tokens,
                        output_tokens: 0,
                        cache_hits: 0,
                        model: ORACLE_MODEL.into(),
                    })
                    .map_err(|_| oracle_error("host budget oracle has no journal consumer"))?;
                while let Some(event) = stream.next().await {
                    let event =
                        event.map_err(|_| oracle_error("host budget journal read failed"))?;
                    if event.seq > prefix
                        && matches!(event.payload.payload(), EventPayload::TokensConsumed {
                        session: actual, prompt_tokens, output_tokens: 0, cache_hits: 0, model, cost_usd: None,
                    } if *actual == session && *prompt_tokens == charge.designation.tokens && model == ORACLE_MODEL)
                    {
                        charge.acknowledged.cancel();
                        return Ok(());
                    }
                }
                Err(oracle_error(
                    "host budget journal stream ended before acknowledgment",
                ))
            };
            let acknowledged = tokio::select! {
                biased;
                () = charge.cancel.cancelled() => Err(SendMessageError::SessionEnded { session, reason: SessionEndReason::ForcedClose }),
                result = tokio::time::timeout(Duration::from_secs(10), wait) => result.unwrap_or_else(|_| Err(oracle_error("host budget journal acknowledgment timed out"))),
            };
            if acknowledged.is_err() {
                charge.cancel.cancel();
            }
            acknowledged?;
        }
        self.inner.send_message(session, content).await
    }
    async fn session_state(&self, session: SessionId) -> Result<SessionState, BridgeError> {
        self.inner.session_state(session).await
    }
    async fn close_session(&self, session: SessionId) -> Result<(), CloseSessionError> {
        if let Some(charge) = self.sessions.lock().await.get(&session) {
            charge.cancel.cancel();
        }
        self.inner.close_session(session).await
    }
    async fn reply_to_tool(
        &self,
        session: SessionId,
        call: String,
        payload: ToolResultPayload,
    ) -> Result<(), ReplyToToolError> {
        self.inner.reply_to_tool(session, call, payload).await
    }
    async fn reply_to_permission(
        &self,
        session: SessionId,
        request: String,
        response: RequestPermissionResponse,
    ) -> Result<(), ReplyToPermissionError> {
        self.inner
            .reply_to_permission(session, request, response)
            .await
    }
    fn subscribe(&self) -> broadcast::Receiver<BridgeEvent> {
        self.events.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Peer {
        events: broadcast::Sender<BridgeEvent>,
        prompts: AtomicUsize,
    }
    #[async_trait::async_trait]
    impl BridgeFacade for Peer {
        async fn open_session(
            &self,
            _: SessionConfig,
        ) -> Result<surge_core::execution_recovery::OpenedSession, OpenSessionError> {
            Err(OpenSessionError::NoDeclaredOutcomes)
        }
        async fn send_message(
            &self,
            _: SessionId,
            _: MessageContent,
        ) -> Result<(), SendMessageError> {
            self.prompts.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        async fn session_state(&self, _: SessionId) -> Result<SessionState, BridgeError> {
            Err(BridgeError::WorkerDead)
        }
        async fn close_session(&self, session: SessionId) -> Result<(), CloseSessionError> {
            let _ = self.events.send(BridgeEvent::SessionEnded {
                session,
                reason: SessionEndReason::Normal,
            });
            Ok(())
        }
        async fn reply_to_tool(
            &self,
            _: SessionId,
            _: String,
            _: ToolResultPayload,
        ) -> Result<(), ReplyToToolError> {
            Ok(())
        }
        async fn reply_to_permission(
            &self,
            _: SessionId,
            _: String,
            _: RequestPermissionResponse,
        ) -> Result<(), ReplyToPermissionError> {
            Ok(())
        }
        fn subscribe(&self) -> broadcast::Receiver<BridgeEvent> {
            self.events.subscribe()
        }
    }
    async fn fixture() -> (
        tempfile::TempDir,
        Arc<Storage>,
        Arc<Peer>,
        Arc<HostBudgetBridge>,
    ) {
        let home = tempfile::tempdir().unwrap();
        let storage = Storage::open(home.path()).await.unwrap();
        let (events, _) = broadcast::channel(16);
        let peer = Arc::new(Peer {
            events,
            prompts: AtomicUsize::new(0),
        });
        let bridge = HostBudgetBridge::new(peer.clone(), storage.clone(), CancellationToken::new());
        (home, storage, peer, bridge)
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn charge_requires_same_session_durable_ack_before_actual_prompt_and_is_once() {
        let (home, storage, peer, bridge) = fixture().await;
        let run = RunId::new();
        let writer = storage.create_run(run, home.path(), None).await.unwrap();
        let session = SessionId::new();
        bridge.sessions.lock().await.insert(
            session,
            Charge {
                designation: Designation { run, tokens: 600 },
                cancel: bridge.cancel.child_token(),
                acknowledged: CancellationToken::new(),
            },
        );
        // A receipt from an earlier helper lifetime must not acknowledge this new charge.
        writer
            .append_event(surge_core::VersionedEventPayload::new(
                EventPayload::TokensConsumed {
                    session,
                    prompt_tokens: 600,
                    output_tokens: 0,
                    cache_hits: 0,
                    model: ORACLE_MODEL.into(),
                    cost_usd: None,
                },
            ))
            .await
            .unwrap();
        let mut events = bridge.subscribe();
        let sending = bridge.clone();
        let mut prompt = tokio::spawn(async move {
            sending
                .send_message(session, MessageContent::Text("actual prompt".into()))
                .await
        });
        let event = tokio::time::timeout(Duration::from_secs(1), events.recv())
            .await
            .expect("host charge must precede actual prompt")
            .unwrap();
        assert!(
            matches!(event, BridgeEvent::TokenUsage { session: charged_session, prompt_tokens: 600, output_tokens: 0, cache_hits: 0, ref model } if charged_session == session && model == ORACLE_MODEL)
        );
        assert_eq!(peer.prompts.load(Ordering::SeqCst), 0);
        // A different session's matching amount must not release the real prompt.
        writer
            .append_event(surge_core::VersionedEventPayload::new(
                EventPayload::TokensConsumed {
                    session: SessionId::new(),
                    prompt_tokens: 600,
                    output_tokens: 0,
                    cache_hits: 0,
                    model: ORACLE_MODEL.into(),
                    cost_usd: None,
                },
            ))
            .await
            .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(250), &mut prompt)
                .await
                .is_err(),
            "historical and wrong-session receipts must not acknowledge this new charge"
        );
        assert_eq!(peer.prompts.load(Ordering::SeqCst), 0);
        writer
            .append_event(surge_core::VersionedEventPayload::new(
                EventPayload::TokensConsumed {
                    session,
                    prompt_tokens: 600,
                    output_tokens: 0,
                    cache_hits: 0,
                    model: ORACLE_MODEL.into(),
                    cost_usd: None,
                },
            ))
            .await
            .unwrap();
        prompt.await.unwrap().unwrap();
        bridge
            .send_message(session, MessageContent::Text("second prompt".into()))
            .await
            .unwrap();
        assert_eq!(peer.prompts.load(Ordering::SeqCst), 2);
        assert!(matches!(
            events.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
        assert!(
            bridge
                .designate(run, home.path().to_path_buf(), 600)
                .await
                .is_err(),
            "unassociated run must not be designated"
        );
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn real_events_are_relayed_and_close_cancels_unacknowledged_charge() {
        let (home, storage, peer, bridge) = fixture().await;
        let run = RunId::new();
        let _writer = storage.create_run(run, home.path(), None).await.unwrap();
        let session = SessionId::new();
        bridge.sessions.lock().await.insert(
            session,
            Charge {
                designation: Designation { run, tokens: 600 },
                cancel: bridge.cancel.child_token(),
                acknowledged: CancellationToken::new(),
            },
        );
        let mut events = bridge.subscribe();
        peer.events
            .send(BridgeEvent::AgentMessage {
                session,
                chunk: "real bridge event".into(),
                meta: None,
            })
            .unwrap();
        assert!(
            matches!(events.recv().await.unwrap(), BridgeEvent::AgentMessage { chunk, .. } if chunk == "real bridge event")
        );
        let sending = bridge.clone();
        let prompt = tokio::spawn(async move {
            sending
                .send_message(session, MessageContent::Text("actual prompt".into()))
                .await
        });
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(1), events.recv())
                .await
                .unwrap()
                .unwrap(),
            BridgeEvent::TokenUsage {
                prompt_tokens: 600,
                ..
            }
        ));
        bridge.close_session(session).await.unwrap();
        assert!(prompt.await.unwrap().is_err());
        assert_eq!(peer.prompts.load(Ordering::SeqCst), 0);
        assert!(
            matches!(events.recv().await.unwrap(), BridgeEvent::SessionEnded { session: closed, .. } if closed == session)
        );
    }
}
