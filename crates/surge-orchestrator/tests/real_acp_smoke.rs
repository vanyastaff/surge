//! Opt-in real-agent smoke test for the ACP bridge + graph engine path.
//!
//! The deterministic CI path stays in `archetypes_mock_test.rs`. This test is
//! intentionally skipped unless a developer provides a real ACP-capable binary:
//!
//! ```text
//! SURGE_REAL_ACP_BIN=/path/to/claude \
//! SURGE_REAL_ACP_PROFILE=implementer@1.0 \
//!   cargo test -p surge-orchestrator --test real_acp_smoke -- --nocapture
//! ```
//!
//! Optional knobs:
//!
//! - `SURGE_REAL_ACP_KIND=claude-code|codex|gemini-cli|custom`
//! - `SURGE_REAL_ACP_ARGS="--flag value"` (split on whitespace)

use std::env;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use surge_acp::bridge::acp_bridge::AcpBridge;
use surge_acp::bridge::error::{
    BridgeError, CloseSessionError, OpenSessionError, ReplyToToolError, SendMessageError,
};
use surge_acp::bridge::event::{BridgeEvent, ToolResultPayload};
use surge_acp::bridge::facade::BridgeFacade;
use surge_acp::bridge::session::{AgentKind, MessageContent, SessionConfig, SessionState};
use surge_core::agent_config::PromptOverride;
use surge_core::graph::Graph;
use surge_core::id::{RunId, SessionId};
use surge_core::keys::{NodeKey, ProfileKey};
use surge_core::node::NodeConfig;
use surge_core::run_event::EventPayload;
use surge_orchestrator::engine::tools::ToolDispatcher;
use surge_orchestrator::engine::tools::worktree::WorktreeToolDispatcher;
use surge_orchestrator::engine::{Engine, EngineConfig, EngineRunConfig, RunOutcome};
use surge_persistence::runs::Storage;
use surge_persistence::runs::seq::EventSeq;
use tokio::sync::broadcast;

const ENV_BIN: &str = "SURGE_REAL_ACP_BIN";
const ENV_PROFILE: &str = "SURGE_REAL_ACP_PROFILE";
const ENV_KIND: &str = "SURGE_REAL_ACP_KIND";
const ENV_ARGS: &str = "SURGE_REAL_ACP_ARGS";
const RUN_TIMEOUT: Duration = Duration::from_secs(180);

#[derive(Clone, Copy, Debug)]
enum RealAcpKind {
    ClaudeCode,
    Codex,
    GeminiCli,
    Custom,
}

#[derive(Clone, Debug)]
struct RealAcpLaunch {
    binary: PathBuf,
    kind: RealAcpKind,
    extra_args: Vec<String>,
}

impl RealAcpLaunch {
    fn to_agent_kind(&self) -> AgentKind {
        match self.kind {
            RealAcpKind::ClaudeCode => AgentKind::ClaudeCode {
                binary: self.binary.clone(),
                extra_args: self.extra_args.clone(),
            },
            RealAcpKind::Codex => AgentKind::Codex {
                binary: self.binary.clone(),
                extra_args: self.extra_args.clone(),
            },
            RealAcpKind::GeminiCli => AgentKind::GeminiCli {
                binary: self.binary.clone(),
                extra_args: self.extra_args.clone(),
            },
            RealAcpKind::Custom => AgentKind::Custom {
                binary: self.binary.clone(),
                args: self.extra_args.clone(),
            },
        }
    }

    fn label(&self) -> &'static str {
        match self.kind {
            RealAcpKind::ClaudeCode => "claude-code",
            RealAcpKind::Codex => "codex",
            RealAcpKind::GeminiCli => "gemini-cli",
            RealAcpKind::Custom => "custom",
        }
    }
}

struct RealAcpBridge {
    inner: AcpBridge,
    launch: RealAcpLaunch,
}

impl RealAcpBridge {
    fn new(launch: RealAcpLaunch) -> Result<Self, BridgeError> {
        Ok(Self {
            inner: AcpBridge::with_defaults()?,
            launch,
        })
    }
}

#[async_trait]
impl BridgeFacade for RealAcpBridge {
    async fn open_session(&self, mut config: SessionConfig) -> Result<SessionId, OpenSessionError> {
        tracing::info!(
            target: "real_acp_smoke",
            binary = %self.launch.binary.display(),
            kind = self.launch.label(),
            "binding real ACP agent for smoke run"
        );
        config.agent_kind = self.launch.to_agent_kind();
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
        response: agent_client_protocol::RequestPermissionResponse,
    ) -> Result<(), surge_acp::bridge::ReplyToPermissionError> {
        self.inner
            .reply_to_permission(session, request_id, response)
            .await
    }

    fn subscribe(&self) -> broadcast::Receiver<BridgeEvent> {
        self.inner.subscribe()
    }
}

fn examples_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("examples")
}

fn load_minimal_agent_graph(profile: &str) -> Graph {
    let path = examples_dir().join("flow_minimal_agent.toml");
    let toml_s =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let mut graph: Graph =
        toml::from_str(&toml_s).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()));

    let node_key = NodeKey::try_from("impl_1").expect("'impl_1' is a valid NodeKey");
    let node = graph
        .nodes
        .get_mut(&node_key)
        .expect("flow_minimal_agent has impl_1 node");
    let NodeConfig::Agent(agent) = &mut node.config else {
        panic!("flow_minimal_agent impl_1 must be an agent node");
    };

    agent.profile = ProfileKey::try_from(profile)
        .unwrap_or_else(|e| panic!("{ENV_PROFILE}={profile:?} is not a valid profile key: {e}"));
    agent.prompt_overrides = Some(PromptOverride {
        system: Some(real_agent_prompt()),
        append_system: None,
    });
    agent.limits.timeout_seconds = RUN_TIMEOUT.as_secs() as u32;
    agent.limits.max_retries = 0;
    graph
}

