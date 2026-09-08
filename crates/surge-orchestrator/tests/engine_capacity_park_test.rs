//! Task 12 M3: integration tests for the capacity-aware dispatch gate at
//! the `Engine::start_run` boundary — the pre-dispatch check
//! (`CapacityPolicy::decide` consulted before every agent-node dispatch)
//! and the post-`StageError::RateLimited` park path, both exercised
//! through the real, production `RunTaskParams` wiring (`Engine`), not by
//! calling `engine::run_task`'s private functions directly.
//!
//! Every test here needs a real `ProfileRegistry` (not the legacy
//! `profile_registry: None` fast path): the capacity gate's pre-dispatch
//! check (`engine::stage::agent::resolve_profile_runtime_id`) returns
//! `None` — skipping the check entirely — on the legacy path by design
//! (Task 12 plan point 10), so a test that wants the gate to actually run
//! must resolve a profile.

#![allow(clippy::too_many_lines)]

mod fixtures;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use surge_acp::bridge::error::SendMessageError;
use surge_acp::bridge::event::BridgeEvent;
use surge_acp::bridge::facade::BridgeFacade;
use surge_core::agent_config::{AgentConfig, NodeLimits};
use surge_core::budget::{BudgetDimension, BudgetGuard, BudgetLimits, BudgetPolicy};
use surge_core::capacity::{CapacityPolicy, RotationPolicy};
use surge_core::capacity_config::CapacityConfig;
use surge_core::edge::{Edge, EdgeKind, EdgePolicy, PortRef};
use surge_core::graph::{Graph, GraphMetadata, SCHEMA_VERSION};
use surge_core::hooks::{Hook, HookFailureMode, HookInheritance, HookTrigger, MatcherSpec};
use surge_core::id::{RunId, SessionId};
use surge_core::keys::{EdgeKey, NodeKey, OutcomeKey, ProfileKey};
use surge_core::node::{Node, NodeConfig, OutcomeDecl, Position};
use surge_core::run_event::EventPayload;
use surge_core::run_status::RunStatus;
use surge_core::terminal_config::{TerminalConfig, TerminalKind};
use surge_orchestrator::engine::tools::ToolDispatcher;
use surge_orchestrator::engine::{Engine, EngineConfig, EngineRunConfig, RunOutcome};
use surge_orchestrator::profile_loader::{DiskProfileSet, ProfileRegistry};
use surge_persistence::memory::MemoryStore;
use surge_persistence::runs::Storage;
use surge_persistence::runs::seq::EventSeq;

use fixtures::mock_bridge::MockBridge;

struct UnusedDispatcher;

#[async_trait::async_trait]
impl ToolDispatcher for UnusedDispatcher {
    async fn dispatch(
        &self,
        _ctx: &surge_orchestrator::engine::tools::ToolDispatchContext<'_>,
        call: &surge_orchestrator::engine::tools::ToolCall,
    ) -> surge_orchestrator::engine::tools::ToolResultPayload {
        surge_orchestrator::engine::tools::ToolResultPayload::Unsupported {
            message: format!("unused: {}", call.tool),
        }
    }
}

/// Write a disk profile whose `[runtime] agent_id` is exactly what the
/// caller supplies (an alias or the canonical id) and whose declared
/// outcomes are exactly what the caller supplies.
fn drop_profile(profiles_dir: &std::path::Path, role_id: &str, agent_id: &str, outcomes: &[&str]) {
    std::fs::create_dir_all(profiles_dir).unwrap();
    let outcomes_toml: String = outcomes
        .iter()
        .map(|id| {
            format!(
                "\n[[outcomes]]\nid = \"{id}\"\ndescription = \"{id} outcome\"\nedge_kind_hint = \"forward\"\n"
            )
        })
        .collect();
    std::fs::write(
        profiles_dir.join(format!("{role_id}-1.0.toml")),
        format!(
            r#"
schema_version = 1

[role]
id = "{role_id}"
version = "1.0.0"
display_name = "{role_id}"
category = "agents"
description = "Task 12 M3 capacity-park test fixture"
when_to_use = "Tests"

[runtime]
recommended_model = "test-model"
agent_id = "{agent_id}"
{outcomes_toml}
[prompt]
system = "test"
"#
        ),
    )
    .unwrap();
}

fn outcome_decl(id: &str) -> OutcomeDecl {
    OutcomeDecl {
        id: OutcomeKey::try_from(id).unwrap(),
        description: format!("{id} outcome"),
        edge_kind_hint: EdgeKind::Forward,
        is_terminal: false,
        ledger_effect: surge_core::node::LedgerEffect::default(),
    }
}

fn agent_node(id: &str, profile: &str, hooks: Vec<Hook>, declared_outcomes: &[&str]) -> Node {
    Node {
        id: NodeKey::try_from(id).unwrap(),
        position: Position::default(),
        declared_outcomes: declared_outcomes
            .iter()
            .copied()
            .map(outcome_decl)
            .collect(),
        config: NodeConfig::Agent(AgentConfig {
            profile: ProfileKey::try_from(profile).unwrap(),
            prompt_overrides: None,
            tool_overrides: None,
            sandbox_override: None,
            approvals_override: None,
            bindings: vec![],
            rules_overrides: None,
            limits: NodeLimits::default(),
            hooks,
            custom_fields: BTreeMap::new(),
        }),
    }
}

fn terminal_node(id: &str) -> Node {
    Node {
        id: NodeKey::try_from(id).unwrap(),
        position: Position::default(),
        declared_outcomes: vec![],
        config: NodeConfig::Terminal(TerminalConfig {
            kind: TerminalKind::Success,
            message: None,
        }),
    }
}

