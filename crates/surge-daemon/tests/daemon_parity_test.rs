//! Task 3.3 — regression-guarding parity test.
//!
//! Runs `flow_terminal_only.toml` through both
//! [`LocalEngineFacade`] (direct in-process) and
//! [`DaemonEngineFacade`] (IPC over a local socket against an
//! inline-spawned `surge-daemon` server) and asserts the resulting
//! event sequence is identical modulo wall-clock fields.
//!
//! Why terminal-only: the engine reaches `RunCompleted` without
//! needing an ACP turn loop, so events are deterministic across
//! both paths. Agent-bearing archetypes require scenario-aware
//! mock-ACP scripting (covered by `archetypes_mock_test.rs` in
//! `surge-orchestrator/tests/`), and their event order against a
//! non-deterministic mock turn loop would not parity-compare
//! cleanly.
//!
//! Future extensions: once `mock_acp_agent.rs` exposes a
//! `report_sequence` scenario that produces a deterministic event
//! log against any of the new archetypes, this test should be
//! extended to include it.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use surge_acp::bridge::error::{
    BridgeError, CloseSessionError, OpenSessionError, ReplyToToolError, SendMessageError,
};
use surge_acp::bridge::event::{BridgeEvent, ToolResultPayload};
use surge_acp::bridge::facade::BridgeFacade;
use surge_acp::bridge::session::{MessageContent, SessionConfig, SessionState};
use surge_core::SessionId;
use surge_core::graph::Graph;
use surge_core::id::RunId;
use surge_daemon::{ServerConfig, run_server};
use surge_orchestrator::engine::daemon_facade::DaemonEngineFacade;
use surge_orchestrator::engine::facade::{EngineFacade, LocalEngineFacade};
use surge_orchestrator::engine::handle::EngineRunEvent;
use surge_orchestrator::engine::tools::ToolDispatcher;
use surge_orchestrator::engine::tools::worktree::WorktreeToolDispatcher;
use surge_orchestrator::engine::{Engine, EngineConfig, EngineRunConfig};
use surge_persistence::runs::Storage;
use tempfile::TempDir;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

fn examples_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("examples")
}

