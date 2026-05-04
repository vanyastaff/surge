//! Integration test: 3 concurrent multi-stage runs against one real engine
//! + AcpBridge. Acceptance #8.
//!
//! `#[ignore]`d by default — run with:
//!   `cargo build -p surge-acp --bin mock_acp_agent`
//!   `cargo test -p surge-orchestrator --test engine_concurrent_runs -- --ignored`

mod fixtures;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use surge_acp::bridge::acp_bridge::AcpBridge;
use surge_acp::bridge::facade::BridgeFacade;
use surge_core::agent_config::{AgentConfig, NodeLimits};
use surge_core::edge::{Edge, EdgeKind, EdgePolicy, PortRef};
use surge_core::graph::{Graph, GraphMetadata, SCHEMA_VERSION};
use surge_core::id::RunId;
use surge_core::keys::{EdgeKey, NodeKey, OutcomeKey, ProfileKey};
use surge_core::node::{Node, NodeConfig, OutcomeDecl, Position};
use surge_core::terminal_config::{TerminalConfig, TerminalKind};
use surge_orchestrator::engine::tools::ToolDispatcher;
use surge_orchestrator::engine::tools::worktree::WorktreeToolDispatcher;
use surge_orchestrator::engine::{Engine, EngineConfig, EngineRunConfig, RunOutcome};
use surge_persistence::runs::Storage;

/// Resolve the `mock_acp_agent` binary path.
///
/// During `cargo test` the `CARGO_BIN_EXE_mock_acp_agent` env var is set by
/// Cargo to the exact binary path. Outside of `cargo test` (e.g. manual `cargo
/// run --test`), fall back to `<CARGO_TARGET_DIR>/debug/mock_acp_agent[.exe]`.
fn mock_agent_path() -> PathBuf {
    // Cargo sets CARGO_BIN_EXE_<name> only for the binary's own crate's tests,
    // not for cross-crate consumers — so we always fall through to discovery.
    let bin = if cfg!(windows) {
        "mock_acp_agent.exe"
    } else {
        "mock_acp_agent"
    };
    if let Ok(target) = std::env::var("CARGO_TARGET_DIR") {
        return PathBuf::from(target).join("debug").join(bin);
    }
    // Walk up from this test crate's manifest dir to the workspace root
    // (where Cargo.lock lives), then target/debug/<bin>.
    let mut cur = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    loop {
        if cur.join("Cargo.lock").exists() {
            return cur.join("target").join("debug").join(bin);
        }
        if !cur.pop() {
            // Last resort: relative path (will fail-loud below).
            return PathBuf::from("target").join("debug").join(bin);
        }
    }
}

/// Build an agent `Node` with a single `"done"` outcome.
fn agent_node(id: &str) -> Node {
    Node {
        id: NodeKey::try_from(id).unwrap(),
        position: Position::default(),
        declared_outcomes: vec![OutcomeDecl {
            id: OutcomeKey::try_from("done").unwrap(),
            description: "stage completed".into(),
            edge_kind_hint: EdgeKind::Forward,
            is_terminal: false,
        }],
        config: NodeConfig::Agent(AgentConfig {
            profile: ProfileKey::try_from("implementer@1.0").unwrap(),
            prompt_overrides: None,
            tool_overrides: None,
            sandbox_override: None,
            approvals_override: None,
            bindings: vec![],
            rules_overrides: None,
            limits: NodeLimits::default(),
            hooks: vec![],
            custom_fields: Default::default(),
        }),
    }
}

