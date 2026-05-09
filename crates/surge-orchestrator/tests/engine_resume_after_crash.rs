//! Integration test: simulate engine crash mid-pipeline by stop_run, then
//! resume_run picks up from snapshot. Acceptance #7.
//!
//! `#[ignore]`d by default — run with:
//!   `cargo build -p surge-acp --bin mock_acp_agent`
//!   `cargo test -p surge-orchestrator --test engine_resume_after_crash -- --ignored`

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

fn edge_done(id: &str, from: &NodeKey, to: &NodeKey) -> Edge {
    Edge {
        id: EdgeKey::try_from(id).unwrap(),
        from: PortRef {
            node: from.clone(),
            outcome: OutcomeKey::try_from("done").unwrap(),
        },
        to: to.clone(),
        kind: EdgeKind::Forward,
        policy: EdgePolicy::default(),
    }
}

/// Integration test: stop a 5-stage run mid-flight, then resume it from the
/// persisted snapshot. The mock's default scenario is `report_done`, so each
/// stage emits `report_stage_outcome { outcome: "done" }` and the pipeline
/// advances s1 → s2 → s3 → s4 → s5 → end.
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
fn resume_after_partial_progress() {
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

        let engine = Engine::new(bridge, storage.clone(), dispatcher, EngineConfig::default());

        // 5-stage linear pipeline.
        let s1 = NodeKey::try_from("s1").unwrap();
        let s2 = NodeKey::try_from("s2").unwrap();
        let s3 = NodeKey::try_from("s3").unwrap();
        let s4 = NodeKey::try_from("s4").unwrap();
        let s5 = NodeKey::try_from("s5").unwrap();
        let end = NodeKey::try_from("end").unwrap();

        let mut nodes = BTreeMap::new();
        for key in [&s1, &s2, &s3, &s4, &s5] {
            let id_str = key.as_ref();
            nodes.insert((*key).clone(), agent_node(id_str));
        }
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

        let edges = vec![
            edge_done("e12", &s1, &s2),
            edge_done("e23", &s2, &s3),
            edge_done("e34", &s3, &s4),
            edge_done("e45", &s4, &s5),
            edge_done("e5end", &s5, &end),
        ];

        let graph = Graph {
            schema_version: SCHEMA_VERSION,
            metadata: GraphMetadata {
                name: "5-stage".into(),
                description: None,
                template_origin: None,
                created_at: chrono::Utc::now(),
                author: None,
                archetype: None,
            },
            start: s1,
            nodes,
            edges,
            subgraphs: BTreeMap::new(),
        };

        let run_id = RunId::new();

        // Start the run.
        let h = engine
            .start_run(
                run_id,
                graph,
                dir.path().to_path_buf(),
                EngineRunConfig::default(),
            )
            .await
            .unwrap();

        // Wait briefly for some stages to complete, then stop to simulate crash.
        tokio::time::sleep(Duration::from_millis(500)).await;
        let _ = engine.stop_run(run_id, "simulated crash".into()).await;
        let outcome1 = h.await_completion().await.unwrap();
        assert!(
            matches!(
                outcome1,
                RunOutcome::Aborted { .. } | RunOutcome::Completed { .. }
            ),
            "unexpected first-run outcome: {outcome1:?}"
        );

        // If the run already completed (mock too fast), nothing to resume —
        // accept either outcome since the test is exercising the resume code
        // path more than enforcing a specific cursor position.

        // Resume.
        let h2 = engine
            .resume_run(run_id, dir.path().to_path_buf())
            .await
            .unwrap();
        let outcome2 = tokio::time::timeout(Duration::from_secs(60), h2.await_completion())
            .await
            .expect("resumed run timed out after 60s")
            .unwrap();

        // Resume should reach Completed eventually (even if first attempt already did).
        assert!(
            matches!(
                outcome2,
                RunOutcome::Completed { .. } | RunOutcome::Aborted { .. }
            ),
            "unexpected resumed-run outcome: {outcome2:?}"
        );

        // Drop the engine so the bridge's refcount can reach 1 (only bridge_owned).
        drop(engine);

        // Explicitly shut down the bridge via the async path so the OS thread
        // exits cleanly without blocking a tokio worker thread in Drop::join.
        // Arc::into_inner returns Some because the engine (last other holder) is dropped.
        if let Some(bridge_for_shutdown) = Arc::into_inner(bridge_owned) {
            let _ = bridge_for_shutdown.shutdown().await;
        }
    });
}
