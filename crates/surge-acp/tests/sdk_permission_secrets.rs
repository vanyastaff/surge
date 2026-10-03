//! Known literal protection for provider permission titles, including logs.
use std::{
    collections::BTreeMap,
    sync::{
        Arc, LazyLock, Mutex, Once,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use surge_acp::bridge::*;

#[derive(Clone)]
struct Capture(Arc<Mutex<String>>);
struct Fields<'a>(&'a mut String);
impl tracing::field::Visit for Fields<'_> {
    fn record_debug(&mut self, _: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        use std::fmt::Write;
        write!(self.0, "{value:?}").unwrap();
    }
}
impl tracing::Subscriber for Capture {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        let mut logs = self.0.lock().unwrap();
        logs.push_str(event.metadata().target());
        logs.push_str(": ");
        event.record(&mut Fields(&mut logs));
        logs.push('\n');
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}
#[derive(Clone)]
struct Policy {
    raw_seen: Arc<AtomicBool>,
    literal: String,
}
impl Sandbox for Policy {
    fn visibility(&self, _: &str, _: Option<&str>) -> SandboxDecision {
        SandboxDecision::Allow
    }
    fn allows_tool(&self, name: &str, _: Option<&str>) -> SandboxDecision {
        self.raw_seen.store(name == self.literal, Ordering::SeqCst);
        SandboxDecision::Elevate {
            capability: "fixture".into(),
        }
    }
    fn boxed_clone(&self) -> Box<dyn Sandbox> {
        Box::new(self.clone())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn permission_title_known_literal_is_redacted_without_changing_policy_input() {
    let logs = tokio::time::timeout(Duration::from_secs(10), exercise())
        .await
        .unwrap();
    let owned: String = logs
        .lines()
        .filter(|line| line.starts_with("surge_acp"))
        .collect();
    assert!(
        owned.contains("elevation requested"),
        "owned-log oracle must observe the actual callback"
    );
    assert!(
        !owned.contains("fixture-auth-literal-679103"),
        "Surge-owned permission logs leaked credential"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn explicitly_enabled_upstream_trace_is_raw_provider_diagnostics() {
    let logs = tokio::time::timeout(Duration::from_secs(10), exercise())
        .await
        .unwrap();
    assert!(
        logs.lines()
            .any(|line| line.starts_with("agent_client_protocol::")
                && line.contains("fixture-auth-literal-679103")),
        "fixture documents raw SDK TRACE; it must remain disabled by production defaults"
    );
}

async fn exercise() -> String {
    let literal = "fixture-auth-literal-679103";
    static LOGS: LazyLock<Arc<Mutex<String>>> =
        LazyLock::new(|| Arc::new(Mutex::new(String::new())));
    static INIT: Once = Once::new();
    INIT.call_once(|| tracing::subscriber::set_global_default(Capture(LOGS.clone())).unwrap());
    let logs = LOGS.clone();
    let raw_seen = Arc::new(AtomicBool::new(false));
    let root = tempfile::tempdir().unwrap();
    let bridge = AcpBridge::with_defaults().unwrap();
    let mut events = bridge.subscribe();
    let session = bridge
        .open_session(SessionConfig {
            writer_id: surge_core::id::ExecutionWriterId::new(),
            invocation: surge_core::id::StageInvocationId::new(),
            runtime: "fixture".into(),
            opening: Default::default(),
            config_selections: Vec::new(),
            stage_mcp: Some(Box::new(surge_acp::bridge::session::StageMcpConfig {
                session: surge_core::SessionId::new(),
                server: surge_acp::bridge::McpServerStdio::new("redaction-fixture-only", "/unused")
                    .env(vec![surge_acp::bridge::EnvVariable::new(
                        "SURGE_STAGE_MCP_AUTH",
                        literal,
                    )]),
            })),
            agent_kind: AgentKind::Custom {
                binary: env!("CARGO_BIN_EXE_mock_acp_agent").into(),
                args: vec!["--permission".into(), "--permission-title-secret".into()],
            },
            working_dir: root.path().into(),
            system_prompt: "fixture".into(),
            declared_outcomes: vec![surge_core::OutcomeKey::try_from("done").unwrap()],
            allows_escalation: true,
            tools: vec![],
            sandbox: Box::new(Policy {
                raw_seen: raw_seen.clone(),
                literal: literal.into(),
            }),
            permission_policy: surge_acp::client::PermissionPolicy::Interactive,
            bindings: BTreeMap::new(),
            env: BTreeMap::from([("SURGE_STAGE_MCP_AUTH".into(), literal.into())]),
        })
        .await
        .unwrap()
        .session;
    let observed;
    {
        let prompt = bridge.send_message(session, MessageContent::Text("work".into()));
        tokio::pin!(prompt);
        let (request_id, tool) = loop {
            tokio::select! {
                result = &mut prompt => panic!("prompt ended before permission: {result:?}"),
                event = events.recv() => {
                    if let BridgeEvent::PermissionRequested { request_id, tool, .. } = event.unwrap() { break (request_id, tool); }
                },
            }
        };
        observed = tool;
        bridge
            .reply_to_permission(
                session,
                request_id,
                RequestPermissionResponse::new(RequestPermissionOutcome::Selected(
                    SelectedPermissionOutcome::new(PermissionOptionId::new("allow")),
                )),
            )
            .await
            .unwrap();
        prompt.await.unwrap();
    }
    bridge.close_session(session).await.unwrap();
    bridge.shutdown().await.unwrap();
    assert!(
        raw_seen.load(Ordering::SeqCst),
        "policy must inspect original title"
    );
    assert!(
        !observed.contains(literal),
        "permission event leaked credential"
    );
    logs.lock().unwrap().clone()
}