/// Integration test: three concurrent runs share a single engine + bridge and
/// each completes independently. The mock's default scenario is `report_done`,
/// so each run advances `agent_<i>` → end and the spawned tasks join with
/// `RunOutcome::Completed`.
///
/// **Test layout note.** Written as a synchronous `#[test]` rather than
/// `#[tokio::test]` so that `set_var` for `CARGO_BIN_EXE_mock_acp_agent`
/// runs in single-threaded context **before** any tokio runtime starts.
/// Under `#[tokio::test(flavor = "multi_thread")]` the runtime worker
/// threads are already alive when the test body executes, so calling
/// `set_var` would race against any thread reading the environment —
/// `unsafe` would be unsound regardless of the safety comment.
#[test]
#[ignore = "requires mock_acp_agent binary built; enable with --ignored"]
fn three_concurrent_real_runs_complete_independently() {
    // Guard: fail fast with a clear message if the binary isn't built yet.
    let bin_raw = mock_agent_path();
    assert!(
        bin_raw.exists(),
        "mock_acp_agent binary missing at {} — run `cargo build -p surge-acp` first",
        bin_raw.display()
    );
    // Canonicalize so the env var is always absolute and CWD-independent;
    // `mock_agent_path` has a last-resort relative fallback that would
    // silently break the bridge worker's `Command::spawn` if it leaked.
    let bin = bin_raw.canonicalize().unwrap_or_else(|e| {
        panic!(
            "canonicalize {} failed: {e} (build the binary with `cargo build -p surge-acp` first)",
            bin_raw.display()
        )
    });

    // Inject the absolute binary path so the AcpBridge worker can find
    // mock_acp_agent regardless of the process CWD. Cargo only sets
    // CARGO_BIN_EXE_<name> for tests in the same crate as the binary;
    // surge-orchestrator is a different crate, so we backfill it here.
    if std::env::var("CARGO_BIN_EXE_mock_acp_agent").is_err() {
        // SAFETY: this runs before we construct any tokio runtime, so the
        // process is single-threaded at this point — no other thread can
        // observe a concurrent read/write of the environment. Rustc 2024
        // requires `unsafe` for `set_var` due to POSIX `setenv` not being
        // thread-safe; the single-threaded prelude makes that sound here.
        unsafe {
            std::env::set_var("CARGO_BIN_EXE_mock_acp_agent", &bin);
        }
    }

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .expect("build multi_thread tokio runtime");

    rt.block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();

        // Keep a typed Arc<AcpBridge> so we can call shutdown() after the engine
        // drops its clone. Dropping AcpBridge from within an async context blocks
        // the tokio thread (Drop::join on the bridge OS thread); calling shutdown()
        // explicitly avoids that.
        let bridge_owned = Arc::new(AcpBridge::with_defaults().unwrap());
        let bridge: Arc<dyn BridgeFacade> = bridge_owned.clone();
        let dispatcher = Arc::new(WorktreeToolDispatcher::new(dir.path().to_path_buf()))
            as Arc<dyn ToolDispatcher>;

        let engine = Arc::new(Engine::new(
            bridge,
            storage.clone(),
            dispatcher,
            EngineConfig::default(),
        ));

        let mut handles = vec![];
        for i in 0..3usize {
            let eng = engine.clone();
            let dir_path = dir.path().to_path_buf();
            handles.push(tokio::spawn(async move {
                // Single-stage agent + terminal — keeps the test fast.
                let agent_id = format!("agent_{i}");
                let agent_key = NodeKey::try_from(agent_id.as_str()).unwrap();
                let end = NodeKey::try_from("end").unwrap();

                let mut nodes = BTreeMap::new();
                nodes.insert(agent_key.clone(), agent_node(&agent_id));
                nodes.insert(
                    end.clone(),
                    Node {
                        id: end.clone(),
                        position: Position::default(),
                        declared_outcomes: vec![],
                        config: NodeConfig::Terminal(TerminalConfig {
                            kind: TerminalKind::Success,
                            message: None,
                        }),
                    },
                );

                let edges = vec![Edge {
                    id: EdgeKey::try_from(format!("e_{i}").as_str()).unwrap(),
                    from: PortRef {
                        node: agent_key.clone(),
                        outcome: OutcomeKey::try_from("done").unwrap(),
                    },
                    to: end.clone(),
                    kind: EdgeKind::Forward,
                    policy: EdgePolicy::default(),
                }];

                let graph = Graph {
                    schema_version: SCHEMA_VERSION,
                    metadata: GraphMetadata {
                        name: format!("run-{i}"),
                        description: None,
                        template_origin: None,
                        created_at: chrono::Utc::now(),
                        author: None,
                    },
                    start: agent_key,
                    nodes,
                    edges,
                    subgraphs: BTreeMap::new(),
                };

                let h = eng
                    .start_run(RunId::new(), graph, dir_path, EngineRunConfig::default())
                    .await
                    .unwrap();
                tokio::time::timeout(Duration::from_secs(60), h.await_completion())
                    .await
                    .expect("run timed out after 60s")
                    .unwrap()
            }));
        }

        for h in handles {
            let outcome = h.await.unwrap();
            assert!(
                matches!(outcome, RunOutcome::Completed { .. }),
                "expected Completed, got {outcome:?}"
            );
        }

        // Drop the engine Arc so the bridge's refcount can reach 1 (only bridge_owned).
        drop(engine);

        // Explicitly shut down the bridge via the async path so the OS thread
        // exits cleanly without blocking a tokio worker thread in Drop::join.
        // Arc::into_inner returns Some because the engine (last other holder) is dropped.
        if let Some(bridge_for_shutdown) = Arc::into_inner(bridge_owned) {
            let _ = bridge_for_shutdown.shutdown().await;
        }
    });
}