fn load_archetype(name: &str) -> Graph {
    let path = examples_dir().join(name);
    let toml_s =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    toml::from_str(&toml_s).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

fn unique_socket_path(temp: &TempDir, prefix: &str) -> PathBuf {
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    temp.path().join(format!("{prefix}_{pid}_{nanos}.sock"))
}

/// Minimal stub bridge for parity-test usage. The terminal-only
/// archetype never opens an ACP session, so every method is
/// allowed to return Err / a default. Mirrors the shape of
/// `daemon_e2e_smoke::StubFacade` but for [`BridgeFacade`].
struct StubBridge {
    tx: broadcast::Sender<BridgeEvent>,
}

impl StubBridge {
    fn new() -> Self {
        let (tx, _) = broadcast::channel(8);
        Self { tx }
    }
}

#[async_trait]
impl BridgeFacade for StubBridge {
    async fn open_session(&self, _config: SessionConfig) -> Result<SessionId, OpenSessionError> {
        // Terminal-only flow never opens an ACP session, so reaching
        // this in the parity test would be a real failure. Return a
        // clearly-labelled error rather than a default success.
        Err(OpenSessionError::Bridge(BridgeError::WorkerDead))
    }

    async fn send_message(
        &self,
        _session: SessionId,
        _content: MessageContent,
    ) -> Result<(), SendMessageError> {
        Err(SendMessageError::Bridge(BridgeError::WorkerDead))
    }

    async fn session_state(&self, _session: SessionId) -> Result<SessionState, BridgeError> {
        Err(BridgeError::WorkerDead)
    }

    async fn close_session(&self, _session: SessionId) -> Result<(), CloseSessionError> {
        Ok(())
    }

    async fn reply_to_tool(
        &self,
        _session: SessionId,
        _call_id: String,
        _payload: ToolResultPayload,
    ) -> Result<(), ReplyToToolError> {
        Ok(())
    }

    fn subscribe(&self) -> broadcast::Receiver<BridgeEvent> {
        self.tx.subscribe()
    }
}

async fn build_local_engine(dir: &Path) -> Arc<Engine> {
    let storage = Storage::open(dir).await.expect("storage");
    let bridge = Arc::new(StubBridge::new()) as Arc<dyn BridgeFacade>;
    let dispatcher =
        Arc::new(WorktreeToolDispatcher::new(dir.to_path_buf())) as Arc<dyn ToolDispatcher>;
    Arc::new(Engine::new(
        bridge,
        storage,
        dispatcher,
        EngineConfig::default(),
    ))
}

/// Strip wall-clock and non-deterministic fields from an event,
/// leaving only the structural shape callers compare for parity.
fn normalize_event(ev: &EngineRunEvent) -> String {
    match ev {
        EngineRunEvent::Persisted { payload, .. } => {
            // Discriminator only — `seq` is monotonic but identical
            // between facades for the same run; payload may include
            // wall-clock fields. The discriminant catches structural
            // regressions without flapping on timestamps.
            format!("persisted:{}", payload.discriminant_str())
        },
        EngineRunEvent::Terminal { outcome } => match outcome {
            surge_orchestrator::engine::handle::RunOutcome::Completed { terminal } => {
                format!("terminal:completed:{terminal}")
            },
            surge_orchestrator::engine::handle::RunOutcome::Failed { error } => {
                format!("terminal:failed:{error}")
            },
            surge_orchestrator::engine::handle::RunOutcome::Aborted { reason } => {
                format!("terminal:aborted:{reason}")
            },
            other => format!("terminal:other:{other:?}"),
        },
        other => format!("other:{other:?}"),
    }
}

async fn run_through_facade<F: EngineFacade>(
    facade: &F,
    archetype: &str,
    worktree: PathBuf,
) -> Vec<String> {
    let graph = load_archetype(archetype);
    let run_id = RunId::new();
    let handle = facade
        .start_run(run_id, graph, worktree, EngineRunConfig::default())
        .await
        .expect("start_run");
    let mut rx = handle.events;
    let mut collected = Vec::new();
    // Per-event timeout is generous (30s) because the daemon path on
    // Windows CI runs through named-pipe IPC + SQLite per-run DB
    // open + bridge thread spawn before the first event flows back —
    // 5s was tight enough that Windows runners (which complete the
    // whole suite in ~2x the macOS time) tripped it. Linux/macOS
    // resolve the first event in well under a second; the cap only
    // exists to surface a genuine hang.
    let per_event_timeout = Duration::from_secs(30);
    loop {
        match tokio::time::timeout(per_event_timeout, rx.recv()).await {
            Ok(Ok(ev)) => {
                let is_terminal = matches!(ev, EngineRunEvent::Terminal { .. });
                collected.push(normalize_event(&ev));
                if is_terminal {
                    break;
                }
            },
            Ok(Err(broadcast::error::RecvError::Closed)) => break,
            Ok(Err(broadcast::error::RecvError::Lagged(_))) => continue,
            Err(_) => panic!(
                "{archetype}: event stream stalled > {}s",
                per_event_timeout.as_secs()
            ),
        }
    }
    let _ = handle.completion.await;
    collected
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn parity_flow_terminal_only_local_vs_daemon() {
    // --- Local path ---
    let local_dir = TempDir::new().unwrap();
    let local_engine = build_local_engine(local_dir.path()).await;
    let local_facade = LocalEngineFacade::new(local_engine);
    let local_events = run_through_facade(
        &local_facade,
        "flow_terminal_only.toml",
        local_dir.path().to_path_buf(),
    )
    .await;

    // --- Daemon path ---
    let daemon_dir = TempDir::new().unwrap();
    let daemon_engine = build_local_engine(daemon_dir.path()).await;
    let daemon_facade_for_server: Arc<dyn EngineFacade> =
        Arc::new(LocalEngineFacade::new(daemon_engine));
    let socket = unique_socket_path(&daemon_dir, "parity");
    let cfg = ServerConfig {
        max_active: 4,
        max_queue: 16,
        socket_path: socket.clone(),
    };
    let shutdown = CancellationToken::new();
    let server_handle = tokio::spawn({
        let facade = daemon_facade_for_server.clone();
        let shutdown = shutdown.clone();
        async move { run_server(cfg, facade, shutdown).await }
    });
    tokio::time::sleep(Duration::from_millis(200)).await;

    let daemon_facade = DaemonEngineFacade::connect(socket).await.expect("connect");
    let daemon_events = run_through_facade(
        &daemon_facade,
        "flow_terminal_only.toml",
        daemon_dir.path().to_path_buf(),
    )
    .await;

    shutdown.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(2), server_handle).await;

    // --- Parity assertion ---
    assert_eq!(
        local_events, daemon_events,
        "local vs daemon event sequences must match modulo wall-clock fields\nLocal: {local_events:?}\nDaemon: {daemon_events:?}"
    );
    // Sanity: at least one Terminal event in both.
    assert!(
        local_events.iter().any(|e| e.starts_with("terminal:")),
        "local sequence must include a Terminal event"
    );
    assert!(
        daemon_events.iter().any(|e| e.starts_with("terminal:")),
        "daemon sequence must include a Terminal event"
    );
}