fn edge(id: &str, from: &str, outcome: &str, to: &str) -> Edge {
    Edge {
        id: EdgeKey::try_from(id).unwrap(),
        from: PortRef {
            node: NodeKey::try_from(from).unwrap(),
            outcome: OutcomeKey::try_from(outcome).unwrap(),
        },
        to: NodeKey::try_from(to).unwrap(),
        kind: EdgeKind::Forward,
        policy: EdgePolicy::default(),
    }
}

fn graph(name: &str, start: &str, nodes: Vec<Node>, edges: Vec<Edge>) -> Graph {
    let mut node_map = BTreeMap::new();
    for node in nodes {
        node_map.insert(node.id.clone(), node);
    }
    Graph {
        schema_version: SCHEMA_VERSION,
        metadata: GraphMetadata {
            name: name.into(),
            description: None,
            template_origin: None,
            created_at: chrono::Utc::now(),
            author: None,
            archetype: None,
        },
        start: NodeKey::try_from(start).unwrap(),
        nodes: node_map,
        edges,
        subgraphs: BTreeMap::new(),
    }
}

fn suppress_to_hook(id: &str, dir: &std::path::Path, outcome: &str) -> Hook {
    let json = format!(r#"{{"action":"suppress","outcome":"{outcome}"}}"#);
    let path = dir.join(format!("suppress-{outcome}.json"));
    std::fs::write(&path, json).unwrap();
    let command = if cfg!(target_os = "windows") {
        let literal_path = path.display().to_string().replace('\'', "''");
        format!("powershell -NoProfile -Command Get-Content -Raw -LiteralPath '{literal_path}'")
    } else {
        format!(r#"cat "{}""#, path.display())
    };
    Hook {
        id: id.into(),
        trigger: HookTrigger::OnError,
        matcher: MatcherSpec::default(),
        command,
        on_failure: HookFailureMode::Warn,
        timeout_seconds: Some(30),
        inherit: HookInheritance::Extend,
    }
}

/// Acceptance criterion D: `estimate.is_none()` (a fresh node, never
/// dispatched before in this run) on a `NeverObserved` runtime must not
/// itself cause a refusal — proved here at the one place this claim was
/// previously vacuous (`decide`/`Park` had no real caller before Task 12
/// M3): a real `Engine::start_run` with a resolved profile.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn estimate_none_and_never_observed_does_not_block_dispatch() {
    let profiles_dir = tempfile::tempdir().unwrap();
    drop_profile(profiles_dir.path(), "fresh-role", "claude-code", &["done"]);
    let disk = DiskProfileSet::scan(profiles_dir.path()).unwrap();
    let registry = Arc::new(ProfileRegistry::new(disk));

    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::open(dir.path()).await.unwrap();
    let mock = Arc::new(MockBridge::new());
    let bridge: Arc<dyn BridgeFacade> = mock.clone();
    let dispatcher: Arc<dyn ToolDispatcher> = Arc::new(UnusedDispatcher);

    let session_id = SessionId::new();
    mock.pin_next_session_id(session_id).await;
    mock.enqueue_event(BridgeEvent::OutcomeReported {
        session: session_id,
        outcome: OutcomeKey::try_from("done").unwrap(),
        summary: "ok".into(),
        artifacts_produced: vec![],
    })
    .await;

    let engine = Engine::new_full(
        bridge,
        storage.clone(),
        dispatcher,
        Arc::new(surge_notify::MultiplexingNotifier::new()),
        None,
        Some(registry),
        EngineConfig::default(),
    );

    let run_id = RunId::new();
    let g = graph(
        "capacity-dispatch-unblocked",
        "agent_1",
        vec![
            agent_node("agent_1", "fresh-role@1.0", vec![], &["done"]),
            terminal_node("end"),
        ],
        vec![edge("agent_to_end", "agent_1", "done", "end")],
    );
    let handle = engine
        .start_run(
            run_id,
            g,
            dir.path().to_path_buf(),
            EngineRunConfig::default(),
        )
        .await
        .expect("start_run");

    let mock_for_pump = mock.clone();
    let pump = tokio::spawn(async move {
        mock_for_pump.pump_after_subscribe(1).await;
    });

    let outcome = tokio::time::timeout(Duration::from_secs(10), handle.await_completion())
        .await
        .expect("run timed out")
        .expect("run handle join");
    pump.await.unwrap();

    match outcome {
        RunOutcome::Completed { terminal } => assert_eq!(terminal.as_ref(), "end"),
        other => {
            panic!("expected Completed (no estimate must never block dispatch), got {other:?}")
        },
    }
}

/// Acceptance criterion B, on the production path: `EngineConfig::capacity`
/// built via `CapacityPolicy::from(&CapacityConfig{..})` — the exact
/// conversion `surge-cli`/`surge-daemon` call on `SurgeConfig.capacity` at
/// startup — must be what `decide` actually uses, not the module's own
/// hardcoded 5-minute default. Proved by configuring an
/// unmistakably-different backoff (42s) and asserting the resulting
/// `wake_at` reflects it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn configured_blind_backoff_reaches_the_park_decision_not_the_hardcoded_default() {
    let profiles_dir = tempfile::tempdir().unwrap();
    drop_profile(profiles_dir.path(), "b-role", "claude-code", &["done"]);
    let disk = DiskProfileSet::scan(profiles_dir.path()).unwrap();
    let registry = Arc::new(ProfileRegistry::new(disk));

    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::open(dir.path()).await.unwrap();
    let mock = Arc::new(MockBridge::new());
    let bridge: Arc<dyn BridgeFacade> = mock.clone();
    let dispatcher: Arc<dyn ToolDispatcher> = Arc::new(UnusedDispatcher);

    mock.fail_next_send_message(SendMessageError::RateLimited {
        retry_after: None,
        details: "usage limit reached, no reset time given".into(),
    })
    .await;

    // Exactly the shape `surge-cli`/`surge-daemon` build from
    // `SurgeConfig.capacity` — see `capacity_config.rs`'s `From` impl.
    let configured_backoff = Duration::from_secs(42);
    let capacity_config = CapacityConfig {
        blind_backoff: Some(configured_backoff),
        blind_park_limit: 5,
        // Task 12 M4: this test's assertion window below is exact-ish
        // (30..=55s) and is about `blind_backoff` plumbing specifically —
        // holding jitter at zero keeps that assertion decoupled from the
        // unrelated jitter default (see the M4 jitter tests in
        // `capacity.rs`/`capacity_config.rs` for jitter's own coverage).
        jitter_max: Duration::ZERO,
    };
    let capacity_policy: CapacityPolicy = (&capacity_config).into();
    assert_eq!(capacity_policy.rotation, RotationPolicy::Disabled);

    let engine = Engine::new_full(
        bridge,
        storage.clone(),
        dispatcher,
        Arc::new(surge_notify::MultiplexingNotifier::new()),
        None,
        Some(registry),
        EngineConfig {
            capacity: capacity_policy,
            ..EngineConfig::default()
        },
    );

    let run_id = RunId::new();
    let g = graph(
        "capacity-configured-backoff",
        "agent_1",
        vec![
            agent_node("agent_1", "b-role@1.0", vec![], &["done"]),
            terminal_node("end"),
        ],
        vec![edge("agent_to_end", "agent_1", "done", "end")],
    );
    let before = chrono::Utc::now();
    let handle = engine
        .start_run(
            run_id,
            g,
            dir.path().to_path_buf(),
            EngineRunConfig::default(),
        )
        .await
        .expect("start_run");

    let outcome = tokio::time::timeout(Duration::from_secs(10), handle.await_completion())
        .await
        .expect("run timed out")
        .expect("run handle join");

    let RunOutcome::Parked { wake_at } = outcome else {
        panic!("expected Parked, got {outcome:?}");
    };
    let elapsed = (wake_at - before).num_seconds();
    assert!(
        (30..=55).contains(&elapsed),
        "wake_at must reflect the configured 42s backoff (got {elapsed}s until wake) — the \
         module's own hardcoded default is 5 minutes (300s), so a value near that would mean \
         SurgeConfig.capacity never reached the decision"
    );
}

