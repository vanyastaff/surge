//! Integration tests: memory write-back at a node's terminal failure, proven
//! through the real engine harness (`Engine::start_run` + `MockBridge`), not
//! by calling `engine::hooks::memory_writeback` directly.
//!
//! `.autopilot/competitive-waves/tickets/07-memory-writeback.md` requires
//! four things and a reachability proof: silence on a clean run, root cause
//! over symptom, update in place over near-duplicate, a verified claim never
//! aged, and — because the memory-locator contract in `interfaces.md` warns
//! that a claim shaped wrong is invisible to `surge memory audit` without
//! ever failing or complaining — a claim this module writes must actually be
//! found by `surge_persistence::memory::run_audit`, the real audit code, not
//! merely shaped like its contract.

#![allow(clippy::too_many_lines)]

mod fixtures;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use surge_acp::bridge::event::BridgeEvent;
use surge_acp::bridge::facade::BridgeFacade;
use surge_core::agent_config::{AgentConfig, NodeLimits};
use surge_core::content_hash::ContentHash;
use surge_core::edge::{Edge, EdgeKind, EdgePolicy, PortRef};
use surge_core::graph::{Graph, GraphMetadata, SCHEMA_VERSION};
use surge_core::hooks::{Hook, HookFailureMode, HookInheritance, HookTrigger, MatcherSpec};
use surge_core::id::SessionId;
use surge_core::keys::{EdgeKey, NodeKey, OutcomeKey, ProfileKey};
use surge_core::loop_config::ToolCallLoopGuardConfig;
use surge_core::memory::{ClaimStatus, Confidence, MemoryClaim, Provenance};
use surge_core::node::{Node, NodeConfig, OutcomeDecl, Position};
use surge_core::terminal_config::{TerminalConfig, TerminalKind};
use surge_core::{MemoryClaimId, RunId, RunStatus};
use surge_orchestrator::engine::tools::ToolDispatcher;
use surge_orchestrator::engine::tools::worktree::WorktreeToolDispatcher;
use surge_orchestrator::engine::{Engine, EngineConfig, EngineRunConfig, RunOutcome};
use surge_persistence::memory::{MemoryStore, run_audit};
use surge_persistence::runs::Storage;

use fixtures::mock_bridge::MockBridge;

/// `EngineRunConfig` with `memory_store_path` pointed at `store_path` so
/// `engine::hooks::memory_writeback::record_node_failure` writes there
/// instead of resolving the real `MemoryStore::default_path()`
/// (`~/.surge/memory.db`). Every test in this file must route through this
/// (or [`instant_wall_clock_trip`], which composes it) rather than
/// `EngineRunConfig::default()` directly, or a write-back would land in the
/// developer's actual memory store.
fn run_config_with_memory_store(store_path: PathBuf) -> EngineRunConfig {
    EngineRunConfig {
        memory_store_path: Some(store_path),
        ..EngineRunConfig::default()
    }
}

/// List every claim in the `MemoryStore` at `store_path`. Tolerates a store
/// that was never created (a clean run leaves no `memory.db` at all).
fn list_claims(store_path: &Path) -> Vec<MemoryClaim> {
    if !store_path.exists() {
        return Vec::new();
    }
    MemoryStore::open(store_path)
        .expect("open memory store")
        .list_claims()
        .expect("list claims")
}

fn terminal_node(id: NodeKey) -> Node {
    Node {
        id: id.clone(),
        position: Position::default(),
        declared_outcomes: vec![],
        config: NodeConfig::Terminal(TerminalConfig {
            kind: TerminalKind::Success,
            message: None,
        }),
    }
}

fn plain_agent_config(hooks: Vec<Hook>, max_retries: u32) -> AgentConfig {
    AgentConfig {
        profile: ProfileKey::try_from("implementer@1.0").unwrap(),
        prompt_overrides: None,
        tool_overrides: None,
        sandbox_override: None,
        approvals_override: None,
        bindings: vec![],
        rules_overrides: None,
        limits: NodeLimits {
            max_retries,
            ..Default::default()
        },
        hooks,
        custom_fields: Default::default(),
    }
}