fn real_agent_prompt() -> String {
    [
        "You are running Surge's real ACP smoke test.",
        "Do not inspect, edit, create, or delete files.",
        "Immediately call the injected report_stage_outcome tool with outcome \"done\",",
        "summary \"real ACP smoke completed\", and an empty artifacts_produced array.",
        "After the tool call, stop.",
    ]
    .join(" ")
}

fn skip_banner(reason: &str) {
    eprintln!(
        "[real_acp_smoke] SKIPPED: {reason}\n\
         Set {ENV_BIN} and {ENV_PROFILE} to run the real ACP smoke test."
    );
}

fn parse_env_args() -> Vec<String> {
    env::var(ENV_ARGS)
        .ok()
        .map(|raw| raw.split_whitespace().map(str::to_owned).collect())
        .unwrap_or_default()
}

fn infer_kind(binary: &Path) -> RealAcpKind {
    let name = binary
        .file_stem()
        .or_else(|| binary.file_name())
        .map(|s| s.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();

    if name.contains("codex") {
        RealAcpKind::Codex
    } else if name.contains("gemini") {
        RealAcpKind::GeminiCli
    } else if name.contains("claude") {
        RealAcpKind::ClaudeCode
    } else {
        RealAcpKind::Custom
    }
}

fn parse_kind(binary: &Path) -> RealAcpKind {
    let Ok(raw) = env::var(ENV_KIND) else {
        return infer_kind(binary);
    };
    match raw.trim().to_ascii_lowercase().as_str() {
        "" => infer_kind(binary),
        "claude" | "claude-code" => RealAcpKind::ClaudeCode,
        "codex" => RealAcpKind::Codex,
        "gemini" | "gemini-cli" => RealAcpKind::GeminiCli,
        "custom" => RealAcpKind::Custom,
        other => panic!(
            "{ENV_KIND}={other:?} is invalid; expected claude-code, codex, gemini-cli, or custom"
        ),
    }
}

fn real_acp_config() -> Option<(RealAcpLaunch, String)> {
    let Some(bin_os) = env::var_os(ENV_BIN) else {
        skip_banner(&format!("{ENV_BIN} is not set"));
        return None;
    };
    if bin_os.is_empty() {
        skip_banner(&format!("{ENV_BIN} is empty"));
        return None;
    }

    let profile = match env::var(ENV_PROFILE) {
        Ok(v) if !v.trim().is_empty() => v,
        _ => {
            skip_banner(&format!("{ENV_PROFILE} is not set"));
            return None;
        },
    };

    let binary = PathBuf::from(bin_os);
    assert!(
        binary.exists(),
        "{ENV_BIN}={} must point to an existing binary; saw missing path",
        binary.display()
    );
    assert!(
        binary.is_file(),
        "{ENV_BIN}={} must point to a file (not a directory)",
        binary.display()
    );

    let launch = RealAcpLaunch {
        kind: parse_kind(&binary),
        binary,
        extra_args: parse_env_args(),
    };
    Some((launch, profile))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn flow_minimal_agent_against_real_acp_agent() {
    let Some((launch, profile)) = real_acp_config() else {
        return;
    };

    let root = tempfile::tempdir().expect("tempdir");
    let storage_dir = root.path().join("storage");
    let worktree_dir = root.path().join("worktree");
    std::fs::create_dir_all(&storage_dir).expect("create storage dir");
    std::fs::create_dir_all(&worktree_dir).expect("create worktree dir");

    eprintln!(
        "[real_acp_smoke] RUNNING: bin={} kind={} profile={}",
        launch.binary.display(),
        launch.label(),
        profile
    );

    let storage = Storage::open(&storage_dir).await.expect("storage");
    let bridge = Arc::new(RealAcpBridge::new(launch).expect("real ACP bridge"));
    let dispatcher =
        Arc::new(WorktreeToolDispatcher::new(worktree_dir.clone())) as Arc<dyn ToolDispatcher>;
    let engine = Engine::new(bridge, storage.clone(), dispatcher, EngineConfig::default());

    let run_id = RunId::new();
    let handle = engine
        .start_run(
            run_id,
            load_minimal_agent_graph(&profile),
            worktree_dir,
            EngineRunConfig::default(),
        )
        .await
        .expect("start real ACP smoke run");

    let outcome = tokio::time::timeout(RUN_TIMEOUT, handle.await_completion())
        .await
        .unwrap_or_else(|_| panic!("real ACP smoke hung for more than {RUN_TIMEOUT:?}"))
        .expect("real ACP smoke completion");
    assert!(
        matches!(outcome, RunOutcome::Completed { .. }),
        "expected Completed, got {outcome:?}"
    );
    drop(engine);

    let reader = storage.open_run_reader(run_id).await.expect("open reader");
    let last = reader.current_seq().await.expect("current seq");
    let events = reader
        .read_events(EventSeq::ZERO..EventSeq(last.0 + 1))
        .await
        .expect("read events");

    assert!(
        events
            .iter()
            .any(|ev| matches!(ev.payload.payload, EventPayload::RunCompleted { .. })),
        "real ACP smoke completed without a persisted RunCompleted event: {events:?}"
    );
    assert!(
        events
            .iter()
            .any(|ev| matches!(ev.payload.payload, EventPayload::TokensConsumed { .. })),
        "real ACP smoke completed without a persisted TokensConsumed event: {events:?}"
    );
}