/// The central park-mechanics test: a rate-limited dispatch parks instead
/// of dispatching a second time, even though this pipeline's `on_error`
/// hook suppresses the failure into an outcome that routes to a *second*
/// agent node on the same runtime — the realistic "retry on failure"
/// pipeline shape the plan calls out as the exact case where an unguarded
/// engine would burn an attempt against a wall that will not move.
///
/// Also proves acceptance criterion A end-to-end, not just at the unit
/// level: `agent_1`'s profile targets `agent_id = "claude"` (an alias);
/// `agent_2`'s targets `agent_id = "claude-code"` (a different alias of
/// the same canonical entry). If the engine's capacity key ever fragmented
/// these two spellings into different rows, `agent_2` would read
/// `NeverObserved` and dispatch — reaching the mock bridge a second time
/// and hanging on an event that was deliberately never scripted (an
/// unmistakable failure, not a silent false pass).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rate_limited_agent_parks_instead_of_dispatching_a_second_node_on_the_same_runtime() {
    let profiles_dir = tempfile::tempdir().unwrap();
    drop_profile(profiles_dir.path(), "alias-claude", "claude", &["continue"]);
    drop_profile(
        profiles_dir.path(),
        "alias-claude-code",
        "claude-code",
        &["done"],
    );
    let disk = DiskProfileSet::scan(profiles_dir.path()).unwrap();
    let registry = Arc::new(ProfileRegistry::new(disk));

    let dir = tempfile::tempdir().unwrap();
    // `agent_2`'s scripted `SessionNotFound` is only reachable if the
    // capacity gate this test guards is broken — but if it ever is, that
    // dispatch is a genuine `StageError` reaching a terminal
    // `RunOutcome::Failed`, which trips
    // `engine::hooks::memory_writeback::record_node_failure`. Route it at a
    // throwaway store instead of the developer's real `~/.surge/memory.db`.
    let memory_dir = tempfile::tempdir().unwrap();
    let store_path = memory_dir.path().join("memory.db");
    let storage = Storage::open(dir.path()).await.unwrap();
    let mock = Arc::new(MockBridge::new());
    let bridge: Arc<dyn BridgeFacade> = mock.clone();
    let dispatcher: Arc<dyn ToolDispatcher> = Arc::new(UnusedDispatcher);

    // First call (agent_1): the rate limit this test is actually about.
    mock.fail_next_send_message(SendMessageError::RateLimited {
        retry_after: None,
        details: "usage limit reached".into(),
    })
    .await;
    // Second call (agent_2, reachable only if the gate is broken): scripted
    // to fail fast and distinctly (a plain `SessionNotFound`, nothing to do
    // with rate limits) rather than left unscripted-but-successful. An
    // unscripted success would hang the run waiting forever for an
    // `OutcomeReported`/`SessionEnded` event nobody enqueued — turning a
    // real bug into a 10-second timeout instead of an assertion that fires
    // in milliseconds naming exactly what happened (Task 12 M3 review,
    // "misc": the zero-retries test must fail fast, not by hanging).
    mock.fail_next_send_message(SendMessageError::SessionNotFound {
        session: SessionId::new(),
    })
    .await;

    let engine = Engine::new_full(
        bridge,
        storage.clone(),
        dispatcher,
        Arc::new(surge_notify::MultiplexingNotifier::new()),
        None,
        Some(registry),
        EngineConfig::default(),
    );

    let run_id = RunId::new();
    let g = graph(
        "capacity-zero-retries",
        "agent_1",
        vec![
            agent_node(
                "agent_1",
                "alias-claude@1.0",
                vec![suppress_to_hook("recover", dir.path(), "continue")],
                &["continue"],
            ),
            agent_node("agent_2", "alias-claude-code@1.0", vec![], &["done"]),
            terminal_node("end"),
        ],
        vec![
            edge("agent1_to_agent2", "agent_1", "continue", "agent_2"),
            edge("agent2_to_end", "agent_2", "done", "end"),
        ],
    );
    let handle = engine
        .start_run(
            run_id,
            g,
            dir.path().to_path_buf(),
            EngineRunConfig {
                memory_store_path: Some(store_path),
                ..EngineRunConfig::default()
            },
        )
        .await
        .expect("start_run");

    // A generous margin over the fixed build's near-instant park, but far
    // short of the old 10s: a regression now fails on the `SessionNotFound`
    // assertion below in milliseconds, not on a timeout in seconds.
    let outcome = tokio::time::timeout(Duration::from_secs(2), handle.await_completion())
        .await
        .expect(
            "run must not hang: agent_2's scripted SessionNotFound (reachable only if the \
             capacity gate is broken) fails fast, so a hang here means something else is wrong",
        )
        .expect("run handle join");

    let RunOutcome::Parked { wake_at } = outcome else {
        panic!(
            "expected Parked after the first rate limit, got {outcome:?} — a retry via the \
             on_error-suppressed route must not have been allowed to dispatch agent_2"
        );
    };

    let send_message_count = mock
        .recorded_calls
        .lock()
        .await
        .iter()
        .filter(|c| matches!(c, fixtures::mock_bridge::RecordedCall::SendMessage { .. }))
        .count();
    assert_eq!(
        send_message_count, 1,
        "zero retries after a 429: exactly one SendMessage (agent_1's) may have reached the \
         bridge, never a second (agent_2's)"
    );

    // Registry-level effect: `Storage::set_run_parked` must have run.
    let summary = storage.get_run(&run_id).await.unwrap().expect("run row");
    assert_eq!(summary.status, RunStatus::Parked);
    assert_eq!(summary.wake_at_ms, Some(wake_at.timestamp_millis()));

    // Per-run event log: `RunParked` with the fields the fold/inbox depend on.
    let reader = storage.open_run_reader(run_id).await.unwrap();
    let last = reader.current_seq().await.unwrap();
    let events = reader
        .read_events(EventSeq(0)..EventSeq(last.0 + 1))
        .await
        .unwrap();
    let mut saw_run_parked = false;
    for ev in &events {
        if let EventPayload::RunParked {
            wake_at: event_wake_at,
            runtime,
            basis,
            worktree,
            reason,
        } = &ev.payload.payload
        {
            saw_run_parked = true;
            assert_eq!(*event_wake_at, wake_at);
            assert_eq!(
                runtime.as_deref(),
                Some("claude-acp"),
                "runtime must be the canonical registry id, not the raw alias \"claude\""
            );
            assert_eq!(*basis, surge_core::capacity::WakeBasis::PolicyBackoff);
            // Real check, not `dir.path().exists()` (that TempDir is
            // guaranteed to exist regardless of anything the code under
            // test does — asserting it proves nothing). This proves the
            // plumbing threaded the *actual* worktree path through, not a
            // placeholder or a reconstructed guess.
            assert_eq!(
                worktree,
                dir.path(),
                "RunParked must carry the run's real worktree path"
            );
            assert!(
                reason.contains("usage limit reached"),
                "the raw provider error text must survive into RunParked.reason for an \
                 operator reading `surge inbox`, got: {reason}"
            );
        }
        assert!(
            !matches!(ev.payload.payload, EventPayload::StageFailed { .. }),
            "a parked rate limit must never also persist StageFailed: {ev:?}"
        );
    }
    assert!(saw_run_parked, "expected a RunParked event in the log");
}