/// A single Agent node ("implement") wired to a Terminal success node on its
/// `"done"` outcome — the same shape
/// `engine_guard_spill_harness_test.rs::agent_to_terminal_graph` uses to
/// prove guard wiring, reused here to prove memory write-back wiring off the
/// same node-fails-hard shape (a wall-clock guard trip, ticket 17).
fn agent_to_terminal_graph() -> Graph {
    let agent_key = NodeKey::try_from("implement").unwrap();
    let end_key = NodeKey::try_from("end").unwrap();

    let mut nodes = BTreeMap::new();
    nodes.insert(
        agent_key.clone(),
        Node {
            id: agent_key.clone(),
            position: Position::default(),
            declared_outcomes: vec![OutcomeDecl {
                id: OutcomeKey::try_from("done").unwrap(),
                description: "stage completed".into(),
                edge_kind_hint: EdgeKind::Forward,
                is_terminal: false,
                ledger_effect: Default::default(),
            }],
            config: NodeConfig::Agent(plain_agent_config(
                vec![],
                NodeLimits::default().max_retries,
            )),
        },
    );
    nodes.insert(end_key.clone(), terminal_node(end_key.clone()));

    Graph {
        schema_version: SCHEMA_VERSION,
        metadata: GraphMetadata::new("memory-writeback-harness", chrono::Utc::now()),
        start: agent_key.clone(),
        nodes,
        edges: vec![Edge {
            id: EdgeKey::try_from("e_done").unwrap(),
            from: PortRef {
                node: agent_key,
                outcome: OutcomeKey::try_from("done").unwrap(),
            },
            to: end_key,
            kind: EdgeKind::Forward,
            policy: EdgePolicy::default(),
        }],
        subgraphs: BTreeMap::new(),
    }
}

fn outcome_reject_hook(id: &str, target_outcome: &str) -> Hook {
    Hook {
        id: id.into(),
        trigger: HookTrigger::OnOutcome,
        matcher: MatcherSpec {
            outcome: Some(OutcomeKey::try_from(target_outcome).unwrap()),
            ..Default::default()
        },
        command: "exit 1".into(),
        on_failure: HookFailureMode::Reject,
        timeout_seconds: Some(5),
        inherit: HookInheritance::Extend,
    }
}

/// An Agent node ("implement") that always gets both its candidate outcomes
/// rejected by two *different* `on_outcome` hooks — `"pass"` by
/// `deny-pass`, then `"fixes_needed"` by `deny-fixes` — exhausting a
/// `max_retries: 1` budget on the second rejection. Both outcomes still
/// need a declared edge for graph validation even though neither is ever
/// taken; both point at the same Terminal node.
fn two_outcome_agent_graph() -> Graph {
    let agent_key = NodeKey::try_from("implement").unwrap();
    let end_key = NodeKey::try_from("end").unwrap();

    let mut nodes = BTreeMap::new();
    nodes.insert(
        agent_key.clone(),
        Node {
            id: agent_key.clone(),
            position: Position::default(),
            declared_outcomes: vec![
                OutcomeDecl {
                    id: OutcomeKey::try_from("pass").unwrap(),
                    description: "accepted".into(),
                    edge_kind_hint: EdgeKind::Forward,
                    is_terminal: false,
                    ledger_effect: Default::default(),
                },
                OutcomeDecl {
                    id: OutcomeKey::try_from("fixes_needed").unwrap(),
                    description: "fallback".into(),
                    edge_kind_hint: EdgeKind::Forward,
                    is_terminal: false,
                    ledger_effect: Default::default(),
                },
            ],
            config: NodeConfig::Agent(plain_agent_config(
                vec![
                    outcome_reject_hook("deny-pass", "pass"),
                    outcome_reject_hook("deny-fixes", "fixes_needed"),
                ],
                1,
            )),
        },
    );
    nodes.insert(end_key.clone(), terminal_node(end_key.clone()));

    Graph {
        schema_version: SCHEMA_VERSION,
        metadata: GraphMetadata::new("memory-writeback-harness", chrono::Utc::now()),
        start: agent_key.clone(),
        nodes,
        edges: vec![
            Edge {
                id: EdgeKey::try_from("e_pass").unwrap(),
                from: PortRef {
                    node: agent_key.clone(),
                    outcome: OutcomeKey::try_from("pass").unwrap(),
                },
                to: end_key.clone(),
                kind: EdgeKind::Forward,
                policy: EdgePolicy::default(),
            },
            Edge {
                id: EdgeKey::try_from("e_fixed").unwrap(),
                from: PortRef {
                    node: agent_key,
                    outcome: OutcomeKey::try_from("fixes_needed").unwrap(),
                },
                to: end_key,
                kind: EdgeKind::Forward,
                policy: EdgePolicy::default(),
            },
        ],
        subgraphs: BTreeMap::new(),
    }
}