/// Task 12 M3 review, BLOCKING #4 (honesty gap #1): the literal R37
/// requirement — "refuse to start work that cannot finish inside the
/// remaining window", i.e. a **pre-dispatch** refusal — had no test that
/// seeds the ledger before `start_run` and checks the bridge was never
/// touched at all. Every existing park test up to this one only proved the
/// *reactive* (post-429) path, which the plan calls out as exactly the
/// mechanism R37 exists to replace as the *only* one. This test seeds
/// `runtime_capacity` directly (as if a previous run or process already
/// observed exhaustion), then starts a run whose first node targets that
/// same runtime, and asserts it parks having never called `send_message`
/// at all.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_runtime_exhausted_before_start_run_parks_with_zero_dispatches() {
    let profiles_dir = tempfile::tempdir().unwrap();
    drop_profile(
        profiles_dir.path(),
        "pre-exhausted-role",
        "claude-code",
        &["done"],
    );
    let disk = DiskProfileSet::scan(profiles_dir.path()).unwrap();
    let registry = Arc::new(ProfileRegistry::new(disk));

    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::open(dir.path()).await.unwrap();

    // Seeded *before* `start_run` — this run never gets a chance to
    // observe anything itself. `resets_at: None` (never learned) so the
    // default `blind_backoff` applies, matching the review's own probe
    // shape (a stale-but-exhausted row with no usable reset time).
    storage
        .observe_capacity(&surge_core::capacity::CapacityWindow::observed_429(
            "claude-acp",
            None,
            chrono::Utc::now() - chrono::Duration::days(365),
        ))
        .await
        .unwrap();

    let mock = Arc::new(MockBridge::new());
    let bridge: Arc<dyn BridgeFacade> = mock.clone();
    let dispatcher: Arc<dyn ToolDispatcher> = Arc::new(UnusedDispatcher);

    let engine = Engine::new_full(
        bridge,
        storage.clone(),
        dispatcher,
        Arc::new(surge_notify::MultiplexingNotifier::new()),
        None,
        Some(registry),
        EngineConfig::default(),
    );

    let run_id = RunId::new();
    let g = graph(
        "capacity-pre-dispatch-refusal",
        "agent_1",
        vec![
            agent_node("agent_1", "pre-exhausted-role@1.0", vec![], &["done"]),
            terminal_node("end"),
        ],
        vec![edge("agent_to_end", "agent_1", "done", "end")],
    );
    let handle = engine
        .start_run(
            run_id,
            g,
            dir.path().to_path_buf(),
            EngineRunConfig::default(),
        )
        .await
        .expect("start_run");

    let outcome = tokio::time::timeout(Duration::from_secs(2), handle.await_completion())
        .await
        .expect("run must not hang")
        .expect("run handle join");

    assert!(
        matches!(outcome, RunOutcome::Parked { .. }),
        "expected Parked (the pre-dispatch check, not a reactive 429), got {outcome:?}"
    );
    assert!(
        mock.recorded_calls.lock().await.is_empty(),
        "the bridge must never be touched at all: R37 is a *pre-dispatch* refusal, not a \
         reactive one — the run's only opportunity to burn a real attempt must never be taken"
    );
}

/// Task 12 M3 review, BLOCKING #4 (honesty gap #2): the existing
/// `configured_blind_backoff_reaches_the_park_decision_not_the_hardcoded_default`
/// test builds a `CapacityPolicy` directly in its own body and proves
/// `EngineConfig -> decide`, but never exercises `surge.toml ->
/// SurgeConfig -> EngineConfig` — the actual path an operator's edit to
/// their config file travels through `surge-cli`/`surge-daemon`'s
/// production wiring. This test writes a real `surge.toml`, loads it via
/// `SurgeConfig::discover_from` (the same call those two crates make),
/// builds `EngineConfig` the identical way they do
/// (`capacity: (&app_config.capacity).into()`), and proves the resulting
/// `wake_at` reflects the file's `blind_backoff`, not the module's own
/// hardcoded 5-minute default.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn surge_toml_blind_backoff_reaches_the_park_decision_through_surge_config_discover() {
    let profiles_dir = tempfile::tempdir().unwrap();
    drop_profile(profiles_dir.path(), "toml-role", "claude-code", &["done"]);
    let disk = DiskProfileSet::scan(profiles_dir.path()).unwrap();
    let registry = Arc::new(ProfileRegistry::new(disk));

    let dir = tempfile::tempdir().unwrap();
    // A distinctive value unlikely to collide with any other test's or the
    // module's own default (300s) or the sibling test's (42s).
    std::fs::write(
        dir.path().join("surge.toml"),
        // jitter_max pinned to 0s for the same reason as the sibling
        // hardcoded-backoff test above: this test's assertion window is
        // about `blind_backoff` plumbing, not jitter.
        "[capacity]\nblind_backoff = \"77s\"\njitter_max = \"0s\"\n",
    )
    .unwrap();
    let app_config = surge_core::config::SurgeConfig::discover_from(dir.path())
        .expect("discover_from must load the surge.toml just written");
    assert_eq!(
        app_config.capacity.blind_backoff,
        Some(Duration::from_secs(77)),
        "test premise: surge.toml's blind_backoff must actually parse to 77s"
    );

    let storage = Storage::open(dir.path()).await.unwrap();
    let mock = Arc::new(MockBridge::new());
    let bridge: Arc<dyn BridgeFacade> = mock.clone();
    let dispatcher: Arc<dyn ToolDispatcher> = Arc::new(UnusedDispatcher);

    mock.fail_next_send_message(SendMessageError::RateLimited {
        retry_after: None,
        details: "usage limit reached, no reset time given".into(),
    })
    .await;

    // The exact conversion `surge-cli::commands::engine`,
    // `surge-cli::commands::bootstrap`, and `surge-daemon::main` all apply
    // to `SurgeConfig.capacity` at startup.
    let engine = Engine::new_full(
        bridge,
        storage.clone(),
        dispatcher,
        Arc::new(surge_notify::MultiplexingNotifier::new()),
        None,
        Some(registry),
        EngineConfig {
            capacity: (&app_config.capacity).into(),
            ..EngineConfig::default()
        },
    );

    let run_id = RunId::new();
    let g = graph(
        "capacity-surge-toml-wiring",
        "agent_1",
        vec![
            agent_node("agent_1", "toml-role@1.0", vec![], &["done"]),
            terminal_node("end"),
        ],
        vec![edge("agent_to_end", "agent_1", "done", "end")],
    );
    let before = chrono::Utc::now();
    let handle = engine
        .start_run(
            run_id,
            g,
            dir.path().to_path_buf(),
            EngineRunConfig::default(),
        )
        .await
        .expect("start_run");

    let outcome = tokio::time::timeout(Duration::from_secs(2), handle.await_completion())
        .await
        .expect("run must not hang")
        .expect("run handle join");

    let RunOutcome::Parked { wake_at } = outcome else {
        panic!("expected Parked, got {outcome:?}");
    };
    let elapsed = (wake_at - before).num_seconds();
    assert!(
        (65..=90).contains(&elapsed),
        "wake_at must reflect surge.toml's 77s blind_backoff (got {elapsed}s until wake) — \
         a value near 300s would mean the config file never reached the decision, and a value \
         near 42s would mean a different test's hardcoded value leaked in"
    );
}