/// Deterministic wall-clock guard config (`node_wall_clock_limit_secs: 0`,
/// mirroring `guard::tests::deadline_past_budget_escalates`): already past
/// budget the instant the stage starts, so the node fails hard with no
/// sleeping or scripted tool calls needed. Routes write-back at `store_path`
/// (see [`run_config_with_memory_store`]).
fn instant_wall_clock_trip(store_path: PathBuf) -> EngineRunConfig {
    EngineRunConfig {
        tool_call_loop_guard: Some(ToolCallLoopGuardConfig {
            max_repeat_tool_calls: 100,
            node_wall_clock_limit_secs: 0,
        }),
        ..run_config_with_memory_store(store_path)
    }
}

/// Drive `graph` through `Engine::start_run` in `dir` (a worktree/run store
/// root the *caller* owns and keeps alive — `Storage`'s registry and event
/// log live under it, so it must outlive every use of the returned
/// `Storage`, including a later `run_audit` against its registry), using
/// `mock` (already scripted by the caller) as the bridge. Returns
/// `(run_id, storage, outcome)`.
async fn drive_run(
    dir: &Path,
    graph: Graph,
    run_config: EngineRunConfig,
    mock: Arc<MockBridge>,
) -> (RunId, Arc<Storage>, RunOutcome) {
    let storage = Storage::open(dir).await.unwrap();
    let bridge: Arc<dyn BridgeFacade> = mock.clone();
    let dispatcher =
        Arc::new(WorktreeToolDispatcher::new(dir.to_path_buf())) as Arc<dyn ToolDispatcher>;

    let mock_for_pump = mock.clone();
    let pump = tokio::spawn(async move {
        // Drains whatever the caller already enqueued once the engine
        // subscribes; a no-op drain when nothing was scripted (the
        // wall-clock-trip scenarios below need no tool call or outcome at
        // all — ticket 17's own harness does the same).
        mock_for_pump.pump_after_subscribe(1).await;
    });

    let engine = Engine::new(bridge, storage.clone(), dispatcher, EngineConfig::default());
    let run_id = RunId::new();
    let handle = engine
        .start_run(run_id, graph, dir.to_path_buf(), run_config)
        .await
        .expect("start_run");
    pump.await.unwrap();

    let outcome = tokio::time::timeout(Duration::from_secs(10), handle.await_completion())
        .await
        .expect("run did not terminate within 10s")
        .unwrap();

    (run_id, storage, outcome)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn clean_run_writes_no_memory_claim() {
    let memory_dir = tempfile::tempdir().unwrap();
    let store_path = memory_dir.path().join("memory.db");
    let dir = tempfile::tempdir().unwrap();

    let mock = Arc::new(MockBridge::new());
    let session_id = SessionId::new();
    mock.pin_next_session_id(session_id).await;
    mock.enqueue_event(BridgeEvent::OutcomeReported {
        session: session_id,
        outcome: OutcomeKey::from_str("done").unwrap(),
        summary: "all good".into(),
        artifacts_produced: vec![],
    })
    .await;

    let (_run_id, _storage, outcome) = drive_run(
        dir.path(),
        agent_to_terminal_graph(),
        run_config_with_memory_store(store_path.clone()),
        mock,
    )
    .await;
    assert!(
        matches!(outcome, RunOutcome::Completed { .. }),
        "expected a clean completion, got {outcome:?}"
    );

    let claims = list_claims(&store_path);
    assert!(
        claims.is_empty(),
        "a clean run must not write any memory claim, got {claims:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failing_run_records_a_claim_visible_to_the_audit() {
    let memory_dir = tempfile::tempdir().unwrap();
    let store_path = memory_dir.path().join("memory.db");
    // Kept alive for the whole test (not just the run) — `storage`'s
    // registry lives under it, and `run_audit` below needs to open that
    // same, still-live registry after the run completes.
    let dir = tempfile::tempdir().unwrap();

    let mock = Arc::new(MockBridge::new());
    let session_id = SessionId::new();
    mock.pin_next_session_id(session_id).await;
    // No scripted events at all — matches ticket 17's own deterministic
    // wall-clock-trip harness (`engine_guard_spill_harness_test.rs`):
    // the timer-driven poll inside the stage's event loop fails the
    // node before any tool call or outcome is ever needed.

    let (run_id, storage, outcome) = drive_run(
        dir.path(),
        agent_to_terminal_graph(),
        instant_wall_clock_trip(store_path.clone()),
        mock,
    )
    .await;
    assert!(
        matches!(outcome, RunOutcome::Failed { .. }),
        "expected the wall-clock trip to fail the run, got {outcome:?}"
    );

    // Mirrors what a real caller (daemon/CLI, `surge-daemon::recovery` /
    // `surge-cli::commands::daemon`) does on a terminal outcome — wiring
    // that up end to end is out of this ticket's zone, but it must
    // happen here to prove the claim this module wrote is genuinely
    // found by `run_audit` against the run's real registry status, not
    // merely shaped like its contract.
    storage
        .set_run_status(&run_id, RunStatus::Failed, Some(1))
        .await
        .unwrap();

    let claims = list_claims(&store_path);

    assert_eq!(
        claims.len(),
        1,
        "expected exactly one memory claim, got {claims:?}"
    );
    let claim = &claims[0];
    assert!(
        claim.text().contains("implement"),
        "claim text should name the failing node: {}",
        claim.text()
    );
    assert!(
        claim.text().contains("wall-clock"),
        "claim text should carry the wall-clock cause: {}",
        claim.text()
    );
    assert_eq!(claim.confidence(), Confidence::Asserted);
    assert_eq!(claim.status(), ClaimStatus::Unverified);

    let expected_source_prefix = format!("transcript:{run_id}#turn-");
    assert!(
        claim
            .provenance()
            .source
            .starts_with(&expected_source_prefix),
        "source must be the transcript locator the memory-locator contract requires \
         (interfaces.md \"Контракт локаторов памяти\"): {}",
        claim.provenance().source
    );

    // The real audit, unmodified — not a reimplementation of its parsing.
    // The claim's source is a `transcript:` locator (asserted above), so
    // `project_root` never enters a path resolution here — `dir` (this
    // run's own worktree root) is passed as the honest value regardless.
    let report = run_audit(&claims, &storage, dir.path()).await.unwrap();
    let correlated = report
        .run_correlated
        .iter()
        .find(|finding| finding.claim_id == claim.id())
        .expect(
            "the write-back claim must be found by the real memory audit, not just shaped \
             like its contract",
        );
    assert_eq!(correlated.run_id, run_id);
    assert_eq!(correlated.run_status, RunStatus::Failed);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn records_the_first_rejecting_hook_not_the_last_one() {
    let memory_dir = tempfile::tempdir().unwrap();
    let store_path = memory_dir.path().join("memory.db");
    let dir = tempfile::tempdir().unwrap();

    let mock = Arc::new(MockBridge::new());
    let session_id = SessionId::new();
    mock.pin_next_session_id(session_id).await;
    mock.enqueue_event(BridgeEvent::OutcomeReported {
        session: session_id,
        outcome: OutcomeKey::from_str("pass").unwrap(),
        summary: "first try".into(),
        artifacts_produced: vec![],
    })
    .await;
    mock.enqueue_event(BridgeEvent::OutcomeReported {
        session: session_id,
        outcome: OutcomeKey::from_str("fixes_needed").unwrap(),
        summary: "second try".into(),
        artifacts_produced: vec![],
    })
    .await;

    let (_run_id, _storage, outcome) = drive_run(
        dir.path(),
        two_outcome_agent_graph(),
        run_config_with_memory_store(store_path.clone()),
        mock,
    )
    .await;
    let claims = list_claims(&store_path);

    match &outcome {
        RunOutcome::Failed { error } => assert!(
            error.contains("rejection budget exhausted"),
            "unexpected failure reason: {error}"
        ),
        other => panic!("expected the rejection budget to fail the run, got {other:?}"),
    }

    assert_eq!(
        claims.len(),
        1,
        "expected exactly one memory claim, got {claims:?}"
    );
    let text = claims[0].text();
    assert!(
        text.contains("deny-pass"),
        "root cause must name the *first* rejecting hook ('deny-pass'), not the last: {text}"
    );
    assert!(
        !text.contains("deny-fixes"),
        "the last rejecting hook ('deny-fixes') is the symptom `resolve_stage_error` already \
         named in `StageFailed`; write-back must prefer the root cause over it: {text}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn second_failure_of_the_same_node_updates_the_claim_in_place() {
    async fn trip_once(store_path: &Path) -> (RunId, MemoryClaimId, String) {
        let dir = tempfile::tempdir().unwrap();
        let mock = Arc::new(MockBridge::new());
        let session_id = SessionId::new();
        mock.pin_next_session_id(session_id).await;

        let (run_id, _storage, outcome) = drive_run(
            dir.path(),
            agent_to_terminal_graph(),
            instant_wall_clock_trip(store_path.to_path_buf()),
            mock,
        )
        .await;
        assert!(matches!(outcome, RunOutcome::Failed { .. }));

        let claims = list_claims(store_path);
        assert_eq!(
            claims.len(),
            1,
            "expected exactly one claim about the failing node after this run, got {claims:?}"
        );
        let claim = &claims[0];
        (run_id, claim.id(), claim.provenance().source.clone())
    }

    let memory_dir = tempfile::tempdir().unwrap();
    let store_path = memory_dir.path().join("memory.db");
    let (first_run, first_id, first_source) = trip_once(&store_path).await;
    let (second_run, second_id, second_source) = trip_once(&store_path).await;

    assert_ne!(
        first_run, second_run,
        "test setup bug: the two runs must have distinct ids"
    );
    assert_eq!(
        first_id, second_id,
        "a second failure of the same node must update the existing claim in place \
         (same claim id), not insert a near-duplicate"
    );
    assert_ne!(
        first_source, second_source,
        "the updated claim must be refreshed to point at the newest run, not left stale"
    );
    assert!(
        second_source.contains(&second_run.to_string()),
        "the updated claim's source should name the second run: {second_source}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_verified_claim_covering_the_same_node_is_left_untouched() {
    let memory_dir = tempfile::tempdir().unwrap();
    let store_path = memory_dir.path().join("memory.db");

    // Seed a `Verified` claim tagged for the same node the harness below
    // fails on, using write-back's own tag (`node 'implement' failed: `) so
    // it is a genuine "covering" candidate — the only reason it must be
    // skipped is that it is `Verified`, not that its text never matched.
    let verified_text = "node 'implement' failed: known transient CI flake, already triaged";
    let verified_id = {
        let store = MemoryStore::open(&store_path).unwrap();
        let hash = ContentHash::compute(verified_text.as_bytes());
        let claim = MemoryClaim::new(
            MemoryClaimId::new(),
            verified_text,
            Provenance::verified("docs/runbook.md", hash, "manual review", 1_700_000_000_000),
            Confidence::Verified,
            ClaimStatus::Verified,
        )
        .expect("verified provenance is complete");
        store.add_claim(&claim).unwrap();
        claim.id()
    };

    let dir = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockBridge::new());
    let session_id = SessionId::new();
    mock.pin_next_session_id(session_id).await;

    let (_run_id, _storage, outcome) = drive_run(
        dir.path(),
        agent_to_terminal_graph(),
        instant_wall_clock_trip(store_path.clone()),
        mock,
    )
    .await;
    assert!(matches!(outcome, RunOutcome::Failed { .. }));

    let claims = list_claims(&store_path);

    assert_eq!(
        claims.len(),
        2,
        "the verified claim must stay and a fresh unverified one must be added \
         alongside it, got {claims:?}"
    );

    let kept = claims
        .iter()
        .find(|c| c.id() == verified_id)
        .expect("the pre-existing verified claim must not be deleted");
    assert_eq!(
        kept.text(),
        verified_text,
        "the verified claim's content must be completely untouched"
    );
    assert_eq!(kept.status(), ClaimStatus::Verified);

    let fresh = claims
        .iter()
        .find(|c| c.id() != verified_id)
        .expect("a new claim about the failing node must still be recorded");
    assert_eq!(fresh.status(), ClaimStatus::Unverified);
    assert!(fresh.text().starts_with("node 'implement' failed: "));
}