/// Task 12 M3 review, BLOCKING #1 — end-to-end proof that the livelock is
/// actually broken, not just that `decide`'s rule order is fixed in
/// isolation. This is the specific shape the fix targets: a runtime whose
/// reset time was **never learned** (`resets_at: None`), so `blind_backoff`
/// is the only applicable rule (the "known, elapsed reset" fix in
/// `decide` does not apply here at all). Without the resume-time bypass
/// and the clear-on-success mechanism, resuming this run would re-consult
/// the same unrefreshed `runtime_capacity` row and park again forever,
/// because nothing but a real dispatch attempt can ever refresh it, and
/// the precheck is what was preventing that attempt.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resuming_a_parked_run_makes_one_real_attempt_and_clears_the_stale_row() {
    let profiles_dir = tempfile::tempdir().unwrap();
    drop_profile(profiles_dir.path(), "resume-role", "claude-code", &["done"]);
    let disk = DiskProfileSet::scan(profiles_dir.path()).unwrap();
    let registry = Arc::new(ProfileRegistry::new(disk));

    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::open(dir.path()).await.unwrap();
    let mock = Arc::new(MockBridge::new());
    let bridge: Arc<dyn BridgeFacade> = mock.clone();
    let dispatcher: Arc<dyn ToolDispatcher> = Arc::new(UnusedDispatcher);

    // First attempt: rate-limited, no reset time learned at all.
    mock.fail_next_send_message(SendMessageError::RateLimited {
        retry_after: None,
        details: "usage limit reached".into(),
    })
    .await;

    let engine = Engine::new_full(
        bridge,
        storage.clone(),
        dispatcher,
        Arc::new(surge_notify::MultiplexingNotifier::new()),
        None,
        Some(registry),
        EngineConfig::default(),
    );

    let run_id = RunId::new();
    let g = graph(
        "capacity-resume-refreshes-row",
        "agent_1",
        vec![
            agent_node("agent_1", "resume-role@1.0", vec![], &["done"]),
            terminal_node("end"),
        ],
        vec![edge("agent_to_end", "agent_1", "done", "end")],
    );
    let handle = engine
        .start_run(
            run_id,
            g,
            dir.path().to_path_buf(),
            EngineRunConfig::default(),
        )
        .await
        .expect("start_run");

    let outcome = tokio::time::timeout(Duration::from_secs(2), handle.await_completion())
        .await
        .expect("run must not hang")
        .expect("run handle join");
    assert!(
        matches!(outcome, RunOutcome::Parked { .. }),
        "expected Parked, got {outcome:?}"
    );
    assert!(
        matches!(
            storage.capacity_status("claude-acp").await.unwrap(),
            surge_core::capacity::CapacityStatus::Known(w) if w.resets_at().is_none()
        ),
        "test premise: the row must have no learned reset time (the exact shape blind_backoff \
         alone governs, and the shape the elapsed-reset fix does not touch)"
    );

    // Simulate the wake (crash-recovery-due today; Task 12 M4's scheduler
    // eventually) by resuming the same run_id directly. Scripted to
    // succeed this time — if the bypass mechanism is broken, this dispatch
    // never happens at all and the run just re-parks (or hangs).
    let second_session = SessionId::new();
    mock.pin_next_session_id(second_session).await;
    mock.enqueue_event(BridgeEvent::OutcomeReported {
        session: second_session,
        outcome: OutcomeKey::try_from("done").unwrap(),
        summary: "ok on resume".into(),
        artifacts_produced: vec![],
    })
    .await;

    let resumed_handle = engine
        .resume_run(run_id, dir.path().to_path_buf())
        .await
        .expect("resume_run");
    // `wait_for_subscribe_count(2)`, not `pump_after_subscribe(1)`:
    // `execute_agent_stage` subscribes *before* `send_message`, so
    // agent_1's first (rate-limited) attempt already brought the
    // cumulative count to 1 — waiting for "1" again would be satisfied
    // immediately by that stale count and pump before this resumed
    // session's own subscription exists, dropping the event.
    let mock_for_pump = mock.clone();
    let pump = tokio::spawn(async move {
        mock_for_pump.wait_for_subscribe_count(2).await;
        mock_for_pump.pump_scripted_events().await;
    });

    let resumed_outcome =
        tokio::time::timeout(Duration::from_secs(2), resumed_handle.await_completion())
            .await
            .expect(
                "resumed run must not hang: the bypass must let a real attempt happen instead \
                 of silently re-parking on the same unrefreshed row",
            )
            .expect("run handle join");
    pump.await.unwrap();

    assert!(
        matches!(resumed_outcome, RunOutcome::Completed { .. }),
        "the resumed run must actually attempt dispatch and succeed, not re-park blindly on \
         the same stale row; got {resumed_outcome:?}"
    );

    let send_message_count = mock
        .recorded_calls
        .lock()
        .await
        .iter()
        .filter(|c| matches!(c, fixtures::mock_bridge::RecordedCall::SendMessage { .. }))
        .count();
    assert_eq!(
        send_message_count, 2,
        "exactly two real attempts: the original 429, then the post-resume retry — never a \
         third (no further blind re-park happened)"
    );

    assert_eq!(
        storage.capacity_status("claude-acp").await.unwrap(),
        surge_core::capacity::CapacityStatus::NeverObserved,
        "a successful post-resume dispatch must clear the stale exhaustion row, or the next \
         run on this runtime would still read it as exhausted"
    );

    let after_resume = storage.get_run(&run_id).await.unwrap().unwrap();
    assert_eq!(
        after_resume.status,
        RunStatus::Running,
        "resume_run must transition the registry out of Parked"
    );
    assert_eq!(
        after_resume.wake_at_ms, None,
        "wake_at must be cleared in the same write as the status transition, not left stale"
    );

    // Task 12 M4: `RunWokeFromPark` was declared and folded (clears
    // `RunState::Pipeline.parked`) since M1, but nothing wrote it — this is
    // the write, and this is its first behavioral test. Must land AFTER
    // the original `RunParked` and appear exactly once.
    let reader = storage.open_run_reader(run_id).await.unwrap();
    let last = reader.current_seq().await.unwrap();
    let events = reader
        .read_events(EventSeq(0)..EventSeq(last.0 + 1))
        .await
        .unwrap();
    let parked_seq = events
        .iter()
        .find(|ev| matches!(ev.payload.payload, EventPayload::RunParked { .. }))
        .map(|ev| ev.seq)
        .expect("RunParked must still be in the log");
    let woke_events: Vec<_> = events
        .iter()
        .filter(|ev| matches!(ev.payload.payload, EventPayload::RunWokeFromPark { .. }))
        .collect();
    assert_eq!(
        woke_events.len(),
        1,
        "expected exactly one RunWokeFromPark event, got {}: {events:?}",
        woke_events.len()
    );
    assert!(
        woke_events[0].seq > parked_seq,
        "RunWokeFromPark must follow RunParked in the log"
    );
}

/// Task 12 M4 acceptance criterion: "the frozen budget survives a wake" —
/// the mechanism commit `1ed5caa` added (persist `RunConfig.budget`,
/// restore it into `resume_run_config.budget` in `Engine::resume_run`)
/// applies to every resume, but nothing proved it specifically for a
/// parked-then-woken run before this test. A behavioral proof, not a
/// field-equality check: if the resumed run's budget had silently reverted
/// to `BudgetGuard::default()` (unlimited) instead of the 1_000-token
/// budget it started with, `enforce_budget` would short-circuit
/// (`is_unlimited()`) and this run would complete instead of aborting.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resuming_a_parked_run_re_arms_its_frozen_token_budget() {
    let profiles_dir = tempfile::tempdir().unwrap();
    drop_profile(profiles_dir.path(), "budget-role", "claude-code", &["done"]);
    let disk = DiskProfileSet::scan(profiles_dir.path()).unwrap();
    let registry = Arc::new(ProfileRegistry::new(disk));

    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::open(dir.path()).await.unwrap();
    let mock = Arc::new(MockBridge::new());
    let bridge: Arc<dyn BridgeFacade> = mock.clone();
    let dispatcher: Arc<dyn ToolDispatcher> = Arc::new(UnusedDispatcher);

    // First attempt: rate-limited, no reset time learned — same shape as
    // the sibling resume test, so the run parks before spending anything.
    mock.fail_next_send_message(SendMessageError::RateLimited {
        retry_after: None,
        details: "usage limit reached".into(),
    })
    .await;

    let engine = Engine::new_full(
        bridge,
        storage.clone(),
        dispatcher,
        Arc::new(surge_notify::MultiplexingNotifier::new()),
        None,
        Some(registry),
        EngineConfig::default(),
    );

    let run_id = RunId::new();
    let g = graph(
        "capacity-resume-budget",
        "agent_1",
        vec![
            agent_node("agent_1", "budget-role@1.0", vec![], &["done"]),
            terminal_node("end"),
        ],
        vec![edge("agent_to_end", "agent_1", "done", "end")],
    );
    // A real, tight token budget — not the default (unlimited). If resume
    // ever reverts to `EngineRunConfig::default()`'s budget, this is the
    // dimension that would silently stop being enforced.
    let run_config = EngineRunConfig {
        budget: BudgetGuard {
            limits: BudgetLimits {
                usd: None,
                tokens: Some(1_000),
                warn_threshold_pct: 80,
            },
            policy: BudgetPolicy::Abort,
        },
        ..EngineRunConfig::default()
    };
    let handle = engine
        .start_run(run_id, g, dir.path().to_path_buf(), run_config)
        .await
        .expect("start_run");

    let outcome = tokio::time::timeout(Duration::from_secs(2), handle.await_completion())
        .await
        .expect("run must not hang")
        .expect("run handle join");
    assert!(
        matches!(outcome, RunOutcome::Parked { .. }),
        "expected Parked before any token spend, got {outcome:?}"
    );

    // Wake: resume the same run, scripted to report 15_000 tokens (well
    // over the 1_000-token budget) before its outcome.
    let second_session = SessionId::new();
    mock.pin_next_session_id(second_session).await;
    mock.enqueue_event(BridgeEvent::TokenUsage {
        session: second_session,
        prompt_tokens: 10_000,
        output_tokens: 5_000,
        cache_hits: 0,
        model: "mock-model".into(),
    })
    .await;
    mock.enqueue_event(BridgeEvent::OutcomeReported {
        session: second_session,
        outcome: OutcomeKey::try_from("done").unwrap(),
        summary: "ok on resume".into(),
        artifacts_produced: vec![],
    })
    .await;

    let resumed_handle = engine
        .resume_run(run_id, dir.path().to_path_buf())
        .await
        .expect("resume_run");
    let mock_for_pump = mock.clone();
    let pump = tokio::spawn(async move {
        mock_for_pump.wait_for_subscribe_count(2).await;
        mock_for_pump.pump_scripted_events().await;
    });

    let resumed_outcome =
        tokio::time::timeout(Duration::from_secs(2), resumed_handle.await_completion())
            .await
            .expect("resumed run must not hang")
            .expect("run handle join");
    pump.await.unwrap();

    match &resumed_outcome {
        RunOutcome::Aborted { reason } => {
            assert!(
                reason.contains("budget"),
                "abort reason should cite the budget, got: {reason}"
            );
        },
        other => panic!(
            "expected Aborted on budget breach after resume (proves the frozen 1_000-token \
             budget survived the wake, per commit 1ed5caa's mechanism) — a resumed run that \
             silently reverted to the unlimited default would instead show Completed; got \
             {other:?}"
        ),
    }

    let reader = storage.open_run_reader(run_id).await.unwrap();
    let last = reader.current_seq().await.unwrap();
    let events = reader
        .read_events(EventSeq(0)..EventSeq(last.0 + 1))
        .await
        .unwrap();
    let saw_exceeded = events.iter().any(|ev| {
        matches!(
            &ev.payload.payload,
            EventPayload::BudgetExceeded {
                dimension: BudgetDimension::Tokens,
                ..
            }
        )
    });
    assert!(
        saw_exceeded,
        "expected a BudgetExceeded(Tokens) event in the post-resume log"
    );
}

/// Task 12 M4 review: `EngineRunConfig::memory_store_path` is deliberately
/// never persisted (it is a test-only override), so `Engine::resume_run`
/// rebuilding `EngineRunConfig::default()` from scratch has no way to
/// recover a per-run store override — before this fix, a resumed run's
/// stage failure would silently write to the real `~/.surge/memory.db`
/// regardless of what the original `start_run` was given.
///
/// The fix moves the override to `EngineConfig::memory_store_path` (engine
/// construction time, not per-run), which both `start_run` and
/// `resume_run` consult. This is the "obvious next test" the review named:
/// park, resume, drive the resumed dispatch to a genuine (non-rate-limit)
/// stage failure, and prove the write-back landed in the *configured*
/// store — the store this test controls — not silently in the developer's
/// real one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resumed_run_driven_to_stage_failure_writes_to_the_configured_memory_store() {
    let profiles_dir = tempfile::tempdir().unwrap();
    drop_profile(
        profiles_dir.path(),
        "memstore-role",
        "claude-code",
        &["done"],
    );
    let disk = DiskProfileSet::scan(profiles_dir.path()).unwrap();
    let registry = Arc::new(ProfileRegistry::new(disk));

    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::open(dir.path()).await.unwrap();
    let mock = Arc::new(MockBridge::new());
    let bridge: Arc<dyn BridgeFacade> = mock.clone();
    let dispatcher: Arc<dyn ToolDispatcher> = Arc::new(UnusedDispatcher);

    // First attempt: rate-limited, no reset time learned — parks before
    // ever reaching the memory-store question.
    mock.fail_next_send_message(SendMessageError::RateLimited {
        retry_after: None,
        details: "usage limit reached".into(),
    })
    .await;

    let memory_dir = tempfile::tempdir().unwrap();
    let configured_store = memory_dir.path().join("memory.db");

    // The fix under test: the override lives on `EngineConfig`, set once
    // at construction — `start_run` below is deliberately given a plain
    // `EngineRunConfig::default()` (no per-run override at all), so any
    // write that lands in `configured_store` can only have come from the
    // engine-level fallback, never from a per-run field this test forgot
    // to set.
    let engine = Engine::new_full(
        bridge,
        storage.clone(),
        dispatcher,
        Arc::new(surge_notify::MultiplexingNotifier::new()),
        None,
        Some(registry),
        EngineConfig {
            memory_store_path: Some(configured_store.clone()),
            ..EngineConfig::default()
        },
    );

    let run_id = RunId::new();
    let g = graph(
        "capacity-resume-memory-store",
        "agent_1",
        vec![
            agent_node("agent_1", "memstore-role@1.0", vec![], &["done"]),
            terminal_node("end"),
        ],
        vec![edge("agent_to_end", "agent_1", "done", "end")],
    );
    let handle = engine
        .start_run(
            run_id,
            g,
            dir.path().to_path_buf(),
            EngineRunConfig::default(),
        )
        .await
        .expect("start_run");

    let outcome = tokio::time::timeout(Duration::from_secs(2), handle.await_completion())
        .await
        .expect("run must not hang")
        .expect("run handle join");
    assert!(
        matches!(outcome, RunOutcome::Parked { .. }),
        "expected Parked before any memory write-back question arises, got {outcome:?}"
    );

    // Wake: resume, scripted to fail hard on the bypass-granted attempt —
    // NOT rate-limited, so the capacity gate lets it fall through to a
    // genuine `StageFailed` instead of re-parking.
    mock.fail_next_send_message(SendMessageError::AgentAuthenticationFailed {
        details: "not a rate limit — must reach StageFailed, not re-park".into(),
    })
    .await;

    let resumed_handle = engine
        .resume_run(run_id, dir.path().to_path_buf())
        .await
        .expect("resume_run");
    let resumed_outcome =
        tokio::time::timeout(Duration::from_secs(2), resumed_handle.await_completion())
            .await
            .expect("resumed run must not hang")
            .expect("run handle join");
    assert!(
        matches!(resumed_outcome, RunOutcome::Failed { .. }),
        "expected the resumed dispatch to fail the stage (not re-park, not succeed), got \
         {resumed_outcome:?}"
    );

    assert!(
        configured_store.exists(),
        "the resumed run's stage failure must write to the store this test configured at the \
         engine level; before the fix, `resume_run` rebuilds EngineRunConfig::default() with \
         no way to recover a per-run override, and this file would never be created"
    );
    let claims = MemoryStore::open(&configured_store)
        .expect("open configured memory store")
        .list_claims()
        .expect("list claims");
    assert_eq!(
        claims.len(),
        1,
        "expected exactly one memory claim from the resumed run's stage failure, got {claims:?}"
    );
    assert!(
        claims[0].text().contains("agent_1"),
        "claim text should name the failing node: {}",
        claims[0].text()
    );
}
